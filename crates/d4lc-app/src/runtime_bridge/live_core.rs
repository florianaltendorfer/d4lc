//! The live-loop core: one pipeline step, rule evaluation against the rules
//! snapshot, overlay presentation and the diagnostics records of the loop.

#[cfg(feature = "real-runtime")]
use super::live_loop::capture_stream_line;
#[cfg(any(test, feature = "real-runtime"))]
use super::live_state;
#[cfg(feature = "real-runtime")]
use super::live_state::LiveLoopLinks;
use super::live_state::Presentation;
#[cfg(any(test, feature = "real-runtime"))]
use super::live_state::{DebugStatus, LiveState};
#[cfg(feature = "real-runtime")]
use super::live_state::{OverlayPresenter, OverlaySurface, PresentOutcome, RulesSnapshot};
#[cfg(feature = "real-runtime")]
use crate::app_state::OverlayTextState;
#[cfg(feature = "real-runtime")]
use d4lc_core::Rect;
use d4lc_overlay_x11::OverlayFrame;
#[cfg(any(test, feature = "real-runtime"))]
use d4lc_overlay_x11::OverlayStatus;
#[cfg(feature = "real-runtime")]
use d4lc_rules::RuleOverlayDescriptor;
#[cfg(feature = "real-runtime")]
use d4lc_runtime::diagnostics::DiscardReason;
use d4lc_runtime::diagnostics::{DiagnosticsOutcome, DiagnosticsPhase};
#[cfg(any(test, feature = "real-runtime"))]
use d4lc_runtime::RuntimeLifecycle;
use std::sync::Arc;

/// Record one runtime state-transition event (start, applied configuration,
/// stop, errors) with a short detail. The detail closure only runs when the
/// diagnostics session is enabled.
pub(super) fn record_state_change(
    context: &d4lc_runtime::DiagnosticsContext,
    frame_id: u64,
    outcome: DiagnosticsOutcome,
    detail: impl FnOnce() -> String,
) {
    if !context.is_enabled() {
        return;
    }
    let event = context
        .begin(frame_id, DiagnosticsPhase::StateChange)
        .with_outcome(outcome)
        .with_detail(detail());
    context.record(event);
}

/// Record the evaluation and presentation-submission events of one drawn frame.
///
/// The events belong to the analysis of `chain_frame` (the frame whose OCR
/// result the drawn content comes from, or the drawn frame when there is none),
/// while `drawn_frame` is the frame actually handed to the backend this cycle.
/// Both are recorded: the split between the observation frame and the
/// publication frame is what this instrumentation exists to make visible,
/// so it is named in the data instead of being
/// smoothed over.
fn record_evaluation_and_submit(
    context: &d4lc_runtime::DiagnosticsContext,
    chain_frame: u64,
    drawn_frame: u64,
    descriptor_count: usize,
    adopted_source_frame: Option<u64>,
    debug_lock: bool,
) {
    if !context.is_enabled() {
        return;
    }
    let adopted = match adopted_source_frame {
        Some(source) => format!(" adopted_source_frame={source}"),
        None => String::new(),
    };
    let evaluate = context
        .begin(chain_frame, DiagnosticsPhase::Evaluate)
        .with_detail(format!(
            "descriptors={descriptor_count} drawn_frame={drawn_frame}{adopted} \
             debug_lock={debug_lock}"
        ));
    context.record(evaluate);

    let submit = context
        .begin(chain_frame, DiagnosticsPhase::PresentationSubmit)
        .with_detail(format!(
            "sequence={drawn_frame} descriptors={descriptor_count} debug_lock={debug_lock}"
        ));
    context.record(submit);
}

/// Record the backend's return from one draw call (`draw_completed_at`).
///
/// This is the draw *call* completion the app observes — not a confirmed
/// visible frame, which would need an external visibility measurement the
/// runtime does not have.
fn record_draw_completed(
    context: &d4lc_runtime::DiagnosticsContext,
    chain_frame: u64,
    drawn_frame: u64,
    descriptor_count: usize,
    outcome: DiagnosticsOutcome,
    skipped_draw: bool,
) {
    let skipped = if skipped_draw {
        " draw=unchanged(skipped)"
    } else {
        ""
    };
    let event = context
        .begin(chain_frame, DiagnosticsPhase::DrawCompleted)
        .with_outcome(outcome)
        .with_detail(format!(
            "sequence={drawn_frame} descriptors={descriptor_count}{skipped}"
        ));
    context.record(event);
}

/// Drain whatever the runtime buffered and close the record with its aggregate
/// statistics line. Every live-loop exit path goes through this, so the last
/// events before an exit are never left in the buffer. `loop_timings` is the
/// per-tick percentile line exported with the aggregate.
pub(super) fn finish_diagnostics(
    diagnostics: &mut crate::diagnostics::AppDiagnostics,
    context: &d4lc_runtime::DiagnosticsContext,
    reason: &str,
    loop_timings: Option<String>,
) {
    diagnostics.set_loop_timings(loop_timings);
    diagnostics.drain_runtime(context.handle());
    diagnostics.finish(reason);
}

/// Assemble the real-runtime live loop's `OverlayFrame` from a pipeline
/// result plus the app-owned rule descriptors. The app counterpart of
/// the CLI's `build_live_overlay_frame_from_result`: pipeline metadata —
/// including the per-frame `cursor_window_relative` the overlay menu
/// hover/lock state machine consumes — is forwarded verbatim, never
/// dropped or defaulted.
///
/// `hotpath_ms` is the duration of `process_one_frame` only: capture,
/// detection, crop-stability/scheduler work and result polling on the loop
/// thread. The asynchronous OCR work and the time until the overlay is
/// visible are deliberately not part of it; the diagnostics
/// events carry those phases separately.
#[cfg(feature = "real-runtime")]
fn live_frame_from_pipeline_result(
    pipeline_result: &d4lc_runtime::ProcessedFrameResult,
    rule_descriptors: Arc<[RuleOverlayDescriptor]>,
    text_font_px: u32,
) -> OverlayFrame {
    OverlayFrame {
        sequence: pipeline_result.overlay_frame.sequence,
        tooltip_found: pipeline_result.overlay_frame.tooltip_found,
        hotpath_ms: pipeline_result.overlay_frame.hotpath_ms,
        cursor_window_relative: pipeline_result.overlay_frame.cursor_window_relative,
        rule_descriptors,
        text_font_px,
    }
}

/// Debug capture lock — the app-side half of the
/// `ToggleDebugLockScreencapture` hotkey (Ctrl+F11 by default).
///
/// Upstream flips `ScreenCaptureHandler.IsScreencaptureLocked`
/// (`ScreenCaptureHandler.cs:86-89`) and the capture task then stops
/// pulling a new bitmap, re-sending the one it already had
/// (`ScreenCaptureHandler.cs:246-264`: `if (!IsScreencaptureLocked)
/// _currentScreen = _screenCapture.GetScreenCapture(windowHandle) ??
/// _currentScreen;`). Everything drawn downstream therefore stays on the
/// latest screencapture — the hotkey tooltip's contract ("Locks the latest
/// screencapture as the default input image").
///
/// The port keeps that user-visible contract on the app side: while the lock
/// is engaged the live loop keeps drawing — and publishing — the frame and
/// the presentation it had when the lock was engaged, so the overlay and the
/// current-item card stop following the game together. The pipeline keeps
/// running (cursor polling, the coordinator, OCR polling), so releasing the
/// lock resumes on the live analysis, and the overlay menu stays interactive
/// while locked. The frozen image is marked as frozen: a hard invalidation
/// (focus lost, minimized, overlay off, stop) still hides it.
#[derive(Debug, Default)]
struct DebugCaptureLock {
    engaged: bool,
    frozen: Option<(OverlayFrame, Arc<Presentation>)>,
}

impl DebugCaptureLock {
    /// Observe the app-owned lock bit (`RuntimeStatus::debug_lock_screencapture`,
    /// flipped by the hotkey handler).
    fn observe(&mut self, engaged: bool) {
        self.engaged = engaged;
    }

    /// The frame and presentation to draw this cycle, and whether they are
    /// the frozen ones: the frozen pair while the lock is engaged, otherwise
    /// the fresh pair — which also becomes what a later lock freezes on. A
    /// lock engaged before the first frame existed freezes that first frame,
    /// the equivalent of upstream's `_currentScreen` still being `null`.
    fn frame_for(
        &mut self,
        fresh: OverlayFrame,
        presentation: Arc<Presentation>,
    ) -> (OverlayFrame, Arc<Presentation>, bool) {
        if self.engaged {
            return match &self.frozen {
                Some((frame, frozen)) => (frame.clone(), Arc::clone(frozen), true),
                None => {
                    self.frozen = Some((fresh.clone(), Arc::clone(&presentation)));
                    (fresh, presentation, true)
                }
            };
        }
        self.frozen = Some((fresh.clone(), Arc::clone(&presentation)));
        (fresh, presentation, false)
    }
}

// ---------------------------------------------------------------------------
// Live-loop lifecycle
// ---------------------------------------------------------------------------

/// The presentation of an analysis without a rules snapshot yet: the
/// explicit empty evaluation.
pub(super) fn unevaluated_presentation(
    analysis: Arc<d4lc_runtime::AnalysisSnapshot>,
) -> Presentation {
    Presentation {
        analysis,
        rules_revision: None,
        descriptors: Arc::from([]),
        affix_traces: Vec::new(),
    }
}

/// What one live-loop step produced.
#[cfg(feature = "real-runtime")]
#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the step outcome fields are read by the live-loop tests"
    )
)]
pub(crate) struct LiveStep {
    /// The frame processed this step.
    pub frame_sequence: u64,
    /// The presentation drawn and published this step.
    pub presentation: Arc<Presentation>,
    /// What the presenter did (`None` without an overlay surface, or when
    /// the loop's epoch was closed).
    pub draw: Option<PresentOutcome>,
    /// Whether the state reached the UI mailbox.
    pub published: bool,
    /// Stage costs of this step's pipeline pass.
    pub stage_us: d4lc_runtime::FrameStageTimings,
    /// Presentation cost of this step: the draw call(s) on the overlay
    /// surface. 0 when nothing was presented.
    pub present_us: u64,
    /// The surface's raster/commit split for its last draw; stages the
    /// surface did not measure stay absent.
    pub present_timings: d4lc_overlay_x11::OverlayDrawTimings,
}

/// The runtime side of the live loop: one step processes a frame through the
/// coordinator, evaluates the accepted analysis against the newest immutable
/// rules snapshot, and hands the same presentation to the overlay presenter
/// and the UI mailbox. Nothing here reads UI-owned state.
#[cfg(feature = "real-runtime")]
pub(crate) struct LiveLoopCore<S: OverlaySurface> {
    epoch: u64,
    links: LiveLoopLinks,
    presenter: Option<OverlayPresenter<S>>,
    /// Status of a loop that could not create any surface: the synthesized
    /// no-surface status whose limitation is the loop's own reason. Published
    /// with every state so the UI reports it instead of a static default.
    absent_surface: Option<OverlayStatus>,
    diagnostics: d4lc_runtime::DiagnosticsContext,
    presentation: Option<Arc<Presentation>>,
    /// Descriptors of the empty analysis per (rules revision, window): a
    /// moving tooltip starts a new generation on every frame, and without
    /// an item every one of them evaluates to the same descriptors.
    empty_descriptors: Option<(u64, Option<Rect>, live_state::RuleEvaluation)>,
    paragon_navigation: super::paragon::ParagonNavigation,
    debug_lock: DebugCaptureLock,
    last_drawn_content: bool,
    stopped: bool,
    /// Per-tick stage-cost rings; the loop thread exports the
    /// percentiles to the Debug panel and the session record.
    loop_timings: d4lc_runtime::diagnostics::LoopTimingRecorder,
    /// Presentation generation whose accepted analysis was last confirmed
    /// drawn (`overlay_committed_at`).
    last_committed_presentation: Option<u64>,
    /// Presentation generation whose invalidation was last confirmed drawn
    /// (`hidden_at`).
    last_hidden_presentation: Option<u64>,
    /// The newest events of the app's bounded diagnostics ring, newest first,
    /// at most `live_state::RECENT_DIAGNOSTICS`. Refreshed
    /// by the loop after each drain and published with every state.
    recent_diagnostics: Vec<d4lc_runtime::DiagnosticsEvent>,
}

#[cfg(feature = "real-runtime")]
impl<S: OverlaySurface> LiveLoopCore<S> {
    pub(crate) fn new(
        epoch: u64,
        links: LiveLoopLinks,
        presenter: Option<OverlayPresenter<S>>,
        diagnostics: d4lc_runtime::DiagnosticsContext,
    ) -> Self {
        Self {
            epoch,
            links,
            presenter,
            absent_surface: None,
            diagnostics,
            presentation: None,
            empty_descriptors: None,
            paragon_navigation: super::paragon::ParagonNavigation::default(),
            debug_lock: DebugCaptureLock::default(),
            last_drawn_content: false,
            stopped: false,
            loop_timings: d4lc_runtime::diagnostics::LoopTimingRecorder::default(),
            last_committed_presentation: None,
            last_hidden_presentation: None,
            recent_diagnostics: Vec::new(),
        }
    }

    /// Record why this loop has no overlay surface. Every published state then
    /// carries that status instead of the UI keeping a pending default
    pub(crate) fn set_absent_surface(&mut self, status: OverlayStatus) {
        self.absent_surface = Some(status);
    }

    /// Refresh the bounded, newest-first diagnostics window the Debug screen
    /// renders from the app's own bounded ring. The list
    /// never grows: it keeps at most `live_state::RECENT_DIAGNOSTICS` events.
    pub(crate) fn set_recent_diagnostics(
        &mut self,
        events: &std::collections::VecDeque<d4lc_runtime::DiagnosticsEvent>,
    ) {
        self.recent_diagnostics = events
            .iter()
            .rev()
            .take(live_state::RECENT_DIAGNOSTICS)
            .cloned()
            .collect();
    }

    pub(crate) fn presenter(&self) -> Option<&OverlayPresenter<S>> {
        self.presenter.as_ref()
    }

    /// Mutable presenter access for the loop's per-tick menu-settings
    /// application.
    pub(crate) fn presenter_mut(&mut self) -> Option<&mut OverlayPresenter<S>> {
        self.presenter.as_mut()
    }

    /// The per-tick stage-cost rings. The loop thread records one
    /// sample per tick and exports the percentiles with the session record.
    pub(crate) fn loop_timings(&self) -> &d4lc_runtime::diagnostics::LoopTimingRecorder {
        &self.loop_timings
    }

    /// The overlay side of the snapshot published this cycle: the started
    /// handle's own status and text diagnostics, or the no-surface status of a
    /// loop without a surface.
    fn overlay_snapshot(&self) -> live_state::OverlaySnapshot {
        match self.presenter.as_ref() {
            Some(presenter) => live_state::OverlaySnapshot {
                status: Some(presenter.status().clone()),
                text: presenter.surface().text_diagnostics(),
            },
            None => live_state::OverlaySnapshot {
                status: self.absent_surface.clone(),
                text: OverlayTextState::default(),
            },
        }
    }

    /// Whether the loop may still present and publish.
    pub(crate) fn is_open(&self) -> bool {
        !self.stopped && self.links.mailbox.is_open(self.epoch)
    }

    /// Apply the pending control inputs to the coordinator. Runs before and
    /// after the frame work, so an invalidation that arrives while a frame
    /// is captured is applied before anything is presented.
    fn apply_controls(
        &mut self,
        runtime: &mut d4lc_runtime::PreparedRuntime,
        session: &mut d4lc_runtime::RuntimeSession,
    ) {
        let pending = self.links.controls.drain();
        if let Some(geometry) = pending.window_geometry {
            // The game window moved or was resized: the capture
            // follows it, the overlay surface moves with it, and the
            // coordinator starts a new window revision at once, so an
            // analysis of the old geometry is never presented at the new one.
            runtime.set_window_geometry(geometry);
            if let Some(presenter) = self.presenter.as_mut() {
                if let Err(error) = presenter.set_window_geometry(geometry) {
                    record_state_change(
                        &self.diagnostics,
                        0,
                        DiagnosticsOutcome::failed(error),
                        || "overlay could not follow the game window".to_string(),
                    );
                }
            }
            if session.coordinator_mut().observe_window(geometry) {
                self.record_invalidation(d4lc_runtime::InvalidationCause::WindowChanged);
            }
        }
        if let Some(window) = pending.game_window {
            if let Some(cause) = session.coordinator_mut().set_game_window(window) {
                self.record_invalidation(cause);
            }
        }
        if let Some(enabled) = pending.overlay_enabled {
            if let Some(cause) = session.coordinator_mut().set_overlay_enabled(enabled) {
                self.record_invalidation(cause);
            }
        }
    }

    fn record_invalidation(&self, cause: d4lc_runtime::InvalidationCause) {
        record_state_change(
            &self.diagnostics,
            0,
            DiagnosticsOutcome::discarded(cause.discard_reason()),
            || format!("invalidated: {}", cause.as_str()),
        );
    }

    /// The newest rules snapshot; its revision becomes the applied rules
    /// revision the tokens and events carry, and its overlay switch gates
    /// the coordinator.
    fn sync_rules(&self, session: &mut d4lc_runtime::RuntimeSession) -> Option<Arc<RulesSnapshot>> {
        let rules = self.links.rules.current()?;
        let revision = d4lc_runtime::Revision::Applied(rules.revision());
        if self.diagnostics.rules_revision() != revision {
            self.diagnostics.set_rules_revision(revision);
            if session.coordinator().overlay_enabled() != rules.overlay_enabled() {
                if let Some(cause) = session
                    .coordinator_mut()
                    .set_overlay_enabled(rules.overlay_enabled())
                {
                    self.record_invalidation(cause);
                }
            }
        }
        Some(rules)
    }

    /// The presentation of the current analysis: re-evaluated only when the
    /// accepted analysis or the rules changed. A pure rules change
    /// re-evaluates the still valid analysis without new OCR.
    fn presentation_for(
        &mut self,
        analysis: Arc<d4lc_runtime::AnalysisSnapshot>,
        rules: Option<&RulesSnapshot>,
    ) -> Arc<Presentation> {
        let rules_revision = rules.map(RulesSnapshot::revision);
        if let Some(current) = &self.presentation {
            let same_analysis =
                Arc::ptr_eq(&current.analysis, &analysis) || current.analysis == analysis;
            if same_analysis && current.rules_revision == rules_revision {
                return Arc::clone(current);
            }
        }
        let window_geometry = self
            .presenter
            .as_ref()
            .map(|presenter| presenter.status().window_geometry);
        let presentation = Arc::new(match rules {
            Some(rules) if analysis.item.is_none() => {
                let cached = self
                    .empty_descriptors
                    .as_ref()
                    .filter(|(revision, window, _)| {
                        *revision == rules.revision() && *window == window_geometry
                    });
                let evaluation = match cached {
                    Some((_, _, evaluation)) => evaluation.clone(),
                    None => {
                        let evaluation = rules.evaluate(
                            &analysis,
                            window_geometry,
                            Some(&self.paragon_navigation),
                        );
                        self.empty_descriptors =
                            Some((rules.revision(), window_geometry, evaluation.clone()));
                        evaluation
                    }
                };
                Presentation {
                    descriptors: evaluation.descriptors,
                    affix_traces: evaluation.affix_traces,
                    rules_revision,
                    analysis,
                }
            }
            Some(rules) => {
                let evaluation =
                    rules.evaluate(&analysis, window_geometry, Some(&self.paragon_navigation));
                Presentation {
                    descriptors: evaluation.descriptors,
                    affix_traces: evaluation.affix_traces,
                    rules_revision,
                    analysis,
                }
            }
            None => unevaluated_presentation(analysis),
        });
        self.presentation = Some(Arc::clone(&presentation));
        presentation
    }

    fn presentation_for_cursor(
        &mut self,
        analysis: Arc<d4lc_runtime::AnalysisSnapshot>,
        rules: Option<&RulesSnapshot>,
        cursor: Option<d4lc_core::Point>,
        now: std::time::Instant,
    ) -> Arc<Presentation> {
        let presentation = self.presentation_for(Arc::clone(&analysis), rules);
        if self
            .paragon_navigation
            .observe(&presentation.descriptors, cursor, now)
        {
            self.presentation = None;
            self.empty_descriptors = None;
            return self.presentation_for(analysis, rules);
        }
        presentation
    }

    /// Process one frame and present its result.
    pub(crate) fn step(
        &mut self,
        runtime: &mut d4lc_runtime::PreparedRuntime,
        session: &mut d4lc_runtime::RuntimeSession,
        debug_capture_locked: bool,
    ) -> Result<LiveStep, d4lc_runtime::RuntimePipelineError> {
        self.apply_controls(runtime, session);
        let rules = self.sync_rules(session);
        // Live debug-bundle gate: the toggle travels the
        // rules snapshot; the OCR worker reads this flag for every job, so
        // with DebugInfo off no stage is allocated at all.
        let debug_info_enabled = rules
            .as_deref()
            .is_some_and(RulesSnapshot::debug_info_enabled);
        if session.debug_bundle_enabled() != debug_info_enabled {
            session.set_debug_bundle_enabled(debug_info_enabled);
        }
        let pipeline_result = d4lc_runtime::process_one_frame(session, runtime)?;
        let stage_us = pipeline_result.stage_us;
        self.apply_controls(runtime, session);

        let surface_cursor = self
            .presenter
            .as_mut()
            .and_then(|presenter| presenter.poll_cursor_window_relative());
        let presentation = self.presentation_for_cursor(
            session.coordinator().snapshot(),
            rules.as_deref(),
            pipeline_result
                .overlay_frame
                .cursor_window_relative
                .or(surface_cursor),
            std::time::Instant::now(),
        );
        let fresh = live_frame_from_pipeline_result(
            &pipeline_result,
            Arc::clone(&presentation.descriptors),
            rules.as_deref().map_or(
                d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
                RulesSnapshot::overlay_font_px,
            ),
        );
        self.debug_lock.observe(debug_capture_locked);
        let (mut frame, mut shown, frozen) = self
            .debug_lock
            .frame_for(fresh.clone(), Arc::clone(&presentation));

        // A stop (closed epoch) may have arrived while the frame was
        // processed: nothing of this session is presented or published.
        if !self.is_open() {
            return Ok(LiveStep {
                frame_sequence: pipeline_result.overlay_frame.sequence,
                presentation,
                draw: None,
                published: false,
                stage_us,
                present_us: 0,
                present_timings: d4lc_overlay_x11::OverlayDrawTimings::default(),
            });
        }

        // ── Present: the overlay draws the accepted presentation ────
        let present_start = std::time::Instant::now();
        let draw = match self.presenter.as_mut() {
            Some(presenter) => {
                let mut outcome = presenter.present(frame.clone(), &shown, frozen);
                if outcome == PresentOutcome::Refused && frozen {
                    // The frozen image is older than a hard clear: the live
                    // (hidden) presentation replaces it.
                    frame = fresh;
                    shown = Arc::clone(&presentation);
                    outcome = presenter.present(frame.clone(), &shown, false);
                }
                Some(outcome)
            }
            None => None,
        };
        let present_us = d4lc_overlay_x11::OverlayDrawTimings::micros(present_start.elapsed());
        let present_timings = self
            .presenter
            .as_ref()
            .map(|presenter| presenter.surface().draw_timings())
            .unwrap_or_default();

        // ── Publish: the UI projects the very same presentation ─────
        // The generation the renderer drew, the frame drawn and what the
        // presenter did with it, the overlay status and text state, and the
        // freeze flag ride along, so the UI reports the renderer's own
        // generation instead of assuming it drew what the UI shows.
        let debug = DebugStatus {
            frame_sequence: frame.sequence,
            hotpath_ms: frame.hotpath_ms,
            tooltip_found: frame.tooltip_found,
            descriptor_count: frame.rule_descriptors.len(),
            generation: shown.generation(),
            source_frame: shown.analysis.token.map(|token| token.source_frame_id),
            drawn_generation: self
                .presenter
                .as_ref()
                .and_then(OverlayPresenter::last_drawn),
            draw: draw.clone(),
            frozen,
            // The asynchronous half of the pipeline: the worker's own
            // cumulative counters, so the UI shows the phase the frame number
            // excludes instead of implying it.
            ocr: session
                .ocr_worker_ref()
                .map(|worker| live_state::OcrPhaseStatus::from_stats(&worker.stats())),
            // Percentiles of the per-tick stage rings, as of the
            // ticks recorded so far.
            loop_timings: {
                let percentiles = self.loop_timings.percentiles();
                if percentiles.is_empty() {
                    None
                } else {
                    Some(percentiles)
                }
            },
            capture_stream: capture_stream_line(runtime),
            // The app's bounded ring, newest first; the
            // list below is the same window the Debug screen renders.
            recent_diagnostics: self.recent_diagnostics.clone(),
        };
        let published = self.links.mailbox.publish(LiveState {
            epoch: self.epoch,
            lifecycle: RuntimeLifecycle::Running,
            presentation: Arc::clone(&shown),
            debug,
            overlay: self.overlay_snapshot(),
            // The shown snapshot's own bundle: it follows
            // the item the UI displays, and DebugInfo off never exposes a
            // bundle the loop may still hold from before the toggle.
            debug_bundle: debug_info_enabled
                .then(|| shown.analysis.debug_bundle.clone())
                .flatten(),
            debug_info_enabled,
            published_at: std::time::SystemTime::now(),
        });

        // ── Evaluation / presentation / draw diagnostics ────────────
        // The chain frame is the frame the accepted analysis was read from
        // (the analysis the drawn content comes from); without one it is the
        // frame drawn this cycle.
        let adopted = shown.analysis.token.map(|token| token.source_frame_id);
        let chain_frame = adopted.unwrap_or(frame.sequence);
        let content_bearing = frame.tooltip_found || !frame.rule_descriptors.is_empty();
        if content_bearing || self.last_drawn_content {
            record_evaluation_and_submit(
                &self.diagnostics,
                chain_frame,
                frame.sequence,
                frame.rule_descriptors.len(),
                adopted,
                frozen,
            );
        }
        if let Some(outcome) = &draw {
            let diagnostics_outcome = match outcome {
                PresentOutcome::Drawn | PresentOutcome::Unchanged => DiagnosticsOutcome::Ok,
                PresentOutcome::Refused => DiagnosticsOutcome::discarded(DiscardReason::Superseded),
                PresentOutcome::Failed(error) => DiagnosticsOutcome::failed(error.clone()),
            };
            record_draw_completed(
                &self.diagnostics,
                chain_frame,
                frame.sequence,
                frame.rule_descriptors.len(),
                diagnostics_outcome,
                outcome == &PresentOutcome::Unchanged,
            );
            if *outcome == PresentOutcome::Drawn {
                self.record_overlay_commitment(&shown, chain_frame);
            }
        }
        self.last_drawn_content = content_bearing;

        Ok(LiveStep {
            frame_sequence: pipeline_result.overlay_frame.sequence,
            presentation: shown,
            draw,
            published,
            stage_us,
            present_us,
            present_timings,
        })
    }

    /// Record the overlay-commitment time points of one confirmed draw
    /// `overlay_committed_at` for the first confirmed draw of a
    /// presentation that carries the accepted analysis, `hidden_at` for the
    /// first confirmed draw of an empty presentation after an invalidation.
    /// Each is recorded once per presentation generation, on the frame the
    /// drawn content was analyzed from. The duration is absent when the
    /// anchor (`first_seen`, the invalidation time) is unknown — never a
    /// fabricated 0.
    fn record_overlay_commitment(&mut self, shown: &Presentation, chain_frame: u64) {
        let generation = shown.generation();
        if shown.analysis.item.is_some() && self.last_committed_presentation != Some(generation) {
            self.last_committed_presentation = Some(generation);
            let mut event = self
                .diagnostics
                .begin(chain_frame, DiagnosticsPhase::OverlayCommit);
            if let Some(first_seen) = shown
                .analysis
                .token
                .and_then(|token| token.first_seen_at_micros)
            {
                event.duration_micros =
                    Some(self.diagnostics.now_micros().saturating_sub(first_seen));
            }
            self.diagnostics.record(event);
        }
        if shown.analysis.invalidation.is_some()
            && !shown.analysis.has_item()
            && self.last_hidden_presentation != Some(generation)
        {
            self.last_hidden_presentation = Some(generation);
            let mut event = self
                .diagnostics
                .begin(chain_frame, DiagnosticsPhase::OverlayHidden);
            if let Some(invalidated) = shown.analysis.invalidated_at_micros {
                event.duration_micros =
                    Some(self.diagnostics.now_micros().saturating_sub(invalidated));
            }
            self.diagnostics.record(event);
        }
    }

    /// Stop: invalidate the analysis and hide the overlay before anything
    /// else ends. Idempotent.
    pub(crate) fn stop(&mut self, session: &mut d4lc_runtime::RuntimeSession, sequence: u64) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        session.coordinator_mut().begin_stop();
        if let Some(presenter) = self.presenter.as_mut() {
            let outcome = match presenter.clear_all(sequence) {
                Ok(()) => DiagnosticsOutcome::Ok,
                Err(error) => DiagnosticsOutcome::failed(error),
            };
            record_draw_completed(&self.diagnostics, sequence, sequence, 0, outcome, false);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[cfg(feature = "real-runtime")]
    use crate::app_state::AppState;
    use crate::runtime_bridge::live_state::LiveStateMailbox;
    #[cfg(feature = "real-runtime")]
    use crate::runtime_bridge::live_state::{LiveStateProjection, RulesPublisher};
    use crate::runtime_bridge::overlay::headless_overlay_status;
    #[cfg(feature = "real-runtime")]
    use crate::runtime_bridge::test_support::rules_state_with_descriptors;
    use crate::runtime_bridge::test_support::{analysis_presentation, live_state_fixture};
    use std::path::PathBuf;
    #[cfg(feature = "real-runtime")]
    use std::sync::atomic::Ordering;
    #[cfg(feature = "real-runtime")]
    use std::time::Duration;

    // -------------------------------------------------------------------
    // Live-loop diagnostics seams
    // -------------------------------------------------------------------

    /// Fixture: an enabled diagnostics session over a bounded handle.
    fn diagnostics_fixture() -> (
        d4lc_runtime::DiagnosticsContext,
        d4lc_runtime::DiagnosticsHandle,
    ) {
        let handle =
            d4lc_runtime::DiagnosticsHandle::new(64, d4lc_runtime::DiagnosticsVerbosity::Summary);
        let context = d4lc_runtime::DiagnosticsContext::new(
            77,
            d4lc_runtime::DiagnosticsClock::new(),
            handle.clone(),
            d4lc_runtime::Revision::Applied(1),
            d4lc_runtime::Revision::Unknown,
        );
        (context, handle)
    }

    /// Temp diagnostics directory inside the workspace target directory (never
    /// the real user home); the caller cleans it up.
    fn diagnostics_temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-diagnostics-dirs")
            .join(format!("runtime-bridge-{label}-{nanos}"));
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|error| panic!("temp diagnostics dir must be creatable: {error}"));
        dir
    }

    #[test]
    fn state_change_events_carry_the_outcome_and_detail() {
        let (context, handle) = diagnostics_fixture();
        record_state_change(&context, 0, DiagnosticsOutcome::Ok, || {
            "live loop started".to_string()
        });
        record_state_change(
            &context,
            3,
            DiagnosticsOutcome::failed("settings reload failed: broken"),
            || "settings hot reload".to_string(),
        );

        let events = handle.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].phase, DiagnosticsPhase::StateChange);
        assert_eq!(events[0].outcome, DiagnosticsOutcome::Ok);
        assert_eq!(events[0].detail.as_deref(), Some("live loop started"));
        assert_eq!(events[1].frame_id, 3);
        assert_eq!(
            events[1].outcome,
            DiagnosticsOutcome::failed("settings reload failed: broken")
        );
    }

    /// The detail closure must not run for a disabled session: instrumentation
    /// costs nothing when it is switched off.
    #[test]
    fn disabled_diagnostics_context_does_not_build_details() {
        let mut built = false;
        let context = d4lc_runtime::DiagnosticsContext::disabled();
        record_state_change(&context, 0, DiagnosticsOutcome::Ok, || {
            built = true;
            "payload".to_string()
        });
        assert!(!built);
        assert_eq!(context.dropped_events(), 0);
    }

    #[test]
    fn presentation_chain_events_name_the_analysis_and_the_drawn_frame() {
        let (context, handle) = diagnostics_fixture();
        // The adopted item came from frame 12; this cycle drew frame 15.
        record_evaluation_and_submit(&context, 12, 15, 4, Some(12), false);
        record_draw_completed(&context, 12, 15, 4, DiagnosticsOutcome::Ok, false);

        let events = handle.drain();
        let phases: Vec<DiagnosticsPhase> = events.iter().map(|event| event.phase).collect();
        assert_eq!(
            phases,
            vec![
                DiagnosticsPhase::Evaluate,
                DiagnosticsPhase::PresentationSubmit,
                DiagnosticsPhase::DrawCompleted
            ]
        );
        // The whole publication chain belongs to the analysed source frame.
        assert!(events.iter().all(|event| event.frame_id == 12));
        assert!(events.iter().all(|event| event.session_id == 77));
        let evaluate_detail = events[0].detail.as_deref().unwrap_or_default();
        assert!(evaluate_detail.contains("drawn_frame=15"));
        assert!(evaluate_detail.contains("adopted_source_frame=12"));
        assert!(evaluate_detail.contains("debug_lock=false"));
        let draw_detail = events[2].detail.as_deref().unwrap_or_default();
        assert!(draw_detail.contains("sequence=15"));
        assert!(draw_detail.contains("descriptors=4"));

        // A draw failure carries the backend's own structured reason.
        record_draw_completed(
            &context,
            12,
            15,
            4,
            DiagnosticsOutcome::failed("draw failed: no display"),
            false,
        );
        let events = handle.drain();
        assert_eq!(
            events[0].outcome,
            DiagnosticsOutcome::failed("draw failed: no display")
        );
    }

    #[test]
    fn finish_diagnostics_drains_the_buffer_before_the_aggregate() {
        let dir = diagnostics_temp_dir("finish-drain");
        let mut diagnostics = crate::diagnostics::AppDiagnostics::with_limits(
            dir.clone(),
            d4lc_runtime::DiagnosticsVerbosity::Summary,
            crate::diagnostics::DiagnosticsLimits::default(),
        );
        let context = d4lc_runtime::DiagnosticsContext::new(
            5,
            d4lc_runtime::DiagnosticsClock::new(),
            diagnostics.handle().clone(),
            d4lc_runtime::Revision::Applied(1),
            d4lc_runtime::Revision::Unknown,
        );
        record_state_change(&context, 0, DiagnosticsOutcome::Ok, || {
            "live loop started".to_string()
        });

        finish_diagnostics(&mut diagnostics, &context, "shutdown signalled", None);

        let mut content = String::new();
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("read dir: {error}"))
            .filter_map(|entry| entry.ok())
        {
            if entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
            {
                content.push_str(
                    &std::fs::read_to_string(entry.path())
                        .unwrap_or_else(|error| panic!("read log: {error}")),
                );
            }
        }
        assert!(
            content.contains("\"phase\":\"state_change\""),
            "the buffered event must be drained into the record: {content}"
        );
        assert!(content.contains("\"record\":\"aggregate\""));
        assert_eq!(diagnostics.stats().recorded, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -------------------------------------------------------------------
    // Debug capture lock (Ctrl+F11 hotkey)
    // -------------------------------------------------------------------

    fn overlay_frame_fixture(sequence: u64, tooltip_found: bool) -> OverlayFrame {
        OverlayFrame {
            sequence,
            tooltip_found,
            hotpath_ms: sequence as u128,
            cursor_window_relative: None,
            rule_descriptors: std::sync::Arc::from([]),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        }
    }

    fn presentation_fixture(generation: u64) -> Arc<Presentation> {
        Arc::new(Presentation {
            analysis: Arc::new(d4lc_runtime::AnalysisSnapshot {
                presentation_generation: generation,
                ..d4lc_runtime::AnalysisSnapshot::empty(1)
            }),
            rules_revision: None,
            descriptors: Arc::from([]),
            affix_traces: Vec::new(),
        })
    }

    /// Frame and presentation generation of one lock decision.
    fn lock_cycle(lock: &mut DebugCaptureLock, sequence: u64, found: bool) -> (u64, u64, bool) {
        let (frame, presentation, frozen) = lock.frame_for(
            overlay_frame_fixture(sequence, found),
            presentation_fixture(sequence),
        );
        (frame.sequence, presentation.generation(), frozen)
    }

    #[test]
    fn debug_capture_lock_freezes_the_drawn_frame_and_its_presentation() {
        // Upstream `ScreenCaptureHandler.IsScreencaptureLocked`
        // (ScreenCaptureHandler.cs:86-89, 246-264): while the lock is on,
        // the capture task re-sends the bitmap it already had, so the
        // visible input image stays the latest screencapture. The port
        // freezes the drawn frame together with the presentation the UI
        // projects, so the overlay and the card stop following the game
        // together.
        let mut debug_lock = DebugCaptureLock::default();
        assert_eq!(lock_cycle(&mut debug_lock, 6, false), (6, 6, false));

        debug_lock.observe(true);
        assert_eq!(
            lock_cycle(&mut debug_lock, 7, false),
            (6, 6, true),
            "the engaging cycle draws the image it had"
        );
        assert_eq!(
            lock_cycle(&mut debug_lock, 8, true),
            (6, 6, true),
            "locked: later frames change nothing"
        );

        // Release: the next cycle follows the live frame again, and the
        // next lock freezes on what was drawn before it.
        debug_lock.observe(false);
        assert_eq!(lock_cycle(&mut debug_lock, 10, false), (10, 10, false));
        debug_lock.observe(true);
        assert_eq!(
            lock_cycle(&mut debug_lock, 11, false),
            (10, 10, true),
            "the second lock freezes the frame drawn just before it"
        );
    }

    #[test]
    fn debug_capture_lock_engaged_before_the_first_frame_freezes_that_frame() {
        let mut debug_lock = DebugCaptureLock::default();
        debug_lock.observe(true);
        assert_eq!(lock_cycle(&mut debug_lock, 1, true), (1, 1, true));
        assert_eq!(
            lock_cycle(&mut debug_lock, 2, false),
            (1, 1, true),
            "with nothing drawn yet the first processed frame becomes the frozen image"
        );
    }

    // -------------------------------------------------------------------
    // Live-loop cursor forwarding tests
    // -------------------------------------------------------------------

    #[cfg(feature = "real-runtime")]
    #[test]
    fn live_frame_from_pipeline_result_carries_the_pipeline_cursor() {
        // The real-runtime loop assembles its OverlayFrame through
        // `live_frame_from_pipeline_result`; the pipeline's per-frame
        // cursor observation (native X11 pointer query in production, synthetic value
        // here) must reach that frame verbatim — it is the input of the
        // overlay menu hover/lock state machine. Production of the value
        // from a real cursor source is proven by d4lc-runtime's
        // synthetic-source pipeline test.
        use d4lc_core::Point;
        use d4lc_runtime::{FrameStatus, ProcessedFrameResult};

        let pipeline_result = ProcessedFrameResult {
            stage_us: Default::default(),
            overlay_frame: OverlayFrame {
                sequence: 3,
                tooltip_found: false,
                hotpath_ms: 7,
                cursor_window_relative: Some(Point { x: 940, y: 40 }),
                rule_descriptors: std::sync::Arc::from([]),
                text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
            },
            current_item: None,
            frame_status: FrameStatus::NoTooltip,
            tooltip_window_relative: None,
            analysis: std::sync::Arc::new(d4lc_runtime::AnalysisSnapshot::empty(1)),
        };

        let frame = live_frame_from_pipeline_result(
            &pipeline_result,
            Arc::from([]),
            d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        );

        assert_eq!(frame.sequence, 3);
        assert_eq!(
            frame.cursor_window_relative,
            Some(Point { x: 940, y: 40 }),
            "the live-loop frame must carry the pipeline's cursor observation"
        );
    }

    // -------------------------------------------------------------------
    // Coordinator adapter: presenter, mailbox, lifecycle
    // -------------------------------------------------------------------

    /// Records every frame the presenter hands to the surface.
    struct RecordingSurface {
        frames: Vec<OverlayFrame>,
        status: OverlayStatus,
        cursor: Arc<std::sync::Mutex<Option<d4lc_core::Point>>>,
    }

    impl Default for RecordingSurface {
        fn default() -> Self {
            Self {
                frames: Vec::new(),
                status: headless_overlay_status(),
                cursor: Arc::new(std::sync::Mutex::new(None)),
            }
        }
    }

    impl live_state::OverlaySurface for RecordingSurface {
        fn update(&mut self, frame: OverlayFrame) -> Result<(), String> {
            self.frames.push(frame);
            Ok(())
        }

        fn status(&self) -> &OverlayStatus {
            &self.status
        }

        fn text_diagnostics(&self) -> crate::app_state::OverlayTextState {
            crate::app_state::OverlayTextState::default()
        }

        fn poll_cursor_window_relative(&mut self) -> Option<d4lc_core::Point> {
            *self.cursor.lock().unwrap()
        }

        fn set_window_geometry(&mut self, geometry: d4lc_core::Rect) -> Result<(), String> {
            self.status.window_geometry = geometry;
            Ok(())
        }
    }

    /// After a confirmed clear, older draw commands never become
    /// visible again. A frozen debug image survives a soft clear of the live
    /// game (tooltip lost) but never a hard one (focus lost).
    #[test]
    fn presenter_never_redraws_a_presentation_older_than_a_confirmed_clear() {
        let mut presenter = live_state::OverlayPresenter::new(RecordingSurface::default());
        let item = analysis_presentation(3, true, None);
        assert_eq!(
            presenter.present(overlay_frame_fixture(1, true), &item, false),
            live_state::PresentOutcome::Drawn
        );

        let lost =
            analysis_presentation(4, false, Some(d4lc_runtime::InvalidationCause::TooltipLost));
        assert_eq!(
            presenter.present(overlay_frame_fixture(2, false), &lost, false),
            live_state::PresentOutcome::Drawn
        );
        assert_eq!(
            presenter.present(overlay_frame_fixture(3, true), &item, false),
            live_state::PresentOutcome::Refused,
            "a live frame older than the confirmed clear"
        );
        assert_eq!(
            presenter.present(overlay_frame_fixture(3, true), &item, true),
            live_state::PresentOutcome::Drawn,
            "a frozen debug image outlives a soft clear"
        );

        let focus_lost =
            analysis_presentation(5, false, Some(d4lc_runtime::InvalidationCause::FocusLost));
        assert_eq!(
            presenter.present(overlay_frame_fixture(4, false), &focus_lost, false),
            live_state::PresentOutcome::Drawn
        );
        assert_eq!(
            presenter.present(overlay_frame_fixture(5, true), &item, true),
            live_state::PresentOutcome::Refused,
            "focus loss hides even the frozen image"
        );
        assert_eq!(presenter.last_drawn(), Some(5));

        assert!(presenter.clear_all(6).is_ok());
        let newer = analysis_presentation(9, true, None);
        assert_eq!(
            presenter.present(overlay_frame_fixture(7, true), &newer, false),
            live_state::PresentOutcome::Refused,
            "nothing draws after the stop clear"
        );
        let drawn = &presenter.surface().frames;
        assert!(drawn
            .last()
            .is_some_and(|frame| !frame.tooltip_found && frame.rule_descriptors.is_empty()));
    }

    /// The live debug bundle travels the same bounded latest-state channel:
    /// a newer publication replaces the shown bundle.
    #[test]
    fn live_state_debug_bundle_is_latest_wins() {
        fn bundle(width: u32) -> Arc<d4lc_runtime::TooltipDebugBundle> {
            let image = || d4lc_runtime::DebugImage {
                width,
                height: 1,
                rgba: Arc::from(vec![255_u8, 0, 0, 255]),
            };
            Arc::new(d4lc_runtime::TooltipDebugBundle {
                tooltip_crop: image(),
                item_type_area: None,
                affix_locations: image(),
                affix_areas: image(),
                aspect_area: image(),
                socket_locations: image(),
                splitter_locations: image(),
                segments: Vec::new(),
                lines: Vec::new(),
            })
        }

        let mailbox = LiveStateMailbox::default();
        let epoch = mailbox.open_epoch();
        let mut first = live_state_fixture(epoch, 1, false);
        first.debug_bundle = Some(bundle(3));
        first.debug_info_enabled = true;
        assert!(mailbox.publish(first));
        let mut second = live_state_fixture(epoch, 2, false);
        second.debug_bundle = Some(bundle(9));
        second.debug_info_enabled = true;
        assert!(mailbox.publish(second));

        let latest = mailbox.take().unwrap_or_else(|| panic!("a latest state"));
        assert_eq!(
            latest
                .debug_bundle
                .as_deref()
                .map(|bundle| bundle.tooltip_crop.width),
            Some(9),
            "the newest accepted snapshot's bundle wins"
        );
        assert!(latest.debug_info_enabled);
        assert!(mailbox.take().is_none(), "one slot: the old bundle is gone");
    }

    /// The mailbox keeps only the newest state and fences a closed epoch: a
    /// stopped loop can never publish into a newer session.
    #[test]
    fn live_state_mailbox_is_bounded_and_fenced_by_epoch() {
        let mailbox = LiveStateMailbox::default();
        let state = |epoch: u64, sequence: u64| LiveState {
            epoch,
            lifecycle: RuntimeLifecycle::Running,
            presentation: Arc::new(analysis_presentation(sequence, false, None)),
            debug: DebugStatus {
                frame_sequence: sequence,
                ..DebugStatus::default()
            },
            overlay: live_state::OverlaySnapshot::default(),
            debug_bundle: None,
            debug_info_enabled: false,
            published_at: std::time::SystemTime::UNIX_EPOCH,
        };
        let old = mailbox.open_epoch();
        for sequence in 0..100 {
            assert!(mailbox.publish(state(old, sequence)));
        }
        assert_eq!(mailbox.replaced(), 99, "one slot, the newest wins");
        assert_eq!(mailbox.take().map(|s| s.debug.frame_sequence), Some(99));
        assert_eq!(mailbox.take(), None);

        assert!(mailbox.publish(state(old, 100)));
        mailbox.close_epoch(old);
        assert_eq!(
            mailbox.take(),
            None,
            "a closed epoch's unread state is dropped"
        );
        let new = mailbox.open_epoch();
        assert!(
            !mailbox.publish(state(old, 101)),
            "the stopped loop is fenced"
        );
        assert!(mailbox.publish(state(new, 0)));
        assert_eq!(mailbox.take().map(|s| s.epoch), Some(new));
        assert_eq!(mailbox.rejected(), 1);
    }

    // -------------------------------------------------------------------
    // Live-loop core through the composed runtime (real-runtime builds)
    // -------------------------------------------------------------------

    #[cfg(feature = "real-runtime")]
    mod live_core {
        use super::*;
        use crate::runtime_bridge::live_loop::apply_settings_reload;
        use d4lc_core::CaptureFrame;
        use std::sync::atomic::AtomicBool;

        fn tooltip_frame(gray_at_row_one: Option<u8>) -> CaptureFrame {
            let mut pixels = vec![255u8; 4 * 4 * 4];
            // Tooltip marker (the template's black pixel) at (1, 3).
            pixels[(3 * 4 + 1) * 4..(3 * 4 + 1) * 4 + 3].copy_from_slice(&[0, 0, 0]);
            if let Some(gray) = gray_at_row_one {
                pixels[(4 + 2) * 4..(4 + 2) * 4 + 3].copy_from_slice(&[gray, gray, gray]);
            }
            CaptureFrame {
                width: 4,
                height: 4,
                stride: 16,
                pixels_bgra: pixels,
            }
        }

        fn synthetic_runtime(frame: CaptureFrame) -> d4lc_runtime::PreparedRuntime {
            let source = d4lc_capture::SyntheticCaptureFrameSource::manual_region(frame)
                .unwrap_or_else(|error| panic!("synthetic source: {error}"));
            let reference = source.reference().clone();
            d4lc_runtime::PreparedRuntime::new(
                d4lc_capture::CaptureBackend::X11Native,
                reference,
                d4lc_detect::prepared::PreparedDetector {
                    preset: "synthetic-live-core".to_string(),
                    mode: d4lc_core::InputMode::Controller,
                    scan_height_percent: 100.0,
                    thresholds: d4lc_detect::prepared::ThresholdSettings {
                        min: 0,
                        max: 255,
                        similarity_tooltip: 0.01,
                    },
                    tooltip_width: 2,
                    templates: vec![(
                        "synthetic-template.png".to_string(),
                        d4lc_core::GrayImage {
                            width: 1,
                            height: 1,
                            pixels: vec![0],
                        },
                    )],
                    scan: d4lc_detect::prepared::DetectScan::default(),
                },
            )
            .with_synthetic_capture_source(source)
        }

        /// A worker whose jobs wait for `gate`; each result is a parsed
        /// item named after its frame.
        fn gated_worker(gate: Arc<AtomicBool>) -> d4lc_runtime::OcrWorker {
            d4lc_runtime::OcrWorker::spawn(move |job| {
                while !gate.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(d4lc_runtime::TooltipTextSnapshot {
                    sequence: 0,
                    source_frame_sequence: job.frame_id(),
                    tooltip_signature: job.tooltip_signature,
                    content_fingerprint: job.content_fingerprint(),
                    token: job.token,
                    ocr_read_top: None,
                    ocr_read_extent: None,
                    parity_path: "synthetic".to_string(),
                    ocr_ms: 0,
                    capture_ms: 0,
                    conversion_ms: 0,
                    splitter_detection_ms: None,
                    segment_count: 1,
                    line_count: 1,
                    raw_text: String::new(),
                    normalized_lines: Vec::new(),
                    segments_preview: Vec::new(),
                    title_preview: None,
                    text_preview: String::new(),
                    item: Some(d4lc_runtime::ItemTooltipParseResult {
                        status: d4lc_runtime::ItemTooltipParseStatus::Parsed,
                        display_name: Some(format!("Item of frame {}", job.frame_id())),
                        ..Default::default()
                    }),
                    error: None,
                    debug: None,
                })
            })
        }

        struct Fixture {
            runtime: d4lc_runtime::PreparedRuntime,
            session: d4lc_runtime::RuntimeSession,
            core: LiveLoopCore<RecordingSurface>,
            links: LiveLoopLinks,
            epoch: u64,
            gate: Arc<AtomicBool>,
        }

        fn fixture() -> Fixture {
            let gate = Arc::new(AtomicBool::new(true));
            let links = LiveLoopLinks::new();
            let epoch = links.mailbox.open_epoch();
            let mut publisher = RulesPublisher::new(links.rules.clone());
            publisher.observe(&rules_state_with_descriptors());
            let context = d4lc_runtime::DiagnosticsContext::new(
                d4lc_runtime::diagnostics::next_session_id(),
                d4lc_runtime::DiagnosticsClock::new(),
                d4lc_runtime::DiagnosticsHandle::new(
                    256,
                    d4lc_runtime::DiagnosticsVerbosity::Summary,
                ),
                d4lc_runtime::Revision::Applied(1),
                d4lc_runtime::Revision::Unknown,
            );
            let mut session = d4lc_runtime::RuntimeSession::new(
                Some(gated_worker(Arc::clone(&gate))),
                Duration::ZERO,
            );
            session.attach_diagnostics(context.clone());
            let core = LiveLoopCore::new(
                epoch,
                links.clone(),
                Some(live_state::OverlayPresenter::new(
                    RecordingSurface::default(),
                )),
                context,
            );
            Fixture {
                runtime: synthetic_runtime(tooltip_frame(None)),
                session,
                core,
                links,
                epoch,
                gate,
            }
        }

        impl Fixture {
            fn step(&mut self) -> LiveStep {
                self.core
                    .step(&mut self.runtime, &mut self.session, false)
                    .unwrap_or_else(|error| panic!("step failed: {error}"))
            }

            fn settle(&self) {
                let worker = self
                    .session
                    .ocr_worker_ref()
                    .unwrap_or_else(|| panic!("worker"));
                for _ in 0..5000 {
                    if !worker.is_busy() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                panic!("worker did not go idle");
            }

            /// Steps until the accepted item is presented.
            fn step_until_accepted(&mut self) -> LiveStep {
                for _ in 0..10 {
                    let step = self.step();
                    if step.presentation.analysis.has_item() {
                        return step;
                    }
                    self.settle();
                }
                panic!("no accepted analysis was presented");
            }
        }

        #[test]
        fn live_paragon_board_hover_changes_the_drawn_and_published_board() {
            assert_paragon_board_hover(false);
            assert_paragon_board_hover(true);
        }

        fn assert_paragon_board_hover(from_surface: bool) {
            use d4lc_rules::{ParagonOverlayElementKind, RuleOverlayTarget};
            let mut fixture = fixture();
            let mut state = rules_state_with_descriptors();
            let routes = state.downloaded_paragon_routes.as_mut().unwrap();
            let mut second = routes[0].boards[0].clone();
            second.name = "Second Board".to_string();
            second.active_nodes = vec![440];
            routes[0].boards.push(second);
            fixture
                .links
                .rules
                .publish(RulesSnapshot::capture(&state, 2));
            let cursor = Arc::new(std::sync::Mutex::new(None));
            let read_cursor = Arc::clone(&cursor);
            fixture.runtime = synthetic_runtime(tooltip_frame(None)).with_synthetic_cursor_source(
                d4lc_runtime::SyntheticCursorSource::new(move || {
                    if from_surface {
                        None
                    } else {
                        *read_cursor.lock().unwrap()
                    }
                }),
            );
            if from_surface {
                fixture.core.presenter =
                    Some(live_state::OverlayPresenter::new(RecordingSurface {
                        cursor: Arc::clone(&cursor),
                        ..RecordingSurface::default()
                    }));
            }
            let initial = fixture.step();
            let entry = initial
                .presentation
                .descriptors
                .iter()
                .find(|descriptor| {
                    matches!(
                        descriptor.target,
                        RuleOverlayTarget::Paragon {
                            kind: ParagonOverlayElementKind::BoardEntry,
                            board_index: Some(1),
                            ..
                        }
                    )
                })
                .unwrap()
                .geometry;
            *cursor.lock().unwrap() = Some(d4lc_core::Point {
                x: entry.x + 1,
                y: entry.y + 1,
            });
            let changed = fixture.step();
            assert!(
                changed.presentation.descriptors.iter().any(|descriptor| {
                    matches!(
                        descriptor.target,
                        RuleOverlayTarget::Paragon {
                            kind: ParagonOverlayElementKind::BoardBorder,
                            board_index: Some(1),
                            ..
                        }
                    )
                }),
                "hover must select the second board without requiring a new OCR result"
            );
            let published = fixture.links.mailbox.take().unwrap();
            assert!(Arc::ptr_eq(&published.presentation, &changed.presentation));
            let drawn = fixture
                .core
                .presenter()
                .unwrap()
                .surface()
                .frames
                .last()
                .unwrap();
            assert_eq!(drawn.rule_descriptors, changed.presentation.descriptors);
            *cursor.lock().unwrap() = None;
            assert_eq!(
                fixture.step().presentation.descriptors,
                changed.presentation.descriptors
            );
        }

        #[test]
        fn paragon_initial_board_index_takes_priority_over_a_name_moved_to_another_row() {
            use d4lc_rules::{ParagonOverlayElementKind as Kind, RuleOverlayTarget as Target};
            let mut fixture = fixture();
            let mut state = rules_state_with_descriptors();
            let routes = state.downloaded_paragon_routes.as_mut().unwrap();
            let mut next = routes[0].clone();
            let mut first = next.boards[0].clone();
            first.name = "New First Board".to_string();
            next.boards.insert(0, first);
            routes.push(next);
            let rules = RulesSnapshot::capture(&state, 2);
            let analysis = Arc::new(d4lc_runtime::AnalysisSnapshot::empty(0));
            let start = std::time::Instant::now();
            let initial = fixture.core.presentation_for_cursor(
                Arc::clone(&analysis),
                Some(&rules),
                None,
                start,
            );
            let step = initial
                .descriptors
                .iter()
                .find(|d| {
                    matches!(
                        d.target,
                        Target::Paragon {
                            kind: Kind::StepPanel,
                            ..
                        }
                    )
                })
                .unwrap()
                .geometry;
            let cursor = Some(d4lc_core::Point {
                x: step.x + 1,
                y: step.y + 1,
            });
            fixture.core.presentation_for_cursor(
                Arc::clone(&analysis),
                Some(&rules),
                cursor,
                start,
            );
            let next = fixture.core.presentation_for_cursor(
                analysis,
                Some(&rules),
                cursor,
                start + Duration::from_millis(500),
            );
            assert!(next.descriptors.iter().any(|d| matches!(&d.target,
                Target::Paragon { kind: Kind::BoardBorder, board_index: Some(0), board_name: Some(name), .. }
                if &**name == "New First Board")));
        }

        #[test]
        fn paragon_step_hover_cancels_repeats_wraps_and_survives_rules_publication() {
            use d4lc_core::Point;
            use d4lc_rules::{ParagonOverlayElementKind as Kind, RuleOverlayTarget as Target};
            use std::time::Instant;
            for collapsed in [false, true] {
                let mut fixture = fixture();
                let mut state = rules_state_with_descriptors();
                state.shell.settings.collapsed_paragonboard_enabled = collapsed;
                let routes = state.downloaded_paragon_routes.as_mut().unwrap();
                let mut board = routes[0].boards[0].clone();
                board.name = "Second Board".to_string();
                routes[0].boards.push(board);
                let mut next = routes[0].clone();
                next.boards.truncate(1);
                next.boards[0].name = "Endgame Board".to_string();
                next.boards[0].active_nodes = vec![440];
                routes.push(next);
                let rules = RulesSnapshot::capture(&state, 2);
                let analysis = Arc::new(d4lc_runtime::AnalysisSnapshot::empty(0));
                let start = Instant::now();
                let initial = fixture.core.presentation_for_cursor(
                    Arc::clone(&analysis),
                    Some(&rules),
                    None,
                    start,
                );
                let step_rect = initial
                    .descriptors
                    .iter()
                    .find(|d| {
                        matches!(
                            d.target,
                            Target::Paragon {
                                kind: Kind::StepPanel,
                                ..
                            }
                        )
                    })
                    .unwrap()
                    .geometry;
                let board_rect = initial
                    .descriptors
                    .iter()
                    .find(|d| {
                        matches!(
                            d.target,
                            Target::Paragon {
                                kind: Kind::BoardEntry,
                                board_index: Some(1),
                                ..
                            }
                        )
                    })
                    .unwrap()
                    .geometry;
                let step_cursor = Some(Point {
                    x: step_rect.x + 1,
                    y: step_rect.y + 1,
                });
                let board_cursor = Some(Point {
                    x: board_rect.x + 1,
                    y: board_rect.y + 1,
                });
                let selected = fixture.core.presentation_for_cursor(
                    Arc::clone(&analysis),
                    Some(&rules),
                    board_cursor,
                    start,
                );
                assert!(selected.descriptors.iter().any(|d| matches!(
                    d.target,
                    Target::Paragon {
                        kind: Kind::BoardBorder,
                        board_index: Some(1),
                        ..
                    }
                )));
                for (ms, cursor, expected_step) in [
                    (0, step_cursor, "Step 1 / 2"),
                    (499, None, "Step 1 / 2"),
                    (900, step_cursor, "Step 1 / 2"),
                    (1399, step_cursor, "Step 1 / 2"),
                    (1400, step_cursor, "Step 2 / 2"),
                    (1900, step_cursor, "Step 1 / 2"),
                ] {
                    let shown = fixture.core.presentation_for_cursor(
                        Arc::clone(&analysis),
                        Some(&rules),
                        cursor,
                        start + Duration::from_millis(ms),
                    );
                    assert!(shown
                        .descriptors
                        .iter()
                        .any(|d| d.text.as_deref() == Some(expected_step)));
                    if ms == 1400 {
                        assert!(shown.descriptors.iter().any(|d| matches!(&d.target,
                            Target::Paragon { kind: Kind::BoardBorder, board_name: Some(name), .. }
                            if &**name == "Endgame Board")));
                    }
                }
                state.shell.settings.overlay_font_size += 1;
                let refreshed_rules = RulesSnapshot::capture(&state, 3);
                let refreshed = fixture.core.presentation_for_cursor(
                    Arc::clone(&analysis),
                    Some(&refreshed_rules),
                    None,
                    start + Duration::from_millis(2000),
                );
                assert!(refreshed.descriptors.iter().any(|d| matches!(
                    d.target,
                    Target::Paragon {
                        kind: Kind::BoardBorder,
                        board_index: Some(1),
                        ..
                    }
                )));
                state.downloaded_paragon_routes = Some(Vec::new());
                let empty_rules = RulesSnapshot::capture(&state, 4);
                let empty = fixture.core.presentation_for_cursor(
                    analysis,
                    Some(&empty_rules),
                    step_cursor,
                    start + Duration::from_millis(3000),
                );
                assert!(empty.descriptors.iter().any(|d| matches!(
                    d.target,
                    Target::Paragon {
                        kind: Kind::WarningPanel,
                        ..
                    }
                )));
            }
        }

        /// The overlay and the UI receive the same accepted
        /// presentation — one value, evaluated once on the runtime side.
        #[test]
        fn ui_and_renderer_receive_the_same_accepted_presentation() {
            let mut fixture = fixture();
            let step = fixture.step_until_accepted();

            let live = fixture
                .links
                .mailbox
                .take()
                .unwrap_or_else(|| panic!("the step published its state"));
            assert!(Arc::ptr_eq(&live.presentation, &step.presentation));
            assert_eq!(live.epoch, fixture.epoch);
            let presenter = fixture
                .core
                .presenter()
                .unwrap_or_else(|| panic!("presenter"));
            assert_eq!(presenter.last_drawn(), Some(step.presentation.generation()));
            let drawn = presenter
                .surface()
                .frames
                .last()
                .unwrap_or_else(|| panic!("a drawn frame"));
            assert_eq!(drawn.rule_descriptors, step.presentation.descriptors);
            assert!(
                !step.presentation.descriptors.is_empty(),
                "evaluated against the published rules snapshot"
            );
            assert_eq!(step.presentation.rules_revision, Some(1));

            // The UI projects exactly that item.
            let mut state = AppState::default_startup();
            let mut projection = LiveStateProjection::default();
            projection.apply(&mut state, &live);
            let accepted_name = step
                .presentation
                .analysis
                .item
                .as_ref()
                .and_then(|item| item.item_name.clone());
            assert!(accepted_name.is_some());
            assert_eq!(state.current_item().item_name, accepted_name);
        }

        /// Through the live loop: focus is lost while OCR runs. The
        /// very next step presents and publishes the hidden state, before
        /// OCR finished; the late result never re-shows the item.
        #[test]
        fn focus_loss_hides_the_overlay_before_ocr_finishes() {
            let mut fixture = fixture();
            fixture.step_until_accepted();

            fixture.gate.store(false, Ordering::SeqCst);
            fixture.runtime = synthetic_runtime(tooltip_frame(Some(150)));
            for _ in 0..3 {
                fixture.step();
            }
            let busy = fixture
                .session
                .ocr_worker_ref()
                .is_some_and(d4lc_runtime::OcrWorker::is_busy);
            assert!(busy, "the new item's OCR is running");

            fixture
                .links
                .controls
                .report_game_window(d4lc_runtime::GameWindowState {
                    focused: false,
                    minimized: false,
                    closed: false,
                });
            let hidden = fixture.step();
            assert!(!hidden.presentation.analysis.has_item());
            assert_eq!(
                hidden.presentation.analysis.invalidation,
                Some(d4lc_runtime::InvalidationCause::FocusLost)
            );
            assert_eq!(hidden.draw, Some(live_state::PresentOutcome::Drawn));
            let live = fixture
                .links
                .mailbox
                .take()
                .unwrap_or_else(|| panic!("published"));
            assert!(!live.presentation.analysis.has_item());
            assert!(
                fixture
                    .session
                    .ocr_worker_ref()
                    .is_some_and(d4lc_runtime::OcrWorker::is_busy),
                "hidden before OCR finished"
            );

            fixture.gate.store(true, Ordering::SeqCst);
            fixture.settle();
            for _ in 0..3 {
                let step = fixture.step();
                assert!(!step.presentation.analysis.has_item());
            }
        }

        /// Wait until the observer's report reached the control inbox.
        fn observed(links: &LiveLoopLinks) {
            for _ in 0..2000 {
                if links.controls.drain_peek() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            panic!("the game-window observer delivered nothing");
        }

        fn observed_fixture() -> (
            Fixture,
            std::sync::mpsc::Sender<Vec<crate::game_window_observer::GameWindowEvent>>,
            crate::game_window_observer::GameWindowObserver,
        ) {
            use crate::game_window_observer::{fake::FakeSource, GameWindowEvent};
            let fixture = fixture();
            let (tx, rx) = std::sync::mpsc::channel();
            let observer = crate::game_window_observer::GameWindowObserver::spawn(
                7,
                FakeSource {
                    initial: vec![GameWindowEvent::ActiveWindow(Some(7))],
                    events: rx,
                },
                fixture.links.controls.clone(),
            );
            observed(&fixture.links);
            (fixture, tx, observer)
        }

        /// End to end: focus loss, minimize and close, as the
        /// game-window observer reports them, hide the overlay through the
        /// coordinator while OCR is still running, and focus regained lets
        /// the analysis resume.
        #[test]
        fn observer_focus_minimize_and_close_hide_the_overlay() {
            use crate::game_window_observer::GameWindowEvent;
            use d4lc_runtime::InvalidationCause;
            let (mut fixture, tx, _observer) = observed_fixture();
            fixture.step_until_accepted();

            for (event, cause) in [
                (
                    GameWindowEvent::ActiveWindow(Some(99)),
                    InvalidationCause::FocusLost,
                ),
                (GameWindowEvent::Hidden(true), InvalidationCause::Minimized),
                (GameWindowEvent::Destroyed, InvalidationCause::WindowLost),
            ] {
                // Recover to an accepted item first (a new item under a
                // closed gate keeps its OCR running).
                if cause != InvalidationCause::FocusLost {
                    tx.send(vec![
                        GameWindowEvent::Hidden(false),
                        GameWindowEvent::ActiveWindow(Some(7)),
                    ])
                    .unwrap_or_else(|error| panic!("send: {error}"));
                    observed(&fixture.links);
                    fixture
                        .links
                        .controls
                        .report_game_window(d4lc_runtime::GameWindowState::default());
                    fixture.step_until_accepted();
                }
                fixture.gate.store(false, Ordering::SeqCst);
                fixture.runtime = synthetic_runtime(tooltip_frame(Some(90 + cause as u8)));
                for _ in 0..3 {
                    fixture.step();
                }

                tx.send(vec![event])
                    .unwrap_or_else(|error| panic!("send: {error}"));
                observed(&fixture.links);
                let hidden = fixture.step();
                assert!(!hidden.presentation.analysis.has_item(), "{cause:?}");
                assert_eq!(hidden.presentation.analysis.invalidation, Some(cause));
                assert_eq!(hidden.draw, Some(live_state::PresentOutcome::Drawn));
                fixture.gate.store(true, Ordering::SeqCst);
                fixture.settle();
                let later = fixture.step();
                assert!(
                    !later.presentation.analysis.has_item(),
                    "the late OCR result of {cause:?} never re-shows the overlay"
                );
            }
        }

        /// End to end: a window move reported by the observer moves
        /// the capture and the overlay surface, bumps the window revision at
        /// once, and the re-analysis carries the new revision with the same
        /// window-relative tooltip rect (transformed exactly once).
        #[test]
        fn observer_window_move_updates_revision_and_coordinates() {
            use crate::game_window_observer::GameWindowEvent;
            let (mut fixture, tx, _observer) = observed_fixture();
            let accepted = fixture.step_until_accepted();
            let before = accepted
                .presentation
                .analysis
                .tooltip_window_relative
                .unwrap_or_else(|| panic!("accepted rect"));
            assert_eq!(fixture.session.coordinator().window_revision(), 0);

            let moved = Rect {
                x: -1920,
                y: 32,
                width: 4,
                height: 4,
            };
            tx.send(vec![GameWindowEvent::Geometry(moved)])
                .unwrap_or_else(|error| panic!("send: {error}"));
            observed(&fixture.links);
            let generation = fixture.session.coordinator().tooltip_generation();
            let step = fixture.step();
            assert_eq!(fixture.session.coordinator().window_revision(), 1);
            // The analysis of the old geometry ended; the still visible
            // tooltip starts a new generation in the same frame.
            assert!(!step.presentation.analysis.has_item());
            assert!(step.presentation.analysis.tooltip_generation > generation);
            assert_eq!(fixture.runtime.capture_geometry(), moved);
            assert_eq!(
                fixture
                    .core
                    .presenter()
                    .map(|presenter| presenter.status().window_geometry),
                Some(moved)
            );

            let again = fixture.step_until_accepted();
            let token = again
                .presentation
                .analysis
                .token
                .unwrap_or_else(|| panic!("token"));
            assert_eq!(token.window_revision, 1);
            assert_eq!(
                again.presentation.analysis.tooltip_window_relative,
                Some(before),
                "window-relative coordinates do not move with the window"
            );
            // A second report of the same geometry is no new revision.
            tx.send(vec![GameWindowEvent::Geometry(moved)])
                .unwrap_or_else(|error| panic!("send: {error}"));
            fixture.step();
            assert_eq!(fixture.session.coordinator().window_revision(), 1);
        }

        /// Through the reload path: a settings change observed by the
        /// monitor while OCR runs re-prepares the runtime as a new
        /// configuration revision; the old configuration's analysis is
        /// never presented, and the re-analysis carries the new revision.
        #[test]
        fn a_settings_change_during_ocr_discards_the_stale_result() {
            use crate::runtime_bridge::settings_reload::{
                SettingsReloadDecision, SettingsReloadMonitor,
            };
            let mut fixture = fixture();
            fixture.step_until_accepted();
            fixture.gate.store(false, Ordering::SeqCst);
            fixture.runtime = synthetic_runtime(tooltip_frame(Some(150)));
            for _ in 0..3 {
                fixture.step();
            }
            assert!(fixture
                .session
                .ocr_worker_ref()
                .is_some_and(d4lc_runtime::OcrWorker::is_busy));

            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("d4lc-reload-monitor-{unique}"));
            std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
            let mut monitor = SettingsReloadMonitor::with_game_data_root(&root, &root);
            let mut settings = d4lc_config::AppSettings::default();
            settings.threshold_similarity_affix_location += 0.05;
            d4lc_config::save_app_settings(&root, &settings)
                .unwrap_or_else(|error| panic!("save: {error}"));
            let signal = d4lc_runtime::RuntimeReloadSignal::new();
            match monitor.poll() {
                SettingsReloadDecision::Changed { reprepare, .. } => {
                    assert!(reprepare
                        .iter()
                        .any(|key| key == "threshold_similarity_affix_location"));
                    signal.request();
                }
                other => panic!("an OCR setting must re-prepare, got {other:?}"),
            }

            let diagnostics = fixture.session.diagnostics().clone();
            let mut revision = 1;
            let outcome = apply_settings_reload(
                &signal,
                &mut fixture.runtime,
                &mut fixture.session,
                &diagnostics,
                &mut revision,
                || {
                    Ok((
                        synthetic_runtime(tooltip_frame(Some(150))),
                        d4lc_runtime::RuntimeSession::new(
                            Some(gated_worker(Arc::new(AtomicBool::new(true)))),
                            Duration::ZERO,
                        ),
                    ))
                },
            );
            assert_eq!(outcome, d4lc_runtime::ReloadOutcome::Applied);
            assert_eq!(revision, 2);
            assert_eq!(
                diagnostics.config_revision(),
                d4lc_runtime::Revision::Applied(2)
            );

            // The old job finishes on the release thread; nothing of it is
            // presented.
            fixture.gate.store(true, Ordering::SeqCst);
            let generation = fixture.session.coordinator().tooltip_generation();
            let step = fixture.step();
            assert!(!step.presentation.analysis.has_item());
            assert!(
                step.presentation.analysis.tooltip_generation > generation,
                "the applied revision ended the old configuration's analysis"
            );
            let again = fixture.step_until_accepted();
            assert_eq!(
                again
                    .presentation
                    .analysis
                    .token
                    .map(|token| token.config_revision),
                Some(d4lc_runtime::Revision::Applied(2))
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        /// Through the live loop: after the stop fence nothing of the
        /// old session is presented or published, the overlay is cleared,
        /// and a new session publishes undisturbed.
        #[test]
        fn a_stopped_loop_presents_and_publishes_nothing() {
            let mut old = fixture();
            old.step_until_accepted();
            let _ = old.links.mailbox.take();

            // The UI-thread half of the stop.
            old.links.mailbox.close_epoch(old.epoch);
            let late = old.step();
            assert!(!late.published);
            assert_eq!(late.draw, None);
            old.core.stop(&mut old.session, late.frame_sequence);
            let cleared = old
                .core
                .presenter()
                .and_then(|presenter| presenter.surface().frames.last().cloned())
                .unwrap_or_else(|| panic!("the stop draws a clear"));
            assert!(!cleared.tooltip_found && cleared.rule_descriptors.is_empty());
            assert_eq!(
                old.session.coordinator().lifecycle(),
                RuntimeLifecycle::Stopping
            );

            // A new loop on the same links (a restart).
            let new_epoch = old.links.mailbox.open_epoch();
            let mut new_core = LiveLoopCore::new(
                new_epoch,
                old.links.clone(),
                Some(live_state::OverlayPresenter::new(
                    RecordingSurface::default(),
                )),
                old.session.diagnostics().clone(),
            );
            let mut new_session = d4lc_runtime::RuntimeSession::new(
                Some(gated_worker(Arc::new(AtomicBool::new(true)))),
                Duration::ZERO,
            );
            let new_step = new_core
                .step(&mut old.runtime, &mut new_session, false)
                .unwrap_or_else(|error| panic!("new step: {error}"));
            assert!(new_step.published);
            let stale = old.step();
            assert!(!stale.published, "the old loop stays fenced");
            assert_eq!(
                old.links.mailbox.take().map(|state| state.epoch),
                Some(new_epoch)
            );
        }
    }
}
