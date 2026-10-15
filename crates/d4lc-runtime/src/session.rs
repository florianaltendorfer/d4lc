use std::time::Duration;

use d4lc_core::Rect;

use crate::coordinator::RuntimeCoordinator;
use crate::diagnostics::DiagnosticsContext;
use crate::{OcrWorker, OcrWorkerStats, TooltipTextSnapshot};

/// Input for computing the tooltip OCR signature.
/// Kept internal — not CLI `ProcessedFrame`-dependent.
pub(crate) struct TooltipOcrSignatureInput<'a> {
    pub crop_rect: Rect,
    pub template_name: &'a str,
    pub template_rect: Rect,
}

const OCR_SIGNATURE_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const OCR_SIGNATURE_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Compute the placement signature of the current tooltip crop.
/// The coordinator judges placement stability on it.
pub(crate) fn tooltip_ocr_signature(input: TooltipOcrSignatureInput<'_>) -> u64 {
    let mut hash = OCR_SIGNATURE_OFFSET_BASIS;
    hash = ocr_update_i32(hash, input.crop_rect.x);
    hash = ocr_update_i32(hash, input.crop_rect.y);
    hash = ocr_update_u32(hash, input.crop_rect.width);
    hash = ocr_update_u32(hash, input.crop_rect.height);
    hash = ocr_update_bytes(hash, input.template_name.as_bytes());
    hash = ocr_update_i32(hash, input.template_rect.x);
    hash = ocr_update_i32(hash, input.template_rect.y);
    hash = ocr_update_u32(hash, input.template_rect.width);
    hash = ocr_update_u32(hash, input.template_rect.height);
    hash
}

fn ocr_update_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(OCR_SIGNATURE_PRIME);
    }
    hash
}

fn ocr_update_i32(hash: u64, value: i32) -> u64 {
    ocr_update_bytes(hash, &value.to_le_bytes())
}

fn ocr_update_u32(hash: u64, value: u32) -> u64 {
    ocr_update_bytes(hash, &value.to_le_bytes())
}

/// Per-session mutable state for the frame pipeline.
///
/// Owns the OCR worker handle, the frame sequence counter and the
/// [`RuntimeCoordinator`] — the single owner of the analysis state, which
/// decides about every observation, OCR submission and result.
///
/// Created before the first frame loop, dropped after shutdown.
/// Does NOT contain Slint handles or UI types.
pub struct RuntimeSession {
    pub(crate) sequence: u64,
    pub(crate) coordinator: RuntimeCoordinator,
    pub(crate) ocr_worker: Option<OcrWorker>,
    pub(crate) ocr_interval: Duration,
    /// Structured diagnostics session (disabled by default).
    pub(crate) diagnostics: DiagnosticsContext,
    /// The previous processed frame's observation, re-fed when the frame
    /// source delivers no new frame. Deliberately not carried
    /// across a runtime swap: it was observed under the old configuration.
    pub(crate) last_frame: Option<crate::pipeline::FrameReplay>,
}

impl RuntimeSession {
    /// Create a new runtime session.
    ///
    /// * `ocr_worker` — optional OCR worker. `None` means OCR explicitly disabled.
    /// * `ocr_interval` — minimum interval between OCR submissions for the same
    ///   tooltip placement (retry pacing)
    pub fn new(ocr_worker: Option<OcrWorker>, ocr_interval: Duration) -> Self {
        Self::with_diagnostics(ocr_worker, ocr_interval, DiagnosticsContext::disabled())
    }

    /// Create a new runtime session that emits into `diagnostics`.
    pub fn with_diagnostics(
        ocr_worker: Option<OcrWorker>,
        ocr_interval: Duration,
        diagnostics: DiagnosticsContext,
    ) -> Self {
        let session = Self {
            sequence: 0,
            coordinator: RuntimeCoordinator::new(diagnostics.clone(), ocr_interval),
            ocr_worker,
            ocr_interval,
            diagnostics,
            last_frame: None,
        };
        session.attach_worker_diagnostics();
        session
    }

    /// Attach (or replace) the diagnostics session of this session and of its
    /// OCR worker. The live loop calls this after the runtime was prepared
    /// from config, because preparation creates the worker before the loop
    /// owns the diagnostics session. The coordinator's analysis session is
    /// the diagnostics session.
    pub fn attach_diagnostics(&mut self, diagnostics: DiagnosticsContext) {
        self.coordinator.attach_diagnostics(diagnostics.clone());
        self.diagnostics = diagnostics;
        self.attach_worker_diagnostics();
    }

    /// Wake the waiters of `signal` whenever the OCR worker stores a result
    /// (see [`OcrWorker::set_result_signal`]). Without a worker this does
    /// nothing.
    pub fn wake_on_ocr_results(&self, signal: Option<&d4lc_capture::FrameSignal>) {
        if let Some(worker) = &self.ocr_worker {
            worker.set_result_signal(signal.cloned());
        }
    }

    fn attach_worker_diagnostics(&self) {
        if let Some(worker) = &self.ocr_worker {
            worker.attach_diagnostics(self.diagnostics.clone());
        }
    }

    /// Carry the analysis session over from `previous` (settings hot reload
    /// replaces the session object, not the analysis session): the
    /// diagnostics record and the coordinator move into this session. The
    /// coordinator keeps its generation history, so late results of the
    /// replaced worker stay discardable, and the applied configuration
    /// change invalidates what the old configuration analyzed.
    pub fn carry_over_from(&mut self, previous: &mut RuntimeSession) {
        self.diagnostics = previous.diagnostics.clone();
        self.attach_worker_diagnostics();
        std::mem::swap(&mut self.coordinator, &mut previous.coordinator);
        self.coordinator.set_retry_interval(self.ocr_interval);
    }

    /// Structured diagnostics session of this runtime session.
    pub fn diagnostics(&self) -> &DiagnosticsContext {
        &self.diagnostics
    }

    /// Session id the diagnostics events and analysis tokens carry.
    pub fn session_id(&self) -> u64 {
        self.diagnostics.session_id()
    }

    /// Current tooltip generation of the coordinator.
    pub fn tooltip_generation(&self) -> u64 {
        self.coordinator.tooltip_generation()
    }

    /// The coordinator that owns the analysis state.
    pub fn coordinator(&self) -> &RuntimeCoordinator {
        &self.coordinator
    }

    /// Mutable access for control inputs (focus, overlay gate, stop).
    pub fn coordinator_mut(&mut self) -> &mut RuntimeCoordinator {
        &mut self.coordinator
    }

    /// Record a one-shot diagnostics event for this session.
    pub fn record_diagnostics(
        &self,
        frame_id: u64,
        phase: crate::diagnostics::DiagnosticsPhase,
        outcome: crate::diagnostics::DiagnosticsOutcome,
    ) -> bool {
        self.diagnostics.record_outcome(frame_id, phase, outcome)
    }

    /// The OCR result of the accepted analysis, if the current tooltip
    /// generation has one. A result of an invalidated generation is never
    /// returned.
    pub fn latest_ocr_result(&self) -> Option<&TooltipTextSnapshot> {
        self.coordinator
            .accepted()
            .map(|accepted| accepted.result.as_ref())
    }

    /// Reference to the optional OCR worker (for setup/status queries).
    pub fn ocr_worker_ref(&self) -> Option<&OcrWorker> {
        self.ocr_worker.as_ref()
    }

    /// Snapshot of OCR worker counters, or None if OCR is disabled.
    pub fn ocr_stats(&self) -> Option<OcrWorkerStats> {
        self.ocr_worker.as_ref().map(OcrWorker::stats)
    }

    /// Whether the OCR worker builds the live stage debug bundle. `false`
    /// without a worker.
    pub fn debug_bundle_enabled(&self) -> bool {
        self.ocr_worker
            .as_ref()
            .is_some_and(OcrWorker::debug_bundle_enabled)
    }

    /// Flip the OCR worker's live debug-bundle gate. The worker reads the
    /// flag for every job; with it off no stage is allocated.
    pub fn set_debug_bundle_enabled(&self, enabled: bool) {
        if let Some(worker) = &self.ocr_worker {
            worker.set_debug_bundle_enabled(enabled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagnosticsClock, DiagnosticsHandle, DiagnosticsVerbosity};
    use crate::{DiagnosticsOutcome, OcrWorker, Revision};
    use std::time::Duration;

    #[test]
    fn runtime_session_new_none_creates_ocr_disabled() {
        let session = RuntimeSession::new(None, Duration::from_millis(250));
        assert!(session.ocr_worker_ref().is_none());
        assert!(session.latest_ocr_result().is_none());
        assert_eq!(session.sequence, 0);
        assert_eq!(session.tooltip_generation(), 0);
        assert_eq!(session.ocr_interval, Duration::from_millis(250));
    }

    #[test]
    fn runtime_session_new_with_worker_allows_queries() {
        let worker = OcrWorker::spawn(|_| Err("mock".into()));
        let session = RuntimeSession::new(Some(worker), Duration::from_millis(250));
        assert!(session.ocr_worker_ref().is_some());
        assert!(session.latest_ocr_result().is_none());
        let stats = session.ocr_stats();
        assert!(stats.is_some());
        if let Some(stats) = stats {
            assert_eq!(stats.submitted, 0);
        }
    }

    /// The debug-bundle gate forwards to the OCR worker and is a no-op
    /// (stays off) without one.
    #[test]
    fn runtime_session_debug_bundle_gate_forwards_to_the_worker() {
        let worker = OcrWorker::spawn(|_| Err("mock".into()));
        let session = RuntimeSession::new(Some(worker), Duration::from_millis(250));
        assert!(!session.debug_bundle_enabled());
        session.set_debug_bundle_enabled(true);
        assert!(session.debug_bundle_enabled());
        session.set_debug_bundle_enabled(false);
        assert!(!session.debug_bundle_enabled());

        let disabled = RuntimeSession::new(None, Duration::from_millis(250));
        assert!(!disabled.debug_bundle_enabled());
        disabled.set_debug_bundle_enabled(true);
        assert!(
            !disabled.debug_bundle_enabled(),
            "without a worker the gate stays off"
        );
    }

    #[test]
    fn runtime_session_exposes_the_attached_diagnostics_session() {
        let session = RuntimeSession::new(None, Duration::from_millis(250));
        let disabled_id = session.session_id();
        assert!(disabled_id > 0);
        assert!(!session.diagnostics().is_enabled());
        assert_eq!(session.tooltip_generation(), 0);

        let handle = DiagnosticsHandle::new(8, DiagnosticsVerbosity::Summary);
        let session = RuntimeSession::with_diagnostics(
            None,
            Duration::from_millis(250),
            DiagnosticsContext::new(
                99,
                DiagnosticsClock::new(),
                handle.clone(),
                Revision::Applied(4),
                Revision::Applied(2),
            ),
        );
        assert_eq!(session.session_id(), 99);
        assert_eq!(
            session.diagnostics().config_revision(),
            Revision::Applied(4)
        );
        assert_eq!(session.diagnostics().rules_revision(), Revision::Applied(2));
        assert!(session.record_diagnostics(
            3,
            crate::diagnostics::DiagnosticsPhase::StateChange,
            DiagnosticsOutcome::Ok,
        ));
        assert_eq!(handle.drain().len(), 1);
    }

    #[test]
    fn tooltip_ocr_signature_is_deterministic() {
        let input = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "test_template",
            template_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 50,
            },
        };
        let hash1 = tooltip_ocr_signature(input);

        let input2 = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "test_template",
            template_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 50,
            },
        };
        let hash2 = tooltip_ocr_signature(input2);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn tooltip_ocr_signature_changes_when_crop_rect_changes() {
        let base = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "test_template",
            template_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 50,
            },
        };
        let hash_base = tooltip_ocr_signature(base);

        let diff_crop = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 11,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "test_template",
            template_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 50,
            },
        };
        assert_ne!(tooltip_ocr_signature(diff_crop), hash_base);
    }

    #[test]
    fn tooltip_ocr_signature_changes_when_template_name_changes() {
        let base = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "test_template",
            template_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 50,
            },
        };
        let hash_base = tooltip_ocr_signature(base);

        let diff_template = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "other_template",
            template_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 50,
            },
        };
        assert_ne!(tooltip_ocr_signature(diff_template), hash_base);
    }

    #[test]
    fn tooltip_ocr_signature_changes_when_template_rect_changes() {
        let base = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "test_template",
            template_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 50,
            },
        };
        let hash_base = tooltip_ocr_signature(base);

        let diff_tmpl_rect = TooltipOcrSignatureInput {
            crop_rect: Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            template_name: "test_template",
            template_rect: Rect {
                x: 5,
                y: 5,
                width: 200,
                height: 30,
            },
        };
        assert_ne!(tooltip_ocr_signature(diff_tmpl_rect), hash_base);
    }
}
