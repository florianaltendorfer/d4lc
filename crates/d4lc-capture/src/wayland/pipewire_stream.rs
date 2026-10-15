//! PipeWire stream pump for ScreenCast frames.
//!
//! Runs in its own thread (the PipeWire mainloop must stay on the thread
//! that created it): connects to the portal-provided PipeWire remote,
//! pins the negotiation to 8-bit BGRx/BGRA, converts every received
//! buffer through the pure [`super::frame`] path (once, straight from the
//! mapped memory into a recycled buffer), and keeps only the
//! latest converted frame in a shared slot — the same newest-frame
//! discipline the runtime pipeline uses everywhere else.
//!
//! Thread shutdown is cooperative: the owner flips an `AtomicBool` and
//! the thread notices within one iterate tick (100 ms), tears the
//! stream/core/context down in drop order, and exits.

use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipewire as pw;
use pipewire::spa::buffer::ChunkFlags;
use pipewire::spa::param::format::FormatProperties;
use pipewire::spa::param::format::{MediaSubtype, MediaType};
use pipewire::spa::param::format_utils;
use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
use pipewire::spa::param::ParamType;
use pipewire::spa::pod::Value;
use pipewire::spa::utils::{Direction, Rectangle, SpaTypes};

use d4lc_core::CaptureFrame;

use crate::FrameSignal;

use super::frame::{
    convert_into_capture_frame, FrameBufferPool, FrameConversionStats, LatestFrameSlot,
    NegotiatedRate, PortalFrameFormat, RateMeter, RawPortalFrame, SlotTake, StreamRateStatus,
    UnmappedBufferCounter, MAX_STREAM_FPS, STREAM_TARGET_INTERVAL,
};
use super::WaylandCaptureError;

/// How long the loop tick sleeps between stop-flag checks.
const LOOP_TICK: Duration = Duration::from_millis(100);

/// State shared between the pump thread and the frame-source handle.
pub(super) struct StreamShared {
    /// Latest converted frame, taken by the consumer.
    latest: Mutex<LatestFrameSlot>,
    /// Recycled pixel buffers for the next conversion: a frame
    /// replaced in the slot, or handed back by the consumer after
    /// processing, returns its buffer here.
    spare: Mutex<FrameBufferPool>,
    /// New-frame signal: bumped on every stored frame; the live
    /// loop waits on it and starts processing at the delivery moment.
    signal: FrameSignal,
    /// Conversion/arrival accounting.
    stats: Mutex<FrameConversionStats>,
    /// First structured negotiation failure observed by the pump; fatal
    /// for the stream and surfaced by `capture()` instead of a silently
    /// stale frame.
    failure: Mutex<Option<String>>,
    /// Latest per-buffer problem. Not fatal: the next good buffer
    /// replaces the frame, and `capture()` only names it while no frame
    /// has converted yet.
    last_buffer_error: Mutex<Option<String>>,
    /// The negotiated stream rate, set by `param_changed`.
    negotiated: Mutex<Option<NegotiatedRate>>,
    /// Delivered (stored) and consumed (taken) frames per second.
    rates: Mutex<(RateMeter, RateMeter)>,
}

impl StreamShared {
    pub(super) fn new() -> Self {
        Self {
            latest: Mutex::new(LatestFrameSlot::default()),
            spare: Mutex::new(FrameBufferPool::default()),
            signal: FrameSignal::new(),
            stats: Mutex::new(FrameConversionStats::default()),
            failure: Mutex::new(None),
            last_buffer_error: Mutex::new(None),
            negotiated: Mutex::new(None),
            rates: Mutex::new((RateMeter::default(), RateMeter::default())),
        }
    }

    /// The new-frame signal handle.
    pub(super) fn signal(&self) -> FrameSignal {
        self.signal.clone()
    }

    /// Take the frame delivered since the previous take: a
    /// move out of the slot, never a pixel copy.
    pub(super) fn take_latest(&self) -> Result<SlotTake, String> {
        let taken = self
            .latest
            .lock()
            .map(|mut slot| slot.take())
            .map_err(|_| "wayland frame slot lock poisoned".to_string())?;
        if matches!(taken, SlotTake::Frame { .. }) {
            if let Ok(mut rates) = self.rates.lock() {
                rates.1.record(Instant::now());
            }
        }
        Ok(taken)
    }

    /// The negotiated stream rate, once `param_changed` saw the format.
    pub(super) fn negotiated_rate(&self) -> Option<NegotiatedRate> {
        self.negotiated.lock().ok().and_then(|guard| *guard)
    }

    /// The Debug screen's stream-rate facts.
    pub(super) fn rate_status(&self) -> Result<StreamRateStatus, String> {
        let now = Instant::now();
        let (delivered_fps, consumed_fps) = self
            .rates
            .lock()
            .map(|mut rates| (rates.0.per_second(now), rates.1.per_second(now)))
            .map_err(|_| "wayland stream rate lock poisoned".to_string())?;
        Ok(StreamRateStatus {
            negotiated: self.negotiated_rate(),
            delivered_fps,
            consumed_fps,
            skipped_unconsumed: self.stats()?.frames_skipped_unconsumed,
        })
    }

    /// Producer-side throttle: whether the next buffer would only
    /// replace a young, untaken frame. Counted when it skips.
    fn skip_unconsumed(&self) -> bool {
        let skip = self
            .latest
            .lock()
            .map(|slot| slot.skip_unconsumed(Instant::now(), STREAM_TARGET_INTERVAL))
            .unwrap_or(false);
        if skip {
            if let Ok(mut stats) = self.stats.lock() {
                stats.record_skipped_unconsumed();
            }
        }
        skip
    }

    fn note_negotiated(&self, rate: NegotiatedRate) {
        if let Ok(mut guard) = self.negotiated.lock() {
            *guard = Some(rate);
        }
    }

    /// Pixel size of the newest delivered frame (taken or not).
    pub(super) fn latest_frame_size(&self) -> Result<Option<(u32, u32)>, String> {
        self.latest
            .lock()
            .map(|slot| slot.delivered_size())
            .map_err(|_| "wayland frame slot lock poisoned".to_string())
    }

    /// A copy of the untaken frame, if any (first-frame wait only).
    pub(super) fn peek_latest(&self) -> Result<Option<CaptureFrame>, String> {
        self.latest
            .lock()
            .map(|slot| slot.peek().cloned())
            .map_err(|_| "wayland frame slot lock poisoned".to_string())
    }

    /// Return a processed frame's pixel buffer for reuse.
    pub(super) fn recycle(&self, frame: CaptureFrame) {
        if let Ok(mut pool) = self.spare.lock() {
            pool.give(frame.pixels_bgra);
        }
    }

    pub(super) fn stats(&self) -> Result<FrameConversionStats, String> {
        self.stats
            .lock()
            .map(|guard| *guard)
            .map_err(|_| "wayland conversion stats lock poisoned".to_string())
    }

    pub(super) fn failure(&self) -> Result<Option<String>, String> {
        self.failure
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_| "wayland stream failure lock poisoned".to_string())
    }

    pub(super) fn last_buffer_error(&self) -> Result<Option<String>, String> {
        self.last_buffer_error
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_| "wayland buffer error lock poisoned".to_string())
    }

    /// A single unusable buffer: counted and remembered, never fatal.
    fn note_buffer_error(&self, reason: String) {
        if let Ok(mut guard) = self.last_buffer_error.lock() {
            *guard = Some(reason);
        }
        if let Ok(mut stats) = self.stats.lock() {
            stats.record_failure();
        }
    }

    fn note_failure(&self, reason: String) {
        if let Ok(mut guard) = self.failure.lock() {
            if guard.is_none() {
                *guard = Some(reason);
            }
        }
        if let Ok(mut stats) = self.stats.lock() {
            stats.record_failure();
        }
    }

    fn store_converted(&self, raw: &RawPortalFrame<'_>) {
        let conversion_start = Instant::now();
        let destination = self
            .spare
            .lock()
            .map(|mut pool| pool.take(raw.converted_len()))
            .unwrap_or_default();
        let converted = convert_into_capture_frame(raw, destination);
        let conversion_micros =
            u64::try_from(conversion_start.elapsed().as_micros()).unwrap_or(u64::MAX);
        match converted {
            Ok(frame) => {
                let now = Instant::now();
                let replaced = self
                    .latest
                    .lock()
                    .ok()
                    .and_then(|mut slot| slot.store(frame, now));
                if let Ok(mut rates) = self.rates.lock() {
                    rates.0.record(now);
                }
                if let Some(replaced) = replaced {
                    self.recycle(replaced);
                }
                if let Ok(mut stats) = self.stats.lock() {
                    stats.record_success(conversion_micros);
                }
                // The loop wakes here — at the delivery moment.
                self.signal.bump();
            }
            Err(reason) => self.note_buffer_error(reason),
        }
    }
}

/// User data handed to the PipeWire listener callbacks.
struct PumpUserData {
    shared: Arc<StreamShared>,
    /// Negotiated video format, set by `param_changed` before the first
    /// buffer is processed.
    negotiated: Option<(PortalFrameFormat, u32, u32)>,
    /// Consecutive buffers without mapped memory.
    unmapped: UnmappedBufferCounter,
    /// The serialized mappable-memory buffers request; must outlive the
    /// `update_params` call that hands it to PipeWire.
    buffers_pod: Vec<u8>,
}

/// Serialize the buffers request: `dataType` = MemFd | MemPtr,
/// the memory d4lc maps and reads directly. A compositor that can only
/// deliver DMA-BUF then fails negotiation or delivers unmapped buffers,
/// which the pump reports by name instead of dropping them silently.
fn serialize_buffers_pod() -> Result<Vec<u8>, String> {
    use pipewire::spa::buffer::DataType;
    use pipewire::spa::pod::{ChoiceValue, Object, Property, PropertyFlags};
    use pipewire::spa::utils::{Choice, ChoiceEnum, ChoiceFlags};

    let data_types = (1_i32 << DataType::MemFd.as_raw()) | (1_i32 << DataType::MemPtr.as_raw());
    let object = Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![Property {
            key: pipewire::spa::sys::SPA_PARAM_BUFFERS_dataType,
            flags: PropertyFlags::empty(),
            value: Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Flags {
                    default: data_types,
                    flags: Vec::new(),
                },
            ))),
        }],
    };
    let (buffer, _) = pw::spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map_err(|error| format!("failed to serialize the buffers pod: {error}"))?;
    Ok(buffer.into_inner())
}

/// Serialize one format negotiation pod: raw video, 8-bit BGRx/BGRA only,
/// at the portal-reported size when known. `capped` offers a framerate
/// range `0/1 .. 60/1` (variable by default)
/// plus `VideoMaxFramerate` up to `60/1`, so the compositor sends at most
/// 60 fps. The uncapped offer keeps the previous `0/1 .. 1000/1` range
/// without a maximum; it is the fallback for a compositor that cannot
/// honor the cap, which the local throttle then covers. The returned
/// bytes must stay alive for as long as the `Pod` parsed from them.
fn serialize_format_pod(hint_size: Option<(u32, u32)>, capped: bool) -> Result<Vec<u8>, String> {
    let (default_width, default_height) = hint_size.unwrap_or((1920, 1080));
    let (min_width, min_height) = hint_size.unwrap_or((1, 1));
    let (max_width, max_height) = hint_size.unwrap_or((7680, 4320));
    let max_rate = if capped { MAX_STREAM_FPS } else { 1000 };
    let mut object = pw::spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pw::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: default_width,
                height: default_height,
            },
            Rectangle {
                width: min_width,
                height: min_height,
            },
            Rectangle {
                width: max_width,
                height: max_height,
            }
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: max_rate,
                denom: 1
            }
        ),
    );
    if capped {
        object.properties.push(pw::spa::pod::property!(
            FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction {
                num: MAX_STREAM_FPS,
                denom: 1
            },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: MAX_STREAM_FPS,
                denom: 1
            }
        ));
    }
    let (buffer, _) = pw::spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map_err(|error| format!("failed to serialize the format negotiation pod: {error}"))?;
    Ok(buffer.into_inner())
}

/// Pump thread body: returns once the stream setup failed or the stop
/// flag was observed. `ready` signals either the connected stream or the
/// setup error, so the constructor can fail fast with a structured
/// reason.
pub(super) fn run_stream_thread(
    fd: OwnedFd,
    node_id: u32,
    hint_size: Option<(u32, u32)>,
    shared: Arc<StreamShared>,
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    if let Err(reason) = run_stream(fd, node_id, hint_size, shared, stop, &ready) {
        let _ = ready.send(Err(reason));
    }
}

fn run_stream(
    fd: OwnedFd,
    node_id: u32,
    hint_size: Option<(u32, u32)>,
    shared: Arc<StreamShared>,
    stop: Arc<AtomicBool>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let mainloop = pw::main_loop::MainLoopBox::new(None)
        .map_err(|error| format!("failed to create the PipeWire main loop: {error}"))?;
    let context = pw::context::ContextBox::new(mainloop.loop_(), None)
        .map_err(|error| format!("failed to create the PipeWire context: {error}"))?;
    // Connects to the portal-provided remote; only the screencast stream
    // nodes are visible on this connection.
    let core = context
        .connect_fd(fd, None)
        .map_err(|error| format!("failed to connect to the PipeWire remote: {error}"))?;

    let stream = pw::stream::StreamBox::new(
        &core,
        "d4lc-capture",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|error| format!("failed to create the PipeWire stream: {error}"))?;

    let user_data = PumpUserData {
        shared: shared.clone(),
        negotiated: None,
        unmapped: UnmappedBufferCounter::default(),
        buffers_pod: serialize_buffers_pod()?,
    };
    let listener = stream
        .add_local_listener_with_user_data(user_data)
        .param_changed(|stream, user_data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
                user_data
                    .shared
                    .note_failure("negotiated format param is not parseable".to_string());
                return;
            };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                user_data
                    .shared
                    .note_failure("negotiated param is not a raw video format".to_string());
                return;
            }
            let mut info = VideoInfoRaw::default();
            if info.parse(param).is_err() {
                user_data
                    .shared
                    .note_failure("negotiated video format is not parseable".to_string());
                return;
            }
            let format = match info.format() {
                VideoFormat::BGRx => PortalFrameFormat::Bgrx,
                VideoFormat::BGRA => PortalFrameFormat::Bgra,
                other => {
                    user_data.shared.note_failure(format!(
                        "negotiated video format {other:?} is not the pinned 8-bit BGRx/BGRA"
                    ));
                    return;
                }
            };
            user_data.negotiated = Some((format, info.size().width, info.size().height));
            let framerate = info.framerate();
            let max_framerate = info.max_framerate();
            user_data.shared.note_negotiated(NegotiatedRate {
                framerate: (framerate.num, framerate.denom),
                max_framerate: (max_framerate.num, max_framerate.denom),
                // The capped offer bounds the maximum at 60/1; no maximum,
                // or one above the cap, can only come from the uncapped
                // fallback offer.
                capped: max_framerate.num > 0
                    && u64::from(max_framerate.num)
                        <= u64::from(MAX_STREAM_FPS) * u64::from(max_framerate.denom.max(1)),
            });
            // With the format fixed, ask for mappable memory.
            match pw::spa::pod::Pod::from_bytes(&user_data.buffers_pod) {
                Some(pod) => {
                    if let Err(error) = stream.update_params(&mut [pod]) {
                        user_data.shared.note_buffer_error(format!(
                            "the mappable-memory buffers request was refused: {error}"
                        ));
                    }
                }
                None => user_data
                    .shared
                    .note_buffer_error("serialized buffers pod is incomplete".to_string()),
            }
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return; // No buffer this tick; not an error.
            };
            // A buffer that would only replace a young, untaken
            // frame is requeued (on drop) without converting it.
            if user_data.shared.skip_unconsumed() {
                return;
            }
            let Some((format, width, height)) = user_data.negotiated else {
                // Buffers can race the Format param; drop them quietly.
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let data = &mut datas[0];
            // Unmapped memory (DMA-BUF, which d4lc cannot import)
            // is checked first, so a DMA-BUF-only stream is named even when
            // its chunks carry no size.
            let data_type = data.type_();
            if data.data().is_none() {
                user_data.shared.note_buffer_error(format!(
                    "stream buffer memory is not mapped (data type {data_type:?}); mappable \
                     MemFd/MemPtr memory was requested"
                ));
                // A run of unmapped buffers is a named stream failure, not
                // an endless silent drop.
                if let Some(consecutive) = user_data.unmapped.record_unmapped() {
                    user_data.shared.note_failure(
                        WaylandCaptureError::MappableMemoryUnavailable {
                            consecutive,
                            data_type: format!("{data_type:?}"),
                        }
                        .to_string(),
                    );
                }
                return;
            }
            user_data.unmapped.record_mapped();
            // Compositors flag buffers without new image content this way:
            // KWin and Mutter send cursor-metadata-only updates (cursor mode
            // Metadata) as CORRUPTED and/or zero-sized chunks. They carry no
            // frame; the previous frame stays current.
            if data.chunk().flags().contains(ChunkFlags::CORRUPTED) || data.chunk().size() == 0 {
                return;
            }
            let row_bytes = usize::try_from(width).unwrap_or(0).saturating_mul(4);
            let chunk_stride = match usize::try_from(data.chunk().stride()).unwrap_or(0) {
                0 => row_bytes,
                stride => stride,
            };
            let chunk_offset = usize::try_from(data.chunk().offset()).unwrap_or(0);
            let chunk_size = usize::try_from(data.chunk().size()).unwrap_or(0);
            let Some(mapped) = data.data() else {
                return; // Checked above; unreachable.
            };
            if chunk_offset >= mapped.len() {
                user_data
                    .shared
                    .note_buffer_error("stream buffer chunk offset is out of bounds".to_string());
                return;
            }
            let region = &mapped[chunk_offset..];
            let region = &region[..region.len().min(chunk_size)];
            // Converted straight from the mapped memory, no
            // intermediate copy of the chunk.
            let frame = RawPortalFrame::new(width, height, chunk_stride, format, region);
            match frame {
                Ok(frame) => user_data.shared.store_converted(&frame),
                Err(reason) => user_data.shared.note_buffer_error(reason),
            }
        })
        .register()
        .map_err(|error| format!("failed to register the stream listener: {error}"))?;

    // The pods borrow their bytes; the bytes live until the end of this
    // function, so the negotiation params stay valid through `connect`.
    // The capped offer comes first; the uncapped one is the fallback.
    let capped_bytes = serialize_format_pod(hint_size, true)?;
    let uncapped_bytes = serialize_format_pod(hint_size, false)?;
    let capped_pod = pw::spa::pod::Pod::from_bytes(&capped_bytes)
        .ok_or_else(|| "serialized format pod is incomplete".to_string())?;
    let uncapped_pod = pw::spa::pod::Pod::from_bytes(&uncapped_bytes)
        .ok_or_else(|| "serialized format pod is incomplete".to_string())?;
    let mut params = [capped_pod, uncapped_pod];
    stream
        .connect(
            Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|error| format!("failed to connect the PipeWire stream: {error}"))?;

    let _ = ready.send(Ok(()));

    // The mainloop must run for the stream to deliver buffers; the stop
    // flag is checked once per tick so teardown stays prompt without any
    // cross-thread loop control.
    while !stop.load(Ordering::Relaxed) {
        mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(LOOP_TICK));
    }
    // Explicit drop order: listener, stream, core, context, mainloop.
    drop(listener);
    drop(stream);
    drop(core);
    drop(context);
    drop(mainloop);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use pipewire::spa::pod::deserialize::PodDeserializer;
    use pipewire::spa::pod::{ChoiceValue, Property};
    use pipewire::spa::utils::{Choice, ChoiceEnum, Fraction};

    fn properties(bytes: &[u8]) -> Vec<Property> {
        let (_, value) = PodDeserializer::deserialize_any_from(bytes)
            .expect("the serialized pod must deserialize");
        let Value::Object(object) = value else {
            panic!("the format pod must be an object");
        };
        object.properties
    }

    fn fraction_range(
        key: FormatProperties,
        props: &[Property],
    ) -> Option<(Fraction, Fraction, Fraction)> {
        props
            .iter()
            .find(|property| property.key == key.as_raw())
            .and_then(|property| match &property.value {
                Value::Choice(ChoiceValue::Fraction(Choice(
                    _,
                    ChoiceEnum::Range { default, min, max },
                ))) => Some((*default, *min, *max)),
                _ => None,
            })
    }

    #[test]
    fn capped_format_pod_requests_at_most_sixty_fps() {
        let bytes = serialize_format_pod(Some((2560, 1440)), true).expect("capped pod");
        let props = properties(&bytes);
        let sixty = Fraction { num: 60, denom: 1 };
        let zero = Fraction { num: 0, denom: 1 };
        assert_eq!(
            fraction_range(FormatProperties::VideoFramerate, &props),
            Some((zero, zero, sixty)),
            "variable by default, at most 60/1"
        );
        assert_eq!(
            fraction_range(FormatProperties::VideoMaxFramerate, &props),
            Some((sixty, zero, sixty)),
            "the max framerate asks for 60/1"
        );
    }

    #[test]
    fn buffers_pod_requests_mappable_memory_only() {
        use pipewire::spa::buffer::DataType;

        let bytes = serialize_buffers_pod().expect("buffers pod");
        let (_, value) = PodDeserializer::deserialize_any_from(&bytes)
            .expect("the serialized pod must deserialize");
        let Value::Object(object) = value else {
            panic!("the buffers pod must be an object");
        };
        assert_eq!(object.type_, SpaTypes::ObjectParamBuffers.as_raw());
        assert_eq!(object.id, ParamType::Buffers.as_raw());
        let data_type = object
            .properties
            .iter()
            .find(|property| property.key == pipewire::spa::sys::SPA_PARAM_BUFFERS_dataType)
            .expect("the dataType property");
        let Value::Choice(ChoiceValue::Int(Choice(_, ChoiceEnum::Flags { default, .. }))) =
            &data_type.value
        else {
            panic!("dataType must be a flags choice: {:?}", data_type.value);
        };
        let mem_fd = 1 << DataType::MemFd.as_raw();
        let mem_ptr = 1 << DataType::MemPtr.as_raw();
        let dma_buf = 1 << DataType::DmaBuf.as_raw();
        assert_eq!(*default, mem_fd | mem_ptr);
        assert_eq!(*default & dma_buf, 0, "DMA-BUF is not requested");
    }

    #[test]
    fn uncapped_fallback_pod_keeps_the_previous_offer() {
        let bytes = serialize_format_pod(None, false).expect("uncapped pod");
        let props = properties(&bytes);
        let zero = Fraction { num: 0, denom: 1 };
        assert_eq!(
            fraction_range(FormatProperties::VideoFramerate, &props),
            Some((
                zero,
                zero,
                Fraction {
                    num: 1000,
                    denom: 1
                }
            ))
        );
        assert!(
            fraction_range(FormatProperties::VideoMaxFramerate, &props).is_none(),
            "the fallback carries no maximum"
        );
    }
}
