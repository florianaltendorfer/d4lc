//! Shared fixtures of the runtime bridge tests.

use crate::app_state::{
    AppState, OverlayDescriptorState, ParagonOverlayStateBundle, RuleOverlayState,
};
use crate::runtime_bridge::debug_commands::DebugCommandController;
use crate::runtime_bridge::events::RuntimeEventSink;
use crate::runtime_bridge::live_loop::{start_live_loop, LiveLoopHandle};
use crate::runtime_bridge::live_state;
use crate::runtime_bridge::live_state::{DebugStatus, LiveState, Presentation};
use crate::runtime_bridge::overlay::headless_overlay_status;
use d4lc_core::Rect;
use d4lc_overlay_x11::{OverlayBackend, OverlayStatus};
use d4lc_rules::{
    RuleAffixMatch, RuleCheck, RuleCheckKind, RuleDecision, RuleEvaluationResult, RuleExplanation,
    RuleOverlayLayout, RuleOverlayMarkerMode, RuleOverlaySettings, RuleSourceNote,
};
use d4lc_runtime::RuntimeLifecycle;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Helper that wraps `start_live_loop` with the right number of arguments
/// regardless of whether `real-runtime` is enabled.
/// Tests always call this instead of `start_live_loop` directly.
pub(super) fn start_test_live_loop(
    app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    #[cfg_attr(
        feature = "real-runtime",
        expect(unused_variables, reason = "the real loop paces itself")
    )]
    interval: Duration,
) -> LiveLoopHandle {
    #[cfg(feature = "real-runtime")]
    {
        start_live_loop(
            app_state,
            event_tx,
            PathBuf::from("/tmp/d4lc-test-root"),
            d4lc_runtime::RuntimeReloadSignal::new(),
        )
    }
    #[cfg(not(feature = "real-runtime"))]
    {
        start_live_loop(app_state, event_tx, interval)
    }
}

// -------------------------------------------------------------------
// Overlay status publication tests
// -------------------------------------------------------------------

/// Overlay status fixture: the shared zero geometry of
/// `headless_overlay_status` with the backend/enabled/limitation
/// under test.
pub(super) fn sample_overlay_status(
    backend: OverlayBackend,
    enabled: bool,
    limitation: Option<&str>,
) -> OverlayStatus {
    OverlayStatus {
        backend,
        enabled,
        limitation: limitation.map(str::to_string),
        ..headless_overlay_status()
    }
}

/// One accepted snapshot of a running loop: an item-bearing analysis of
/// `generation`/`frame_sequence`, the renderer's outcome for it, and the
/// overlay status the handle reported that cycle.
pub(super) fn live_state_fixture(epoch: u64, sequence: u64, frozen: bool) -> LiveState {
    let presentation = Arc::new(analysis_presentation(sequence, true, None));
    LiveState {
        epoch,
        lifecycle: RuntimeLifecycle::Running,
        debug: DebugStatus {
            frame_sequence: sequence,
            hotpath_ms: 42,
            tooltip_found: true,
            descriptor_count: presentation.descriptors.len(),
            generation: presentation.generation(),
            source_frame: Some(sequence),
            drawn_generation: Some(presentation.generation()),
            draw: Some(live_state::PresentOutcome::Drawn),
            frozen,
            // The live async phase of the running loop.
            ocr: Some(live_state::OcrPhaseStatus {
                completed: 3,
                total_ocr_ms: 120,
                pending: true,
                max_queue_depth: 2,
                ..live_state::OcrPhaseStatus::default()
            }),
            loop_timings: None,
            capture_stream: None,
            recent_diagnostics: Vec::new(),
        },
        presentation,
        overlay: live_state::OverlaySnapshot::default(),
        debug_bundle: None,
        debug_info_enabled: false,
        published_at: std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(3600),
    }
}

// -------------------------------------------------------------------
// Live loop descriptor dispatch tests
// -------------------------------------------------------------------

/// App state whose rule projection yields descriptors without a live
/// item: an active paragon board on a resolved game window.
pub(super) fn rules_state_with_descriptors() -> AppState {
    let mut state = AppState::default_startup();
    // The paragon overlay draws for the selected preset (OverlayHandler.cs:573).
    state
        .affix_preset_editor
        .create_preset("Speed")
        .unwrap_or_else(|error| panic!("seed preset: {error:?}"));
    state.affix_preset_editor.selected_preset_name = Some("Speed".to_string());
    state.shell.settings.overlay_enabled = true;
    state.shell.settings.paragon_mode_active = true;
    state.overlay_window_geometry = Some(Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    });
    state.downloaded_paragon_routes = Some(vec![d4lc_rules::ImportedParagonRoute {
        step_name: Some("Starter".to_string()),
        boards: vec![d4lc_rules::ImportedParagonBoard {
            index: 0,
            name: "Start".to_string(),
            glyph: String::new(),
            rotation: "0°".to_string(),
            active_nodes: vec![105],
        }],
    }]);
    state
}

// -------------------------------------------------------------------
// Fixtures
// -------------------------------------------------------------------

pub(super) fn sample_overlay_descriptor_state() -> OverlayDescriptorState {
    OverlayDescriptorState {
        rule: Some(sample_rule_overlay_state()),
        paragon: None,
        paragon_mode_active: false,
    }
}

pub(super) fn sample_rule_overlay_state() -> RuleOverlayState {
    use d4lc_rules::RuleAffixOverlayLocation;

    RuleOverlayState {
        result: RuleEvaluationResult {
            final_decision: RuleDecision::Wanted,
            final_source: None,
            matched_affixes: vec![RuleAffixMatch {
                affix_id: "Affix_AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                color: "#00FF00".to_string(),
                value_text: Some("+10.0%".to_string()),
                source_text: "+10.0% Attack Speed".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
                required_by: vec!["Speed".to_string()],
                is_greater_required: false,
                is_any_type_required: false,
                value_status: RuleCheck::not_evaluated(
                    RuleCheckKind::MinimalAffixValue,
                    RuleSourceNote::new("test", "test", "test"),
                ),
                tuning_prisms: Vec::new(),
                source: RuleSourceNote::new("test", "test", "test"),
                column_offset: 0,
            }],
            missing_affixes: Vec::new(),
            item_power_status: RuleCheck::not_evaluated(
                RuleCheckKind::ItemPowerLimit,
                RuleSourceNote::new("test", "test", "test"),
            ),
            minimal_value_status: RuleCheck::not_evaluated(
                RuleCheckKind::MinimalAffixValue,
                RuleSourceNote::new("test", "test", "test"),
            ),
            matched_power: None,
            matched_powers: Vec::new(),
            matched_runes: Vec::new(),
            matched_sigil: None,
            matched_sigils: Vec::new(),
            trade_marker: None,
            overlay_descriptors: Vec::new(),
            affix_traces: Vec::new(),
            explanation: RuleExplanation {
                active_preset_names: vec!["Speed".to_string()],
                matched_build_names: vec!["Speed".to_string()],
                selected_build_name: Some("Speed".to_string()),
                item_type: Some("Amulet".to_string()),
                rarity: Some("Legendary".to_string()),
                branches: Vec::new(),
            },
        },
        settings: RuleOverlaySettings {
            marker_mode: RuleOverlayMarkerMode::HideUnwanted,
            is_minimal_affix_value_filter_enabled: false,
            is_trade_overlay_enabled: false,
            item_power_limit_check_ok: true,
            is_tuning_prism_hints_enabled: false,
        },
        layout: RuleOverlayLayout {
            tooltip_rect: d4lc_core::Rect {
                x: 100,
                y: 100,
                width: 400,
                height: 300,
            },
            affix_locations: vec![RuleAffixOverlayLocation {
                row_id: Some(d4lc_core::AffixRowId(0)),
                geometry: d4lc_core::Rect {
                    x: 120,
                    y: 140,
                    width: 360,
                    height: 20,
                },
                text: None,
            }],
            power_location: None,
            power_locations: vec![],
            rune_locations: vec![],
            sigil_location: None,
            sigil_locations: vec![],
        },
    }
}

pub(super) fn sample_paragon_state() -> ParagonOverlayStateBundle {
    use d4lc_rules::{ImportedParagonBoard, ImportedParagonRoute, ParagonOverlayMode};

    let window_geometry = d4lc_core::Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };

    ParagonOverlayStateBundle {
        state: d4lc_rules::ParagonOverlayState {
            build_name: "Test Build".to_string(),
            routes: vec![ImportedParagonRoute {
                step_name: Some("Step 1".to_string()),
                boards: vec![ImportedParagonBoard {
                    index: 0,
                    name: "Starting Board".to_string(),
                    glyph: "".to_string(),
                    rotation: "0".to_string(),
                    active_nodes: vec![0, 1, 2],
                }],
            }],
            current_route_index: 0,
            current_board_index: Some(0),
            current_board_name: Some("Starting Board".to_string()),
            mode: ParagonOverlayMode::Full,
        },
        layout: d4lc_rules::ParagonOverlayLayout {
            window_geometry,
            panel_left: 100,
            panel_top: 100,
            panel_height: 400,
            panel_gap: 10,
            panel_text_offset: 12,
            font_px: 18,
            tile_gap: 2,
            tile_count: 25,
            node_size_full: 40,
            node_size_collapsed: 20,
            node_border_size: 2,
            collapsed_left_offset: 0,
            collapsed_top_offset: 0,
        },
    }
}

// -------------------------------------------------------------------
// DebugCommandController tests
// -------------------------------------------------------------------

pub(super) fn fresh_controller() -> DebugCommandController {
    let app_state = Arc::new(Mutex::new(AppState::default_startup()));
    let (event_tx, _event_rx) = RuntimeEventSink::new();
    DebugCommandController::new(app_state, event_tx, PathBuf::from("/tmp/d4lc-test-root"))
}

// -------------------------------------------------------------------
// Overlay backend start seam tests
// -------------------------------------------------------------------

// -------------------------------------------------------------------
// Production overlay start tests
// -------------------------------------------------------------------

/// Fresh empty config root: `load_app_settings` yields the defaults
/// (`auto` display backend) for a missing settings file.
#[cfg(feature = "real-runtime")]
pub(super) fn empty_config_root(label: &str) -> crate::test_root::TestRoot {
    crate::test_root::TestRoot::new(label)
}

pub(super) fn analysis_presentation(
    generation: u64,
    item: bool,
    invalidation: Option<d4lc_runtime::InvalidationCause>,
) -> Presentation {
    let item = item.then(|| d4lc_runtime::CurrentItemSnapshot {
        item_name: Some("Helm".to_string()),
        parse_status: "Parsed".to_string(),
        item_type: Some("helm".to_string()),
        base_type: None,
        item_power: Some(800),
        affixes: Vec::new(),
        affix_row_geometry: Vec::new(),
        non_affix_rows: Vec::new(),
    });
    Presentation {
        analysis: Arc::new(d4lc_runtime::AnalysisSnapshot {
            presentation_generation: generation,
            item,
            invalidation,
            ..d4lc_runtime::AnalysisSnapshot::empty(1)
        }),
        rules_revision: Some(1),
        descriptors: Arc::from([]),
        affix_traces: Vec::new(),
    }
}
