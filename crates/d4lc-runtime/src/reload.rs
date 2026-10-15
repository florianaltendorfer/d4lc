//! Settings hot-reload seam for the live frame loop.
//!
//! The live loop owns its `PreparedRuntime` / `RuntimeSession` pair for the
//! whole session.  Hot reload lets the settings owner (Slint UI thread or an
//! external editor of `Config/Settings.json`) hand the running loop a freshly
//! prepared pair without stopping the loop:
//!
//! ```text
//! producer                          consumer (live-loop thread)
//! ────────                          ───────────────────────────
//! signal.request()  ──► atomic ───► apply_pending_reload(&signal, …, prepare)
//!                                       ├─ Ok(pair) → swap in, Applied
//!                                       └─ Err(msg) → keep previous pair
//!                                                     (non-fatal), Failed
//! ```
//!
//! A failed re-preparation never aborts the frame loop: evaluation continues
//! with the last known good runtime and the error is reported to the caller
//! for a non-fatal UI status.  This mirrors the Alacritty
//! `live_config_reload` contract (changed settings apply to the next
//! evaluated frame) without an in-process file-watcher dependency.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::prepared::PreparedRuntime;
use crate::session::RuntimeSession;

/// Shared request flag between the settings owner and the live loop.
///
/// Cloning shares the same underlying flag; the producer calls
/// [`RuntimeReloadSignal::request`] and the frame loop consumes the request
/// exactly once through [`apply_pending_reload`].
#[derive(Clone, Default)]
pub struct RuntimeReloadSignal {
    pending: Arc<AtomicBool>,
}

impl RuntimeReloadSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request that the live loop re-prepare and swap its runtime before the
    /// next frame. Multiple requests before the loop consumes one coalesce
    /// into a single pending request.
    pub fn request(&self) {
        self.pending.store(true, Ordering::Relaxed);
    }

    /// Whether a reload request is currently pending. Non-consuming peek.
    pub fn is_requested(&self) -> bool {
        self.pending.load(Ordering::Relaxed)
    }

    /// Consume a pending request. Returns `true` exactly once per
    /// `request()` (or per coalesced burst of requests).
    pub fn take_request(&self) -> bool {
        self.pending.swap(false, Ordering::Relaxed)
    }
}

/// Outcome of one [`apply_pending_reload`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// No reload was pending; the runtime and session are untouched.
    Idle,
    /// The freshly prepared pair was swapped in and applies to the next
    /// evaluated frame.
    Applied,
    /// Re-preparation failed; the previous pair stays active (non-fatal).
    /// The live loop keeps running.
    Failed { message: String },
}

/// Drain a pending reload request against the caller-owned pair.
///
/// When a reload is pending, `prepare` builds the replacement pair (the
/// caller decides what "re-prepare" means — in the app this re-runs the full
/// preparation from the on-disk settings). On success the new pair is
/// swapped in; the frame sequence continues from the old session so
/// overlay/debug consumers never observe a sequence reset mid-session. On
/// failure the previous pair stays active and the error is reported through
/// [`ReloadOutcome::Failed`]; the loop keeps running with the last known
/// good settings.
pub fn apply_pending_reload(
    signal: &RuntimeReloadSignal,
    runtime: &mut PreparedRuntime,
    session: &mut RuntimeSession,
    prepare: impl FnOnce() -> Result<(PreparedRuntime, RuntimeSession), String>,
) -> ReloadOutcome {
    if !signal.take_request() {
        return ReloadOutcome::Idle;
    }

    match prepare() {
        Ok((new_runtime, mut new_session)) => {
            new_session.sequence = session.sequence;
            // The analysis session (its diagnostics record and its
            // coordinator) continues across the swap: the freshly prepared
            // session and its OCR worker emit into the same session timeline
            // from here on, and the coordinator's generation history keeps the
            // replaced worker's late results discardable.
            new_session.carry_over_from(session);
            *runtime = new_runtime;
            let replaced = std::mem::replace(session, new_session);
            // Dropping the replaced session joins its OCR worker, which may be
            // inside a native OCR call that cannot be interrupted. That wait
            // happens on its own thread, never on the frame loop: invalidation
            // must not wait for OCR.
            release_off_thread(replaced);
            ReloadOutcome::Applied
        }
        Err(message) => ReloadOutcome::Failed { message },
    }
}

/// Drop `value` on a short-lived thread (its `Drop` may block on a native
/// call). Falls back to dropping in place if no thread can be spawned.
pub(crate) fn release_off_thread<T: Send + 'static>(value: T) {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(value)));
    let thread_slot = std::sync::Arc::clone(&slot);
    let spawned = std::thread::Builder::new()
        .name("d4lc-release".to_string())
        .spawn(move || {
            let value = thread_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            drop(value);
        });
    if spawned.is_err() {
        let value = slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        drop(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d4lc_capture::{CaptureBackend, CaptureReference, CaptureSource};
    use d4lc_core::{InputMode, Rect};
    use d4lc_detect::prepared::{PreparedDetector, ThresholdSettings};
    use std::time::Duration;

    fn capture_reference() -> CaptureReference {
        CaptureReference {
            source: CaptureSource::ManualRegion(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }),
            geometry: Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        }
    }

    fn detector(preset: &str) -> PreparedDetector {
        PreparedDetector {
            preset: preset.to_string(),
            mode: InputMode::Mouse,
            scan_height_percent: 50.0,
            thresholds: ThresholdSettings {
                min: 0,
                max: 255,
                similarity_tooltip: 0.8,
            },
            tooltip_width: 300,
            templates: Vec::new(),
            scan: d4lc_detect::prepared::DetectScan::default(),
        }
    }

    fn pair(preset: &str, ocr_interval_ms: u64) -> (PreparedRuntime, RuntimeSession) {
        (
            PreparedRuntime::new(
                CaptureBackend::X11Native,
                capture_reference(),
                detector(preset),
            ),
            RuntimeSession::new(None, Duration::from_millis(ocr_interval_ms)),
        )
    }

    #[test]
    fn reload_signal_request_is_consumed_exactly_once() {
        let signal = RuntimeReloadSignal::new();
        assert!(!signal.is_requested());
        assert!(!signal.take_request(), "no request pending yet");

        signal.request();
        assert!(signal.is_requested());
        assert!(signal.take_request(), "pending request must be consumed");
        assert!(!signal.is_requested());
        assert!(!signal.take_request(), "request must not fire twice");
    }

    #[test]
    fn apply_pending_reload_is_idle_without_request() {
        let signal = RuntimeReloadSignal::new();
        let (mut runtime, mut session) = pair("before", 250);

        let outcome = apply_pending_reload(&signal, &mut runtime, &mut session, || {
            Ok(pair("after", 999))
        });

        assert_eq!(outcome, ReloadOutcome::Idle);
        assert_eq!(runtime.detector.preset, "before");
        assert_eq!(session.ocr_interval, Duration::from_millis(250));
    }

    #[test]
    fn apply_pending_reload_swaps_pair_and_continues_frame_sequence() {
        let signal = RuntimeReloadSignal::new();
        let (mut runtime, mut session) = pair("before", 250);
        session.sequence = 41;

        signal.request();
        let outcome = apply_pending_reload(&signal, &mut runtime, &mut session, || {
            Ok(pair("after", 999))
        });

        assert_eq!(outcome, ReloadOutcome::Applied);
        assert_eq!(runtime.detector.preset, "after");
        assert_eq!(
            session.ocr_interval,
            Duration::from_millis(999),
            "reloaded preparation values must be live on the session"
        );
        assert_eq!(
            session.sequence, 41,
            "frame sequence must continue across the swap, not reset"
        );
        assert!(!signal.is_requested(), "request must be consumed");
    }

    #[test]
    fn apply_pending_reload_keeps_previous_pair_on_failure() {
        let signal = RuntimeReloadSignal::new();
        let (mut runtime, mut session) = pair("before", 250);
        session.sequence = 7;

        signal.request();
        let outcome = apply_pending_reload(&signal, &mut runtime, &mut session, || {
            Err("template load failed: preset 'broken' is missing".to_string())
        });

        assert_eq!(
            outcome,
            ReloadOutcome::Failed {
                message: "template load failed: preset 'broken' is missing".to_string()
            }
        );
        assert_eq!(
            runtime.detector.preset, "before",
            "a failed re-preparation must keep the previous runtime"
        );
        assert_eq!(session.ocr_interval, Duration::from_millis(250));
        assert_eq!(session.sequence, 7);
        assert!(!signal.is_requested(), "failed request must be consumed");
    }

    #[test]
    fn coalesced_requests_apply_exactly_one_reload() {
        let signal = RuntimeReloadSignal::new();
        let (mut runtime, mut session) = pair("before", 250);

        signal.request();
        signal.request();
        signal.request();

        let first = apply_pending_reload(&signal, &mut runtime, &mut session, || {
            Ok(pair("after", 999))
        });
        assert_eq!(first, ReloadOutcome::Applied);

        let second = apply_pending_reload(&signal, &mut runtime, &mut session, || {
            Ok(pair("should-not-apply", 1))
        });
        assert_eq!(second, ReloadOutcome::Idle);
        assert_eq!(runtime.detector.preset, "after");
    }
}
