#[cfg(feature = "wayland-portal")]
use std::sync::Arc;

use std::time::Duration;

#[cfg(feature = "wayland-portal")]
use d4lc_capture::WaylandFrameSource;
use d4lc_capture::{CaptureBackend, CaptureReference, FrameSignal, SyntheticCaptureFrameSource};
use d4lc_core::Point;
use d4lc_detect::prepared::PreparedDetector;

/// Live-loop pacing: one display frame, at most
/// 60 Hz, taken from the best rate the capture path can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePacing {
    /// The target interval between live-loop ticks.
    pub interval: Duration,
    /// Where the interval came from — the Capture engine debug line names
    /// it, so a fallback is never silent.
    pub source: PacingSource,
}

/// The rate source behind [`FramePacing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacingSource {
    /// The negotiated ScreenCast stream rate; a variable rate
    /// without a maximum reports the fallback instead.
    StreamRate,
    /// The RandR refresh rate of the captured output (X11).
    RandR,
    /// The 60 Hz fallback: the capture path could not name a rate.
    Fallback,
}

impl FramePacing {
    /// The maximum tick rate (one display frame, at most
    /// 60 Hz, no host-specific numbers).
    pub const MAX_HZ: u32 = 60;

    /// The 60 Hz fallback pacing.
    pub fn fallback() -> Self {
        Self {
            interval: Duration::from_micros(1_000_000 / u64::from(Self::MAX_HZ)),
            source: PacingSource::Fallback,
        }
    }

    /// Pacing from a display rate in Hz, capped at [`Self::MAX_HZ`].
    pub fn from_hz(hz: f64, source: PacingSource) -> Self {
        let hz = hz.clamp(1.0, f64::from(Self::MAX_HZ));
        Self {
            interval: Duration::from_secs_f64(1.0 / hz),
            source,
        }
    }

    /// The Capture engine debug line's pacing part, about the
    /// fallback (an unknown rate is shown as 60 Hz).
    pub fn debug_text(&self) -> String {
        let hz = 1_000_000.0 / self.interval.as_micros() as f64;
        match self.source {
            PacingSource::StreamRate => format!("loop target {:.0} Hz (stream rate)", hz),
            PacingSource::RandR => format!("loop target {:.0} Hz (RandR refresh)", hz),
            PacingSource::Fallback => format!("loop target {:.0} Hz (display rate unknown)", hz),
        }
    }
}

/// The runtime's frame-source seam: every capture path shares this one
/// dispatch (`pipeline::process_one_frame`). The X11 variant is the
/// historical default (reference + X11-internal capture backend); the
/// synthetic variant is the explicit non-production test source; the
/// Wayland variant carries the ScreenCast portal stream source behind
/// the `wayland-portal` build feature.
pub(crate) enum FrameSource {
    /// The X11 paths, unchanged: `capture_reference` +
    /// `capture_backend` resolve the frame.
    X11,
    /// Explicit non-production source for deterministic tests/harnesses.
    Synthetic(SyntheticCaptureFrameSource),
    /// Wayland ScreenCast portal + PipeWire stream.
    #[cfg(feature = "wayland-portal")]
    Wayland(Arc<WaylandFrameSource>),
    /// Test-only scripted source with the Wayland stream's no-new-frame
    /// contract, so the repeat path is tested without a
    /// compositor.
    #[cfg(test)]
    Scripted(ScriptedFrameSource),
}

/// One step of a [`ScriptedFrameSource`].
#[cfg(test)]
pub(crate) enum ScriptStep {
    /// Deliver this frame (as a manual-region capture).
    Frame(d4lc_core::CaptureFrame),
    /// No new frame since the previous capture.
    NoNewFrame,
}

/// Test-only frame source that plays a script of fresh frames and
/// no-new-frame ticks, with the Wayland stream's delivery numbering.
#[cfg(test)]
pub(crate) struct ScriptedFrameSource {
    steps: std::sync::Mutex<(std::collections::VecDeque<ScriptStep>, u64)>,
}

#[cfg(test)]
impl ScriptedFrameSource {
    pub(crate) fn new(steps: impl IntoIterator<Item = ScriptStep>) -> Self {
        Self {
            steps: std::sync::Mutex::new((steps.into_iter().collect(), 0)),
        }
    }

    pub(crate) fn capture(&self) -> Result<crate::pipeline::FrameCapture, String> {
        let mut guard = self
            .steps
            .lock()
            .map_err(|_| "scripted source lock poisoned".to_string())?;
        let (steps, delivered) = &mut *guard;
        match steps.pop_front() {
            Some(ScriptStep::Frame(frame)) => {
                *delivered += 1;
                SyntheticCaptureFrameSource::manual_region(frame)?
                    .capture(d4lc_capture::CaptureRequest::Full)
                    .map(crate::pipeline::FrameCapture::Fresh)
            }
            Some(ScriptStep::NoNewFrame) if *delivered > 0 => {
                Ok(crate::pipeline::FrameCapture::NoNewFrame {
                    last_seq: *delivered,
                })
            }
            Some(ScriptStep::NoNewFrame) => Err("no frame delivered yet".to_string()),
            None => Err("frame script exhausted".to_string()),
        }
    }
}

/// The runtime's cursor-source seam: how each frame obtains the
/// window-relative pointer position carried by `OverlayFrame`
/// (upstream `OverlayHandler.HandleMouseUpdatedMessage`,
/// OverlayHandler.cs:1254-1309).
pub(crate) enum CursorSource {
    /// Poll the pointer through d4lc-capture's cursor reader (a native
    /// `XQueryPointer` on a connection held per thread, no subprocess),
    /// translated into the capture reference geometry. The X11
    /// default.
    X11,
    /// The display backend reports the pointer itself (the Wayland
    /// layer-shell overlay consumes surface-local pointer events), so
    /// the frame carries no runtime-polled cursor value.
    #[cfg(feature = "wayland-portal")]
    BackendManaged,
    /// Deterministic per-poll cursor values for tests and harnesses.
    Synthetic(SyntheticCursorSource),
}

/// Deterministic cursor source: calls a closure once per poll so tests
/// and harnesses can script per-frame cursor positions (including
/// "no position this frame").
pub struct SyntheticCursorSource {
    read: Box<dyn Fn() -> Option<Point> + Send>,
}

impl SyntheticCursorSource {
    /// Create a synthetic cursor source from a per-poll closure.
    pub fn new(read: impl Fn() -> Option<Point> + Send + 'static) -> Self {
        Self {
            read: Box::new(read),
        }
    }

    pub(crate) fn read(&self) -> Option<Point> {
        (self.read)()
    }
}

/// Prepared runtime resources — built once by the entry point, used read-only
/// by each `process_one_frame()` call.
///
/// Construction is side-effect-free: no I/O, no window lookup, no template
/// loading. The entry point (CLI or Slint app) prepares each piece and calls
/// `PreparedRuntime::new(...)`.
pub struct PreparedRuntime {
    pub(crate) capture_backend: CaptureBackend,
    pub(crate) capture_reference: CaptureReference,
    pub(crate) detector: PreparedDetector,
    pub(crate) frame_source: FrameSource,
    pub(crate) cursor_source: CursorSource,
    /// Live-loop pacing. Defaults to the 60 Hz fallback.
    pacing: FramePacing,
    /// The Wayland capture stream's new-frame signal; `None` for the X11
    /// and synthetic sources, which pace themselves.
    frame_signal: Option<FrameSignal>,
}

impl PreparedRuntime {
    /// Create a new PreparedRuntime from already-prepared pieces.
    ///
    /// # Parameters
    ///
    /// * `capture_backend` — which capture backend to use
    /// * `capture_reference` — resolved capture target (window or manual region)
    /// * `detector` — prepared detector with loaded templates and thresholds
    pub fn new(
        capture_backend: CaptureBackend,
        capture_reference: CaptureReference,
        detector: PreparedDetector,
    ) -> Self {
        Self {
            capture_backend,
            capture_reference,
            detector,
            frame_source: FrameSource::X11,
            cursor_source: CursorSource::X11,
            pacing: FramePacing::fallback(),
            frame_signal: None,
        }
    }

    /// Set the live-loop pacing. Preparation derives it from the
    /// capture path's rate; the loop never invents one itself.
    pub fn with_frame_pacing(mut self, pacing: FramePacing) -> Self {
        self.pacing = pacing;
        self
    }

    /// The live-loop pacing of this runtime.
    pub const fn pacing(&self) -> FramePacing {
        self.pacing
    }

    /// The Wayland capture stream's new-frame signal. `None`
    /// for the X11 and synthetic sources, which pace themselves.
    pub const fn frame_signal(&self) -> Option<&FrameSignal> {
        self.frame_signal.as_ref()
    }

    /// Route the frame dispatch through a deterministic synthetic source —
    /// the explicit non-production source tests and harnesses drive the
    /// composed runtime with (the app's live-loop tests use it).
    pub fn with_synthetic_capture_source(
        mut self,
        synthetic_capture_source: SyntheticCaptureFrameSource,
    ) -> Self {
        self.frame_source = FrameSource::Synthetic(synthetic_capture_source);
        self
    }

    /// Route the per-frame cursor dispatch through a deterministic
    /// synthetic source. Tests and harnesses use this to script cursor
    /// positions; production preparation keeps the X11 poll default (or
    /// the Wayland backend-managed behavior).
    pub fn with_synthetic_cursor_source(mut self, source: SyntheticCursorSource) -> Self {
        self.cursor_source = CursorSource::Synthetic(source);
        self
    }

    /// Route the frame dispatch through a Wayland ScreenCast portal
    /// stream source. The caller (runtime preparation on the Wayland
    /// display-backend path) established the portal session and owns the
    /// restore-token rotation; this only switches the dispatch.
    ///
    /// The Wayland overlay reports the pointer through surface-local
    /// events, so this also switches the cursor dispatch to
    /// backend-managed: an X11 pointer poll would read coordinates
    /// against the placeholder capture reference and fabricate
    /// window-relative geometry.
    #[cfg(feature = "wayland-portal")]
    pub fn with_wayland_capture_source(self, source: WaylandFrameSource) -> Self {
        self.with_shared_wayland_capture_source(Arc::new(source))
    }

    /// [`Self::with_wayland_capture_source`] for a source that is already
    /// shared: a settings re-preparation keeps the established portal
    /// session instead of opening a second one (portal sessions are
    /// exclusive per process, and a new one would consume the restore
    /// token or show the compositor dialog again).
    #[cfg(feature = "wayland-portal")]
    pub fn with_shared_wayland_capture_source(mut self, source: Arc<WaylandFrameSource>) -> Self {
        // The loop wakes on the stream's new-frame signal.
        self.frame_signal = Some(source.frame_signal());
        self.frame_source = FrameSource::Wayland(source);
        self.cursor_source = CursorSource::BackendManaged;
        self
    }

    /// The Wayland capture source this runtime reads, shared, so a
    /// re-preparation can carry the established session over. `None` for
    /// every other source.
    #[cfg(feature = "wayland-portal")]
    pub fn shared_wayland_capture_source(&self) -> Option<Arc<WaylandFrameSource>> {
        match &self.frame_source {
            FrameSource::Wayland(source) => Some(Arc::clone(source)),
            _ => None,
        }
    }

    /// The resolved capture target this runtime was prepared with.
    pub const fn capture_reference(&self) -> &CaptureReference {
        &self.capture_reference
    }

    /// The observed on-screen geometry of the captured game window (window
    /// moved or resized). The next capture reads the window at its new size,
    /// and its geometry reaches the coordinator as a new window revision.
    /// Returns whether anything changed; a manual region, a synthetic or a
    /// Wayland source has no game window to follow and stays unchanged.
    ///
    /// A synthetic source stands in for a captured window in tests: it
    /// follows a same-size move.
    pub fn set_window_geometry(&mut self, geometry: d4lc_core::Rect) -> bool {
        match &mut self.frame_source {
            FrameSource::X11 => {}
            FrameSource::Synthetic(source) => {
                let moved = source.move_to(geometry).unwrap_or(false);
                if moved {
                    self.capture_reference.geometry = geometry;
                }
                return moved;
            }
            #[cfg(feature = "wayland-portal")]
            FrameSource::Wayland(_) => return false,
            #[cfg(test)]
            FrameSource::Scripted(_) => return false,
        }
        let d4lc_capture::CaptureSource::Window(candidate) = &mut self.capture_reference.source
        else {
            return false;
        };
        if candidate.info.rect == geometry && self.capture_reference.geometry == geometry {
            return false;
        }
        candidate.info.rect = geometry;
        self.capture_reference.geometry = geometry;
        true
    }

    /// The captured game window's X11 id, when the runtime captures one.
    pub fn game_window_id(&self) -> Option<u64> {
        match (&self.frame_source, &self.capture_reference.source) {
            (FrameSource::X11, d4lc_capture::CaptureSource::Window(candidate)) => {
                Some(candidate.info.id)
            }
            _ => None,
        }
    }

    /// Whether frames come from a Wayland ScreenCast stream.
    pub fn captures_wayland_stream(&self) -> bool {
        #[cfg(feature = "wayland-portal")]
        if matches!(self.frame_source, FrameSource::Wayland(_)) {
            return true;
        }
        false
    }

    /// Global logical position of the monitor a Wayland ScreenCast stream
    /// shares; `None` for window streams and every non-Wayland source.
    pub fn wayland_stream_position(&self) -> Option<(i32, i32)> {
        #[cfg(feature = "wayland-portal")]
        if let FrameSource::Wayland(source) = &self.frame_source {
            return source.stream_position();
        }
        None
    }

    /// The captured geometry the runtime currently follows.
    pub fn capture_geometry(&self) -> d4lc_core::Rect {
        self.capture_reference.geometry
    }

    /// The prepared detector (templates, thresholds, matcher).
    pub fn detector(&self) -> &PreparedDetector {
        &self.detector
    }

    /// Name of the detector preset this runtime was prepared with.
    ///
    /// Exposed so callers (e.g. the settings hot-reload path) can report
    /// which preset is live after a runtime swap without reaching into the
    /// detector internals.
    pub fn preset_name(&self) -> &str {
        &self.detector.preset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The 60 Hz fallback is the announced default (an
    /// unknown display rate is shown, never guessed).
    #[test]
    fn fallback_pacing_is_sixty_hz_and_named() {
        let pacing = FramePacing::fallback();
        assert_eq!(pacing.source, PacingSource::Fallback);
        assert_eq!(
            pacing.interval,
            Duration::from_micros(1_000_000 / u64::from(FramePacing::MAX_HZ))
        );
        assert_eq!(
            pacing.debug_text(),
            "loop target 60 Hz (display rate unknown)"
        );
    }

    /// A display rate caps at 60 Hz and names its source.
    #[test]
    fn display_rates_cap_at_sixty_hz() {
        let fast = FramePacing::from_hz(165.0, PacingSource::RandR);
        assert_eq!(fast.source, PacingSource::RandR);
        assert_eq!(fast.interval, Duration::from_secs_f64(1.0 / 60.0));
        assert_eq!(fast.debug_text(), "loop target 60 Hz (RandR refresh)");

        let slow = FramePacing::from_hz(30.0, PacingSource::StreamRate);
        assert_eq!(slow.interval, Duration::from_secs_f64(1.0 / 30.0));
        assert_eq!(slow.debug_text(), "loop target 30 Hz (stream rate)");
    }
}
