use super::*;
use crate::app_state::test_support::{
    apply_two_row_item_event, sample_paragon_routes, seed_attack_speed_preset,
};
use d4lc_core::Rect;
use d4lc_rules::RuleAffixMatch;

// -----------------------------------------------------------------------
// OverlayDescriptorState tests
// -----------------------------------------------------------------------

#[test]
fn overlay_descriptor_state_has_any_state_when_rule_present() {
    let state = OverlayDescriptorState {
        rule: Some(sample_rule_overlay_state()),
        paragon: None,
        paragon_mode_active: false,
    };
    assert!(state.has_any_state());
}

#[test]
fn overlay_descriptor_state_has_any_state_when_paragon_present() {
    let state = OverlayDescriptorState {
        rule: None,
        paragon: Some(sample_paragon_state()),
        paragon_mode_active: true,
    };
    assert!(state.has_any_state());
}

#[test]
fn overlay_descriptor_state_is_empty_when_both_none() {
    let state = OverlayDescriptorState {
        rule: None,
        paragon: None,
        paragon_mode_active: false,
    };
    assert!(!state.has_any_state());
}

#[test]
fn overlay_descriptor_state_new_constructor() {
    let rule_result = sample_rule_eval_result();
    let rule_settings = sample_rule_overlay_settings();
    let rule_layout = sample_rule_overlay_layout();
    let paragon_state = sample_paragon_state();

    let desc = OverlayDescriptorState::new(
        Some((rule_result, rule_settings, rule_layout)),
        Some((paragon_state.state.clone(), paragon_state.layout.clone())),
    );

    assert!(desc.rule.is_some());
    assert!(desc.paragon.is_some());
}

#[test]
fn rebuild_overlay_descriptor_state_projects_selected_preset_and_trade_marker() {
    let mut state = AppState::default_startup();
    state.shell.settings.overlay_enabled = true;
    state.affix_preset_editor.create_preset("Speed").unwrap();
    state
        .affix_preset_editor
        .edit_entry(
            "Speed",
            d4lc_gui::PresetEntry {
                kind: d4lc_gui::PresetEntryKind::Affix,
                id: "Affix_AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                item_type: Some("Amulet".to_string()),
                color: "Green".to_string(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
        )
        .unwrap();
    state
        .trade_imports_mut()
        .upsert_trade_item(d4lc_gui::TradeItemRow {
            trade_item_id: "Trade_Amulet".to_string(),
            item_type: "Amulet".to_string(),
            value_text: "10M".to_string(),
            affixes: vec![],
        });

    state.rebuild_overlay_descriptor_state();

    let projected = state
        .overlay_descriptor_state
        .as_ref()
        .expect("overlay state should be projected");
    let rule = projected.rule.as_ref().expect("rule state should exist");
    // Per-row evaluation semantics: the current item is empty, so no
    // item row is evaluated and no affix marker is projected — the
    // preset selection keeps surfacing only the trade marker.
    assert!(rule.result.matched_affixes.is_empty());
    assert!(rule.layout.affix_locations.is_empty());
    assert_eq!(
        rule.result
            .trade_marker
            .as_ref()
            .map(|marker| marker.trade_item_id.as_str()),
        Some("Trade_Amulet")
    );
    assert_eq!(
        rule.result.explanation.active_preset_names,
        vec!["Speed".to_string()]
    );
}

/// Real-runtime only: the geometry refresh's X11 branch is behind the
/// feature (without it there is no window discovery at all).
#[cfg(feature = "real-runtime")]
#[test]
fn paragon_geometry_refresh_follows_the_published_wayland_stream_rect() {
    // The resolved-engine contract: on the Wayland portal engine the
    // refresh takes the stream rect the live loop published with the
    // engine resolution and never runs the X11 window query — on this
    // display-less machine that query fails, so a resolved geometry
    // proves the portal branch was taken.
    let mut state = AppState::default_startup();
    state.shell.settings.paragon_mode_active = true;
    let stream_rect = Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    };
    state.set_capture_engine(
        "auto → Wayland portal: no X11 game window, wayland session".to_string(),
        Some(stream_rect),
    );

    state.rebuild_overlay_descriptor_state();

    assert_eq!(state.overlay_window_geometry, Some(stream_rect));
}

#[test]
fn rebuild_overlay_descriptor_state_projects_downloaded_paragon_boards() {
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    state.shell.settings.paragon_mode_active = true;
    // The real game-window geometry the overlay surface covers: the
    // paragon board centers on this window (upstream draws on
    // `_window`, OverlayHandler.cs:673-674) — never a fake 1920x1080.
    state.overlay_window_geometry = Some(Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    });
    state.trade_imports_mut().import_state.latest_result = Some(sample_import_result_view(2));
    state.downloaded_paragon_routes = Some(sample_paragon_routes());

    state.rebuild_overlay_descriptor_state();

    let projected = state
        .overlay_descriptor_state
        .as_ref()
        .expect("overlay state should be projected");
    let paragon = projected
        .paragon
        .as_ref()
        .expect("paragon state should exist when mode is active");
    // The layout carries the real window rect and the upstream
    // 21x21 node grid (OverlayHandler.cs:670/:806).
    assert_eq!(
        paragon.layout.window_geometry,
        Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        }
    );
    assert_eq!(paragon.layout.tile_count, 21);
    assert_eq!(paragon.state.build_name, "Speed");
    // The real journey steps, not a fabricated single route.
    assert_eq!(paragon.state.routes.len(), 2);
    let first = &paragon.state.routes[0].boards[0];
    assert_eq!(first.name, "Start");
    assert_eq!(first.rotation, "0°");
    assert_eq!(first.active_nodes, vec![105]);
    let second = &paragon.state.routes[1].boards[0];
    assert_eq!(second.name, "Hemorrhage");
    assert_eq!(second.glyph, "Enchanter");
    assert_eq!(second.rotation, "90°");
    assert_eq!(second.active_nodes, vec![36, 57]);
    assert_eq!(paragon.state.current_board_name.as_deref(), Some("Start"));
    assert!(paragon
        .state
        .routes
        .iter()
        .flat_map(|route| route.boards.iter())
        .all(|board| board.name != "Board 1"));
}

#[test]
fn paragon_layout_uses_configured_node_sizes_and_upstream_panel_geometry() {
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    state.shell.settings.paragon_mode_active = true;
    state.shell.settings.overlay_font_size = 28;
    state.shell.settings.type_area_offset_left = 75;
    state.shell.settings.paragon_node_size = 55;
    state.shell.settings.paragon_node_size_collapsed = 30;
    state.overlay_window_geometry = Some(Rect {
        x: 1920,
        y: 0,
        width: 2560,
        height: 1440,
    });
    state.downloaded_paragon_routes = Some(sample_paragon_routes());

    let projected = evaluate_overlay_descriptor_state(&state).unwrap();
    let paragon = projected.paragon.unwrap();
    assert_eq!(paragon.layout.node_size_full, 55);
    assert_eq!(paragon.layout.node_size_collapsed, 30);
    assert_eq!(paragon.layout.tile_gap, 5);
    assert_eq!(paragon.layout.panel_left, 0);
    assert_eq!(paragon.layout.panel_height, 50);
    assert_eq!(paragon.layout.panel_gap, 2);
    for (mode, size) in [
        (ParagonOverlayMode::Full, 55),
        (ParagonOverlayMode::Collapsed, 30),
    ] {
        let mut paragon = paragon.clone();
        paragon.state.mode = mode;
        paragon.layout.node_border_size = 4;
        let descriptors = d4lc_rules::build_paragon_overlay_descriptors(
            &paragon.state,
            &paragon.layout,
            &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
        );
        let node = |index| {
            descriptors
                .iter()
                .find(|d| {
                    matches!(d.target,
            d4lc_rules::RuleOverlayTarget::Paragon {
                kind: d4lc_rules::ParagonOverlayElementKind::Node,
                node_index: Some(node), ..
            } if node == index)
                })
                .unwrap()
        };
        assert_eq!(node(0).geometry.width, size);
        assert_eq!(node(1).geometry.x - node(0).geometry.x, (size + 5) as i32);
        assert_eq!(node(21).geometry.y - node(0).geometry.y, (size + 5) as i32);
        assert_eq!(
            node(105).color,
            d4lc_rules::OverlayColor::Paragon(d4lc_rules::ParagonColor::BorderActive)
        );
        assert_eq!(
            node(105).shape,
            d4lc_rules::RuleOverlayShape::StrokedRectangle { stroke: 4 }
        );
        let output = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../debug-output/paragon-overlay");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            output.join(format!("{mode:?}-descriptors.json")),
            serde_json::to_vec_pretty(&descriptors).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn rebuild_overlay_descriptor_state_clears_when_no_inputs_exist() {
    let mut state = AppState::default_startup();
    state.rebuild_overlay_descriptor_state();
    assert!(state.overlay_descriptor_state.is_none());
}

// -----------------------------------------------------------------------
// Sample fixtures (minimal but non-trivial)
// -----------------------------------------------------------------------

fn sample_rule_eval_result() -> RuleEvaluationResult {
    RuleEvaluationResult {
        final_decision: RuleDecision::Wanted,
        final_source: None,
        matched_affixes: vec![RuleAffixMatch {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            color: "Green".to_string(),
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
    }
}

fn sample_rule_overlay_settings() -> RuleOverlaySettings {
    RuleOverlaySettings {
        marker_mode: RuleOverlayMarkerMode::HideUnwanted,
        is_minimal_affix_value_filter_enabled: false,
        is_trade_overlay_enabled: false,
        item_power_limit_check_ok: true,
        is_tuning_prism_hints_enabled: false,
    }
}

fn sample_rule_overlay_layout() -> RuleOverlayLayout {
    use d4lc_core::Rect;

    RuleOverlayLayout {
        tooltip_rect: Rect {
            x: 100,
            y: 100,
            width: 400,
            height: 300,
        },
        affix_locations: vec![RuleAffixOverlayLocation {
            row_id: Some(d4lc_core::AffixRowId(0)),
            geometry: Rect {
                x: 120,
                y: 140,
                width: 360,
                height: 20,
            },
            text: None,
        }],
        power_location: None,
        power_locations: Vec::new(),
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: Vec::new(),
    }
}

fn sample_rule_overlay_state() -> RuleOverlayState {
    RuleOverlayState {
        result: sample_rule_eval_result(),
        settings: sample_rule_overlay_settings(),
        layout: sample_rule_overlay_layout(),
    }
}

fn sample_paragon_state() -> ParagonOverlayStateBundle {
    use d4lc_core::Rect;
    use d4lc_rules::{ImportedParagonBoard, ImportedParagonRoute, ParagonOverlayMode};

    ParagonOverlayStateBundle {
        state: ParagonOverlayState {
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
        layout: ParagonOverlayLayout {
            window_geometry: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
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

fn sample_import_result_view(paragon_board_count: usize) -> d4lc_gui::ImportWorkflowResultView {
    d4lc_gui::ImportWorkflowResultView {
        provider: "Mobalytics".to_string(),
        source_url_or_file: "mobalytics/source".to_string(),
        source_build_id: "build-1".to_string(),
        source_build_name: "Imported Build".to_string(),
        class_name: "Sorcerer".to_string(),
        variant_name: None,
        affix_preset_name: "Speed".to_string(),
        items: vec![],
        paragon_board_count,
        warnings: vec![],
        unmapped_entries: vec![],
        source_trace: vec!["import".to_string()],
    }
}

// -----------------------------------------------------------------------
// Clone + Debug coverage for public types
// -----------------------------------------------------------------------

#[test]
fn overlay_descriptor_state_is_clone_and_debug() {
    let state = OverlayDescriptorState {
        rule: Some(sample_rule_overlay_state()),
        paragon: Some(sample_paragon_state()),
        paragon_mode_active: true,
    };
    let cloned = state.clone();
    assert!(!format!("{:?}", cloned).is_empty());
}

// -----------------------------------------------------------------------
// Overlay live-state defect tests (upstream parity closure)
// -----------------------------------------------------------------------

#[test]
fn overlay_disabled_projects_no_rule_marker_state() {
    // Upstream OH:133: with the overlay off, the draw pass clears the
    // tooltip every frame — no affix/aspect/rune/sigil/trade marker can
    // anchor. The persisted `overlay_enabled` power switch gates the
    // whole rule-marker projection at the one seam.
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    apply_two_row_item_event(&mut state);

    state.shell.settings.overlay_enabled = false;
    state.rebuild_overlay_descriptor_state();
    assert!(
        state
            .overlay_descriptor_state
            .as_ref()
            .and_then(|projected| projected.rule.as_ref())
            .is_none(),
        "overlay off: no rule marker state may project"
    );

    // The same state with the overlay on projects the full marker set.
    state.shell.settings.overlay_enabled = true;
    state.rebuild_overlay_descriptor_state();
    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.rule.as_ref())
        .expect("overlay on: rule marker state projects");
    assert_eq!(rule.result.matched_affixes.len(), 2);
}

#[test]
fn confirming_the_collapsed_setting_switches_the_paragon_mode_without_a_restart() {
    // The collapsed switch has one owner (`shell.settings`): the dialog
    // confirm applies it to the next projection, not only after a restart.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let config_root = std::env::temp_dir().join(format!(
        "d4lc-test-collapsed-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&config_root).unwrap();

    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    state.shell.settings.paragon_mode_active = true;
    state.overlay_window_geometry = Some(Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    });
    state.downloaded_paragon_routes = Some(sample_paragon_routes());
    state.rebuild_overlay_descriptor_state();
    let mode = |state: &AppState| {
        state
            .overlay_descriptor_state
            .as_ref()
            .and_then(|projected| projected.paragon.as_ref())
            .map(|paragon| paragon.state.mode)
            .expect("paragon projects with real geometry")
    };
    assert_eq!(mode(&state), ParagonOverlayMode::Full);

    let draft = crate::actions::settings::ParagonConfigDraft {
        collapsed_paragonboard_enabled: true,
        ..crate::actions::settings::ParagonConfigDraft::from_settings(&state.shell.settings)
    };
    crate::actions::settings::confirm_paragon_config(&draft, &mut state, &config_root).unwrap();
    assert_eq!(mode(&state), ParagonOverlayMode::Collapsed);
    let _ = std::fs::remove_dir_all(&config_root);
}

#[test]
fn paragon_collapsed_setting_selects_the_collapsed_board_mode() {
    // Upstream OH:154-163: `IsCollapsedParagonboardEnabled` selects
    // `DrawGraphicsParagonCollapsed` (offset board, collapsed node
    // size) over `DrawGraphicsParagon` (centered board).
    let seed = |state: &mut AppState| {
        seed_attack_speed_preset(state);
        state.shell.settings.paragon_mode_active = true;
        state.overlay_window_geometry = Some(Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        });
        state.downloaded_paragon_routes = Some(sample_paragon_routes());
    };

    let mut state = AppState::default_startup();
    seed(&mut state);
    state.rebuild_overlay_descriptor_state();
    let full = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.paragon.as_ref())
        .expect("full mode projects with real geometry");
    assert_eq!(full.state.mode, ParagonOverlayMode::Full);

    // The persisted collapsed setting (hydrated from the config
    // settings) selects the collapsed mode.
    let mut collapsed_state = AppState::default_startup();
    seed(&mut collapsed_state);
    collapsed_state
        .shell
        .settings
        .collapsed_paragonboard_enabled = true;
    collapsed_state.rebuild_overlay_descriptor_state();
    let collapsed = collapsed_state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.paragon.as_ref())
        .expect("collapsed mode projects with real geometry");
    assert_eq!(collapsed.state.mode, ParagonOverlayMode::Collapsed);
    assert_eq!(
        collapsed.layout.collapsed_left_offset, -200,
        "upstream SettingsD4 ParagonLeftOffsetCollapsed default"
    );

    // The offsets come from the persisted settings, not from a
    // hardcoded constant: a non-default pair projects verbatim, the
    // upstream `SettingsD4.cs:72/:75` draw-time read.
    collapsed_state.shell.settings.paragon_left_offset_collapsed = -120;
    collapsed_state.shell.settings.paragon_top_offset_collapsed = 75;
    collapsed_state.rebuild_overlay_descriptor_state();
    let tuned = collapsed_state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.paragon.as_ref())
        .expect("collapsed mode projects with real geometry");
    assert_eq!(
        tuned.layout.collapsed_left_offset, -120,
        "the configured collapsed X offset must reach the layout"
    );
    assert_eq!(
        tuned.layout.collapsed_top_offset, 75,
        "the configured collapsed Y offset must reach the layout"
    );

    // The board nodes center on the real window rect in full mode.
    let descriptors = d4lc_rules::build_paragon_overlay_descriptors(
        &full.state,
        &full.layout,
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
    );
    let node: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Paragon {
                    kind: d4lc_rules::ParagonOverlayElementKind::Node,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(node.len(), 441, "21x21 node grid like upstream tileCount");
    // The frozen builder centers the board on the layout window:
    // `(width - node_size * tile_count) / 2` (OverlayHandler.cs:673).
    let node_size = full.layout.node_size_full;
    let expected_left = (full.layout.window_geometry.width as i32
        - (node_size * full.layout.tile_count) as i32)
        / 2;
    assert_eq!(
        node[0].geometry.x, expected_left,
        "the board centers horizontally on the real window"
    );
}

#[test]
fn paragon_without_real_window_geometry_projects_nothing() {
    // Upstream `DrawGraphicsParagon` returns without a window
    // (OverlayHandler.cs:570): no real geometry, no paragon
    // descriptors — never a board drawn at invented coordinates.
    let mut state = AppState::default_startup();
    state.shell.settings.paragon_mode_active = true;
    state.downloaded_paragon_routes = Some(sample_paragon_routes());
    assert!(state.overlay_window_geometry.is_none());

    state.rebuild_overlay_descriptor_state();

    assert!(
        state
            .overlay_descriptor_state
            .as_ref()
            .and_then(|projected| projected.paragon.as_ref())
            .is_none(),
        "no window geometry: no paragon descriptors"
    );
}

#[test]
fn trade_panel_is_suppressed_while_no_tooltip_is_live() {
    // Upstream draws the trade panel only while a parsed tooltip is
    // live (OH:524-566). The steady-state projection between captures
    // keeps the configured trade marker in the rule state, but the
    // empty tooltip rect suppresses the phantom panel at the window
    // origin.
    let mut state = AppState::default_startup();
    state.shell.settings.overlay_enabled = true;
    state.shell.settings.trade_overlay_enabled = true;
    state
        .trade_imports_mut()
        .upsert_trade_item(d4lc_gui::TradeItemRow {
            trade_item_id: "Trade_Helm".to_string(),
            item_type: "helm".to_string(),
            value_text: "10M".to_string(),
            affixes: vec![],
        });

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.rule.as_ref())
        .expect("the configured trade marker keeps projecting rule state");
    assert!(rule.result.trade_marker.is_some());
    assert_eq!(rule.layout.tooltip_rect, Rect::default());
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    assert!(
        descriptors
            .iter()
            .all(|descriptor| !matches!(descriptor.target, d4lc_rules::RuleOverlayTarget::Trade)),
        "no live tooltip: the trade panel draws nowhere"
    );
}

#[test]
fn item_power_gate_exempts_sigils_and_fails_unparsed_power() {
    // Upstream OH:149-151: the gate passes when the limit is disabled,
    // for sigil tooltips regardless of item power, and fails when the
    // tooltip carries no parsed item power (upstream compares against
    // an item power of 0).
    let mut state = AppState::default_startup();
    state.shell.settings.item_power_limit_enabled = true;
    state.shell.settings.item_power_limit = 900;

    state.current_item_mut().item_power = None;
    state.current_item_mut().item_type = Some("helm".to_string());
    assert!(
        !item_power_limit_check_ok(&state),
        "unparsed item power must fail the gate, not pass it"
    );

    state.current_item_mut().item_power = Some(925);
    assert!(item_power_limit_check_ok(&state), "925 meets the 900 limit");
    state.current_item_mut().item_power = Some(800);
    assert!(!item_power_limit_check_ok(&state));

    // The sigil exemption applies whatever the parsed power is.
    state.current_item_mut().item_type = Some("Sigil".to_string());
    assert!(item_power_limit_check_ok(&state), "sigils bypass the gate");

    // And the gate suppresses the rule markers in the projection.
    let mut gated = AppState::default_startup();
    seed_attack_speed_preset(&mut gated);
    apply_two_row_item_event(&mut gated);
    gated.shell.settings.item_power_limit_enabled = true;
    gated.shell.settings.item_power_limit = 926;
    gated.current_item_mut().item_power = Some(925);
    gated.rebuild_overlay_descriptor_state();
    let rule = gated
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.rule.as_ref())
        .expect("matched rows still project");
    assert!(!rule.settings.item_power_limit_check_ok);
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    assert!(
        descriptors.iter().all(|descriptor| !matches!(
            descriptor.target,
            d4lc_rules::RuleOverlayTarget::Affix { .. }
        )),
        "a failed item power gate draws no rule markers"
    );
}

fn paragon_window() -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    }
}

#[test]
fn paragon_mode_replaces_the_affix_markers() {
    // Upstream draws the paragon overlay instead of the affix, aspect and
    // trade markers (`if (IsParagonModeActive) ... else ...`,
    // OverlayHandler.cs:154-163).
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    apply_two_row_item_event(&mut state);
    state.shell.settings.overlay_enabled = true;
    state.overlay_window_geometry = Some(paragon_window());
    state.downloaded_paragon_routes = Some(sample_paragon_routes());
    let is_affix = |descriptor: &d4lc_rules::RuleOverlayDescriptor| {
        matches!(
            descriptor.target,
            d4lc_rules::RuleOverlayTarget::Affix { .. }
        )
    };

    state.rebuild_overlay_descriptor_state();
    let affix_mode =
        crate::runtime_bridge::convert_descriptors(state.overlay_descriptor_state.as_ref());
    assert!(affix_mode.iter().any(is_affix), "affix mode draws markers");

    state.shell.settings.paragon_mode_active = true;
    state.rebuild_overlay_descriptor_state();
    let projected = state
        .overlay_descriptor_state
        .as_ref()
        .expect("paragon mode projects");
    assert!(
        projected.rule.is_some(),
        "the rule evaluation itself keeps running for the traces"
    );
    let paragon_mode = crate::runtime_bridge::convert_descriptors(Some(projected));
    assert!(!paragon_mode.is_empty());
    assert!(
        paragon_mode
            .iter()
            .all(|descriptor| descriptor.marker_type == d4lc_rules::RuleOverlayMarkerType::Paragon),
        "paragon mode draws only paragon descriptors"
    );
}

#[test]
fn paragon_mode_without_a_selected_preset_draws_nothing() {
    // `preset == null` returns without a warning (OverlayHandler.cs:573).
    let mut state = AppState::default_startup();
    state.shell.settings.paragon_mode_active = true;
    state.overlay_window_geometry = Some(paragon_window());
    state.affix_preset_editor.selected_preset_name = Some("Gone".to_string());

    state.rebuild_overlay_descriptor_state();

    assert!(state
        .overlay_descriptor_state
        .as_ref()
        .is_none_or(|projected| projected.paragon.is_none()));
}

#[test]
fn paragon_mode_with_a_preset_without_boards_draws_the_warning() {
    // `ParagonBoardsList.Count == 0` draws the warning panel
    // (OverlayHandler.cs:574-578).
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    state.shell.settings.paragon_mode_active = true;
    state.overlay_window_geometry = Some(paragon_window());

    state.rebuild_overlay_descriptor_state();

    let projected = state
        .overlay_descriptor_state
        .as_ref()
        .expect("the preset exists");
    let paragon = projected.paragon.as_ref().expect("warning state projects");
    assert!(paragon.state.routes.is_empty());
    assert_eq!(paragon.state.build_name, "Speed", "preset.Name (OH:611)");
    let descriptors = crate::runtime_bridge::convert_descriptors(Some(projected));
    assert!(descriptors.iter().any(|descriptor| matches!(
        descriptor.target,
        d4lc_rules::RuleOverlayTarget::Paragon {
            kind: d4lc_rules::ParagonOverlayElementKind::WarningPanel,
            ..
        }
    )));
}

#[test]
fn downloaded_boards_need_a_selected_preset_and_carry_its_name() {
    // The download path replaces the preset's boards but not the
    // `preset == null` return (OverlayHandler.cs:573) or the build panel's
    // `preset.Name` (:608).
    let mut state = AppState::default_startup();
    state.shell.settings.paragon_mode_active = true;
    state.overlay_window_geometry = Some(paragon_window());
    state.downloaded_paragon_routes = Some(sample_paragon_routes());
    state.trade_imports_mut().import_state.latest_result = Some(sample_import_result_view(2));

    state.rebuild_overlay_descriptor_state();
    assert!(state
        .overlay_descriptor_state
        .as_ref()
        .is_none_or(|projected| projected.paragon.is_none()));

    seed_attack_speed_preset(&mut state);
    state.rebuild_overlay_descriptor_state();
    let paragon = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.paragon.as_ref())
        .expect("a selected preset draws the downloaded boards");
    assert_eq!(paragon.state.build_name, "Speed");
    assert_eq!(paragon.state.routes.len(), 2);
}

#[test]
fn the_overlay_font_size_is_bounded_to_the_setting_range() {
    // A hand-edited Settings.json can hold any number; the paragon layout
    // never measures or draws outside the 10..=50 range of the setting.
    let mut state = AppState::default_startup();
    let font_px = |state: &AppState| project_paragon_overlay_layout(state).font_px;
    for (stored, expected) in [
        (0, 10),
        (9, 10),
        (10, 10),
        (18, 18),
        (50, 50),
        (51, 50),
        (4000, 50),
    ] {
        state.shell.settings.overlay_font_size = stored;
        assert_eq!(font_px(&state), expected, "stored {stored}");
    }
}
