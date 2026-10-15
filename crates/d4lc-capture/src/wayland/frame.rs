//! Pure ScreenCast frame conversion: portal pixel formats → `CaptureFrame`.
//!
//! The PipeWire stream thread negotiates an 8-bit BGRx/BGRA SHM format and
//! hands each received buffer to [`convert_to_capture_frame`] as a
//! [`RawPortalFrame`]. Conversion is a pure function so the pixel mapping,
//! stride normalization, and alpha guarantee are unit-tested without any
//! Wayland session, and the per-frame conversion cost is measurable against
//! the X11 hot path.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use d4lc_core::{CaptureFrame, Rect};

use crate::{CaptureRequest, CaptureScopeResult, RoiOrigin};

/// Pixel layout of a negotiated ScreenCast SHM stream.
///
/// The portal/PipeWire negotiation is pinned to these two 8-bit packed
/// formats (the wire order of `BGRx` on little-endian SHM is byte-wise
/// B, G, R, padding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalFrameFormat {
    /// 8-bit BGRx: byte order B, G, R, undefined padding.
    Bgrx,
    /// 8-bit BGRA: byte order B, G, R, A.
    Bgra,
}

/// One raw frame exactly as received from the PipeWire stream buffer,
/// borrowed from the mapped buffer memory (no intermediate copy).
///
/// `stride` is the source row pitch in bytes and may exceed
/// `width * 4`; [`convert_to_capture_frame`] normalizes rows to the
/// tight layout `CaptureFrame` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPortalFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PortalFrameFormat,
    pub bytes: &'a [u8],
}

impl<'a> RawPortalFrame<'a> {
    /// Validate and wrap a raw stream buffer region.
    ///
    /// # Errors
    ///
    /// Returns a structured error when the dimensions are empty, the
    /// stride cannot hold one row, or the buffer is shorter than
    /// `stride * height` (the converter must never read past the
    /// received frame region).
    pub fn new(
        width: u32,
        height: u32,
        stride: usize,
        format: PortalFrameFormat,
        bytes: &'a [u8],
    ) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err(format!(
                "portal frame dimensions {width}x{height} must be non-empty"
            ));
        }
        let row_bytes = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| "portal frame width overflowed row size".to_string())?;
        if stride < row_bytes {
            return Err(format!(
                "portal frame stride {stride} cannot hold a {width} pixel row ({row_bytes} bytes)"
            ));
        }
        let required = stride.checked_mul(usize::try_from(height).unwrap_or(usize::MAX));
        let required = match required {
            Some(required) => required,
            None => return Err("portal frame buffer size overflowed".to_string()),
        };
        if bytes.len() < required {
            return Err(format!(
                "portal frame buffer holds {} bytes but stride {stride} x height {height} requires {required}",
                bytes.len()
            ));
        }
        Ok(Self {
            width,
            height,
            stride,
            format,
            bytes,
        })
    }

    /// Size in bytes of the tight BGRA frame this raw frame converts to.
    pub fn converted_len(&self) -> usize {
        usize::try_from(self.width)
            .unwrap_or(0)
            .saturating_mul(4)
            .saturating_mul(usize::try_from(self.height).unwrap_or(0))
    }
}

/// Convert one raw portal frame into the opaque BGRA `CaptureFrame` the
/// runtime pipeline consumes (the same tight-row layout the X11 paths
/// and `crop_frame` produce).
///
/// Rows are copied stride-aware, BGRx padding is replaced with alpha
/// 255, BGRA alpha is forced to 255 so downstream detection always sees
/// an opaque screen frame like the X11 captures.
///
/// # Errors
///
/// Returns a structured error when the raw frame failed its own
/// validation (empty dimensions, undersized buffer).
pub fn convert_to_capture_frame(raw: &RawPortalFrame<'_>) -> Result<CaptureFrame, String> {
    convert_into_capture_frame(raw, Vec::new())
}

/// [`convert_to_capture_frame`] into a caller-provided pixel buffer:
/// one pass per row straight from the mapped stream memory, the
/// alpha force fused into the copy. `destination` is reused as-is when
/// its length is exactly the converted size (every byte is overwritten);
/// any other buffer is replaced by a fresh allocation, so a stale buffer
/// of a different size is never written into.
///
/// # Errors
///
/// Same as [`convert_to_capture_frame`].
pub fn convert_into_capture_frame(
    raw: &RawPortalFrame<'_>,
    mut destination: Vec<u8>,
) -> Result<CaptureFrame, String> {
    // Re-validate: converters must fail closed on any malformed input.
    let row_bytes = usize::try_from(raw.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| "portal frame width overflowed row size".to_string())?;
    if raw.width == 0 || raw.height == 0 || raw.stride < row_bytes {
        return Err(format!(
            "portal frame {}x{} with stride {} is not convertible",
            raw.width, raw.height, raw.stride
        ));
    }
    let height = usize::try_from(raw.height).map_err(|error| error.to_string())?;
    let required = raw
        .stride
        .checked_mul(height)
        .ok_or_else(|| "portal frame buffer size overflowed".to_string())?;
    if raw.bytes.len() < required {
        return Err(format!(
            "portal frame buffer holds {} bytes but {} are required",
            raw.bytes.len(),
            required
        ));
    }
    let converted_len = row_bytes
        .checked_mul(height)
        .ok_or_else(|| "portal frame size overflowed".to_string())?;
    if destination.len() != converted_len {
        destination = vec![0u8; converted_len];
    }

    const OPAQUE: u32 = u32::from_le_bytes([0, 0, 0, 255]);
    for (dst_row, src_row) in destination
        .chunks_exact_mut(row_bytes)
        .zip(raw.bytes.chunks_exact(raw.stride))
    {
        let (dst_pixels, _) = dst_row.as_chunks_mut::<4>();
        let (src_pixels, _) = src_row[..row_bytes].as_chunks::<4>();
        for (dst, src) in dst_pixels.iter_mut().zip(src_pixels) {
            // Copy B, G, R and force alpha (byte 3) to opaque in one store.
            *dst = (u32::from_le_bytes(*src) | OPAQUE).to_le_bytes();
        }
    }

    Ok(CaptureFrame {
        width: raw.width,
        height: raw.height,
        stride: row_bytes,
        pixels_bgra: destination,
    })
}

/// Most spare pixel buffers the stream keeps for reuse.
pub const FRAME_BUFFER_POOL_CAPACITY: usize = 2;

/// A small recycle pool of converted-frame pixel buffers: a
/// frame replaced in (or handed back to) the stream slot returns its
/// buffer here, and the next conversion reuses it instead of allocating.
#[derive(Debug, Default)]
pub struct FrameBufferPool {
    spare: Vec<Vec<u8>>,
}

impl FrameBufferPool {
    /// A buffer of exactly `len` bytes when one is pooled; otherwise an
    /// empty `Vec` (the converter allocates). A miss drops every pooled
    /// buffer: after a size change they can never fit again.
    pub fn take(&mut self, len: usize) -> Vec<u8> {
        match self.spare.iter().position(|buffer| buffer.len() == len) {
            Some(index) => self.spare.swap_remove(index),
            None => {
                self.spare.clear();
                Vec::new()
            }
        }
    }

    /// Return a buffer for reuse; beyond the capacity it is dropped.
    pub fn give(&mut self, buffer: Vec<u8>) {
        if self.spare.len() < FRAME_BUFFER_POOL_CAPACITY && !buffer.is_empty() {
            self.spare.push(buffer);
        }
    }

    /// Number of pooled buffers.
    pub fn len(&self) -> usize {
        self.spare.len()
    }

    /// Whether no buffer is pooled.
    pub fn is_empty(&self) -> bool {
        self.spare.is_empty()
    }
}

/// What a consumer's take from the [`LatestFrameSlot`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotTake {
    /// A frame delivered since the previous take, moved out of the slot
    /// (no pixel copy); `seq` is its delivery sequence number (from 1).
    Frame { seq: u64, frame: CaptureFrame },
    /// Frames were delivered, but none since the previous take.
    NoNewFrame { last_seq: u64 },
    /// The stream has not delivered any frame yet.
    NeverDelivered,
}

/// The newest-frame slot between the PipeWire pump and the consumer:
/// the pump stores, the consumer takes by move. Storing over an
/// untaken frame hands the replaced frame back for buffer recycling.
#[derive(Debug, Default)]
pub struct LatestFrameSlot {
    frame: Option<(u64, CaptureFrame)>,
    delivered_seq: u64,
    delivered_size: Option<(u32, u32)>,
    stored_at: Option<Instant>,
}

impl LatestFrameSlot {
    /// Store a newly converted frame at `now`; returns the untaken frame
    /// it replaced, if any.
    pub fn store(&mut self, frame: CaptureFrame, now: Instant) -> Option<CaptureFrame> {
        self.delivered_seq = self.delivered_seq.saturating_add(1);
        self.delivered_size = Some((frame.width, frame.height));
        self.stored_at = Some(now);
        self.frame
            .replace((self.delivered_seq, frame))
            .map(|(_, replaced)| replaced)
    }

    /// The producer-side throttle: skip converting a new buffer
    /// while the stored frame is younger than half of `target_interval`
    /// and still untaken. The consumer reads at most one frame per target
    /// interval, so such a buffer would only replace a frame nobody read.
    /// This only engages for compositors delivering faster than the
    /// requested maximum rate; a stream at or below it never skips.
    pub fn skip_unconsumed(&self, now: Instant, target_interval: Duration) -> bool {
        self.frame.is_some()
            && self
                .stored_at
                .is_some_and(|at| now.saturating_duration_since(at) < target_interval / 2)
    }

    /// Take the frame delivered since the previous take.
    pub fn take(&mut self) -> SlotTake {
        match self.frame.take() {
            Some((seq, frame)) => SlotTake::Frame { seq, frame },
            None if self.delivered_seq == 0 => SlotTake::NeverDelivered,
            None => SlotTake::NoNewFrame {
                last_seq: self.delivered_seq,
            },
        }
    }

    /// The untaken frame, if any, without taking it.
    pub fn peek(&self) -> Option<&CaptureFrame> {
        self.frame.as_ref().map(|(_, frame)| frame)
    }

    /// Delivery sequence number of the newest stored frame (0 = none yet).
    pub fn delivered_seq(&self) -> u64 {
        self.delivered_seq
    }

    /// Pixel size (width, height) of the newest delivered frame, whether
    /// or not it was taken since; `None` before the first delivery.
    pub fn delivered_size(&self) -> Option<(u32, u32)> {
        self.delivered_size
    }
}

/// The highest frame rate d4lc asks the compositor for (at
/// most 60 fps, no host-specific number).
pub const MAX_STREAM_FPS: u32 = 60;

/// The consumer's target interval at [`MAX_STREAM_FPS`]; the producer-side
/// throttle compares against half of it.
pub const STREAM_TARGET_INTERVAL: Duration =
    Duration::from_nanos(1_000_000_000 / MAX_STREAM_FPS as u64);

/// Events per second over a sliding one-second window (delivered and
/// consumed stream frames).
#[derive(Debug, Default)]
pub struct RateMeter {
    events: VecDeque<Instant>,
}

impl RateMeter {
    /// Upper bound of remembered events (far above any real stream rate).
    const CAPACITY: usize = 1024;
    const WINDOW: Duration = Duration::from_secs(1);

    /// Record one event at `now`.
    pub fn record(&mut self, now: Instant) {
        self.prune(now);
        if self.events.len() == Self::CAPACITY {
            self.events.pop_front();
        }
        self.events.push_back(now);
    }

    /// Events within the last second before `now`.
    pub fn per_second(&mut self, now: Instant) -> usize {
        self.prune(now);
        self.events.len()
    }

    fn prune(&mut self, now: Instant) {
        while self
            .events
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= Self::WINDOW)
        {
            self.events.pop_front();
        }
    }
}

/// The stream rates the PipeWire negotiation settled on, as fractions
/// `(num, denom)`; `0/1` is a variable rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedRate {
    pub framerate: (u32, u32),
    pub max_framerate: (u32, u32),
    /// Whether the compositor accepted the capped offer (maximum rate at
    /// most [`MAX_STREAM_FPS`]) rather than the uncapped fallback offer.
    pub capped: bool,
}

impl NegotiatedRate {
    /// The negotiated maximum rate in Hz: the max framerate when set, else
    /// a fixed framerate; `None` for a variable rate without a maximum.
    pub fn max_hz(&self) -> Option<f64> {
        [self.max_framerate, self.framerate]
            .into_iter()
            .find(|(num, denom)| *num > 0 && *denom > 0)
            .map(|(num, denom)| f64::from(num) / f64::from(denom))
    }
}

/// The stream-rate line for the Debug screen's Capture engine row
/// negotiated rate, delivered and consumed frames per second,
/// and the throttle's skips.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamRateStatus {
    pub negotiated: Option<NegotiatedRate>,
    pub delivered_fps: usize,
    pub consumed_fps: usize,
    pub skipped_unconsumed: u64,
}

impl StreamRateStatus {
    pub fn render(&self) -> String {
        let negotiated = match &self.negotiated {
            None => "stream rate not negotiated yet".to_string(),
            Some(rate) => {
                let (num, denom) = rate.framerate;
                let framerate = if num == 0 {
                    "variable".to_string()
                } else {
                    format!("{num}/{denom}")
                };
                let (max_num, max_denom) = rate.max_framerate;
                let max = if max_num == 0 {
                    "no maximum".to_string()
                } else {
                    format!("max {max_num}/{max_denom}")
                };
                let offer = if rate.capped {
                    format!("{MAX_STREAM_FPS} fps cap accepted")
                } else {
                    format!(
                        "{MAX_STREAM_FPS} fps cap refused, uncapped fallback; local throttle applies"
                    )
                };
                format!("stream {framerate} fps, {max} ({offer})")
            }
        };
        format!(
            "{negotiated}; delivered {} fps, consumed {} fps, {} unconsumed frames skipped",
            self.delivered_fps, self.consumed_fps, self.skipped_unconsumed
        )
    }
}

/// Consecutive unmapped stream buffers after which the stream fails by
/// name: about half a second of a 60 fps stream, far beyond a
/// transient hiccup.
pub const UNMAPPED_BUFFER_LIMIT: u32 = 30;

/// Counts consecutive stream buffers without mapped memory.
#[derive(Debug, Default)]
pub struct UnmappedBufferCounter {
    consecutive: u32,
}

impl UnmappedBufferCounter {
    /// Record one unmapped buffer; returns the count exactly once, when it
    /// reaches [`UNMAPPED_BUFFER_LIMIT`] — the moment to fail by name.
    pub fn record_unmapped(&mut self) -> Option<u32> {
        self.consecutive = self.consecutive.saturating_add(1);
        (self.consecutive == UNMAPPED_BUFFER_LIMIT).then_some(self.consecutive)
    }

    /// A mapped buffer ends the run.
    pub fn record_mapped(&mut self) {
        self.consecutive = 0;
    }
}

/// Per-frame conversion cost accounting for the ScreenCast path.
///
/// The conversion cost is recorded per frame so it can be compared against
/// the X11 hot path; live runs carry the totals into `debug-output/`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameConversionStats {
    pub frames_received: u64,
    pub frames_converted: u64,
    pub frames_failed: u64,
    pub last_conversion_micros: u64,
    pub total_conversion_micros: u64,
    /// Buffers requeued unconverted by the producer-side throttle:
    /// the stored frame was younger than half the target interval
    /// and still untaken.
    pub frames_skipped_unconsumed: u64,
}

impl FrameConversionStats {
    /// Record one buffer the throttle requeued without converting.
    pub fn record_skipped_unconsumed(&mut self) {
        self.frames_received += 1;
        self.frames_skipped_unconsumed += 1;
    }

    /// Record one frame that arrived and converted successfully within
    /// `conversion_micros` microseconds.
    pub fn record_success(&mut self, conversion_micros: u64) {
        self.frames_received += 1;
        self.frames_converted += 1;
        self.last_conversion_micros = conversion_micros;
        self.total_conversion_micros = self
            .total_conversion_micros
            .saturating_add(conversion_micros);
    }

    /// Record one frame that arrived but could not be converted.
    pub fn record_failure(&mut self) {
        self.frames_received += 1;
        self.frames_failed += 1;
    }
}

/// How one [`CaptureRequest`] maps onto a full portal stream frame.
///
/// Wayland has no window geometry: `WindowRelativeRoi` addresses the
/// stream space directly (the compositor-dialog selection fills it),
/// `ScreenGlobalRoi` is translated by the stream's global position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturePlan {
    /// Region to crop from the full frame (`None` = keep the frame).
    pub crop_region: Option<Rect>,
    pub requested_capture_rect: Rect,
    pub actual_capture_rect: Rect,
    pub roi_origin: RoiOrigin,
    pub capture_scope: CaptureScopeResult,
}

/// Plan the request against the frame rect (mirrors the synthetic
/// source's mapping so ROI semantics stay identical across sources).
///
/// The caller crops the full frame with `crop_frame` when
/// [`CapturePlan::crop_region`] is `Some`.
pub fn plan_capture(
    request: CaptureRequest,
    stream_origin: (i32, i32),
    frame_rect: Rect,
) -> CapturePlan {
    let full = frame_rect;
    match request {
        CaptureRequest::Full => CapturePlan {
            crop_region: None,
            requested_capture_rect: full,
            actual_capture_rect: full,
            roi_origin: RoiOrigin::FullFrame,
            capture_scope: CaptureScopeResult::FullFrame,
        },
        CaptureRequest::WindowRelativeRoi(rect) => CapturePlan {
            crop_region: Some(rect),
            requested_capture_rect: rect,
            actual_capture_rect: rect,
            roi_origin: RoiOrigin::WindowRelative,
            capture_scope: CaptureScopeResult::Roi,
        },
        CaptureRequest::ScreenGlobalRoi(rect) => CapturePlan {
            crop_region: Some(Rect {
                x: rect.x - stream_origin.0,
                y: rect.y - stream_origin.1,
                width: rect.width,
                height: rect.height,
            }),
            requested_capture_rect: rect,
            actual_capture_rect: rect,
            roi_origin: RoiOrigin::ScreenGlobal,
            capture_scope: CaptureScopeResult::Roi,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crop_frame;

    // ── BGRx → BGRA pixel mapping ────────────────────────────────

    #[test]
    fn bgrx_conversion_reorders_to_bgra_and_forces_opaque_alpha() {
        // Two BGRx pixels: blue, then red. The wire padding byte (X)
        // is undefined input and must come out as opaque alpha 255.
        let raw = RawPortalFrame::new(
            2,
            1,
            8,
            PortalFrameFormat::Bgrx,
            &[10, 20, 30, 0, 40, 50, 60, 77],
        )
        .unwrap_or_else(|error| panic!("raw frame should validate: {error}"));

        let frame = convert_to_capture_frame(&raw)
            .unwrap_or_else(|error| panic!("conversion should succeed: {error}"));

        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.stride, 8);
        assert_eq!(frame.pixels_bgra, vec![10, 20, 30, 255, 40, 50, 60, 255]);
    }

    // ── BGRA passthrough + stride normalization ──────────────────

    #[test]
    fn bgra_conversion_preserves_channels_and_normalizes_stride_padding() {
        // One 2x2 frame with a padded stride (12 bytes per row for a
        // 8-byte row): the padding column must not leak into rows.
        let raw = RawPortalFrame::new(
            2,
            2,
            12,
            PortalFrameFormat::Bgra,
            &[
                1, 2, 3, 255, 4, 5, 6, 255, 9, 9, 9, 9, // row 0 (+ padding)
                7, 8, 9, 255, 10, 11, 12, 255, 9, 9, 9, 9, // row 1 (+ padding)
            ],
        )
        .unwrap_or_else(|error| panic!("raw frame should validate: {error}"));

        let frame = convert_to_capture_frame(&raw)
            .unwrap_or_else(|error| panic!("conversion should succeed: {error}"));

        assert_eq!(frame.stride, 8, "output rows must be tight");
        assert_eq!(
            frame.pixels_bgra,
            vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }

    // ── Fail-closed on malformed frames ──────────────────────────

    #[test]
    fn undersized_or_malformed_frames_are_rejected_with_structured_errors() {
        // Buffer shorter than stride x height must be refused — the
        // converter never reads past the received region.
        let raw = RawPortalFrame::new(2, 2, 8, PortalFrameFormat::Bgrx, &[0; 12]);
        assert!(raw.is_err(), "12 bytes cannot hold a 2x2 BGRx frame");

        // Empty dimensions are refused.
        let raw = RawPortalFrame::new(0, 2, 8, PortalFrameFormat::Bgrx, &[0; 16]);
        assert!(raw.is_err());

        // A stride smaller than one pixel row is refused.
        let raw = RawPortalFrame::new(4, 2, 8, PortalFrameFormat::Bgra, &[0; 16 * 2]);
        assert!(raw.is_err());

        // Conversion itself fails closed on a frame whose stored buffer
        // no longer matches its geometry (defensive re-validation).
        let bytes = [0u8; 16];
        let mut raw = RawPortalFrame::new(2, 1, 8, PortalFrameFormat::Bgrx, &bytes)
            .unwrap_or_else(|error| panic!("raw frame should validate: {error}"));
        raw.bytes = &bytes[..4];
        let Err(error) = convert_to_capture_frame(&raw) else {
            panic!("truncated frame must be refused");
        };
        assert!(
            error.contains("bytes"),
            "structured error must name the size mismatch: {error}"
        );
    }

    // ── ROI planning from full stream frames ─────────────────────

    fn stream_frame_rect() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn full_request_keeps_the_whole_stream_frame() {
        let plan = plan_capture(CaptureRequest::Full, (0, 0), stream_frame_rect());
        assert_eq!(plan.crop_region, None);
        assert_eq!(plan.requested_capture_rect, stream_frame_rect());
        assert_eq!(plan.actual_capture_rect, stream_frame_rect());
        assert_eq!(plan.roi_origin, RoiOrigin::FullFrame);
        assert_eq!(plan.capture_scope, CaptureScopeResult::FullFrame);
    }

    #[test]
    fn window_relative_roi_addresses_the_stream_space_directly() {
        // No window geometry exists on Wayland: the ROI is stream
        // space, untranslated.
        let roi = Rect {
            x: 480,
            y: 270,
            width: 960,
            height: 540,
        };
        let plan = plan_capture(
            CaptureRequest::WindowRelativeRoi(roi),
            (0, 0),
            stream_frame_rect(),
        );
        assert_eq!(plan.crop_region, Some(roi));
        assert_eq!(plan.roi_origin, RoiOrigin::WindowRelative);
        assert_eq!(plan.capture_scope, CaptureScopeResult::Roi);
    }

    #[test]
    fn screen_global_roi_is_translated_by_the_stream_origin() {
        // A monitor stream at global position (1920, 0) (second screen):
        // a global ROI must be translated into stream coordinates.
        let roi = Rect {
            x: 2400,
            y: 100,
            width: 400,
            height: 300,
        };
        let plan = plan_capture(
            CaptureRequest::ScreenGlobalRoi(roi),
            (1920, 0),
            stream_frame_rect(),
        );
        assert_eq!(
            plan.crop_region,
            Some(Rect {
                x: 480,
                y: 100,
                width: 400,
                height: 300,
            })
        );
        assert_eq!(plan.requested_capture_rect, roi);
        assert_eq!(plan.roi_origin, RoiOrigin::ScreenGlobal);
    }

    #[test]
    fn planned_roi_outside_the_frame_is_refused_by_the_crop() {
        // The planner maps; the crop is the guard rail: an ROI outside
        // the stream frame fails closed there instead of producing a
        // partial frame.
        let roi = Rect {
            x: 1900,
            y: 0,
            width: 100,
            height: 100,
        };
        let plan = plan_capture(
            CaptureRequest::ScreenGlobalRoi(roi),
            (0, 0),
            stream_frame_rect(),
        );
        let Some(crop_region) = plan.crop_region else {
            panic!("global roi plans a crop");
        };
        let bytes = vec![0; 1920 * 1080 * 4];
        let frame = RawPortalFrame::new(1920, 1080, 1920 * 4, PortalFrameFormat::Bgra, &bytes)
            .unwrap_or_else(|error| panic!("raw frame should validate: {error}"));
        let converted = convert_to_capture_frame(&frame)
            .unwrap_or_else(|error| panic!("conversion should succeed: {error}"));
        let Err(error) = crop_frame(&converted, crop_region) else {
            panic!("an ROI past the frame bounds must be refused");
        };
        assert!(
            error.contains("exceeds captured frame"),
            "structured crop error must name the bounds: {error}"
        );
    }

    // ── Single-copy conversion ────────────────────────

    /// The pre-2.1 two-copy path, kept as the test oracle: the mapped
    /// chunk region was copied into an owned `Vec` first, then rows were
    /// copied into a fresh frame and alpha forced in a second pass.
    fn two_copy_oracle(width: u32, height: u32, stride: usize, region: &[u8]) -> CaptureFrame {
        let owned = region.to_vec();
        let row_bytes = width as usize * 4;
        let height = height as usize;
        let mut pixels_bgra = vec![0u8; row_bytes * height];
        for row in 0..height {
            let src_row = &owned[row * stride..row * stride + row_bytes];
            let dst_row = &mut pixels_bgra[row * row_bytes..(row + 1) * row_bytes];
            dst_row.copy_from_slice(src_row);
            for alpha in dst_row.iter_mut().skip(3).step_by(4) {
                *alpha = 255;
            }
        }
        CaptureFrame {
            width,
            height: height as u32,
            stride: row_bytes,
            pixels_bgra,
        }
    }

    #[test]
    fn borrowed_conversion_matches_the_two_copy_oracle_with_padding_and_offset() {
        // A mapped buffer with a chunk offset prefix, a padded stride and
        // trailing bytes past the chunk; pixel bytes cover every value and
        // varied alpha/padding bytes.
        let (width, height, stride, offset) = (7u32, 5u32, 7 * 4 + 12, 20usize);
        let chunk_size = stride * height as usize;
        let mapped: Vec<u8> = (0..offset + chunk_size + 9)
            .map(|index| (index * 37 + 11) as u8)
            .collect();
        let region = &mapped[offset..offset + chunk_size];
        let expected = two_copy_oracle(width, height, stride, region);

        for format in [PortalFrameFormat::Bgrx, PortalFrameFormat::Bgra] {
            let raw = RawPortalFrame::new(width, height, stride, format, region)
                .unwrap_or_else(|error| panic!("raw frame should validate: {error}"));
            // Fresh destination, and a reused same-size buffer full of
            // stale bytes: both must be byte-identical to the oracle.
            let fresh = convert_to_capture_frame(&raw)
                .unwrap_or_else(|error| panic!("conversion should succeed: {error}"));
            assert_eq!(fresh, expected);
            let stale = vec![0xA5u8; raw.converted_len()];
            let reused = convert_into_capture_frame(&raw, stale)
                .unwrap_or_else(|error| panic!("conversion should succeed: {error}"));
            assert_eq!(reused, expected);
            // A wrong-size destination is replaced, never partly written.
            let wrong = vec![0x5Au8; raw.converted_len() + 4];
            let replaced = convert_into_capture_frame(&raw, wrong)
                .unwrap_or_else(|error| panic!("conversion should succeed: {error}"));
            assert_eq!(replaced, expected);
        }
    }

    #[test]
    fn buffer_pool_never_reuses_a_buffer_of_the_wrong_size() {
        let mut pool = FrameBufferPool::default();
        assert!(pool.take(16).is_empty(), "an empty pool hands out nothing");

        pool.give(vec![1; 16]);
        pool.give(vec![2; 16]);
        pool.give(vec![3; 16]);
        assert_eq!(
            pool.len(),
            FRAME_BUFFER_POOL_CAPACITY,
            "capacity bounds the pool"
        );

        let reused = pool.take(16);
        assert_eq!(reused.len(), 16, "a same-size buffer is reused");
        assert_eq!(pool.len(), 1);

        // Size change: the pooled 16-byte buffer must not be handed out
        // for a 32-byte frame, and it is dropped (it can never fit again).
        assert!(pool.take(32).is_empty());
        assert!(pool.is_empty());

        pool.give(Vec::new());
        assert!(pool.is_empty(), "an empty buffer is not worth pooling");
    }

    // ── Frame handover ────────────────────────────────

    fn solid_frame(value: u8) -> CaptureFrame {
        CaptureFrame {
            width: 1,
            height: 1,
            stride: 4,
            pixels_bgra: vec![value, value, value, 255],
        }
    }

    #[test]
    fn latest_frame_slot_moves_frames_out_and_names_repeats() {
        let now = Instant::now();
        let mut slot = LatestFrameSlot::default();
        assert_eq!(slot.take(), SlotTake::NeverDelivered);

        assert_eq!(slot.store(solid_frame(1), now), None);
        let replaced = slot.store(solid_frame(2), now);
        assert_eq!(
            replaced,
            Some(solid_frame(1)),
            "an untaken frame is handed back for recycling"
        );
        assert_eq!(slot.peek(), Some(&solid_frame(2)));
        assert_eq!(
            slot.take(),
            SlotTake::Frame {
                seq: 2,
                frame: solid_frame(2)
            }
        );
        assert_eq!(slot.peek(), None, "the consumer took the frame");
        assert_eq!(slot.take(), SlotTake::NoNewFrame { last_seq: 2 });
        assert_eq!(slot.take(), SlotTake::NoNewFrame { last_seq: 2 });

        assert_eq!(slot.store(solid_frame(3), now), None);
        assert_eq!(
            slot.take(),
            SlotTake::Frame {
                seq: 3,
                frame: solid_frame(3)
            }
        );
        assert_eq!(slot.delivered_seq(), 3);
    }

    /// The delivered size follows the newest frame, also once it was taken
    /// (a settings reload reads it while the loop has consumed the frame).
    #[test]
    fn latest_frame_slot_reports_the_newest_delivered_size_after_a_take() {
        let now = Instant::now();
        let mut slot = LatestFrameSlot::default();
        assert_eq!(slot.delivered_size(), None);
        slot.store(solid_frame(1), now);
        assert_eq!(slot.delivered_size(), Some((1, 1)));
        let _ = slot.take();
        slot.store(
            CaptureFrame {
                width: 2,
                height: 3,
                stride: 8,
                pixels_bgra: vec![0; 24],
            },
            now,
        );
        let _ = slot.take();
        assert_eq!(slot.delivered_size(), Some((2, 3)));
    }

    // ── Producer-side throttle and rates ──────────────

    #[test]
    fn throttle_skips_only_a_young_untaken_frame() {
        // Fake clock: offsets from one base instant.
        let base = Instant::now();
        let at = |micros: u64| base + Duration::from_micros(micros);
        let half = STREAM_TARGET_INTERVAL / 2;
        let mut slot = LatestFrameSlot::default();
        assert!(
            !slot.skip_unconsumed(at(0), STREAM_TARGET_INTERVAL),
            "an empty slot never skips"
        );

        slot.store(solid_frame(1), at(0));
        let young = base + half - Duration::from_nanos(1);
        let old = base + half;
        assert!(slot.skip_unconsumed(young, STREAM_TARGET_INTERVAL));
        assert!(
            !slot.skip_unconsumed(old, STREAM_TARGET_INTERVAL),
            "half the target interval or older is converted"
        );

        // Taken: the next buffer is always converted, however young.
        let _ = slot.take();
        assert!(!slot.skip_unconsumed(young, STREAM_TARGET_INTERVAL));

        // A stream at the requested rate never skips.
        let mut paced = LatestFrameSlot::default();
        let mut stats = FrameConversionStats::default();
        for tick in 0..120u32 {
            let now = base + STREAM_TARGET_INTERVAL * tick;
            if paced.skip_unconsumed(now, STREAM_TARGET_INTERVAL) {
                stats.record_skipped_unconsumed();
            } else {
                paced.store(solid_frame(0), now);
            }
        }
        assert_eq!(stats.frames_skipped_unconsumed, 0);

        // A 180 fps stream nobody reads: every buffer within half an
        // interval of the stored frame is skipped and counted.
        let mut fast = LatestFrameSlot::default();
        let mut stats = FrameConversionStats::default();
        let third = STREAM_TARGET_INTERVAL / 3;
        for tick in 0..8u32 {
            let now = base + third * tick;
            if fast.skip_unconsumed(now, STREAM_TARGET_INTERVAL) {
                stats.record_skipped_unconsumed();
            } else {
                fast.store(solid_frame(0), now);
            }
        }
        assert_eq!(stats.frames_skipped_unconsumed, 4);
        assert_eq!(stats.frames_received, 4);
    }

    #[test]
    fn rate_meter_counts_the_last_second() {
        let base = Instant::now();
        let mut meter = RateMeter::default();
        for tick in 0..90u32 {
            meter.record(base + Duration::from_millis(u64::from(tick) * 20));
        }
        // Events at 0..1780 ms; the window before 1790 ms holds 800..1780.
        assert_eq!(meter.per_second(base + Duration::from_millis(1790)), 50);
        assert_eq!(meter.per_second(base + Duration::from_secs(5)), 0);
    }

    #[test]
    fn stream_rate_status_names_the_negotiation_and_the_rates() {
        let capped = StreamRateStatus {
            negotiated: Some(NegotiatedRate {
                framerate: (0, 1),
                max_framerate: (60, 1),
                capped: true,
            }),
            delivered_fps: 58,
            consumed_fps: 57,
            skipped_unconsumed: 0,
        };
        assert_eq!(
            capped.render(),
            "stream variable fps, max 60/1 (60 fps cap accepted); delivered 58 fps, \
             consumed 57 fps, 0 unconsumed frames skipped"
        );
        assert_eq!(capped.negotiated.and_then(|rate| rate.max_hz()), Some(60.0));

        let fallback = StreamRateStatus {
            negotiated: Some(NegotiatedRate {
                framerate: (0, 1),
                max_framerate: (0, 1),
                capped: false,
            }),
            delivered_fps: 144,
            consumed_fps: 60,
            skipped_unconsumed: 84,
        };
        assert!(fallback.render().contains("cap refused, uncapped fallback"));
        assert_eq!(fallback.negotiated.and_then(|rate| rate.max_hz()), None);

        let pending = StreamRateStatus {
            negotiated: None,
            delivered_fps: 0,
            consumed_fps: 0,
            skipped_unconsumed: 0,
        };
        assert!(pending
            .render()
            .starts_with("stream rate not negotiated yet"));
    }

    // ── Mappable memory ───────────────────────────────

    #[test]
    fn unmapped_buffers_fail_by_name_once_after_the_limit() {
        let mut counter = UnmappedBufferCounter::default();
        for _ in 1..UNMAPPED_BUFFER_LIMIT {
            assert_eq!(counter.record_unmapped(), None);
        }
        // A mapped buffer in between resets the run.
        counter.record_mapped();
        for _ in 1..UNMAPPED_BUFFER_LIMIT {
            assert_eq!(counter.record_unmapped(), None);
        }
        assert_eq!(counter.record_unmapped(), Some(UNMAPPED_BUFFER_LIMIT));
        assert_eq!(counter.record_unmapped(), None, "the failure is named once");

        let error = super::super::WaylandCaptureError::MappableMemoryUnavailable {
            consecutive: UNMAPPED_BUFFER_LIMIT,
            data_type: "DataType::DmaBuf".to_string(),
        };
        let text = error.to_string();
        assert!(text.contains("DMA-BUF import is not supported"), "{text}");
        assert!(text.contains("30 consecutive"), "{text}");
        assert!(error.missing_piece().contains("MemFd/MemPtr"));
    }

    // ── Conversion cost accounting ───────────────────────────────

    #[test]
    fn conversion_stats_track_successes_and_failures() {
        let mut stats = FrameConversionStats::default();
        stats.record_success(42);
        stats.record_success(58);
        stats.record_failure();

        assert_eq!(stats.frames_received, 3);
        assert_eq!(stats.frames_converted, 2);
        assert_eq!(stats.frames_failed, 1);
        assert_eq!(stats.last_conversion_micros, 58);
        assert_eq!(stats.total_conversion_micros, 100);
    }
}
