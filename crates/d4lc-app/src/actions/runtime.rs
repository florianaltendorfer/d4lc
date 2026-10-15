//! Runtime lifecycle action handlers.
//!
//! ## Upstream evidence
//!
//! - `ScreenProcessHandler.cs:88` — `IsEnabled` property toggled by overlay messages
//! - `ScreenProcessHandler.cs:157-169` — `HandleToggleOverlayMessage` / `HandleToggleOverlayFromGUIMessage`

use crate::actions::logging::LogProjection;
use crate::app_state::AppState;
use crate::runtime_bridge::{
    runtime_ui_projection, LiveState, LiveStateMailbox, LiveStateProjection, RuntimeEvent,
    RuntimeEventReceiver,
};
use crate::slint_generated::AppWindow;
use crate::wiring::DEBUG_TAB_INDEX;

/// Apply a single `RuntimeEvent` to `AppState` and update Slint UI properties.
///
/// This is the single dispatch point called by the UI thread timer when it
/// receives a runtime event from the live loop.  It updates AppState first,
/// then computes a projection and writes each field into the `.slint` file.
#[cfg(test)]
pub fn apply_event_to_ui(app: &AppWindow, state: &mut AppState, event: &RuntimeEvent) {
    let mut logs = LogProjection::default();
    apply_event_and_project(app, state, event, &mut logs);
}

/// Apply one notification to `AppState` (no projection).
fn apply_event(state: &mut AppState, event: &RuntimeEvent) {
    crate::runtime_bridge::apply_runtime_event(state, event);
}

/// Apply one notification and project every surface it can reach.
///
/// The state change comes first, then the item-dependent projection, then the
/// runtime block. Both entry points — `apply_event_to_ui` and the drain
/// loop — funnel through here, so a `CurrentItemUpdate` can never update the
/// state without re-deriving the surfaces that read it.
fn apply_event_and_project(
    app: &AppWindow,
    state: &mut AppState,
    event: &RuntimeEvent,
    logs: &mut LogProjection,
) {
    apply_event(state, event);
    // A parsed current item also feeds the TradeScreen's current-item card
    // (upstream TradeViewModel.HandleTooltipDataReadyMessage,
    // TradeViewModel.cs:148-163): re-project the trade state so the card
    // reflects the live parse. `project_trade_state` is the card's single
    // projector — the accepted-snapshot path re-derives the same card through
    // it, never through a fragment update of its own.
    if matches!(event, RuntimeEvent::CurrentItemUpdate { .. }) {
        crate::actions::trade::project_trade_state(app, state);
    }
    project_runtime_state_to_ui(app, state, logs);
}

/// Write the runtime projection into the Slint properties.
///
/// The single UI-thread writer of the runtime surface: every field comes out
/// of `runtime_ui_projection`, i.e. out of one `AppState` — and, for the
/// accepted-snapshot block, out of the one `LiveState` the live loop published
/// The Logging screen's model is brought in step here too, through
/// the caller's incremental projection.
fn project_runtime_state_to_ui(app: &AppWindow, state: &AppState, logs: &mut LogProjection) {
    let projection = runtime_ui_projection(state);
    app.set_runtime_running(projection.runtime_running);
    app.set_runtime_preparing(projection.runtime_preparing);
    app.set_runtime_status_text(projection.runtime_status.into());
    // The Debug screen's "Overlay:" field renders the live loop's last
    // published overlay status; before any report it keeps the pending
    // default ("Offline") rather than claiming a surface.
    app.set_overlay_status_text(projection.overlay_status_text.into());
    app.set_overlay_text_status_text(projection.overlay_text_status_text.into());
    app.set_accepted_generation_text(projection.accepted_generation_text.into());
    app.set_overlay_renderer_text(projection.overlay_renderer_text.into());
    app.set_frame_state_text(projection.frame_state_text.into());
    app.set_debug_frozen(projection.debug_frozen);
    app.set_evaluation_text(projection.evaluation_text.into());
    app.set_published_at_text(projection.published_at_text.into());
    app.set_hotpath_scope_text(projection.hotpath_scope_text.into());
    app.set_ocr_phase_text(projection.ocr_phase_text.into());
    app.set_loop_timings_text(projection.loop_timings_text.into());
    app.set_game_window_status_text(projection.game_window_status_text.into());
    app.set_capture_engine_text(projection.capture_engine_text.into());
    app.set_settings_status_text(projection.settings_status_text.into());
    app.set_current_item_name(projection.current_item_name.into());
    app.set_current_item_parse_status(projection.current_item_parse_status.into());
    app.set_debug_frame_sequence(projection.debug_frame_sequence as i32);
    app.set_debug_hotpath_ms(projection.debug_hotpath_ms as i32);
    app.set_debug_tooltip_found(projection.debug_tooltip_found);
    // The `LoggingViewModel` already lives on `AppState`; this is a pure
    // projection of it, kept incremental through the caller's projection.
    logs.sync(app, &state.logging);
}

/// What the drain loop keeps between ticks.
///
/// The projected snapshot (so the item card is re-derived only for a new
/// accepted generation), the incremental log model, and the sink's drop
/// count so a log burst that overflowed its bounded channel is reported once
/// instead of vanishing.
#[derive(Debug, Default)]
pub struct RuntimeDrain {
    live: LiveStateProjection,
    logs: LogProjection,
    reported_dropped_logs: u64,
    /// The live debug content the Debug panels currently show: the DebugInfo
    /// flag plus, when a live bundle exists, its accepted generation. The
    /// stage/OCR models are re-projected only when this changed, so a flood of frames costs one comparison.
    live_debug_key: Option<(bool, Option<u64>)>,
    /// The diagnostics window the event list currently shows.
    live_diagnostics: Vec<d4lc_runtime::DiagnosticsEvent>,
}

impl RuntimeDrain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one discrete notification and project the result.
    fn apply_event(&mut self, app: &AppWindow, state: &mut AppState, event: &RuntimeEvent) {
        if matches!(event, RuntimeEvent::Stopped { .. }) {
            // The stopped loop's card was reset by the event; whatever the
            // next loop publishes must be projected again.
            self.live.reset();
            // The stopped loop holds no live debug data: the next projection
            // must rebuild the panels from the off/stored state.
            self.live_debug_key = None;
            self.live_diagnostics.clear();
        }
        apply_event_and_project(app, state, event, &mut self.logs);
        if matches!(event, RuntimeEvent::Stopped { .. }) && app.get_active_tab() == DEBUG_TAB_INDEX
        {
            crate::actions::debug::project_live_debug_stages(app, state);
            crate::actions::debug::project_live_diagnostics(app, state);
        }
    }

    /// Apply one accepted snapshot and project the result.
    fn apply_live(&mut self, app: &AppWindow, state: &mut AppState, live: &LiveState) {
        // The item card, the tooltip state, the debug fields, the
        // overlay status and the evaluation are projected together from this
        // one accepted snapshot.
        if self.live.apply(state, live) {
            crate::actions::trade::project_trade_state(app, state);
        }
        project_runtime_state_to_ui(app, state, &mut self.logs);
        self.project_live_debug_when_changed(app, state, live);
    }

    /// Re-project the live stage/OCR/diagnostics panels only when the live
    /// debug content changed and the Debug tab is visible. While the tab is closed the panels keep the last projected
    /// state and cost nothing.
    fn project_live_debug_when_changed(
        &mut self,
        app: &AppWindow,
        state: &AppState,
        live: &LiveState,
    ) {
        if app.get_active_tab() != DEBUG_TAB_INDEX {
            return;
        }
        let key = (
            live.debug_info_enabled,
            live.debug_bundle.as_ref().map(|_| live.debug.generation),
        );
        if self.live_debug_key != Some(key) {
            self.live_debug_key = Some(key);
            crate::actions::debug::project_live_debug_stages(app, state);
        }
        if self.live_diagnostics != live.debug.recent_diagnostics {
            self.live_diagnostics = live.debug.recent_diagnostics.clone();
            crate::actions::debug::project_live_diagnostics(app, state);
        }
    }

    /// Report the log lines the sink had to drop since the last report.
    ///
    /// The flood path stays bounded, but it is never silent: the user sees one
    /// line saying how much of the flood the UI could not keep up with.
    fn report_dropped_logs(
        &mut self,
        app: &AppWindow,
        state: &mut AppState,
        events: &RuntimeEventReceiver,
    ) {
        let dropped = events.dropped_logs();
        if dropped == self.reported_dropped_logs {
            return;
        }
        let new = dropped.saturating_sub(self.reported_dropped_logs);
        self.reported_dropped_logs = dropped;
        state.append_log_message(
            format!("logging: {new} line(s) dropped (the UI fell behind the runtime)"),
            crate::clock::hh_mm_now(),
        );
        project_runtime_state_to_ui(app, state, &mut self.logs);
    }
}

/// Drain every pending notification and the newest live state, and apply them
/// to AppState + Slint UI.
///
/// Called by the periodic timer (every 50ms) from the Slint event loop.
/// Non-blocking: uses `try_recv` / a mailbox take, so it never blocks the UI
/// thread. Discrete notifications come through the two bounded channels (a
/// control event waits for the UI, a log line is dropped and counted); the
/// current state (accepted presentation, item card, overlay status, debug
/// fields) comes through the bounded latest-state mailbox, so a status flood
/// can neither queue up here nor delay a control command's own confirmation.
pub fn drain_runtime_events(
    app: &AppWindow,
    state: &mut AppState,
    events: &RuntimeEventReceiver,
    live_mailbox: &LiveStateMailbox,
    drain: &mut RuntimeDrain,
) {
    for event in events.try_iter() {
        drain.apply_event(app, state, &event);
    }
    if let Some(live) = live_mailbox.take() {
        drain.apply_live(app, state, &live);
    }
    drain.report_dropped_logs(app, state, events);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use d4lc_gui::logging::LoggingViewModel;

    /// `LiveState` / `DebugStatus` imports for the snapshot fixtures.
    use crate::runtime_bridge::live_state::{OverlaySnapshot, PresentOutcome};
    use crate::runtime_bridge::{DebugStatus, LiveState, Presentation};
    use d4lc_runtime::RuntimeLifecycle;

    /// One accepted snapshot of a running loop: an item-bearing analysis, the
    /// renderer's outcome for it and a fixed publication instant.
    fn live_state(epoch: u64, sequence: u64) -> LiveState {
        let presentation = std::sync::Arc::new(Presentation {
            analysis: std::sync::Arc::new(d4lc_runtime::AnalysisSnapshot {
                presentation_generation: sequence,
                item: Some(d4lc_runtime::CurrentItemSnapshot {
                    item_name: Some("Helm".to_string()),
                    parse_status: "Parsed".to_string(),
                    item_type: Some("helm".to_string()),
                    base_type: None,
                    item_power: Some(800),
                    affixes: Vec::new(),
                    affix_row_geometry: Vec::new(),
                    non_affix_rows: Vec::new(),
                }),
                ..d4lc_runtime::AnalysisSnapshot::empty(1)
            }),
            rules_revision: Some(1),
            descriptors: std::sync::Arc::from([]),
            affix_traces: Vec::new(),
        });
        LiveState {
            epoch,
            lifecycle: RuntimeLifecycle::Running,
            presentation,
            debug: DebugStatus {
                frame_sequence: sequence,
                hotpath_ms: 42,
                tooltip_found: true,
                descriptor_count: 0,
                generation: sequence,
                source_frame: Some(sequence),
                drawn_generation: Some(sequence),
                draw: Some(PresentOutcome::Drawn),
                frozen: false,
                ocr: Some(crate::runtime_bridge::live_state::OcrPhaseStatus {
                    completed: 2,
                    total_ocr_ms: 68,
                    pending: true,
                    max_queue_depth: 1,
                    ..crate::runtime_bridge::live_state::OcrPhaseStatus::default()
                }),
                loop_timings: None,
                capture_stream: None,
                recent_diagnostics: Vec::new(),
            },
            overlay: OverlaySnapshot::default(),
            debug_bundle: None,
            debug_info_enabled: false,
            published_at: std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3600),
        }
    }

    /// Read the row count from a Slint model without pulling in
    /// `slint::Model` (which would require additional trait method
    /// imports). We only need the count for these tests.
    fn model_row_count(model: slint::ModelRc<crate::slint_generated::LogEntry>) -> usize {
        // `ModelRc` is `Rc<dyn Model>`. We use the generated
        // `len()` style by iterating via the trait.
        // The simplest cross-version path is the row_count method on
        // `slint::Model` which is in the prelude of `slint::Model`
        // trait; we import it via the trait here.
        use slint::Model as _;
        model.row_count()
    }

    #[test]
    fn log_message_event_appends_to_app_state() {
        let _app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let event = RuntimeEvent::LogMessage {
            timestamp: "13:45".to_string(),
            message: "live loop emitted a message".to_string(),
        };

        apply_event_to_ui(&_app, &mut state, &event);

        assert_eq!(state.logging.messages.len(), 1);
        assert_eq!(
            state.logging.messages[0].message,
            "live loop emitted a message"
        );
    }

    #[test]
    fn log_message_event_projects_to_slint_log_messages() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        apply_event_to_ui(
            &app,
            &mut state,
            &RuntimeEvent::LogMessage {
                timestamp: "13:45".to_string(),
                message: "hello world".to_string(),
            },
        );
        let model = app.get_log_messages();
        assert_eq!(model_row_count(model), 1);
    }

    #[test]
    fn started_event_propagates_without_touching_log_model() {
        let _app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        apply_event_to_ui(&_app, &mut state, &RuntimeEvent::Started);
        assert!(state.runtime_status.is_running);
        // No log message is appended by a plain Started event;
        // the projection refresh is a no-op for the log model.
        assert!(state.logging.messages.is_empty());
    }

    #[test]
    fn error_event_propagates_and_log_model_still_consistent() {
        let _app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let event = RuntimeEvent::Error {
            message: "boom".to_string(),
        };
        apply_event_to_ui(&_app, &mut state, &event);
        assert_eq!(state.runtime_status.status_message, "Error: boom");
        assert!(state.logging.messages.is_empty());
    }

    /// The Debug screen's runtime block is written from the one accepted
    /// snapshot the mailbox carries: the item card, the tooltip status, the
    /// accepted generation, the renderer outcome, the evaluation and the
    /// overlay status/text state all describe that same snapshot.
    #[test]
    fn an_accepted_snapshot_projects_the_whole_runtime_block() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();

        // Before the loop reports, the Slint properties keep their pending
        // defaults; the projection invents no surface claim and no snapshot.
        assert_eq!(app.get_overlay_status_text().as_str(), "Offline");
        assert_eq!(
            app.get_accepted_generation_text().as_str(),
            "no snapshot yet"
        );
        assert_eq!(
            app.get_overlay_text_status_text().as_str(),
            "No overlay surface reported yet"
        );

        // A published snapshot (here: the synthesized no-surface status of a
        // loop whose game window could not be resolved) reaches the Slint
        // properties with the state it carries verbatim.
        let status = d4lc_overlay_x11::OverlayStatus {
            limitation: Some("no overlay surface: no window".to_string()),
            ..crate::runtime_bridge::headless_overlay_status()
        };
        let mailbox = LiveStateMailbox::default();
        let epoch = mailbox.open_epoch();
        let mut live = live_state(epoch, 9);
        live.overlay.status = Some(status);
        assert!(mailbox.publish(live));

        let mut drain = RuntimeDrain::new();
        drain_runtime_events(&app, &mut state, &never_events(), &mailbox, &mut drain);

        assert_eq!(
            app.get_overlay_status_text().as_str(),
            "headless_noop (disabled): no overlay surface: no window"
        );
        assert_eq!(
            app.get_overlay_text_status_text().as_str(),
            "no text path on this surface: no overlay surface: no window"
        );
        assert_eq!(
            app.get_accepted_generation_text().as_str(),
            "9 (source frame 9)"
        );
        assert_eq!(
            app.get_overlay_renderer_text().as_str(),
            "drawn 9 = accepted generation"
        );
        assert_eq!(
            app.get_frame_state_text().as_str(),
            "Live (following the running loop)"
        );
        assert!(!app.get_debug_frozen());
        assert_eq!(app.get_debug_frame_sequence(), 9);
        assert_eq!(app.get_debug_hotpath_ms(), 42);
        assert!(app.get_debug_tooltip_found());
        assert_eq!(app.get_current_item_name().as_str(), "Helm");
        assert_eq!(
            app.get_evaluation_text().as_str(),
            "0 of 0 affix row(s) matched, 0 overlay marker(s)"
        );
        // Local wall-clock time of the fixture instant (UNIX_EPOCH + 1 h).
        assert_eq!(
            app.get_published_at_text().as_str(),
            crate::clock::hh_mm_ss(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3600)
            )
        );
        // The live frame number carries its own scope, and the async
        // OCR phase of the same loop is reported as itself.
        assert!(app
            .get_hotpath_scope_text()
            .contains("live frame pass only"));
        assert_eq!(
            app.get_ocr_phase_text().as_str(),
            "result ready: 2 completed, 34 ms average per completed pass, 0 dropped, 0 failed, \
             deepest queue 1"
        );
    }

    /// A frozen debug capture is recognizable in the Slint block, and a
    /// stopped loop leaves no accepted snapshot behind.
    #[test]
    fn a_frozen_snapshot_and_a_stop_reach_the_slint_block() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let mailbox = LiveStateMailbox::default();
        let epoch = mailbox.open_epoch();
        let mut live = live_state(epoch, 9);
        live.debug.frozen = true;
        assert!(mailbox.publish(live));

        let mut drain = RuntimeDrain::new();
        drain_runtime_events(&app, &mut state, &never_events(), &mailbox, &mut drain);
        assert!(app.get_debug_frozen());
        assert!(
            app.get_frame_state_text()
                .as_str()
                .starts_with("Frozen (debug capture lock)"),
            "the Slint block names the frozen state: {}",
            app.get_frame_state_text()
        );

        // The loop stops: nothing is accepted any more, and the block says so
        // instead of keeping the last snapshot's numbers.
        apply_event_to_ui(
            &app,
            &mut state,
            &RuntimeEvent::Stopped {
                reason: "test stop".to_string(),
            },
        );
        assert_eq!(
            app.get_accepted_generation_text().as_str(),
            "no snapshot yet"
        );
        assert!(!app.get_debug_frozen());
        assert_eq!(app.get_frame_state_text().as_str(), "no snapshot yet");
        assert_eq!(state.runtime_status.accepted, None);
    }

    /// A log burst that overflowed the bounded channel is reported to
    /// the user exactly once, and the visible list stays bounded.
    #[test]
    fn dropped_log_lines_are_reported_once_in_the_visible_log() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let (sink, events) = crate::runtime_bridge::RuntimeEventSink::new();
        let overflow = 3usize;
        for index in 0..(crate::runtime_bridge::LOG_EVENT_CAPACITY + overflow) {
            sink.send(RuntimeEvent::LogMessage {
                timestamp: "12:00".to_string(),
                message: format!("line {index}"),
            });
        }

        let mailbox = LiveStateMailbox::default();
        let mut drain = RuntimeDrain::new();
        drain_runtime_events(&app, &mut state, &events, &mailbox, &mut drain);

        let report = format!("logging: {overflow} line(s) dropped");
        assert!(
            state
                .logging
                .messages
                .iter()
                .any(|entry| entry.message.starts_with(&report)),
            "the drop is reported in the visible log: {:?}",
            state.logging.messages.last()
        );
        assert_eq!(
            state.logging.messages.len(),
            crate::runtime_bridge::LOG_EVENT_CAPACITY + 1,
            "the visible list holds what arrived plus the one report line"
        );

        // No new drops: the report is not repeated on the next tick.
        let reported = state.logging.messages.len();
        drain_runtime_events(&app, &mut state, &events, &mailbox, &mut drain);
        assert_eq!(state.logging.messages.len(), reported);
    }

    /// The drain loop reaches the Slint item card through the accepted
    /// snapshot: a No-Tooltip snapshot empties the card.
    #[test]
    fn a_no_tooltip_snapshot_empties_the_slint_item_card() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let mailbox = LiveStateMailbox::default();
        let epoch = mailbox.open_epoch();
        let mut drain = RuntimeDrain::new();

        assert!(mailbox.publish(live_state(epoch, 9)));
        drain_runtime_events(&app, &mut state, &never_events(), &mailbox, &mut drain);
        assert_eq!(app.get_current_item_name().as_str(), "Helm");

        let mut no_tooltip = live_state(epoch, 10);
        no_tooltip.presentation = std::sync::Arc::new(crate::runtime_bridge::Presentation {
            analysis: std::sync::Arc::new(d4lc_runtime::AnalysisSnapshot::empty(1)),
            rules_revision: Some(1),
            descriptors: std::sync::Arc::from([]),
            affix_traces: Vec::new(),
        });
        no_tooltip.debug.generation = 0;
        no_tooltip.debug.source_frame = None;
        assert!(mailbox.publish(no_tooltip));
        drain_runtime_events(&app, &mut state, &never_events(), &mailbox, &mut drain);

        assert_eq!(app.get_current_item_name().as_str(), "");
        assert_eq!(app.get_current_item_parse_status().as_str(), "No tooltip");
        assert_eq!(state.current_item().item_name, None);
    }

    /// A discrete `CurrentItemUpdate` re-projects the TradeScreen card.
    ///
    /// The event is the caller-facing contract for a parsed tooltip (upstream
    /// `TooltipDataReadyMessage` → `TradeViewModel.HandleTooltipDataReadyMessage`,
    /// TradeViewModel.cs:148-163): applying it must move the card onto the
    /// event's payload instead of leaving the previous projection's item in
    /// place. The card is re-derived by the one `project_trade_state`
    /// projection — the same one the accepted-snapshot path uses — so this
    /// pins the discrete path onto it.
    #[test]
    fn a_discrete_current_item_update_reprojects_the_slint_item_card() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();

        // The card stands on an earlier parsed item before the event arrives.
        let current = state.current_item_mut();
        current.item_type = Some("Boots".to_string());
        current.base_type = Some("Boots".to_string());
        current.item_power = Some(862);
        crate::actions::trade::project_trade_state(&app, &state);
        assert_eq!(app.get_current_item_type().as_str(), "Boots");

        apply_event_to_ui(
            &app,
            &mut state,
            &RuntimeEvent::CurrentItemUpdate {
                item_name: Some("Amulet of Speed".to_string()),
                parse_status: "Parsed".to_string(),
                item_type: Some("Amulet".to_string()),
                base_type: Some("Amulet".to_string()),
                item_power: Some(925),
                affixes: vec![d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(0),
                    affix_id: Some("Affix_AttackSpeed".to_string()),
                    label: "Attack Speed".to_string(),
                    value_text: Some("18.0%".to_string()),
                    source_text: "+18.0% Attack Speed".to_string(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                }],
                affix_row_geometry: Vec::new(),
                non_affix_rows: Vec::new(),
                tooltip_window_relative: None,
            },
        );

        assert_eq!(
            app.get_current_item_type().as_str(),
            "Amulet",
            "the event payload must re-project the card's item type"
        );
        assert_eq!(
            app.get_current_item_power(),
            925,
            "the event payload must re-project the card's item power"
        );
        let affixes = app.get_current_affixes();
        use slint::Model as _;
        assert_eq!(
            affixes.row_count(),
            1,
            "the event payload must re-project the card's parsed affixes"
        );
        assert_eq!(
            affixes
                .row_data(0)
                .map(|label| label.to_string())
                .as_deref(),
            Some("Attack Speed"),
            "the card's affix row must render the payload's label"
        );
    }

    /// A receiver that never yields an event.
    fn never_events() -> RuntimeEventReceiver {
        crate::runtime_bridge::RuntimeEventSink::new().1
    }

    #[test]
    fn append_then_clear_round_trip() {
        let _app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        apply_event_to_ui(
            &_app,
            &mut state,
            &RuntimeEvent::LogMessage {
                timestamp: "00:00".to_string(),
                message: "first".to_string(),
            },
        );
        apply_event_to_ui(
            &_app,
            &mut state,
            &RuntimeEvent::LogMessage {
                timestamp: "00:01".to_string(),
                message: "second".to_string(),
            },
        );
        assert_eq!(state.logging.messages.len(), 2);

        // Use the LoggingViewModel path directly to keep this test
        // independent of the Slint callback wiring.
        let _ = state.logging.clear_log_messages();
        apply_event_to_ui(
            &_app,
            &mut state,
            &RuntimeEvent::LogMessage {
                timestamp: "00:02".to_string(),
                message: "third".to_string(),
            },
        );
        // After clear + new push, only the new entry is present.
        assert_eq!(state.logging.messages.len(), 1);

        // Sanity: a brand-new LoggingViewModel starts empty.
        let fresh = LoggingViewModel::new();
        assert!(fresh.messages.is_empty());
    }
}
