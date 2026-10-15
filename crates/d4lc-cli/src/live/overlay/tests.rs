use super::*;
use crate::live::frame::LiveTimings;
use d4lc_capture::{
    CaptureBackend, CaptureMethod, CaptureResult, CaptureScopeResult, WindowCandidate,
};
use d4lc_core::{CaptureFrame, InputMode, Rect};
use d4lc_detect::prepared::DetectFrameResult;
use d4lc_detect::{RoiPlan, RoiRole};
use std::path::{Path, PathBuf};

#[test]
fn overlay_capture_policy_marks_real_x11_window_capture_as_excluded() {
    let status = OverlayStatus {
        backend: OverlayBackend::RealX11,
        enabled: true,
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        interaction_zone: Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        },
        menu_locked: true,
        menu_visible: true,
        transparent: false,
        transparent_requested: true,
        transparent_confirmed: false,
        always_on_top: false,
        always_on_top_requested: true,
        always_on_top_confirmed: false,
        click_through: false,
        keyboard_interactive_requested: false,
        keyboard_interactive_confirmed: false,
        draw_error_count: 0,
        last_error: None,
        limitation: None,
    };
    let source = CaptureSource::Window(WindowCandidate {
        info: d4lc_core::WindowInfo {
            id: 42,
            title: "Diablo IV".to_string(),
            class_name: Some("steam_app".to_string()),
            rect: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        },
        pid: Some(1234),
        is_active: true,
        matches_query: true,
    });

    let state = overlay_window_state_for_capture(Some(&status), &source);
    assert_eq!(state, OverlayWindowState::Excluded);
}

#[test]
fn overlay_accepts_diagnostic_frame_without_real_x11() {
    // Verify OverlayFrame can be constructed and passed to update() without
    // requiring a real X11 display (headless backend).
    use d4lc_core::Rect;
    use d4lc_overlay_x11::OverlayConfig;

    let saved = std::env::var("DISPLAY").ok();
    std::env::remove_var("DISPLAY");

    let mut handle = d4lc_overlay_x11::create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    })
    .unwrap_or_else(|e| panic!("overlay creation must not fail: {e}"));

    let live_overlay_state = sample_live_overlay_descriptor_state();
    let result = handle.update(build_live_overlay_frame(
        &sample_processed_frame(),
        true,
        Some(&live_overlay_state),
        22,
    ));

    if let Some(val) = saved {
        std::env::set_var("DISPLAY", val);
    }

    // Headless backend always succeeds on update.
    assert!(result.is_ok(), "overlay update failed: {result:?}");
}

#[test]
fn overlay_start_routing_follows_the_display_backend_setting() {
    // auto (the persisted default) and explicit x11 keep today's
    // real X11 overlay create.
    for setting in ["auto", "x11"] {
        let start = resolve_overlay_backend_start(setting)
            .unwrap_or_else(|error| panic!("'{setting}' must resolve to the X11 start: {error}"));
        assert!(
            matches!(start, OverlayBackendStart::X11),
            "setting '{setting}' must keep the X11 overlay create: {start:?}"
        );
    }

    // A wayland selection must never silently
    // create the X11 overlay. In this binary (which does not link the
    // layer-shell backend) it resolves to the structured
    // unavailability that the live session surfaces instead.
    let unavailable = match resolve_overlay_backend_start("wayland") {
        Ok(OverlayBackendStart::WaylandUnavailable { reason }) => reason,
        other => {
            panic!("wayland selection must resolve to the structured unavailability: {other:?}")
        }
    };
    assert!(
        unavailable.contains("wayland") && unavailable.contains("not part of this binary"),
        "unavailability must name the missing backend: {unavailable}"
    );
    // Whitespace tolerance mirrors the app display-backend parser.
    assert!(matches!(
        resolve_overlay_backend_start(" wayland "),
        Ok(OverlayBackendStart::WaylandUnavailable { .. })
    ));

    // An unparsable stored token is a hard structured error naming
    // the accepted tokens — never a silent X11 fallback.
    let error = match resolve_overlay_backend_start("pipewire") {
        Ok(_) => panic!("invalid display backend must be rejected"),
        Err(error) => error,
    };
    assert!(
        error.contains("unsupported display backend") && error.contains("auto|x11|wayland"),
        "structured parse error must travel: {error}"
    );
}

#[test]
fn live_overlay_frame_uses_rule_descriptors_when_state_present() {
    let frame = build_live_overlay_frame(
        &sample_processed_frame(),
        true,
        Some(&LiveOverlayDescriptorState {
            rule: Some(sample_rule_overlay_state()),
            paragon: None,
        }),
        22,
    );

    assert!(!frame.rule_descriptors.is_empty());
    assert!(frame.rule_descriptors.iter().any(|descriptor| {
        matches!(
            descriptor.target,
            d4lc_rules::RuleOverlayTarget::Affix { .. }
        )
    }));

    write_live_overlay_debug_artifact("live-loop-rule-descriptors.json", &frame.rule_descriptors);
}

#[test]
fn live_overlay_frame_uses_paragon_descriptors_when_state_present() {
    let frame = build_live_overlay_frame(
        &sample_processed_frame(),
        true,
        Some(&LiveOverlayDescriptorState {
            rule: None,
            paragon: Some(sample_paragon_overlay_state()),
        }),
        22,
    );

    assert!(!frame.rule_descriptors.is_empty());
    assert!(frame.rule_descriptors.iter().any(|descriptor| {
        matches!(
            descriptor.target,
            d4lc_rules::RuleOverlayTarget::Paragon {
                kind: d4lc_rules::ParagonOverlayElementKind::BuildPanel,
                ..
            }
        )
    }));

    write_live_overlay_debug_artifact(
        "live-loop-paragon-descriptors.json",
        &frame.rule_descriptors,
    );
}

#[test]
fn live_overlay_frame_keeps_empty_descriptors_without_state() {
    let frame = build_live_overlay_frame(&sample_processed_frame(), false, None, 22);
    assert!(frame.rule_descriptors.is_empty());
    assert_eq!(
        frame.text_font_px, 22,
        "the configured size, not the default"
    );
}

fn sample_live_overlay_descriptor_state() -> LiveOverlayDescriptorState {
    LiveOverlayDescriptorState {
        rule: Some(sample_rule_overlay_state()),
        paragon: Some(sample_paragon_overlay_state()),
    }
}

fn sample_rule_overlay_state() -> LiveRuleOverlayState {
    LiveRuleOverlayState {
        result: RuleEvaluationResult {
            final_decision: d4lc_rules::RuleDecision::Wanted,
            final_source: None,
            matched_affixes: vec![d4lc_rules::RuleAffixMatch {
                affix_id: "Affix_AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                color: "Green".to_string(),
                value_text: Some("+10.0%".to_string()),
                source_text: "+10.0% Attack Speed".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
                required_by: vec!["Speed".to_string()],
                is_greater_required: false,
                is_any_type_required: false,
                value_status: d4lc_rules::RuleCheck::not_evaluated(
                    d4lc_rules::RuleCheckKind::MinimalAffixValue,
                    d4lc_rules::RuleSourceNote::new("test", "test", "test"),
                ),
                tuning_prisms: Vec::new(),
                source: d4lc_rules::RuleSourceNote::new("test", "test", "test"),
                column_offset: 0,
            }],
            missing_affixes: Vec::new(),
            item_power_status: d4lc_rules::RuleCheck::not_evaluated(
                d4lc_rules::RuleCheckKind::ItemPowerLimit,
                d4lc_rules::RuleSourceNote::new("test", "test", "test"),
            ),
            minimal_value_status: d4lc_rules::RuleCheck::not_evaluated(
                d4lc_rules::RuleCheckKind::MinimalAffixValue,
                d4lc_rules::RuleSourceNote::new("test", "test", "test"),
            ),
            matched_power: None,
            matched_powers: Vec::new(),
            matched_runes: Vec::new(),
            matched_sigil: None,
            matched_sigils: Vec::new(),
            trade_marker: None,
            overlay_descriptors: Vec::new(),
            affix_traces: Vec::new(),
            explanation: d4lc_rules::RuleExplanation {
                active_preset_names: vec!["Speed".to_string()],
                matched_build_names: vec!["Speed".to_string()],
                selected_build_name: Some("Speed".to_string()),
                item_type: Some("amulet".to_string()),
                rarity: Some("legendary".to_string()),
                branches: Vec::new(),
            },
        },
        settings: d4lc_rules::RuleOverlaySettings {
            marker_mode: d4lc_rules::RuleOverlayMarkerMode::ShowAll,
            is_minimal_affix_value_filter_enabled: false,
            is_trade_overlay_enabled: false,
            item_power_limit_check_ok: true,
            is_tuning_prism_hints_enabled: false,
        },
        layout: RuleOverlayLayout {
            tooltip_rect: Rect {
                x: 100,
                y: 200,
                width: 320,
                height: 500,
            },
            affix_locations: vec![d4lc_rules::RuleAffixOverlayLocation {
                row_id: Some(d4lc_core::AffixRowId(0)),
                geometry: Rect {
                    x: 120,
                    y: 240,
                    width: 20,
                    height: 20,
                },
                text: Some("AS".to_string()),
            }],
            power_location: None,
            power_locations: Vec::new(),
            rune_locations: Vec::new(),
            sigil_location: None,
            sigil_locations: Vec::new(),
        },
    }
}

fn sample_paragon_overlay_state() -> LiveParagonOverlayState {
    LiveParagonOverlayState {
        state: ParagonOverlayState {
            build_name: "Whirlwind".to_string(),
            routes: vec![d4lc_rules::ImportedParagonRoute {
                step_name: Some("Step 1".to_string()),
                boards: vec![d4lc_rules::ImportedParagonBoard {
                    index: 0,
                    name: "Starting Board".to_string(),
                    glyph: "Marshal".to_string(),
                    rotation: "0°".to_string(),
                    active_nodes: vec![0, 1, 2],
                }],
            }],
            current_route_index: 0,
            current_board_index: Some(0),
            current_board_name: Some("Starting Board".to_string()),
            mode: d4lc_rules::ParagonOverlayMode::Full,
        },
        layout: ParagonOverlayLayout {
            window_geometry: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            panel_left: 0,
            panel_top: 100,
            panel_height: 50,
            panel_gap: 2,
            panel_text_offset: 20,
            font_px: 20,
            tile_gap: 5,
            tile_count: 21,
            node_size_full: 20,
            node_size_collapsed: 10,
            node_border_size: 2,
            collapsed_left_offset: 0,
            collapsed_top_offset: 0,
        },
    }
}

fn sample_processed_frame() -> ProcessedFrame {
    ProcessedFrame {
        sequence: 1,
        dropped_frames: 0,
        capture: CaptureResult {
            backend: CaptureBackend::X11Native,
            fallback_reason: None,
            source: CaptureSource::ManualRegion(Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            geometry: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            scale: d4lc_capture::ScaleFactor::one(),
            requested_capture_rect: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            actual_capture_rect: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            roi_origin: d4lc_capture::RoiOrigin::WindowRelative,
            capture_scope: CaptureScopeResult::FullFrame,
            overlay_window: OverlayWindowState::Ignored,
            capture_method: CaptureMethod::X11NativeXGetImage,
            frame: CaptureFrame {
                width: 10,
                height: 10,
                stride: 40,
                pixels_bgra: vec![0; 400],
            },
        },
        cursor: None,
        roi_plan: RoiPlan {
            capture_rect: None,
            detection_rect: None,
            planned_tooltip_rect: None,
            role: RoiRole::FullFrame,
            fallback_reason: None,
        },
        detection: DetectFrameResult {
            preset: "SteamDeck".to_string(),
            mode: InputMode::Controller,
            roi: Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            roi_role: RoiRole::FullFrame,
            planned_tooltip_roi: None,
            tooltip_detection: d4lc_detect::TooltipDetectionTrace {
                search_roi: Rect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                detected_tooltip_rect: None,
                final_tooltip_crop_rect: None,
                crop_source: "none",
                upstream_rule: d4lc_detect::UPSTREAM_TOOLTIP_CROP_RULE,
                final_tooltip_size: None,
            },
            thresholds: d4lc_detect::ThresholdSettings {
                min: 0,
                max: 255,
                similarity_tooltip: 0.05,
            },
            timings: d4lc_detect::DetectionTimings::default(),
            candidates: Vec::new(),
            selected: None,
            crop: None,
            trace_output_path: None,
            crop_output_path: None,
            thresholded_crop_output_path: None,
        },
        crop_stability: None,
        content_roi_capture_requested: false,
        frame_output_path: None,
        roi_output_path: None,
        planned_tooltip_roi_output_path: None,
        timings: LiveTimings {
            capture_method: CaptureMethod::X11NativeXGetImage,
            hotpath_total_ms: 8,
            ..LiveTimings::default()
        },
    }
}

fn write_live_overlay_debug_artifact(
    file_name: &str,
    descriptors: &[d4lc_rules::RuleOverlayDescriptor],
) {
    let json = serde_json::Value::Array(
        descriptors
            .iter()
            .map(|descriptor| {
                serde_json::json!({
                    "target": format!("{:?}", descriptor.target),
                    "marker_type": format!("{:?}", descriptor.marker_type),
                    "shape": format!("{:?}", descriptor.shape),
                    "geometry": {
                        "x": descriptor.geometry.x,
                        "y": descriptor.geometry.y,
                        "width": descriptor.geometry.width,
                        "height": descriptor.geometry.height,
                    },
                    "color": descriptor.color,
                    "text": descriptor.text,
                    "geometry_source": descriptor.geometry_source,
                    "source": {
                        "upstream_file": descriptor.source.upstream_file,
                        "upstream_method": descriptor.source.upstream_method,
                        "branch": descriptor.source.branch,
                    }
                })
            })
            .collect(),
    );
    let path = workspace_root()
        .join("debug-output/overlay")
        .join(file_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", parent.display()));
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json)
            .unwrap_or_else(|error| panic!("failed to serialize {file_name}: {error}")),
    )
    .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|error| panic!("failed to resolve workspace root: {error}"))
}
