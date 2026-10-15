//! Runtime event application and the Slint-facing UI projection.

use super::events::RuntimeEvent;
use super::live_state;
use super::overlay::overlay_status_text;
use crate::app_state::{AppState, OverlayTextState};
use d4lc_overlay_x11::{OverlayBackend, OverlayStatus};

// ---------------------------------------------------------------------------
// Runtime event application — apply RuntimeEvent → AppState
// ---------------------------------------------------------------------------

/// Apply a `RuntimeEvent` to `AppState`, updating the relevant fields.
///
/// This is the single dispatch point called by the UI thread when it receives
/// a runtime event from the live loop.  Each variant maps to a subset of state
/// fields, keeping the projection narrow and predictable.
pub fn apply_runtime_event(state: &mut AppState, event: &RuntimeEvent) {
    match event {
        RuntimeEvent::CurrentItemUpdate {
            item_name,
            parse_status,
            item_type,
            base_type,
            item_power,
            affixes,
            affix_row_geometry,
            non_affix_rows,
            tooltip_window_relative,
        } => {
            // Forward item info into both the shell's current-item panel and
            // the runtime status projection so the UI can read either path.
            // The parsed payload (type, power, affixes) replaces the card's
            // fields wholesale — a `None`/empty payload (no parsed item)
            // resets the card, mirroring the upstream
            // HandleTooltipDataReadyMessage overwrite. The row geometry, the
            // non-affix rows, and the tooltip rect reset with the same
            // payload: stale geometry must never anchor markers onto a
            // tooltip that is gone.
            let current = state.current_item_mut();
            current.item_name = item_name.clone();
            current.parse_status = parse_status.clone();
            current.item_type = item_type.clone();
            current.base_type = base_type.clone();
            current.item_power = *item_power;
            current.affixes = affixes.clone();
            current.affix_row_geometry = affix_row_geometry.clone();
            current.non_affix_rows = non_affix_rows.clone();
            current.tooltip_window_relative = *tooltip_window_relative;
            state.runtime_status.current_item_name = item_name.clone();
            state.runtime_status.current_item_parse_status = parse_status.clone();
        }
        RuntimeEvent::Preparing { detail } => {
            // The loop is alive and stoppable, but it processes no frame
            // yet: the status names the wait instead of claiming "Running".
            state.runtime_status.is_running = true;
            state.runtime_status.is_preparing = true;
            state.runtime_status.status_message = format!("Starting: {detail}");
        }
        RuntimeEvent::Started => {
            state.runtime_status.is_running = true;
            state.runtime_status.is_preparing = false;
            state.runtime_status.status_message = "Running".to_string();
            // The live loop prepares from the current settings as its first
            // applied configuration revision.
            state.runtime_status.settings.started(1);
        }
        RuntimeEvent::Stopped { reason } => {
            state.runtime_status.is_running = false;
            state.runtime_status.is_preparing = false;
            state.runtime_status.settings.stopped();
            state.runtime_status.status_message = format!("Stopped: {reason}");
            // A stopped loop presents nothing: the card shows the explicit
            // empty state instead of the last accepted item, and there is no
            // accepted snapshot left to report. The overlay fields keep the
            // last surface report — the standing capability record is still
            // what the user sees on screen.
            state.runtime_status.accepted = None;
            // The stopped loop holds no accepted snapshot, so its live debug
            // bundle and its newest diagnostics window leave with it — the
            // screen falls back to its named off/empty states.
            state.runtime_status.live_debug_bundle = None;
            state.runtime_status.live_debug_bundle_current = false;
            state.runtime_status.live_debug_generation = 0;
            state.runtime_status.live_debug_published_at = None;
            state.runtime_status.recent_diagnostics.clear();
            let stopped = live_state::stopped_analysis();
            apply_runtime_event(state, &live_state::current_item_event(&stopped, &[]));
        }
        RuntimeEvent::Error { message } => {
            state.runtime_status.is_running = false;
            state.runtime_status.is_preparing = false;
            state.runtime_status.status_message = format!("Error: {message}");
        }
        RuntimeEvent::SettingsReloaded { detail, revision } => {
            // Non-fatal: the live loop keeps running on the swapped runtime.
            state.runtime_status.status_message = format!("Settings reloaded: {detail}");
            state.runtime_status.settings.applied(*revision);
        }
        RuntimeEvent::SettingsReloadFailed { message } => {
            // Non-fatal: the live loop keeps running with the previously
            // prepared runtime; `is_running` is deliberately untouched. The
            // failed switch stays visible on the settings status until a
            // later preparation applies.
            state.runtime_status.status_message = format!("Settings reload failed: {message}");
            state.runtime_status.settings.failed(message);
        }
        RuntimeEvent::GameWindowObservation { text } => {
            state.runtime_status.game_window_status = Some(text.clone());
        }
        RuntimeEvent::LogMessage { timestamp, message } => {
            // Forward to the same `LoggingViewModel` the Clear Log
            // action clears, so all message sources (hotkey, Debug
            // action, future runtime events) share one model.
            state.append_log_message(message.clone(), timestamp.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// UI projection — pure function computing Slint-visible values from AppState
// ---------------------------------------------------------------------------

/// Projection of runtime state intended for direct consumption by the Slint UI.
///
/// Every field corresponds to a property in the `.slint` file that the live
/// loop's status needs to surface.  Computed from `AppState` on the UI thread
/// so the .slint file never holds a reference to the runtime's internal types.
///
/// The accepted-snapshot block (`accepted_generation_text`,
/// `overlay_renderer_text`, `frame_state_text`, `debug_frozen`,
/// `evaluation_text`, `published_at_text`, `overlay_text_status_text`) is
/// computed from the one accepted snapshot's view on `AppState`
/// (`RuntimeStatus::accepted`, `RuntimeStatus::overlay_status`), so the item,
/// the tooltip state, the evaluation, the overlay status and the frame numbers
/// always describe the same generation.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeUiProjection {
    pub runtime_running: bool,
    /// The started loop is still preparing: the Debug "Runtime:" rows read
    /// "Starting", never "Running".
    pub runtime_preparing: bool,
    pub runtime_status: String,
    pub current_item_name: String,
    pub current_item_parse_status: String,
    /// The Debug screen's "Overlay text:" line: the surface's static
    /// text state (font assignment, or the exact reason text is disabled) plus
    /// the characters its bounded text path flagged.
    pub overlay_text_status_text: String,
    /// Accepted generation and the frame the accepted analysis was read from.
    pub accepted_generation_text: String,
    /// Generation the overlay renderer drew and its outcome for the accepted
    /// snapshot.
    pub overlay_renderer_text: String,
    /// Live versus frozen: the presented frame is the frozen debug capture.
    pub frame_state_text: String,
    /// The presented frame is frozen (drives the warning color).
    pub debug_frozen: bool,
    /// The accepted snapshot's rule evaluation: rows and markers.
    pub evaluation_text: String,
    /// When the loop published the accepted snapshot (local HH:MM:SS).
    pub published_at_text: String,
    /// What the live per-frame number measures — and what it deliberately does
    /// not. Rendered beside the live values so neither the stored run
    /// below nor another phase can be read into them.
    pub hotpath_scope_text: String,
    /// The live asynchronous OCR phase of the same loop, or why it is
    /// absent.
    pub ocr_phase_text: String,
    /// The live loop's per-tick stage-cost percentiles, one
    /// `p50/p95/p99` triple per field; empty until the loop recorded ticks.
    pub loop_timings_text: String,
    /// The Debug screen's "Overlay:" text, rendered from the live loop's
    /// last published `OverlayStatus`. Before the loop has
    /// reported it holds [`OVERLAY_STATUS_PENDING_TEXT`], matching the
    /// `.slint` property default — the field never claims a surface
    /// state the loop has not reported.
    pub overlay_status_text: String,
    /// The Debug screen's "Game window:" text: the game-window observer's
    /// capability report.
    pub game_window_status_text: String,
    /// The Debug screen's "Capture engine:" text: the resolved engine and
    /// reason of the live runtime's start (`auto` detection included).
    /// Before a runtime start published it holds
    /// [`CAPTURE_ENGINE_PENDING_TEXT`], matching the `.slint` property
    /// default.
    pub capture_engine_text: String,
    /// The Debug screen's "Settings:" text: desired versus applied.
    pub settings_status_text: String,
    pub debug_frame_sequence: u64,
    pub debug_hotpath_ms: u128,
    pub debug_tooltip_found: bool,
}

/// Debug-screen "Overlay:" text before the live loop published any
/// status. Same string as the `overlay-status-text` property default in
/// `ui/main_window.slint` / `ui/screens/debug_screen.slint`; the
/// projection keeps the two in lockstep.
const OVERLAY_STATUS_PENDING_TEXT: &str = "Offline";

/// Debug-screen "Game window:" text before the live loop reported its
/// observer. Same string as the `game-window-status-text` property default.
const GAME_WINDOW_STATUS_PENDING_TEXT: &str = "Not observed";

/// Debug-screen "Capture engine:" text before a runtime start published
/// its resolution. Same string as the `capture-engine-text` property
/// default.
const CAPTURE_ENGINE_PENDING_TEXT: &str = "Not resolved";

/// Debug-screen "Overlay text:" line before the live loop published a state.
/// Same shape as the `overlay-text-status-text` property default.
const OVERLAY_TEXT_PENDING_TEXT: &str = "No overlay surface reported yet";

/// What the live per-frame number measures, rendered as the live
/// values' own scope note. The per-stage rows further down come from a stored
/// export run, not from this frame, and the async OCR pass is reported as its
/// own live line; neither is a hidden component of `hotpath_ms`.
const HOTPATH_SCOPE_TEXT: &str =
    "live frame pass only: capture, detection, crop stability, result poll. The async OCR work \
     runs on the worker's own thread (reported below) and the time until the overlay is visible \
     is not measured per frame — neither is part of this number.";

/// The live asynchronous OCR phase, or why there is none.
///
/// The counters are the worker's own; the average pass duration is derived from
/// its two recorded totals, never synthesized. A loop without a worker says so
/// instead of showing zeros.
fn ocr_phase_text(phases: Option<&crate::app_state::OcrPhaseView>) -> String {
    let Some(phases) = phases else {
        return "no OCR worker in this loop".to_string();
    };
    let state = if phases.in_progress {
        "working"
    } else if phases.pending {
        "result ready"
    } else {
        "idle"
    };
    let average = match phases.average_pass_ms() {
        Some(ms) => format!("{ms} ms average per completed pass"),
        None => "no completed pass yet".to_string(),
    };
    let mut text = format!(
        "{state}: {} completed, {average}, {} dropped, {} failed, deepest queue {}",
        phases.completed, phases.dropped, phases.failed, phases.max_queue_depth
    );
    if let Some(error) = &phases.last_error {
        text.push_str(&format!("; last error: {error}"));
    }
    text
}

/// The Debug screen's overlay text state: the surface's own static
/// text state plus what its bounded text path flagged.
///
/// The static state travels in `OverlayStatus.limitation`: a live handle
/// reports its text entry as part of the joined capability list, and the
/// shared renderer's vocabulary names it `overlay text…` (font assignment, or
/// the reason text is disabled). A degraded handle carries no text path at
/// all, so its limitation *is* the reason. The flagged codepoints and clipped
/// characters come from the handle's own per-frame diagnostics.
fn overlay_text_status_text(status: Option<&OverlayStatus>, text: &OverlayTextState) -> String {
    let head = match status {
        None => OVERLAY_TEXT_PENDING_TEXT.to_string(),
        Some(status) => match status.limitation.as_deref() {
            Some(limitation) => match text_entry(limitation) {
                Some(entry) => entry,
                None if matches!(
                    status.backend,
                    OverlayBackend::HeadlessNoop | OverlayBackend::Failed
                ) =>
                {
                    format!("no text path on this surface: {limitation}")
                }
                None => "no text-path report in the surface status".to_string(),
            },
            None => "no text-path report in the surface status".to_string(),
        },
    };
    match text.flagged_text() {
        Some(flagged) => format!("{head} — {flagged}"),
        None => head,
    }
}

/// The `overlay text…` entry of a joined capability list. The shared text
/// renderer's own vocabulary, produced by both overlay backends.
fn text_entry(limitation: &str) -> Option<String> {
    limitation
        .split("; ")
        .find(|entry| entry.starts_with("overlay text"))
        .map(str::to_string)
}

/// Compute `RuntimeUiProjection` from `AppState`.
///
/// Pure function — no side effects, no mutable state.  Called by the Slint UI
/// thread each render cycle to obtain the latest live-loop snapshot.
pub fn runtime_ui_projection(state: &AppState) -> RuntimeUiProjection {
    let accepted = state.runtime_status.accepted.as_ref();
    RuntimeUiProjection {
        runtime_running: state.runtime_status.is_running,
        runtime_preparing: state.runtime_status.is_preparing,
        runtime_status: state.runtime_status.status_message.clone(),
        overlay_text_status_text: overlay_text_status_text(
            state.runtime_status.overlay_status.as_ref(),
            &state.runtime_status.overlay_text,
        ),
        accepted_generation_text: accepted.map_or_else(
            || "no snapshot yet".to_string(),
            crate::app_state::AcceptedSnapshotView::generation_text,
        ),
        overlay_renderer_text: accepted.map_or_else(
            || "no snapshot yet".to_string(),
            crate::app_state::AcceptedSnapshotView::renderer_text,
        ),
        frame_state_text: accepted.map_or_else(
            || "no snapshot yet".to_string(),
            crate::app_state::AcceptedSnapshotView::frame_state_text,
        ),
        debug_frozen: accepted.is_some_and(|accepted| accepted.frozen),
        evaluation_text: accepted.map_or_else(
            || "no evaluation yet".to_string(),
            crate::app_state::AcceptedSnapshotView::evaluation_text,
        ),
        published_at_text: accepted.map_or_else(
            || "—".to_string(),
            |accepted| crate::clock::hh_mm_ss(accepted.published_at),
        ),
        hotpath_scope_text: HOTPATH_SCOPE_TEXT.to_string(),
        ocr_phase_text: ocr_phase_text(state.runtime_status.ocr_phases.as_ref()),
        loop_timings_text: state
            .runtime_status
            .loop_timings
            .as_ref()
            .map(d4lc_runtime::diagnostics::LoopTimingPercentiles::render)
            .unwrap_or_default(),
        current_item_name: state
            .runtime_status
            .current_item_name
            .clone()
            .unwrap_or_default(),
        current_item_parse_status: state.runtime_status.current_item_parse_status.clone(),
        overlay_status_text: state
            .runtime_status
            .overlay_status
            .as_ref()
            .map(overlay_status_text)
            .unwrap_or_else(|| OVERLAY_STATUS_PENDING_TEXT.to_string()),
        game_window_status_text: state
            .runtime_status
            .game_window_status
            .clone()
            .unwrap_or_else(|| GAME_WINDOW_STATUS_PENDING_TEXT.to_string()),
        capture_engine_text: match (&state.capture_engine, &state.runtime_status.capture_stream) {
            (Some(engine), Some(stream)) => format!("{}; {stream}", engine.text),
            (Some(engine), None) => engine.text.clone(),
            (None, _) => CAPTURE_ENGINE_PENDING_TEXT.to_string(),
        },
        settings_status_text: state.runtime_status.settings.text(),
        debug_frame_sequence: state.runtime_status.debug_frame_sequence,
        debug_hotpath_ms: state.runtime_status.debug_hotpath_ms,
        debug_tooltip_found: state.runtime_status.debug_tooltip_found,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::runtime_bridge::live_state::{LiveStateProjection, Presentation};
    use crate::runtime_bridge::overlay::{convert_descriptors, overlay_status_without_surface};
    use crate::runtime_bridge::test_support::{
        analysis_presentation, live_state_fixture, sample_overlay_descriptor_state,
        sample_overlay_status,
    };
    use d4lc_core::Rect;
    use d4lc_gui::{
        CurrentItemAffixRowGeometryView, CurrentItemAffixView, CurrentItemNonAffixRowView,
    };
    use std::sync::Arc;
    use std::time::Duration;

    // -------------------------------------------------------------------
    // Runtime event application tests
    // -------------------------------------------------------------------

    #[test]
    fn apply_runtime_event_started_sets_running_flag() {
        let mut state = AppState::default_startup();
        assert!(!state.runtime_status.is_running);

        apply_runtime_event(&mut state, &RuntimeEvent::Started);

        assert!(state.runtime_status.is_running);
        assert_eq!(state.runtime_status.status_message, "Running");
    }

    #[test]
    fn preparing_reports_the_wait_and_never_claims_running() {
        let mut state = AppState::default_startup();

        apply_runtime_event(
            &mut state,
            &RuntimeEvent::Preparing {
                detail: "waiting for the screen-share selection".to_string(),
            },
        );

        // Stoppable (Stop is offered, Start is not), but not running.
        assert!(state.runtime_status.is_running);
        assert!(state.runtime_status.is_preparing);
        assert_eq!(
            state.runtime_status.status_message,
            "Starting: waiting for the screen-share selection"
        );
        let projection = runtime_ui_projection(&state);
        assert!(projection.runtime_running);
        assert!(projection.runtime_preparing);
        assert_ne!(projection.runtime_status, "Running");
        // Settings are not applied before the preparation succeeded.
        assert!(state.runtime_status.settings.text().contains("not applied"));

        apply_runtime_event(&mut state, &RuntimeEvent::Started);
        assert!(state.runtime_status.is_running);
        assert!(!state.runtime_status.is_preparing);
        assert_eq!(state.runtime_status.status_message, "Running");
        assert!(!runtime_ui_projection(&state).runtime_preparing);
    }

    #[test]
    fn stop_or_error_during_preparation_clears_the_preparing_state() {
        for terminal in [
            RuntimeEvent::Stopped {
                reason: "stopped during preparation".to_string(),
            },
            RuntimeEvent::Error {
                message: "runtime preparation failed: dialog cancelled".to_string(),
            },
        ] {
            let mut state = AppState::default_startup();
            apply_runtime_event(
                &mut state,
                &RuntimeEvent::Preparing {
                    detail: "preparing the runtime".to_string(),
                },
            );
            apply_runtime_event(&mut state, &terminal);
            assert!(!state.runtime_status.is_running, "{terminal:?}");
            assert!(!state.runtime_status.is_preparing, "{terminal:?}");
        }
    }

    #[test]
    fn apply_runtime_event_stopped_clears_running_flag() {
        let mut state = AppState::default_startup();
        state.runtime_status.is_running = true;

        apply_runtime_event(
            &mut state,
            &RuntimeEvent::Stopped {
                reason: "test stop".to_string(),
            },
        );

        assert!(!state.runtime_status.is_running);
        assert_eq!(state.runtime_status.status_message, "Stopped: test stop");
    }

    #[test]
    fn apply_runtime_event_current_item_update_propagates_to_both_paths() {
        let mut state = AppState::default_startup();
        assert!(state.current_item().item_name.is_none());
        assert_eq!(state.current_item().parse_status, "not_item");

        apply_runtime_event(
            &mut state,
            &RuntimeEvent::CurrentItemUpdate {
                item_name: Some("Helm".to_string()),
                parse_status: "ok".to_string(),
                item_type: None,
                base_type: None,
                item_power: None,
                affixes: Vec::new(),
                affix_row_geometry: Vec::new(),
                non_affix_rows: Vec::new(),
                tooltip_window_relative: None,
            },
        );

        // Shell current-item panel
        assert_eq!(state.current_item().item_name.as_deref(), Some("Helm"));
        assert_eq!(state.current_item().parse_status, "ok");
        // Runtime status projection
        assert_eq!(
            state.runtime_status.current_item_name.as_deref(),
            Some("Helm")
        );
        assert_eq!(state.runtime_status.current_item_parse_status, "ok");
        // Runtime status should still be in its default stopped state
        assert!(!state.runtime_status.is_running);
    }

    #[test]
    fn apply_runtime_event_current_item_update_applies_parsed_payload() {
        // The event carries the parsed
        // item type, power, and affix rows, and the current-item panel
        // receives them wholesale so the trade screen and the live
        // trade lookup derive from the parsed tooltip.
        let mut state = AppState::default_startup();

        apply_runtime_event(
            &mut state,
            &RuntimeEvent::CurrentItemUpdate {
                item_name: Some("Ring of Spec".to_string()),
                parse_status: "Parsed".to_string(),
                item_type: Some("ring".to_string()),
                base_type: Some("Ring".to_string()),
                item_power: Some(912),
                affixes: vec![CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(0),
                    affix_id: Some("Affix_AttackSpeed".to_string()),
                    label: "Attack Speed".to_string(),
                    value_text: Some("18.0%".to_string()),
                    source_text: "+18.0% Attack Speed".to_string(),
                    is_greater: true,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                }],
                affix_row_geometry: vec![CurrentItemAffixRowGeometryView {
                    row_id: d4lc_core::AffixRowId(0),
                    source_segment_index: 0,
                    source_line_index: 3,
                    global_line_index: 4,
                    marker_ordinal: None,
                    text_rect: Rect {
                        x: 610,
                        y: 290,
                        width: 230,
                        height: 21,
                    },
                    affix_area: Rect {
                        x: 610,
                        y: 290,
                        width: 230,
                        height: 21,
                    },
                    marker_anchor: None,
                }],
                non_affix_rows: vec![CurrentItemNonAffixRowView {
                    kind: d4lc_gui::CurrentItemMatchKind::Rune,
                    id: Some("Item_Rune_Condition_OnMove".to_string()),
                    label: Some("Travel meters".to_string()),
                    source_line_index: 0,
                    global_line_index: 8,
                    rect: Some(Rect {
                        x: 600,
                        y: 420,
                        width: 240,
                        height: 20,
                    }),
                    marker_anchor: None,
                }],
                tooltip_window_relative: Some(Rect {
                    x: 590,
                    y: 110,
                    width: 410,
                    height: 300,
                }),
            },
        );

        let current = state.current_item();
        assert_eq!(current.item_type.as_deref(), Some("ring"));
        assert_eq!(
            current.base_type.as_deref(),
            Some("Ring"),
            "the display derivation travels beside the canonical evaluation id"
        );
        assert_eq!(current.item_power, Some(912));
        assert_eq!(current.affixes.len(), 1);
        assert_eq!(
            current.affixes[0].affix_id.as_deref(),
            Some("Affix_AttackSpeed")
        );
        assert!(current.affixes[0].is_greater);
        // The row geometry and the tooltip rect ride the same event and
        // land on the panel — the overlay's anchoring inputs.
        assert_eq!(current.affix_row_geometry.len(), 1);
        assert_eq!(current.affix_row_geometry[0].source_line_index, 3);
        assert_eq!(
            current.affix_row_geometry[0].text_rect,
            Rect {
                x: 610,
                y: 290,
                width: 230,
                height: 21,
            }
        );
        assert_eq!(
            current.tooltip_window_relative,
            Some(Rect {
                x: 590,
                y: 110,
                width: 410,
                height: 300,
            })
        );
        // The classified non-affix rows ride the same event and land on the
        // panel next to the affix geometry.
        assert_eq!(current.non_affix_rows.len(), 1);
        assert_eq!(
            current.non_affix_rows[0].kind,
            d4lc_gui::CurrentItemMatchKind::Rune
        );
        assert_eq!(
            current.non_affix_rows[0].id.as_deref(),
            Some("Item_Rune_Condition_OnMove")
        );
        assert_eq!(current.non_affix_rows[0].global_line_index, 8);
        assert_eq!(
            current.non_affix_rows[0].rect,
            Some(Rect {
                x: 600,
                y: 420,
                width: 240,
                height: 20,
            })
        );

        // A follow-up event without parsed data resets the card — and the
        // anchoring inputs with it, so stale geometry never survives.
        apply_runtime_event(
            &mut state,
            &RuntimeEvent::CurrentItemUpdate {
                item_name: None,
                parse_status: "Parse failed".to_string(),
                item_type: None,
                base_type: None,
                item_power: None,
                affixes: Vec::new(),
                affix_row_geometry: Vec::new(),
                non_affix_rows: Vec::new(),
                tooltip_window_relative: None,
            },
        );
        let current = state.current_item();
        assert!(current.item_type.is_none());
        assert!(current.base_type.is_none());
        assert_eq!(current.item_power, None);
        assert!(current.affixes.is_empty());
        assert!(current.affix_row_geometry.is_empty());
        assert!(current.non_affix_rows.is_empty());
        assert_eq!(current.tooltip_window_relative, None);
    }

    /// The per-frame debug fields are not an event of their own: they are part
    /// of the accepted snapshot the mailbox carries.
    #[test]
    fn debug_status_projects_from_the_accepted_snapshot() {
        let mut state = AppState::default_startup();
        let mut projection = LiveStateProjection::default();
        let live = live_state_fixture(1, 7, false);

        assert!(projection.apply(&mut state, &live));

        assert_eq!(state.runtime_status.debug_frame_sequence, 7);
        assert_eq!(state.runtime_status.debug_hotpath_ms, 42);
        assert!(state.runtime_status.debug_tooltip_found);
    }

    #[test]
    fn apply_runtime_event_error_sets_status_message() {
        let mut state = AppState::default_startup();
        state.runtime_status.is_running = true;

        apply_runtime_event(
            &mut state,
            &RuntimeEvent::Error {
                message: "capture failed".to_string(),
            },
        );

        assert!(!state.runtime_status.is_running);
        assert!(state
            .runtime_status
            .status_message
            .contains("capture failed"));
    }

    #[test]
    fn apply_runtime_event_settings_reloaded_is_non_fatal() {
        let mut state = AppState::default_startup();
        state.runtime_status.is_running = true;

        apply_runtime_event(
            &mut state,
            &RuntimeEvent::SettingsReloaded {
                detail: "runtime re-prepared: preset '1440p_SMF'".to_string(),
                revision: 2,
            },
        );
        assert_eq!(state.runtime_status.settings.applied_revision, Some(2));

        assert_eq!(
            state.runtime_status.status_message,
            "Settings reloaded: runtime re-prepared: preset '1440p_SMF'"
        );
        assert!(
            state.runtime_status.is_running,
            "a settings reload must never stop the live loop"
        );
    }

    #[test]
    fn apply_runtime_event_settings_reload_failed_is_non_fatal() {
        let mut state = AppState::default_startup();
        state.runtime_status.is_running = true;

        apply_runtime_event(
            &mut state,
            &RuntimeEvent::SettingsReloadFailed {
                message: "template load failed: preset 'broken' is missing".to_string(),
            },
        );

        assert_eq!(
            state.runtime_status.status_message,
            "Settings reload failed: template load failed: preset 'broken' is missing"
        );
        assert!(
            state.runtime_status.is_running,
            "a failed reload keeps the loop running with the previous runtime"
        );
        assert!(runtime_ui_projection(&state)
            .settings_status_text
            .contains("FAILED to apply change: template load failed"));
    }

    // -------------------------------------------------------------------
    // UI projection tests
    // -------------------------------------------------------------------

    #[test]
    fn runtime_ui_projection_computes_running_state() {
        let mut state = AppState::default_startup();
        state.runtime_status.is_running = true;
        state.runtime_status.status_message = "Running".to_string();

        let projection = runtime_ui_projection(&state);

        assert!(projection.runtime_running);
        assert_eq!(projection.runtime_status, "Running");
    }

    #[test]
    fn runtime_ui_projection_shows_current_item() {
        let mut state = AppState::default_startup();
        state.runtime_status.current_item_name = Some("Helm".to_string());
        state.runtime_status.current_item_parse_status = "ok".to_string();

        let projection = runtime_ui_projection(&state);

        assert_eq!(projection.current_item_name, "Helm");
        assert_eq!(projection.current_item_parse_status, "ok");
    }

    #[test]
    fn runtime_ui_projection_carries_the_resolved_capture_engine() {
        // The "Capture engine:" row is the recorded decision of one
        // runtime start (engine + reason). Before any start published it,
        // the row holds the pending default — never a claimed engine.
        let state = AppState::default_startup();
        let projection = runtime_ui_projection(&state);
        assert_eq!(projection.capture_engine_text, "Not resolved");

        // The publication the live loop performs after preparation lands
        // engine and reason in the projection as one snapshot value.
        let mut state = AppState::default_startup();
        state.set_capture_engine(
            "auto → Wayland portal: no X11 game window, wayland session".to_string(),
            Some(d4lc_core::Rect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            }),
        );
        let projection = runtime_ui_projection(&state);
        assert_eq!(
            projection.capture_engine_text,
            "auto → Wayland portal: no X11 game window, wayland session"
        );

        // The live stream-rate line joins the recorded decision.
        state.runtime_status.capture_stream = Some(
            "stream variable fps, max 60/1 (60 fps cap accepted); delivered 58 fps, \
             consumed 57 fps, 0 unconsumed frames skipped"
                .to_string(),
        );
        let projection = runtime_ui_projection(&state);
        assert_eq!(
            projection.capture_engine_text,
            "auto → Wayland portal: no X11 game window, wayland session; stream variable \
             fps, max 60/1 (60 fps cap accepted); delivered 58 fps, consumed 57 fps, 0 \
             unconsumed frames skipped"
        );
    }

    #[test]
    fn runtime_ui_projection_renders_the_published_overlay_status() {
        // Before the loop reports, the Debug field keeps the pending
        // default: the projection never claims a surface on its own.
        let mut state = AppState::default_startup();
        assert_eq!(
            runtime_ui_projection(&state).overlay_status_text,
            OVERLAY_STATUS_PENDING_TEXT
        );

        // A live real backend renders its own vocabulary, keeps the
        // status on AppState, and stays a single line.
        for backend in [OverlayBackend::RealX11, OverlayBackend::RealWayland] {
            let mut live = live_state_fixture(1, 4, false);
            live.overlay.status = Some(sample_overlay_status(
                backend,
                true,
                Some("standing deferral note"),
            ));
            let mut projection = LiveStateProjection::default();
            projection.apply(&mut state, &live);

            assert_eq!(
                runtime_ui_projection(&state).overlay_status_text,
                format!("{backend} (enabled)")
            );
            assert_eq!(
                state
                    .runtime_status
                    .overlay_status
                    .as_ref()
                    .map(|status| status.backend),
                Some(backend)
            );
        }
    }

    #[test]
    fn degraded_overlay_status_names_its_limitation_in_the_debug_field() {
        // A handle that degraded to headless_noop or failed, and the
        // synthesized no-surface status of a WindowUnavailable loop, all
        // name what the handle carries instead of only the backend.
        let cases = [
            (
                sample_overlay_status(
                    OverlayBackend::HeadlessNoop,
                    true,
                    Some("real X11 overlay requires DISPLAY"),
                ),
                "headless_noop (enabled): real X11 overlay requires DISPLAY",
            ),
            (
                sample_overlay_status(
                    OverlayBackend::Failed,
                    true,
                    Some("X11 init failed: boom; using headless fallback"),
                ),
                "failed (enabled): X11 init failed: boom; using headless fallback",
            ),
            (
                overlay_status_without_surface(
                    "no overlay surface: the game window geometry could not be resolved: no window",
                ),
                "headless_noop (disabled): no overlay surface: the game window geometry could not be resolved: no window",
            ),
        ];

        for (status, expected) in cases {
            assert_eq!(overlay_status_text(&status), expected);
            let mut state = AppState::default_startup();
            let mut live = live_state_fixture(1, 4, false);
            live.overlay.status = Some(status);
            LiveStateProjection::default().apply(&mut state, &live);
            assert_eq!(
                runtime_ui_projection(&state).overlay_status_text,
                expected,
                "the Debug field must render the published status verbatim"
            );
        }
    }

    /// Absence is an explicit state: the accepted snapshot's projection of an
    /// empty analysis resets the card instead of leaving the last item
    /// standing.
    #[test]
    fn an_empty_analysis_projects_an_explicit_empty_card() {
        let mut state = AppState::default_startup();
        let mut projection = LiveStateProjection::default();
        projection.apply(&mut state, &live_state_fixture(1, 4, false));
        assert_eq!(state.current_item().item_name.as_deref(), Some("Helm"));

        // No-Tooltip: the coordinator publishes the explicit empty analysis,
        // and the very same snapshot reaches the card.
        let mut no_tooltip = live_state_fixture(1, 5, false);
        no_tooltip.presentation = Arc::new(Presentation {
            analysis: Arc::new(d4lc_runtime::AnalysisSnapshot::empty(1)),
            rules_revision: Some(1),
            descriptors: Arc::from([]),
            affix_traces: Vec::new(),
        });
        no_tooltip.debug.generation = 0;
        assert!(
            projection.apply(&mut state, &no_tooltip),
            "a new accepted generation re-projects the card"
        );

        assert_eq!(state.current_item().item_name, None);
        assert!(state.current_item().affixes.is_empty());
        assert_eq!(state.current_item().tooltip_window_relative, None);
        assert_eq!(state.current_item().parse_status, "No tooltip");
        assert_eq!(state.runtime_status.current_item_name, None);
        assert_eq!(state.runtime_status.current_item_parse_status, "No tooltip");
    }

    /// Criterion a: the UI and the renderer describe one accepted snapshot —
    /// its generation, the frame the analysis was read from, the renderer's
    /// outcome for that very generation, the evaluation and the overlay text
    /// state all come out of the one `LiveState`.
    #[test]
    fn the_accepted_snapshot_reports_the_generation_the_renderer_drew() {
        let mut state = AppState::default_startup();
        let mut projection = LiveStateProjection::default();
        let mut live = live_state_fixture(1, 4, false);
        live.overlay.status = Some(sample_overlay_status(
            OverlayBackend::RealX11,
            true,
            Some("overlay text: Sans 13px, wrapped panels with single-line markers"),
        ));
        live.overlay.text = crate::app_state::OverlayTextState {
            unsupported_codepoints: vec!['\u{2600}'],
            clipped_chars: 2,
            ..crate::app_state::OverlayTextState::default()
        };
        projection.apply(&mut state, &live);

        let view = state
            .runtime_status
            .accepted
            .as_ref()
            .expect("the snapshot projects an accepted view");
        assert_eq!(view.generation, 4);
        assert_eq!(view.source_frame, Some(4));
        assert!(view.renderer_drew_accepted());
        assert!(!view.frozen);

        let ui = runtime_ui_projection(&state);
        assert_eq!(ui.accepted_generation_text, "4 (source frame 4)");
        assert_eq!(ui.overlay_renderer_text, "drawn 4 = accepted generation");
        assert_eq!(ui.frame_state_text, "Live (following the running loop)");
        assert_eq!(
            ui.evaluation_text,
            "0 of 0 affix row(s) matched, 0 overlay marker(s)"
        );
        assert_eq!(
            ui.published_at_text,
            crate::clock::hh_mm_ss(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(3600))
        );
        assert_eq!(
            ui.overlay_text_status_text,
            "overlay text: Sans 13px, wrapped panels with single-line markers — 1 unsupported \
             codepoint(s): U+2600; 2 clipped character(s)"
        );
    }

    /// Criterion c: a frozen debug capture is explicitly recognizable, and the
    /// renderer line still names the generation it drew.
    #[test]
    fn a_frozen_snapshot_is_explicitly_recognizable() {
        let mut state = AppState::default_startup();
        let mut projection = LiveStateProjection::default();
        let live = live_state_fixture(2, 9, true);
        projection.apply(&mut state, &live);

        let ui = runtime_ui_projection(&state);
        assert!(ui.debug_frozen);
        assert!(
            ui.frame_state_text
                .starts_with("Frozen (debug capture lock): frame 9"),
            "the frozen image names the frame it holds: {}",
            ui.frame_state_text
        );
        assert!(
            ui.frame_state_text.contains("release the lock"),
            "the frozen state says how to leave it: {}",
            ui.frame_state_text
        );
    }

    /// A renderer that refused a later snapshot reports the generation it
    /// really drew instead of claiming the accepted one.
    #[test]
    fn a_refused_draw_reports_the_generation_that_stays_visible() {
        let mut state = AppState::default_startup();
        let mut projection = LiveStateProjection::default();
        let mut live = live_state_fixture(1, 12, false);
        live.debug.drawn_generation = Some(7);
        live.debug.draw = Some(live_state::PresentOutcome::Refused);
        projection.apply(&mut state, &live);

        let view = state.runtime_status.accepted.as_ref().expect("view");
        assert!(!view.renderer_drew_accepted());
        assert_eq!(
            runtime_ui_projection(&state).overlay_renderer_text,
            "refused (older than a confirmed clear) 7 (accepted generation 12)"
        );
    }

    /// The item card follows a new accepted generation only; republications of
    /// the same generation cost one comparison.
    #[test]
    fn the_item_card_is_reprojected_only_for_a_new_accepted_generation() {
        let mut state = AppState::default_startup();
        let mut projection = LiveStateProjection::default();
        let live = live_state_fixture(1, 4, false);

        assert!(projection.apply(&mut state, &live));
        assert!(
            !projection.apply(&mut state, &live),
            "the same accepted generation is not projected twice"
        );

        // A new loop (new epoch) re-projects even the same generation.
        assert!(projection.apply(&mut state, &live_state_fixture(2, 4, false)));
    }

    /// The item projection of an accepted presentation is the item event the
    /// CurrentItemUpdate consumers already apply, and it carries the accepted
    /// evaluation's markers: item and evaluation come from one snapshot.
    #[test]
    fn the_item_projection_is_the_accepted_snapshots_item_event() {
        let presentation = analysis_presentation(3, true, None);
        let RuntimeEvent::CurrentItemUpdate { item_name, .. } =
            live_state::item_event_for(&presentation)
        else {
            panic!("the item projection is a CurrentItemUpdate");
        };
        assert_eq!(item_name.as_deref(), Some("Helm"));

        let mut with_markers = analysis_presentation(4, true, None);
        with_markers.descriptors =
            convert_descriptors(Some(&sample_overlay_descriptor_state())).into();
        assert!(!with_markers.descriptors.is_empty());
        let summary = live_state::evaluation_summary(&with_markers);
        assert_eq!(summary.markers, with_markers.descriptors.len());
        assert_eq!(summary.evaluated_rows, 0);
    }
}
