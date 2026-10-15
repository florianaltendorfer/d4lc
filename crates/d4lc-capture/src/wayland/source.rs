//! The Wayland frame source: portal session + PipeWire pump + capture.
//!
//! Constructed once per runtime preparation on the Wayland display
//! backend path. On success the caller receives the source (to inject
//! into the runtime frame-source dispatch) plus the restore-token
//! rotation to persist. Window selection is user-driven in the
//! compositor dialog; ROI requests crop the full stream frame in
//! process, mirroring the synthetic source's request mapping.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::Screencast;
use ashpd::desktop::Session;
use zbus::block_on;

use d4lc_core::{CaptureFrame, Rect};

use crate::{
    crop_frame, CaptureBackend, CaptureMethod, CaptureRequest, CaptureResult, CaptureSource,
    OverlayWindowState, ScaleFactor,
};

use super::frame::{
    plan_capture, FrameConversionStats, NegotiatedRate, SlotTake, StreamRateStatus,
};
use super::pipewire_stream::{run_stream_thread, StreamShared};
use super::portal::AshpdScreenCastPortal;
use super::session::{
    PortalSessionGate, RestoreTokenState, RestoreTokenUpdate, SessionRequest, SessionStateMachine,
    SOURCE_TYPE_MONITOR, SOURCE_TYPE_WINDOW,
};
use super::{session, WaylandCaptureError};

/// How long to wait for the pump thread to report a connected stream.
const STREAM_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a new connect waits for the previous portal session's slot
/// before failing with [`WaylandCaptureError::SessionConflict`].
///
/// The wait covers the legitimate handoff — a just-stopped live loop tears
/// its source down within a frame tick (stop-flag check, portal session
/// close, pump join) — while a hopeless attempt (a live loop re-preparing
/// beside its own source, or a debug export beside a live loop) fails with
/// the named conflict instead of waiting forever or, worse, opening a
/// second portal session.
const SESSION_SLOT_WAIT: Duration = Duration::from_secs(2);

/// The process-wide portal-session slot: the pure [`PortalSessionGate`]
/// plus the condvar competing connects sleep on. Portal sessions are
/// exclusive per process (see the gate's docs); holding the slot through
/// the whole source lifetime makes that unbreakable for every
/// `WaylandFrameSource::connect` caller.
static PORTAL_SESSION_SLOT: OnceLock<PortalSessionSlot> = OnceLock::new();

struct PortalSessionSlot {
    gate: Mutex<PortalSessionGate>,
    released: Condvar,
}

impl PortalSessionSlot {
    fn new() -> Self {
        Self {
            gate: Mutex::new(PortalSessionGate::default()),
            released: Condvar::new(),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, PortalSessionGate>, WaylandCaptureError> {
        self.gate
            .lock()
            .map_err(|_| WaylandCaptureError::SessionConflict {
                detail: "the portal-session slot lock is poisoned".to_string(),
            })
    }
}

/// The exclusive portal-session slot of one live `WaylandFrameSource`.
/// Dropping the lease releases the slot and wakes the next waiter; the
/// source holds it as its last field, so the release happens after the
/// portal session is closed and the pump thread is torn down.
struct PortalSessionLease {
    slot: &'static PortalSessionSlot,
    generation: u64,
}

impl Drop for PortalSessionLease {
    fn drop(&mut self) {
        if let Ok(mut gate) = self.slot.lock() {
            gate.release(self.generation);
        }
        self.slot.released.notify_all();
    }
}

/// Take the process-wide portal-session slot, waiting up to
/// [`SESSION_SLOT_WAIT`] for a previous source to finish its teardown
/// (portal session closed, pump thread stopped).
///
/// # Errors
///
/// Returns [`WaylandCaptureError::SessionConflict`] when the slot is still
/// held after the wait — no second portal session is attempted.
fn acquire_portal_session_slot() -> Result<PortalSessionLease, WaylandCaptureError> {
    let slot = PORTAL_SESSION_SLOT.get_or_init(PortalSessionSlot::new);
    let deadline = Instant::now() + SESSION_SLOT_WAIT;
    let mut gate = slot.lock()?;
    loop {
        match gate.try_acquire() {
            Ok(generation) => return Ok(PortalSessionLease { slot, generation }),
            Err(_) if Instant::now() < deadline => {
                let timeout = deadline.saturating_duration_since(Instant::now());
                let (locked, _) = slot.released.wait_timeout(gate, timeout).map_err(|_| {
                    WaylandCaptureError::SessionConflict {
                        detail: "the portal-session slot lock is poisoned".to_string(),
                    }
                })?;
                gate = locked;
            }
            Err(_) => {
                return Err(WaylandCaptureError::SessionConflict {
                    detail: format!(
                        "the previous source kept the portal-session slot for over {SESSION_SLOT_WAIT:?}"
                    ),
                });
            }
        }
    }
}

/// Request parameters for one Wayland capture session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandFrameSourceConfig {
    /// Stored restore token from the previous successful session, if
    /// any (`AppSettings::wayland_restore_token`).
    pub stored_restore_token: Option<String>,
    /// Requested source types (bitmask of the pure session constants);
    /// intersected with the portal's advertised set before anything is
    /// sent. Defaults to Monitor | Window.
    pub source_types: u32,
    /// Allow selecting multiple sources in the compositor dialog.
    pub multiple: bool,
}

impl Default for WaylandFrameSourceConfig {
    fn default() -> Self {
        Self {
            stored_restore_token: None,
            source_types: SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW,
            multiple: false,
        }
    }
}

/// Cursor capability of the established session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaylandCursorStatus {
    /// The session negotiated cursor Metadata (4): the stream carries
    /// cursor metadata alongside the frames. (Cursor coordinates are not
    /// extracted from that metadata for the pipeline's cursor surface;
    /// nothing pretends to have them.)
    MetadataStream,
    /// Cursor state is unavailable for this session. The reason names
    /// the negotiated mode; it is surfaced through `fallback_reason`
    /// instead of pretending.
    Unavailable { reason: String },
}

/// One [`WaylandFrameSource::capture`] outcome. Returned by
/// value once per tick and destructured at once, so the size difference
/// costs nothing a `Box` allocation per frame would save.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum WaylandCapture {
    /// A frame delivered since the previous capture.
    Frame(CaptureResult),
    /// No buffer arrived since the previous capture; `last_seq` is the
    /// delivery sequence number of the newest frame the stream delivered.
    /// The old pixels are not re-delivered.
    NoNewFrame { last_seq: u64 },
}

/// Portal-backed ScreenCast frame source for a Wayland session.
///
/// Owns the portal session (closed on drop) and the PipeWire pump
/// thread. `capture()` takes the frame delivered since the previous
/// capture; ROI requests are cropped in process via `crop_frame`.
pub struct WaylandFrameSource {
    shared: Arc<StreamShared>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// Set once the pump thread reported a connected stream; only then
    /// is joining guaranteed to terminate promptly.
    worker_started: bool,
    session: Option<Session<Screencast>>,
    /// Global position of the shared monitor (portal stream position);
    /// the origin for ScreenGlobalRoi translation.
    stream_origin: (i32, i32),
    /// The portal's stream position as reported (not defaulted).
    stream_position: Option<(i32, i32)>,
    negotiated_cursor_mode: u32,
    /// The process-wide portal-session lease, held for its `Drop` only.
    /// Declared last on purpose: field drops run after `Drop::drop` closed
    /// the portal session and tore down the pump thread, so the slot frees
    /// only after the full teardown and a competing connect can never
    /// overlap this session.
    _session_slot: PortalSessionLease,
}

impl WaylandFrameSource {
    /// Establish the portal session and start the PipeWire stream.
    ///
    /// Returns the frame source plus the restore-token rotation to
    /// persist ([`RestoreTokenUpdate`]): portal tokens are single-use,
    /// so every successful session rotates the stored value.
    ///
    /// # Errors
    ///
    /// Returns a structured [`WaylandCaptureError`] naming the missing
    /// piece: unreachable portal, no advertised overlap with the
    /// request, dialog cancelled, or a PipeWire setup failure.
    pub fn connect(
        config: WaylandFrameSourceConfig,
    ) -> Result<(Self, RestoreTokenUpdate), WaylandCaptureError> {
        // Portal sessions are exclusive per process: take the session slot
        // (waiting out a previous source's teardown) before touching the
        // portal, so a second session can never race the first one's
        // stream target on the compositor side.
        let session_slot = acquire_portal_session_slot()?;
        let mut portal = AshpdScreenCastPortal::new()?;
        let established = SessionStateMachine::new(&mut portal).establish(&SessionRequest {
            source_types: config.source_types,
            multiple: config.multiple,
            restore_token: config.stored_restore_token.clone(),
        })?;
        let session = portal.take_session();

        let token_update = RestoreTokenState::new(config.stored_restore_token)
            .rotate(established.new_restore_token);

        let hint_size = established.stream_size.map(|(width, height)| {
            (
                u32::try_from(width.max(0)).unwrap_or(0),
                u32::try_from(height.max(0)).unwrap_or(0),
            )
        });
        let stream_origin = established.stream_position.unwrap_or((0, 0));

        let shared = Arc::new(StreamShared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let worker_stop = Arc::clone(&stop);
        let worker_node_id = established.node_id;
        let worker = match std::thread::Builder::new()
            .name("d4lc-wayland-capture".to_string())
            .spawn(move || {
                run_stream_thread(
                    established.pipe_wire_fd,
                    worker_node_id,
                    hint_size,
                    worker_shared,
                    worker_stop,
                    ready_tx,
                );
            }) {
            Ok(worker) => worker,
            Err(error) => {
                // The established session must not leak open: ashpd's
                // Session does not close on drop.
                if let Some(session) = session {
                    let _ = block_on(session.close());
                }
                return Err(WaylandCaptureError::PipeWireStreamFailed {
                    reason: format!("failed to spawn the PipeWire pump thread: {error}"),
                });
            }
        };

        let source = Self {
            shared,
            stop,
            worker: Some(worker),
            worker_started: false,
            session,
            stream_origin,
            stream_position: established.stream_position,
            negotiated_cursor_mode: established.negotiated_cursor_mode,
            _session_slot: session_slot,
        };

        match ready_rx.recv_timeout(STREAM_READY_TIMEOUT) {
            Ok(Ok(())) => {
                let mut source = source;
                source.worker_started = true;
                Ok((source, token_update))
            }
            Ok(Err(reason)) => {
                let error = WaylandCaptureError::PipeWireStreamFailed { reason };
                drop(source);
                Err(error)
            }
            Err(_) => {
                let error = WaylandCaptureError::PipeWireStreamFailed {
                    reason: format!(
                        "the PipeWire stream thread did not report readiness within {STREAM_READY_TIMEOUT:?}"
                    ),
                };
                drop(source);
                Err(error)
            }
        }
    }

    /// Take the ScreenCast frame delivered since the previous capture.
    ///
    /// The frame moves out of the stream slot (no pixel copy);
    /// when no new buffer arrived since the previous capture the result is
    /// the structured [`WaylandCapture::NoNewFrame`] instead of the old
    /// pixels again. Full requests keep the frame; ROI requests crop it in
    /// process (`crop_frame`), so `WindowRelativeRoi` and `ScreenGlobalRoi`
    /// work from monitor streams exactly like the synthetic source maps
    /// them. Hand a processed full frame back through [`Self::recycle`].
    ///
    /// # Errors
    ///
    /// Returns a structured reason when the stream has failed, has not
    /// delivered a frame yet, or the ROI is outside the frame.
    pub fn capture(&self, request: CaptureRequest) -> Result<WaylandCapture, String> {
        if let Some(failure) = self.shared.failure()? {
            return Err(format!("wayland ScreenCast stream failed: {failure}"));
        }
        let latest = match self.shared.take_latest()? {
            SlotTake::Frame { frame, .. } => frame,
            SlotTake::NoNewFrame { last_seq } => {
                return Ok(WaylandCapture::NoNewFrame { last_seq })
            }
            SlotTake::NeverDelivered => {
                return Err(match self.shared.last_buffer_error()? {
                    Some(reason) => format!(
                        "no usable ScreenCast frame received yet; the last buffer was refused: {reason}"
                    ),
                    None => "no ScreenCast frame received yet; the compositor stream has not \
                             delivered a buffer"
                        .to_string(),
                });
            }
        };

        let full_rect = Rect {
            x: 0,
            y: 0,
            width: latest.width,
            height: latest.height,
        };
        let plan = plan_capture(request, self.stream_origin, full_rect);

        let frame = match plan.crop_region {
            Some(region) => {
                let cropped = crop_frame(&latest, region);
                self.shared.recycle(latest);
                cropped?
            }
            None => latest,
        };

        Ok(WaylandCapture::Frame(CaptureResult {
            // `CaptureBackend` is the X11-internal method choice and does
            // not describe the portal path; the markers here are
            // `capture_method` and `fallback_reason`.
            backend: CaptureBackend::Auto,
            fallback_reason: Some(self.fallback_reason()),
            source: CaptureSource::PortalStream(full_rect),
            geometry: full_rect,
            scale: ScaleFactor::one(),
            requested_capture_rect: plan.requested_capture_rect,
            actual_capture_rect: plan.actual_capture_rect,
            roi_origin: plan.roi_origin,
            capture_scope: plan.capture_scope,
            overlay_window: OverlayWindowState::Ignored,
            capture_method: CaptureMethod::WaylandPortal,
            frame,
        }))
    }

    /// Hand a processed frame's pixel buffer back to the stream's recycle
    /// pool. Optional: without it the pool refills by
    /// allocation.
    pub fn recycle(&self, frame: CaptureFrame) {
        self.shared.recycle(frame);
    }

    /// Global logical position of the shared monitor as the portal
    /// reported it; `None` for window streams, which carry no position.
    pub fn stream_position(&self) -> Option<(i32, i32)> {
        self.stream_position
    }

    /// Pixel size (width, height) of the newest frame the stream
    /// delivered, taken or not; `None` before the first delivery. A
    /// resolution change of the shared monitor or window shows here from
    /// its first delivered frame on.
    ///
    /// # Errors
    ///
    /// Returns a reason when the frame slot lock is poisoned.
    pub fn latest_frame_size(&self) -> Result<Option<(u32, u32)>, String> {
        self.shared.latest_frame_size()
    }

    /// The new-frame signal of the capture pump: the live loop
    /// waits on it and starts processing at the frame-delivery moment.
    pub fn frame_signal(&self) -> crate::FrameSignal {
        self.shared.signal()
    }

    /// Cursor capability of the established session.
    pub fn cursor_status(&self) -> WaylandCursorStatus {
        if self.negotiated_cursor_mode & session::CURSOR_MODE_METADATA != 0 {
            WaylandCursorStatus::MetadataStream
        } else {
            WaylandCursorStatus::Unavailable {
                reason: format!(
                    "the portal session negotiated cursor mode 0x{:x} without metadata; \
                     cursor state is unavailable on this stream",
                    self.negotiated_cursor_mode
                ),
            }
        }
    }

    /// The stream rate the PipeWire negotiation settled on;
    /// `None` until the format was negotiated.
    pub fn negotiated_rate(&self) -> Option<NegotiatedRate> {
        self.shared.negotiated_rate()
    }

    /// The Debug screen's stream-rate facts: negotiated rate,
    /// delivered and consumed frames per second, throttle skips.
    pub fn stream_rate_status(&self) -> Result<StreamRateStatus, String> {
        self.shared.rate_status()
    }

    /// Per-frame conversion accounting (compare against the
    /// X11 hot path; live runs record it under `debug-output/`).
    pub fn conversion_stats(&self) -> Result<FrameConversionStats, String> {
        self.shared.stats()
    }

    /// Wait for the first converted frame (used by the opt-in live test
    /// and diagnostics).
    ///
    /// # Errors
    ///
    /// Returns the structured stream failure, or a timeout reason when
    /// no frame arrived within `timeout`.
    pub fn wait_for_first_frame(&self, timeout: Duration) -> Result<CaptureFrame, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(failure) = self.shared.failure()? {
                return Err(format!("wayland ScreenCast stream failed: {failure}"));
            }
            if let Some(frame) = self.shared.peek_latest()? {
                return Ok(frame);
            }
            if Instant::now() >= deadline {
                return Err(match self.shared.last_buffer_error()? {
                    Some(reason) => format!(
                        "no usable ScreenCast frame arrived within {timeout:?}; the last buffer \
                         was refused: {reason}"
                    ),
                    None => format!("no ScreenCast frame arrived within {timeout:?}"),
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn fallback_reason(&self) -> String {
        let mut reason = "wayland screencast portal frame source".to_string();
        if let WaylandCursorStatus::Unavailable { reason: cursor } = self.cursor_status() {
            reason.push_str("; ");
            reason.push_str(&cursor);
        }
        reason
    }
}

impl Drop for WaylandFrameSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(session) = self.session.take() {
            // Best effort: closing the portal session ends the
            // compositor's screen-share; a failed close only means the
            // portal reaps the session on its own.
            let _ = block_on(session.close());
        }
        match (self.worker.take(), self.worker_started) {
            // The pump loop checks the stop flag every tick, so a
            // started worker terminates promptly.
            (Some(worker), true) => {
                let _ = worker.join();
            }
            // A worker that never became ready may be stuck inside a
            // PipeWire setup call; joining could hang preparation.
            // Detach it instead — its resources die with the thread.
            (Some(worker), false) => drop(worker),
            (None, _) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::CaptureScopeResult;

    /// The process-wide slot serializes connect attempts: a second
    /// connect blocks until the holder is fully dropped, and is refused
    /// with the named conflict when it never is. One test drives the
    /// global slot through all its states so parallel tests cannot
    /// interleave on it.
    #[test]
    fn portal_session_slot_blocks_a_second_connect_until_the_holder_is_dropped() {
        // Held slot: a competing connect blocks on the condvar and
        // acquires right after the holder's release (the stop-then-start
        // handoff of a just-stopped live loop).
        let lease = acquire_portal_session_slot().expect("a free slot must be acquirable");
        let waiter = std::thread::spawn(acquire_portal_session_slot);
        std::thread::sleep(Duration::from_millis(100));
        drop(lease);
        let handed_off = waiter
            .join()
            .expect("the waiting connect must acquire after the holder's release");
        drop(handed_off);

        // Never released: the competing connect is refused with the named
        // conflict after the bounded wait instead of opening a second
        // portal session beside the holder.
        let held = acquire_portal_session_slot().expect("the slot must be free after both drops");
        let refused = std::thread::spawn(acquire_portal_session_slot)
            .join()
            .expect("the refusing connect thread must finish");
        drop(held);
        let Err(error) = refused else {
            panic!("a held slot must refuse a second connect, not open a second session");
        };
        assert!(
            matches!(error, WaylandCaptureError::SessionConflict { .. }),
            "the refusal must name the session conflict: {error}"
        );
        assert!(
            error.to_string().contains("ScreenCast session"),
            "the conflict must be user-facing about the exclusivity: {error}"
        );

        // After the holder's release the slot is reusable again.
        let _reacquired =
            acquire_portal_session_slot().expect("a released slot must be acquirable again");
    }

    /// Opt-in live test: exercises the real ScreenCast portal session on
    /// a real Wayland session (compositor consent dialog included).
    ///
    /// Run manually on a Wayland machine with xdg-desktop-portal and
    /// libpipewire installed:
    ///
    /// ```text
    /// cargo test -p d4lc-capture --features wayland-portal \
    ///   live_portal_session -- --ignored --nocapture
    /// ```
    ///
    /// It needs a real Wayland session and a consent dialog, so it is not
    /// part of the default suite. The test never prints the restore token
    /// value itself (it grants screen access).
    #[test]
    #[ignore = "requires a real Wayland session with xdg-desktop-portal + PipeWire; shows the compositor consent dialog"]
    fn live_portal_session_connects_and_delivers_frames() {
        let config = WaylandFrameSourceConfig {
            stored_restore_token: std::env::var("D4LC_WAYLAND_RESTORE_TOKEN").ok(),
            ..WaylandFrameSourceConfig::default()
        };

        let (source, token_update) = WaylandFrameSource::connect(config)
            .map_err(|error| {
                format!(
                    "portal session failed: {error} (missing piece: {})",
                    error.missing_piece()
                )
            })
            .expect("live portal session should establish");

        println!("portal session established");
        println!(
            "restore-token rotation: {:?}",
            match &token_update {
                RestoreTokenUpdate::Keep => "keep",
                RestoreTokenUpdate::Store(_) => "store (single-use token rotated)",
                RestoreTokenUpdate::Clear => "clear (consumed token dropped)",
            }
        );
        println!("cursor status: {:?}", source.cursor_status());

        let first = source
            .wait_for_first_frame(Duration::from_secs(60))
            .map_err(|error| format!("no frame from the stream: {error}"))
            .expect("live stream should deliver a frame");
        println!(
            "first frame: {}x{} (stride {})",
            first.width, first.height, first.stride
        );

        let full = match source
            .capture(CaptureRequest::Full)
            .map_err(|error| format!("full capture failed: {error}"))
            .expect("live full capture should succeed")
        {
            WaylandCapture::Frame(full) => full,
            WaylandCapture::NoNewFrame { last_seq } => {
                panic!("the first capture must take the first frame, got a repeat of {last_seq}")
            }
        };
        assert_eq!(full.capture_method, CaptureMethod::WaylandPortal);
        assert_eq!(full.frame.width, first.width);
        source.recycle(full.frame);
        let signal = source.frame_signal();
        signal.wait_for(signal.generation(), Duration::from_secs(5));

        // Center-quarter ROI cropped in process from the full frame.
        let roi = Rect {
            x: i32::try_from(first.width / 4).unwrap_or(0),
            y: i32::try_from(first.height / 4).unwrap_or(0),
            width: first.width / 2,
            height: first.height / 2,
        };
        let WaylandCapture::Frame(cropped) = source
            .capture(CaptureRequest::ScreenGlobalRoi(roi))
            .map_err(|error| format!("roi capture failed: {error}"))
            .expect("live roi capture should succeed")
        else {
            panic!("no new frame arrived for the roi capture within 5 s");
        };
        assert_eq!(cropped.frame.width, roi.width);
        assert_eq!(cropped.capture_scope, CaptureScopeResult::Roi);
        println!(
            "roi crop: {}x{} from a {}x{} stream",
            cropped.frame.width, cropped.frame.height, first.width, first.height
        );

        let stats = source
            .conversion_stats()
            .map_err(|error| format!("stats failed: {error}"))
            .expect("live stats should be readable");
        println!(
            "conversion: {} frames, {} converted, {} failed, last {} µs, total {} µs",
            stats.frames_received,
            stats.frames_converted,
            stats.frames_failed,
            stats.last_conversion_micros,
            stats.total_conversion_micros
        );
        assert!(stats.frames_converted > 0, "frames must have converted");
        assert_eq!(stats.frames_failed, 0, "no conversion failures expected");

        // Evidence artifact for manual review.
        let artifact_dir = std::path::PathBuf::from(
            std::env::var("D4LC_WAYLAND_LIVE_ARTIFACTS")
                .unwrap_or_else(|_| "debug-output/wayland-screencast-capture".to_string()),
        );
        if std::fs::create_dir_all(&artifact_dir).is_ok() {
            let report = artifact_dir.join("live-session-report.txt");
            let content = format!(
                "status=passed\nframes_converted={}\nframes_failed={}\nlast_conversion_micros={}\ntotal_conversion_micros={}\nstream_width={}\nstream_height={}\n",
                stats.frames_converted,
                stats.frames_failed,
                stats.last_conversion_micros,
                stats.total_conversion_micros,
                first.width,
                first.height
            );
            if std::fs::write(&report, content).is_ok() {
                println!("artifact: {}", report.display());
            }
        }

        // Dropping the source closes the portal session and stops the
        // pump thread; a clean drop is part of the live contract.
        drop(source);
        println!("session closed cleanly");
    }
}
