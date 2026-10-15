use super::*;

#[test]
fn default_shell_exposes_required_tabs_without_core_logic() {
    let shell = GuiShellState::default_startup();

    assert_eq!(shell.window_title, "d4lc");
    assert_eq!(
        shell.tabs.iter().map(|tab| tab.kind).collect::<Vec<_>>(),
        vec![
            GuiTabKind::CurrentItem,
            GuiTabKind::AffixPresets,
            GuiTabKind::TradeImports,
            GuiTabKind::Debug,
            GuiTabKind::Settings,
        ]
    );
    assert_eq!(shell.active_tab, GuiTabKind::CurrentItem);
    assert_eq!(shell.debug.active_language, "enUS");
    assert!(!shell.owns_capture_logic);
    assert!(!shell.owns_ocr_logic);
    assert!(!shell.owns_parser_logic);
    assert!(!shell.owns_rule_logic);
    assert!(!shell.owns_import_logic);
    assert!(!shell.owns_overlay_logic);
}

#[test]
fn shell_can_be_driven_by_serializable_state_fixture() {
    let shell = GuiShellState::from_fixture(GuiShellFixture {
        window_title: "d4lc test".to_string(),
        current_item_name: Some("Legendary Amulet".to_string()),
        selected_affix_preset: Some("Speed".to_string()),
        matched_trade_value: Some("5b".to_string()),
        debug_artifact_count: 4,
        selected_system_preset: "1080p_SMF".to_string(),
    });

    assert_eq!(shell.window_title, "d4lc test");
    assert_eq!(
        shell.current_item.item_name.as_deref(),
        Some("Legendary Amulet")
    );
    assert_eq!(
        shell.affix_presets.selected_preset.as_deref(),
        Some("Speed")
    );
    assert_eq!(
        shell.trade_imports.matched_trade_value.as_deref(),
        Some("5b")
    );
    assert_eq!(shell.debug.debug_artifact_count, 4);
    assert_eq!(shell.settings.selected_system_preset, "1080p_SMF");
}

#[test]
fn panel_update_check_effective_is_one_gate_over_both_rows() {
    // The live gate the release check consults: the upstream
    // `CheckForUpdates` row and the port-specific release-banner row
    // must both be on, so turning off the upstream row stops the check
    // instead of being shadowed by the port default.
    let defaults =
        SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
    assert!(defaults.update_check_effective());

    let upstream_row_off = SettingsPanelState {
        check_for_updates: false,
        ..defaults.clone()
    };
    assert!(!upstream_row_off.update_check_effective());

    let port_row_off = SettingsPanelState {
        update_check_enabled: false,
        ..defaults
    };
    assert!(!port_row_off.update_check_effective());
}

#[test]
fn settings_panel_projects_persisted_config_and_controller_images() {
    let settings = d4lc_config::AppSettings {
        selected_system_preset: "1080p_SMF".to_string(),
        selected_affix_language: "deDE".to_string(),
        selected_affix_preset: String::new(),
        capture_backend: "x11-native".to_string(),
        controller_mode: true,
        dev_mode: false,
        scan_height_percent: 45.0,
        tooltip_width: 640,
        tooltip_max_height: 260,
        affix_area_height_offset_top: 7,
        affix_area_height_offset_bottom: 9,
        affix_aspect_area_width_offset: 11,
        aspect_area_height_offset_top: 13,
        screen_capture_delay: 120,
        overlay_update_delay: 15,
        threshold_min: 80,
        threshold_max: 240,
        threshold_similarity_tooltip: 0.03,
        threshold_similarity_affix_location: 0.04,
        threshold_similarity_aspect_location: 0.05,
        threshold_similarity_socket_location: 0.06,
        threshold_similarity_splitter_location: 0.07,
        is_socket_detection_enabled: false,
        is_tempered_affix_detection_enabled: false,
        overlay_enabled: true,
        is_top_most: true,
        overlay_font_size: 22,
        overlay_menu_locked: true,
        is_show_browser_mobalytics_enabled: true,
        type_area_offset_right: 9,
        aspect_area_max_height: 280,
        is_tuning_prism_hints_enabled: true,
        is_import_paragon_infinity_builds_enabled: false,
        selected_build_import_website: "Mobalytics.gg".to_string(),
        default_colors: d4lc_config::DefaultColors {
            greater: "#FFAA00".to_string(),
            implicit: "Blue".to_string(),
            ..d4lc_config::DefaultColors::default()
        },
        paragon_border_size: 4,
        paragon_node_size: 55,
        paragon_node_size_collapsed: 30,
        paragon_left_offset_collapsed: -120,
        paragon_top_offset_collapsed: 75,
        show_overlay_icon: true,
        overlay_icon_pos_x: 120,
        overlay_icon_pos_y: 340,
        rule_settings: d4lc_config::RuleSettingsConfig {
            item_power: d4lc_config::ItemPowerRuleSettings {
                enabled: true,
                limit: 925,
            },
            minimal_affix_value_enabled: true,
            multi_build_enabled: true,
            trade_overlay_enabled: false,
            overlay_marker_mode: d4lc_config::OverlayMarkerModeSetting::HideUnwanted,
            aspect_detection_enabled: false,
            unique_detection_enabled: false,
            rune_detection_enabled: false,
            sigil_detection_enabled: false,
            sigil_display_mode: d4lc_config::SigilDisplayModeSetting::Blacklisting,
            dungeon_tiers_enabled: false,
            toggles: d4lc_config::RuleToggleSettings {
                class_toggles: d4lc_config::ClassToggleSettings {
                    barbarian: false,
                    ..d4lc_config::ClassToggleSettings::default()
                },
                rune_effect: false,
                ..d4lc_config::RuleToggleSettings::default()
            },
            ..d4lc_config::RuleSettingsConfig::default()
        },
        ..d4lc_config::AppSettings::default()
    };

    let panel = SettingsPanelState::from_config(
        &settings,
        vec![
            "tooltip_gc_x.png".to_string(),
            "tooltip_kb_all.png".to_string(),
        ],
    );

    assert_eq!(panel.selected_system_preset, "1080p_SMF");
    assert_eq!(panel.language, "deDE");
    assert_eq!(panel.capture_backend, "x11-native");
    assert!(panel.controller_mode);
    assert_eq!(panel.active_controller_images.len(), 2);
    assert_eq!(panel.scan_height_percent, 45.0);
    assert_eq!(panel.tooltip_width, 640);
    assert_eq!(panel.tooltip_max_height, 260);
    assert_eq!(panel.affix_area_height_offset_top, 7);
    assert_eq!(panel.affix_area_height_offset_bottom, 9);
    assert_eq!(panel.affix_aspect_area_width_offset, 11);
    assert_eq!(panel.aspect_area_height_offset_top, 13);
    assert_eq!(panel.screen_capture_delay, 120);
    assert_eq!(panel.overlay_update_delay, 15);
    assert_eq!(panel.threshold_min, 80);
    assert_eq!(panel.threshold_max, 240);
    assert_eq!(panel.threshold_similarity_tooltip, 0.03);
    assert_eq!(panel.threshold_similarity_affix_location, 0.04);
    assert_eq!(panel.threshold_similarity_aspect_location, 0.05);
    assert_eq!(panel.threshold_similarity_socket_location, 0.06);
    assert_eq!(panel.threshold_similarity_splitter_location, 0.07);
    assert_eq!(panel.minimal_ocr_match_type, 75);
    assert!(panel.item_power_limit_enabled);
    assert_eq!(panel.item_power_limit, 925);
    assert!(panel.minimal_affix_value_filter_enabled);
    assert!(panel.multi_build_mode_enabled);
    assert!(!panel.trade_overlay_enabled);
    assert_eq!(panel.overlay_marker_mode.as_str(), "Hide Unwanted");
    assert!(!panel.aspect_detection_enabled);
    assert!(!panel.unique_detection_enabled);
    assert!(!panel.rune_detection_enabled);
    assert!(!panel.sigil_detection_enabled);
    assert_eq!(panel.sigil_display_mode.as_str(), "Blacklisting");
    assert!(!panel.dungeon_tiers_enabled);
    assert!(!panel.toggles.class_toggles.barbarian);
    assert!(!panel.toggles.rune_effect);
    assert!(!panel.socket_detection_enabled);
    assert!(!panel.tempered_detection_enabled);
    assert!(panel.overlay_enabled);
    assert!(panel.overlay_top_most);
    assert_eq!(panel.overlay_font_size, 22);
    assert!(panel.overlay_menu_locked);
    assert!(panel.mobalytics_show_browser);
    assert_eq!(panel.type_area_offset_right, 9);
    assert_eq!(panel.aspect_area_max_height, 280);
    assert!(panel.is_tuning_prism_hints_enabled);
    assert!(!panel.is_import_paragon_infinity_builds_enabled);
    assert_eq!(panel.selected_build_import_website, "Mobalytics.gg");
    assert_eq!(panel.default_colors.greater, "#FFAA00");
    assert_eq!(panel.default_colors.implicit, "Blue");
    assert_eq!(panel.default_colors.normal, d4lc_config::DEFAULT_COLOR);
    assert_eq!(panel.paragon_border_size, 4);
    assert_eq!(panel.paragon_node_size, 55);
    assert_eq!(panel.paragon_node_size_collapsed, 30);
    assert_eq!(panel.paragon_left_offset_collapsed, -120);
    assert_eq!(panel.paragon_top_offset_collapsed, 75);
    assert!(panel.show_overlay_icon);
    assert_eq!(panel.overlay_icon_pos_x, 120);
    assert_eq!(panel.overlay_icon_pos_y, 340);
    assert!(!panel.collapsed_paragonboard_enabled);

    // The round trip back onto `AppSettings` writes every projected
    // field (the dialog confirm path).
    let mut written = d4lc_config::AppSettings::default();
    panel.apply_to_config(&mut written);
    assert_eq!(written.default_colors, panel.default_colors);
    assert_eq!(written.paragon_border_size, 4);
    assert_eq!(written.paragon_node_size, 55);
    assert_eq!(written.paragon_node_size_collapsed, 30);
    assert_eq!(written.paragon_left_offset_collapsed, -120);
    assert_eq!(written.paragon_top_offset_collapsed, 75);
    assert!(written.show_overlay_icon);
    assert_eq!(written.overlay_icon_pos_x, 120);
    assert_eq!(written.overlay_icon_pos_y, 340);
    assert_eq!(
        written.rule_settings.is_collapsed_paragonboard_enabled,
        panel.collapsed_paragonboard_enabled
    );
}

#[test]
fn settings_panel_round_trip_preserves_unowned_config() {
    let settings = d4lc_config::AppSettings {
        rule_settings: d4lc_config::RuleSettingsConfig {
            multi_builds: vec![d4lc_config::MultiBuildRuleConfig {
                name: "Speed".to_string(),
                index: 1,
                color: "Green".to_string(),
            }],
            ..d4lc_config::RuleSettingsConfig::default()
        },
        ..d4lc_config::AppSettings::default()
    };

    let panel = SettingsPanelState::from_config(&settings, Vec::new());

    let mut persisted = settings.clone();
    panel.apply_to_config(&mut persisted);

    assert_eq!(persisted.rule_settings.multi_builds.len(), 1);
    assert_eq!(persisted.rule_settings.multi_builds[0].name, "Speed");
}

#[test]
fn test_type_area_offset_left_persistence() {
    let mut state =
        SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
    assert_eq!(state.type_area_offset_left, 5);

    state.type_area_offset_left = 10;
    let mut settings = d4lc_config::AppSettings::default();
    state.apply_to_config(&mut settings);
    assert_eq!(settings.type_area_offset_left, 10);
}

#[test]
fn test_type_area_offset_left_roundtrip() {
    let settings = d4lc_config::AppSettings {
        type_area_offset_left: 15,
        ..d4lc_config::AppSettings::default()
    };
    let state = SettingsPanelState::from_config(&settings, Vec::new());
    assert_eq!(state.type_area_offset_left, 15);
}

#[test]
fn test_debug_numerics_project_and_apply_round_trip() {
    // Upstream DebugViewModel.cs exposes TypeAreaOffsetRight
    // (line 444) and AspectAreaMaxHeight (line 154) as editable
    // numerics backed by SettingsD4. The Rust debug-exposure
    // projection point is `SettingsPanelState` (same as
    // `type_area_offset_left`).
    let defaults =
        SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
    assert_eq!(defaults.type_area_offset_right, 5);
    assert_eq!(defaults.aspect_area_max_height, 300);

    let mut state = defaults;
    state.type_area_offset_right = 11;
    state.aspect_area_max_height = 260;
    let mut settings = d4lc_config::AppSettings::default();
    state.apply_to_config(&mut settings);
    assert_eq!(settings.type_area_offset_right, 11);
    assert_eq!(settings.aspect_area_max_height, 260);

    let reloaded = SettingsPanelState::from_config(&settings, Vec::new());
    assert_eq!(reloaded.type_area_offset_right, 11);
    assert_eq!(reloaded.aspect_area_max_height, 260);
}

#[test]
fn test_new_v5_4_contract_fields_project_and_apply_round_trip() {
    // IsTuningPrismHintsEnabled,
    // IsImportParagonInfinityBuildsEnabled, and
    // SelectedBuildImportWebsite project through the panel and
    // back into `AppSettings` without loss.
    let defaults =
        SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
    assert!(!defaults.is_tuning_prism_hints_enabled);
    assert!(defaults.is_import_paragon_infinity_builds_enabled);
    assert_eq!(defaults.selected_build_import_website, "Maxroll.gg");

    let mut state = defaults;
    state.is_tuning_prism_hints_enabled = true;
    state.is_import_paragon_infinity_builds_enabled = false;
    state.selected_build_import_website = "D4Builds.gg".to_string();
    let mut settings = d4lc_config::AppSettings::default();
    state.apply_to_config(&mut settings);
    assert!(settings.is_tuning_prism_hints_enabled);
    assert!(!settings.is_import_paragon_infinity_builds_enabled);
    assert_eq!(settings.selected_build_import_website, "D4Builds.gg");
}

#[test]
fn language_change_effects_request_ocr_and_fuzzy_rebuilds() {
    let mut panel =
        SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());

    let unchanged = panel.set_language("enUS");
    assert!(!unchanged.ocr_profile_rebuild_required);
    assert!(!unchanged.fuzzy_index_rebuild_required);
    assert!(!unchanged.localized_parser_reload_required);
    assert!(!unchanged.localized_import_reload_required);
    assert!(!unchanged.localized_paragon_reload_required);
    assert_eq!(unchanged.active_language, "enUS");

    let changed = panel.set_language("frFR");
    assert!(changed.ocr_profile_rebuild_required);
    assert!(changed.fuzzy_index_rebuild_required);
    assert!(changed.localized_parser_reload_required);
    assert!(changed.localized_import_reload_required);
    assert!(changed.localized_paragon_reload_required);
    assert_eq!(changed.selected_language.as_deref(), Some("frFR"));
    assert_eq!(changed.active_language, "frFR");
    assert_eq!(panel.language, "frFR");
    assert_eq!(changed.reload_error, None);

    // The selection persists through the normal config projection and
    // survives a restart: `apply_to_config` writes the token, and a
    // panel rebuilt from the applied config re-seeds the same language.
    let mut config = d4lc_config::AppSettings::default();
    panel.apply_to_config(&mut config);
    assert_eq!(config.selected_affix_language, "frFR");
    let restarted = SettingsPanelState::from_config(&config, Vec::new());
    assert_eq!(restarted.language, "frFR");
}

#[test]
fn shell_language_switch_updates_debug_state() {
    let mut shell = GuiShellState::default_startup();

    let effects = shell.apply_language_change("frFR");

    assert!(effects.ocr_profile_rebuild_required);
    assert!(effects.fuzzy_index_rebuild_required);
    assert!(effects.localized_parser_reload_required);
    assert!(effects.localized_import_reload_required);
    assert!(effects.localized_paragon_reload_required);
    assert_eq!(effects.active_language, "frFR");
    assert_eq!(shell.settings.language, "frFR");
    assert_eq!(shell.debug.active_language, "frFR");
}

#[test]
fn debug_panel_projects_required_debug_output_links_and_timings() {
    let panel = DebugPanelState::from_fixture(DebugPanelFixture {
        artifact_links: vec![
            DebugArtifactLink::new(DebugArtifactKind::RawCapture, "debug-output/live/frame.png"),
            DebugArtifactLink::new(DebugArtifactKind::RoiCrop, "debug-output/live/roi.png"),
            DebugArtifactLink::new(
                DebugArtifactKind::TooltipCrop,
                "debug-output/live/tooltip-crop.png",
            ),
            DebugArtifactLink::new(
                DebugArtifactKind::ThresholdMask,
                "debug-output/live/tooltip-crop-thresholded.png",
            ),
            DebugArtifactLink::new(
                DebugArtifactKind::TemplateTrace,
                "debug-output/live/detect-trace.json",
            ),
            DebugArtifactLink::new(DebugArtifactKind::OcrText, "debug-output/ocr/tooltip.txt"),
            DebugArtifactLink::new(
                DebugArtifactKind::ParsedItemJson,
                "debug-output/rules/parsed-item.json",
            ),
            DebugArtifactLink::new(
                DebugArtifactKind::RuleEvaluationJson,
                "debug-output/rules/rule-evaluation.json",
            ),
            DebugArtifactLink::new(
                DebugArtifactKind::OverlayStatus,
                "debug-output/overlay-status/overlay-status.json",
            ),
            DebugArtifactLink::new(
                DebugArtifactKind::Timings,
                "debug-output/live/live-once.json",
            ),
        ],
        active_language: "frFR".to_string(),
        overlay_status: Some("headless_noop".to_string()),
        timing_metrics: vec![DebugTimingMetric {
            name: "total_ms".to_string(),
            value_ms: 4.25,
        }],
        debug_info_enabled: true,
        stage_panels: Vec::new(),
        perf_rows: Vec::new(),
        ocr_panels: Vec::new(),
    });

    assert_eq!(panel.debug_artifact_count, required_debug_artifacts().len());
    assert_eq!(panel.active_language, "frFR");
    assert_eq!(panel.overlay_status.as_deref(), Some("headless_noop"));
    assert_eq!(panel.timing_metrics[0].name, "total_ms");
    assert!(panel
        .artifacts
        .iter()
        .all(|artifact| artifact.path.as_deref().is_some_and(is_debug_output_path)));
    assert!(panel
        .artifact(DebugArtifactKind::RuleEvaluationJson)
        .is_some_and(|artifact| artifact.present));
}

#[test]
fn debug_panel_marks_absent_or_external_artifacts_missing() {
    let panel = DebugPanelState::from_fixture(DebugPanelFixture {
        artifact_links: vec![
            DebugArtifactLink::new(DebugArtifactKind::RawCapture, "debug-output/live/frame.png"),
            DebugArtifactLink::new(DebugArtifactKind::OcrText, "/tmp/tooltip.txt"),
        ],
        active_language: "deDE".to_string(),
        overlay_status: None,
        timing_metrics: Vec::new(),
        debug_info_enabled: false,
        stage_panels: Vec::new(),
        perf_rows: Vec::new(),
        ocr_panels: Vec::new(),
    });

    assert_eq!(panel.debug_artifact_count, 1);
    assert_eq!(panel.active_language, "deDE");
    assert!(panel
        .artifact(DebugArtifactKind::RawCapture)
        .is_some_and(|artifact| artifact.present));
    assert!(panel
        .artifact(DebugArtifactKind::OcrText)
        .is_some_and(|artifact| !artifact.present && artifact.path.is_none()));
    assert!(panel
        .artifact(DebugArtifactKind::ParsedItemJson)
        .is_some_and(|artifact| !artifact.present && artifact.path.is_none()));
}

#[test]
fn current_item_projection_includes_parsed_affix_rule_and_trade_surfaces() {
    let panel = CurrentItemPanelState::from_parse_and_rules(
        &sample_parse_result(),
        Some(&sample_rule_result()),
    );

    assert_eq!(panel.item_name.as_deref(), Some("Storm Amulet"));
    assert_eq!(panel.rarity.as_deref(), Some("legendary"));
    // The evaluation identity is the canonical catalog type id; the
    // descriptive derivation of the tooltip line stays a display field.
    assert_eq!(panel.item_type.as_deref(), Some("amulet"));
    assert_eq!(panel.base_type.as_deref(), Some("Amulet"));
    assert_eq!(panel.item_power, Some(925));
    assert_eq!(panel.affixes.len(), 1);
    assert_eq!(
        panel.affixes[0].affix_id.as_deref(),
        Some("Affix_AttackSpeed")
    );
    assert!(panel.affixes[0].matched_by_rule);
    assert_eq!(
        panel
            .non_affix_matches
            .iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>(),
        vec![
            CurrentItemMatchKind::Aspect,
            CurrentItemMatchKind::Rune,
            CurrentItemMatchKind::Sigil,
        ]
    );
    assert_eq!(
        panel
            .rule_decision
            .as_ref()
            .map(|summary| summary.decision.as_str()),
        Some("wanted")
    );
    assert_eq!(
        panel
            .trade_marker
            .as_ref()
            .map(|marker| marker.trade_item_id.as_str()),
        Some("Trade_AmuletSpeed")
    );
    assert_eq!(panel.parse_confidence.status, "partial");
    assert_eq!(panel.parse_confidence.total_line_count, 4);
    assert_eq!(panel.parse_confidence.unclassified_line_count, 1);
    assert_eq!(panel.unmatched_lines, vec!["???".to_string()]);
}

#[test]
fn trade_imports_state_supports_create_edit_remove_and_match_value_projection() {
    let mut panel = TradeImportsPanelState::with_providers(vec![
        "D2Core".to_string(),
        "D4Builds".to_string(),
        "Maxroll".to_string(),
        "Mobalytics".to_string(),
    ]);

    panel.upsert_trade_item(TradeItemRow {
        trade_item_id: "Trade_AmuletSpeed".to_string(),
        item_type: "Amulet".to_string(),
        value_text: "5b".to_string(),
        affixes: vec![TradeItemAffixView {
            affix_id: "Affix_AttackSpeed".to_string(),
            is_greater: false,
        }],
    });
    panel.upsert_trade_item(TradeItemRow {
        trade_item_id: "Trade_SwordCrit".to_string(),
        item_type: "Sword".to_string(),
        value_text: "8b".to_string(),
        affixes: vec![TradeItemAffixView {
            affix_id: "Affix_CriticalStrikeChance".to_string(),
            is_greater: true,
        }],
    });

    assert_eq!(panel.import_provider_count, 4);
    assert_eq!(panel.trade_items.len(), 2);
    assert_eq!(
        panel.selected_trade_item_id.as_deref(),
        Some("Trade_SwordCrit")
    );

    panel.upsert_trade_item(TradeItemRow {
        trade_item_id: "Trade_AmuletSpeed".to_string(),
        item_type: "Amulet".to_string(),
        value_text: "6b".to_string(),
        affixes: vec![TradeItemAffixView {
            affix_id: "Affix_AttackSpeed".to_string(),
            is_greater: false,
        }],
    });
    panel
        .remove_trade_item("Trade_SwordCrit")
        .unwrap_or_else(|error| panic!("trade item remove should work: {error}"));
    panel.apply_rule_evaluation(&sample_rule_result());

    assert_eq!(panel.trade_items.len(), 1);
    assert_eq!(panel.trade_items[0].value_text, "6b");
    assert_eq!(
        panel.selected_trade_item_id.as_deref(),
        Some("Trade_AmuletSpeed")
    );
    assert_eq!(panel.matched_trade_value.as_deref(), Some("5b"));
    assert_eq!(
        panel
            .current_trade_marker
            .as_ref()
            .map(|marker| marker.trade_item_id.as_str()),
        Some("Trade_AmuletSpeed")
    );
}

#[test]
fn trade_item_insert_order_matches_id_synthesis_order_for_same_type_rows() {
    let mut panel = TradeImportsPanelState::with_providers(Vec::new());
    // Ids exactly as `d4lc_config::synthesize_trade_item_id`
    // produces for eleven same-type rows in file order: a save
    // persists this panel's order and a load re-synthesizes ids in
    // file order, so the panel order must be the synthesis order
    // or the id↔row mapping rotates on the round trip.
    let expected_ids: Vec<String> = ["Trade_Amulet".to_string()]
        .into_iter()
        .chain((2..=11).map(|suffix| format!("Trade_Amulet_{suffix}")))
        .collect();
    for (index, id) in expected_ids.iter().enumerate() {
        panel.upsert_trade_item(TradeItemRow {
            trade_item_id: id.clone(),
            item_type: "Amulet".to_string(),
            value_text: format!("{}M", 10 + index),
            affixes: vec![],
        });
    }

    let ids: Vec<&str> = panel
        .trade_items
        .iter()
        .map(|row| row.trade_item_id.as_str())
        .collect();
    assert_eq!(
        ids,
        expected_ids.iter().map(String::as_str).collect::<Vec<_>>(),
        "panel order must keep the file-order id↔row mapping (byte order would sort Trade_Amulet_10 before Trade_Amulet_2)"
    );
    // The selection keeps identifying the same row it selected on
    // the last upsert.
    assert_eq!(
        panel.selected_trade_item_id.as_deref(),
        Some("Trade_Amulet_11")
    );
    assert_eq!(
        panel.trade_items.last().map(|row| row.value_text.as_str()),
        Some("20M")
    );
}

#[test]
fn import_workflow_result_projection_keeps_warnings_unmapped_and_trace() {
    let mut panel = TradeImportsPanelState::with_providers(vec![
        "D2Core".to_string(),
        "D4Builds".to_string(),
        "Maxroll".to_string(),
        "Mobalytics".to_string(),
    ]);

    panel.apply_import_state(ImportWorkflowState {
        available_providers: vec![
            "D2Core".to_string(),
            "D4Builds".to_string(),
            "Maxroll".to_string(),
            "Mobalytics".to_string(),
        ],
        selected_provider: Some("Maxroll".to_string()),
        available_variants: vec![ImportVariantView {
            source_build_id: "spin-win".to_string(),
            source_build_name: "Spin to Win".to_string(),
            variant_name: Some("Starter".to_string()),
        }],
        selected_variant: Some("Starter".to_string()),
        status: ImportWorkflowStatus::Success,
        latest_result: Some(ImportWorkflowResultView {
            provider: "Maxroll".to_string(),
            source_url_or_file: "https://maxroll.gg/d4/planner/spin-win".to_string(),
            source_build_id: "spin-win".to_string(),
            source_build_name: "Spin to Win".to_string(),
            class_name: "Barbarian".to_string(),
            variant_name: Some("Starter".to_string()),
            affix_preset_name: "Spin to Win Starter".to_string(),
            items: vec![ImportResultItemView {
                item_type: "Amulet".to_string(),
                affix_count: 3,
                aspect_count: 1,
                unique_count: 0,
                rune_count: 0,
            }],
            paragon_board_count: 5,
            warnings: vec![ImportWarningView {
                provider: "Maxroll".to_string(),
                source_field: "implicitAffixes".to_string(),
                source_value: "UnknownImplicit".to_string(),
                message: "Unknown implicit affix".to_string(),
                upstream_method: "BuildsManagerMaxroll.CreatePresetFromMaxrollBuild".to_string(),
            }],
            unmapped_entries: vec![ImportUnmappedEntryView {
                source_field: "paragon.glyph".to_string(),
                source_value: "MissingGlyph".to_string(),
                message: "Glyph was not mapped".to_string(),
                upstream_method: "BuildsManagerMaxroll.CreatePresetFromMaxrollBuild".to_string(),
            }],
            source_trace: vec![
                "BuildsManagerMaxroll.LoadAvailableMaxrollBuilds".to_string(),
                "BuildsManagerMaxroll.CreatePresetFromMaxrollBuild".to_string(),
            ],
        }),
        last_error: None,
    });

    assert_eq!(
        panel.import_state.selected_provider.as_deref(),
        Some("Maxroll")
    );
    assert_eq!(
        panel.import_state.selected_variant.as_deref(),
        Some("Starter")
    );
    assert_eq!(panel.import_state.available_variants.len(), 1);
    assert_eq!(panel.import_state.status, ImportWorkflowStatus::Success);
    let result = panel
        .import_state
        .latest_result
        .as_ref()
        .unwrap_or_else(|| panic!("import result should be present"));
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.warnings[0].source_field, "implicitAffixes");
    assert_eq!(result.unmapped_entries[0].source_value, "MissingGlyph");
    assert_eq!(result.source_trace.len(), 2);
}

#[test]
fn affix_preset_editor_represents_create_rename_remove_import_export() {
    let mut editor = AffixPresetEditorState::new(Vec::new());

    editor
        .create_preset("Speed")
        .unwrap_or_else(|error| panic!("create should work: {error}"));
    assert_eq!(editor.rules_revision, 1);
    assert_eq!(editor.selected_preset_name.as_deref(), Some("Speed"));

    editor
        .rename_preset("Speed", "Boss")
        .unwrap_or_else(|error| panic!("rename should work: {error}"));
    assert_eq!(editor.rules_revision, 2);
    assert_eq!(editor.selected_preset_name.as_deref(), Some("Boss"));

    editor.import_preset(
        AffixPresetEditorPreset {
            name: "Imported".to_string(),
            entries: Vec::new(),
        },
        vec![ImportWarningView {
            provider: "Maxroll".to_string(),
            source_field: "affixSno".to_string(),
            source_value: "999".to_string(),
            message: "unknown affix".to_string(),
            upstream_method: "CreatePresetFromMaxrollBuild".to_string(),
        }],
    );
    assert_eq!(editor.import_warnings.len(), 1);
    assert_eq!(editor.rules_revision, 3);

    let export = editor
        .export_preset("Imported")
        .unwrap_or_else(|error| panic!("export should work: {error}"));
    assert_eq!(export.preset_name, "Imported");
    assert_eq!(export.suggested_path, "Exports/Imported.json");

    editor
        .remove_preset("Boss")
        .unwrap_or_else(|error| panic!("remove should work: {error}"));
    assert_eq!(editor.rules_revision, 4);
    assert!(editor.presets.iter().all(|preset| preset.name != "Boss"));
}

#[test]
fn rename_preset_moves_paragon_routes_and_remove_deletes_them() {
    // Upstream keeps paragon boards INSIDE the preset
    // (`AffixPreset.ParagonBoardsList`), so the keyed store must move
    // with a rename and vanish with a removal — the boards share the
    // preset's lifetime, not the name string's.
    let mut editor = AffixPresetEditorState::new(Vec::new());
    editor
        .create_preset("Whirlwind")
        .unwrap_or_else(|error| panic!("create should work: {error}"));
    editor.set_paragon_routes(
        "Whirlwind",
        vec![d4lc_rules::ImportedParagonRoute {
            step_name: Some("Starter".to_string()),
            boards: vec![d4lc_rules::ImportedParagonBoard {
                index: 0,
                name: "Start".to_string(),
                glyph: "Enchanter".to_string(),
                rotation: "90°".to_string(),
                active_nodes: vec![105],
            }],
        }],
    );
    assert!(editor.paragon_routes_for("Whirlwind").is_some());

    editor
        .rename_preset("Whirlwind", "Whirlwind Alt")
        .unwrap_or_else(|error| panic!("rename should work: {error}"));
    assert!(
        editor.paragon_routes_for("Whirlwind").is_none(),
        "the old key must not keep the boards after a rename"
    );
    let routes = editor
        .paragon_routes_for("Whirlwind Alt")
        .unwrap_or_else(|| panic!("the renamed preset must carry its boards"));
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].boards[0].name, "Start");

    editor
        .remove_preset("Whirlwind Alt")
        .unwrap_or_else(|error| panic!("remove should work: {error}"));
    assert!(
        editor.paragon_routes_for("Whirlwind Alt").is_none(),
        "removing the preset deletes its stored boards"
    );
    assert!(editor.paragon_boards_by_preset.is_empty());
}

#[test]
fn affix_preset_editor_entries_feed_rules_without_restart() {
    let mut editor = AffixPresetEditorState::new(vec![AffixPresetEditorPreset {
        name: "Speed".to_string(),
        entries: Vec::new(),
    }]);

    editor
        .edit_entry(
            "Speed",
            PresetEntry {
                kind: PresetEntryKind::Affix,
                id: "attack_speed".to_string(),
                label: Some("Attack Speed".to_string()),
                item_type: Some("Gloves".to_string()),
                color: "Green".to_string(),
                is_any_type: false,
                is_greater: true,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
        )
        .unwrap_or_else(|error| panic!("affix edit should work: {error}"));
    editor
        .edit_entry(
            "Speed",
            PresetEntry::non_affix(PresetEntryKind::Aspect, "rapid", Some("Gloves"), "Cyan"),
        )
        .unwrap_or_else(|error| panic!("aspect edit should work: {error}"));
    editor
        .edit_entry(
            "Speed",
            PresetEntry::non_affix(PresetEntryKind::Unique, "frostburn", None, "Orange"),
        )
        .unwrap_or_else(|error| panic!("unique edit should work: {error}"));
    editor
        .edit_entry(
            "Speed",
            PresetEntry::non_affix(PresetEntryKind::Rune, "xol", None, "Yellow"),
        )
        .unwrap_or_else(|error| panic!("rune edit should work: {error}"));
    editor
        .edit_entry(
            "Speed",
            PresetEntry::non_affix(PresetEntryKind::Sigil, "aldurwood", None, "Red"),
        )
        .unwrap_or_else(|error| panic!("sigil edit should work: {error}"));

    let rule_presets = editor.to_rule_presets();

    assert_eq!(editor.rules_revision, 5);
    assert_eq!(
        rule_presets.affix_presets[0].affixes[0].affix_id,
        "attack_speed"
    );
    assert!(rule_presets.affix_presets[0].affixes[0].is_greater);
    assert_eq!(rule_presets.non_affix_presets[0].powers.len(), 2);
    assert_eq!(rule_presets.non_affix_presets[0].runes[0].id, "xol");
    assert_eq!(rule_presets.non_affix_presets[0].sigils[0].id, "aldurwood");

    editor
        .remove_entry("Speed", PresetEntryKind::Rune, "xol", None)
        .unwrap_or_else(|error| panic!("rune remove should work: {error}"));
    assert_eq!(editor.rules_revision, 6);
    assert!(editor.presets[0]
        .entries
        .iter()
        .all(|entry| entry.id != "xol"));

    // The requirement carries the entry's own `IsAnyType` flag
    // and prism list instead of deriving the flag away and dropping
    // the prisms.
    editor
        .edit_entry(
            "Speed",
            PresetEntry {
                kind: PresetEntryKind::Affix,
                id: "life".to_string(),
                label: None,
                item_type: Some("Boots".to_string()),
                color: "Green".to_string(),
                is_any_type: true,
                is_greater: false,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms: vec!["Prism of Control".to_string()],
            },
        )
        .unwrap_or_else(|error| panic!("any-type affix edit should work: {error}"));
    let any_type = editor
        .to_rule_presets()
        .affix_presets
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("one preset"))
        .affixes
        .into_iter()
        .find(|affix| affix.affix_id == "life")
        .unwrap_or_else(|| panic!("the any-type entry projects"));
    assert!(
        any_type.is_any_type && any_type.item_type == "Boots",
        "the flag is independent of the item type"
    );
    assert_eq!(any_type.tuning_prisms, vec!["Prism of Control".to_string()]);
}

#[test]
fn aspect_color_upsert_shares_the_color_across_every_slot() {
    // Imported presets carry one aspect entry per gear slot
    // (`build_editor_entries`); upstream's SetAspectColorExecute copies
    // the chosen color to every same-id aspect of the preset
    // (AffixViewModel.cs:886-894).
    let mut editor = AffixPresetEditorState::new(vec![AffixPresetEditorPreset {
        name: "Imported".to_string(),
        entries: vec![
            PresetEntry::non_affix(PresetEntryKind::Aspect, "rapid", Some("Helm"), "#008000"),
            PresetEntry::non_affix(PresetEntryKind::Aspect, "rapid", Some("Chest"), "#008000"),
            PresetEntry::non_affix(PresetEntryKind::Aspect, "other", Some("Helm"), "#008000"),
            PresetEntry::non_affix(PresetEntryKind::Unique, "rapid", Some("Helm"), "#008000"),
        ],
    }]);

    editor
        .edit_entry(
            "Imported",
            PresetEntry::non_affix(PresetEntryKind::Aspect, "rapid", Some("Helm"), "#0088FF"),
        )
        .unwrap_or_else(|error| panic!("aspect edit should work: {error}"));

    let entries = &editor.presets[0].entries;
    let color_of = |kind: PresetEntryKind, id: &str, item_type: &str| {
        entries
            .iter()
            .find(|entry| {
                entry.kind == kind
                    && entry.id == id
                    && entry.item_type.as_deref() == Some(item_type)
            })
            .map(|entry| entry.color.clone())
    };
    assert_eq!(
        color_of(PresetEntryKind::Aspect, "rapid", "Helm").as_deref(),
        Some("#0088FF")
    );
    assert_eq!(
        color_of(PresetEntryKind::Aspect, "rapid", "Chest").as_deref(),
        Some("#0088FF"),
        "the aspect color reaches every slot sharing the id"
    );
    assert_eq!(
        color_of(PresetEntryKind::Aspect, "other", "Helm").as_deref(),
        Some("#008000"),
        "a different aspect id keeps its color"
    );
    assert_eq!(
        color_of(PresetEntryKind::Unique, "rapid", "Helm").as_deref(),
        Some("#008000"),
        "a unique with the same id keeps its own color"
    );
}

#[test]
fn aspect_add_keeps_the_other_slots_colors() {
    // Adding an aspect to a new slot takes the AddAspect path, not the
    // color dialog: upstream appends the new slot with
    // `DefaultColorAspects` and leaves the existing same-id entries
    // untouched (AffixManager.cs:178-195), so a custom color on another
    // slot must survive the append.
    let mut editor = AffixPresetEditorState::new(vec![AffixPresetEditorPreset {
        name: "Imported".to_string(),
        entries: vec![PresetEntry::non_affix(
            PresetEntryKind::Aspect,
            "rapid",
            Some("Helm"),
            "#0088FF",
        )],
    }]);

    editor
        .edit_entry(
            "Imported",
            PresetEntry::non_affix(PresetEntryKind::Aspect, "rapid", Some("Chest"), "#008000"),
        )
        .unwrap_or_else(|error| panic!("aspect add should work: {error}"));

    let entries = &editor.presets[0].entries;
    let color_of = |item_type: &str| {
        entries
            .iter()
            .find(|entry| {
                entry.kind == PresetEntryKind::Aspect
                    && entry.id == "rapid"
                    && entry.item_type.as_deref() == Some(item_type)
            })
            .map(|entry| entry.color.clone())
    };
    assert_eq!(
        color_of("Chest").as_deref(),
        Some("#008000"),
        "the appended slot carries the incoming color"
    );
    assert_eq!(
        color_of("Helm").as_deref(),
        Some("#0088FF"),
        "adding a slot must not re-color the existing same-id aspect"
    );
}

fn sample_parse_result() -> d4lc_parse::ItemTooltipParseResult {
    d4lc_parse::ItemTooltipParseResult {
        status: d4lc_parse::ItemTooltipParseStatus::Partial,
        name_lines: vec!["Storm Amulet".to_string()],
        name_line_sources: Vec::new(),
        display_name: Some("Storm Amulet".to_string()),
        rarity: Some(d4lc_parse::ItemTooltipRarity::Legendary),
        is_ancestral: Some(true),
        is_sacred: Some(false),
        base_type: Some("Amulet".to_string()),
        item_type_id: Some("amulet".to_string()),
        item_power: Some(925),
        quality: None,
        primary_stats: Vec::new(),
        weapon_details: Vec::new(),
        affixes: vec![d4lc_parse::ItemTooltipAffix {
            name: "Attack Speed".to_string(),
            raw_text: "+32.0% Attack Speed".to_string(),
            value_text: Some("+32.0%".to_string()),
            range_text: None,
            text_clean: Some("Attack Speed".to_string()),
            text_value: Some(32.0),
            affix_id: Some("Affix_AttackSpeed".to_string()),
            affix_label: Some("Attack Speed".to_string()),
            fuzzy: None,
            conversion_method: Some("ConvertToAffix"),
            is_greater_affix: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
            row_id: d4lc_core::AffixRowId(0),
            source: d4lc_parse::ItemTooltipSourceLine {
                source_segment_index: 0,
                source_line_index: 0,
                global_line_index: 1,
                segment_kind: d4lc_parse::TooltipOcrSegmentKind::Affix,
                raw_text: "+32.0% Attack Speed".to_string(),
                converted_text: Some("Attack Speed".to_string()),
                normalized_text: "attack speed".to_string(),
                rect: None,
                marker_name: None,
            },
        }],
        legendary_power: Some(d4lc_parse::ItemTooltipLegendaryPower {
            raw_text: "Aspect of Swiftness".to_string(),
            joined_text: "Aspect of Swiftness".to_string(),
            is_imprinted: false,
            text_clean: Some("Aspect of Swiftness".to_string()),
            power_id: Some("Aspect_Swiftness".to_string()),
            power_label: Some("Aspect of Swiftness".to_string()),
            fuzzy: None,
            conversion_method: Some("ConvertToAspect"),
            is_unique: false,
            sources: Vec::new(),
        }),
        runes: Vec::new(),
        sigils: Vec::new(),
        sockets: Vec::new(),
        metadata: d4lc_parse::ItemTooltipMetadata::default(),
        comparison_delta: Vec::new(),
        ui_chrome_ignored: Vec::new(),
        right_aligned_filtered: Vec::new(),
        unclassified_item_lines: vec![d4lc_parse::ItemTooltipSourceLine {
            source_segment_index: 0,
            source_line_index: 1,
            global_line_index: 3,
            segment_kind: d4lc_parse::TooltipOcrSegmentKind::Generic,
            raw_text: "???".to_string(),
            converted_text: None,
            normalized_text: "???".to_string(),
            rect: None,
            marker_name: None,
        }],
        affix_row_geometry: Vec::new(),
        non_affix_rows: Vec::new(),
        source: d4lc_parse::ItemTooltipSourceSummary {
            segment_count: 2,
            line_count: 4,
        },
    }
}

fn sample_rule_result() -> d4lc_rules::RuleEvaluationResult {
    let source = d4lc_rules::RuleSourceNote::new(
        "D4Companion.Services/AffixManager.cs",
        "AffixManager.GetAffix",
        "preset affix match",
    );
    let trade_source = d4lc_rules::RuleSourceNote::new(
        "D4Companion.Services/TradeItemManager.cs",
        "TradeItemManager.FindTradeItem",
        "trade item type and affix match",
    );

    d4lc_rules::RuleEvaluationResult {
        final_decision: d4lc_rules::RuleDecision::Wanted,
        final_source: Some(source.clone()),
        matched_affixes: vec![d4lc_rules::RuleAffixMatch {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            color: "Green".to_string(),
            value_text: Some("+32.0%".to_string()),
            source_text: "+32.0% Attack Speed".to_string(),
            row_id: Some(d4lc_core::AffixRowId(1)),
            required_by: vec!["Speed".to_string()],
            is_greater_required: false,
            is_any_type_required: false,
            value_status: d4lc_rules::RuleCheck::passed(
                d4lc_rules::RuleCheckKind::MinimalAffixValue,
                Some(32.0),
                Some(30.0),
                source.clone(),
            ),
            tuning_prisms: Vec::new(),
            source: source.clone(),
            column_offset: 0,
        }],
        missing_affixes: vec![d4lc_rules::RuleMissingAffix {
            affix_id: "Affix_CriticalStrikeChance".to_string(),
            label: Some("Critical Strike Chance".to_string()),
            color: "Red".to_string(),
            required_by: vec!["Speed".to_string()],
            source: source.clone(),
        }],
        item_power_status: d4lc_rules::RuleCheck::passed(
            d4lc_rules::RuleCheckKind::ItemPowerLimit,
            Some(925.0),
            Some(900.0),
            source.clone(),
        ),
        minimal_value_status: d4lc_rules::RuleCheck::passed(
            d4lc_rules::RuleCheckKind::MinimalAffixValue,
            Some(32.0),
            Some(30.0),
            source.clone(),
        ),
        matched_power: Some(d4lc_rules::RulePowerMatch {
            power_id: "Aspect_Swiftness".to_string(),
            power_kind: d4lc_rules::RulePowerKind::Aspect,
            source: source.clone(),
        }),
        matched_powers: Vec::new(),
        matched_runes: vec!["Rune_Bac".to_string()],
        matched_sigil: Some("Sigil_Uldur".to_string()),
        matched_sigils: Vec::new(),
        trade_marker: Some(d4lc_rules::RuleTradeMarker {
            trade_item_id: "Trade_AmuletSpeed".to_string(),
            value_text: "5b".to_string(),
            source: trade_source.clone(),
        }),
        overlay_descriptors: Vec::new(),
        affix_traces: Vec::new(),
        explanation: d4lc_rules::RuleExplanation {
            active_preset_names: vec!["Speed".to_string()],
            matched_build_names: vec!["Build 1".to_string()],
            selected_build_name: Some("Build 1".to_string()),
            item_type: Some("Amulet".to_string()),
            rarity: Some("legendary".to_string()),
            branches: vec![
                d4lc_rules::RuleBranchExplanation {
                    decision: d4lc_rules::RuleDecision::Wanted,
                    message: "matched affix Affix_AttackSpeed".to_string(),
                    source,
                },
                d4lc_rules::RuleBranchExplanation {
                    decision: d4lc_rules::RuleDecision::Wanted,
                    message: "trade item Trade_AmuletSpeed matched".to_string(),
                    source: trade_source,
                },
            ],
        },
    }
}

// ── Build merge (upstream MergeBuildsExecute) ─────────────────────

fn merge_entry(
    kind: PresetEntryKind,
    id: &str,
    item_type: Option<&str>,
    color: &str,
) -> PresetEntry {
    PresetEntry {
        kind,
        id: id.to_string(),
        label: None,
        item_type: item_type.map(str::to_string),
        color: color.to_string(),
        is_any_type: item_type.is_none(),
        is_greater: kind == PresetEntryKind::Affix,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    }
}

fn merge_options(
    change1: bool,
    color1: &str,
    change2: bool,
    color2: &str,
    change12: bool,
    color12: &str,
) -> MergeColorOptions {
    MergeColorOptions {
        change_color_build1: change1,
        color_build1: color1.to_string(),
        change_color_build2: change2,
        color_build2: color2.to_string(),
        change_color_build12: change12,
        color_build12: color12.to_string(),
    }
}

#[test]
fn merge_takes_build1_entries_and_recolors_on_checkbox() {
    let build1 = AffixPresetEditorPreset {
        name: "Speed".to_string(),
        entries: vec![
            merge_entry(PresetEntryKind::Affix, "Affix_A", Some("Amulet"), "#FF0000"),
            merge_entry(PresetEntryKind::Aspect, "Aspect_A", None, "#00FF00"),
            merge_entry(PresetEntryKind::Sigil, "Sigil_A", None, "#0000FF"),
        ],
    };
    let options = merge_options(true, "#FFFF00", false, "", false, "");
    let merged = merge_affix_presets(
        &build1,
        &AffixPresetEditorPreset {
            name: "Boss".to_string(),
            entries: Vec::new(),
        },
        "",
        &options,
    );
    assert_eq!(
        merged.name, "Speed",
        "empty name falls back to Build 1's name"
    );
    assert_eq!(
        merged
            .entries
            .iter()
            .map(|entry| entry.color.as_str())
            .collect::<Vec<_>>(),
        vec!["#FFFF00", "#FFFF00", "#FFFF00"],
        "ChangeColorBuild1 recolors affixes, aspects, and sigils"
    );
    assert!(
        merged.entries[0].is_greater,
        "Build 1 affix flags carry over"
    );

    // Checkbox off: the Build 1 colors stay.
    let options = merge_options(false, "#FFFF00", false, "", false, "");
    let merged = merge_affix_presets(&build1, &build1, "", &options);
    assert_eq!(merged.entries[0].color, "#FF0000");
    assert_eq!(merged.entries[2].color, "#0000FF");
}

#[test]
fn merge_appends_new_build2_entries_and_recolors_duplicates() {
    let build1 = AffixPresetEditorPreset {
        name: "Speed".to_string(),
        entries: vec![
            merge_entry(
                PresetEntryKind::Affix,
                "Affix_Shared",
                Some("Amulet"),
                "#FF0000",
            ),
            merge_entry(PresetEntryKind::Affix, "Affix_Only1", None, "#FF0000"),
        ],
    };
    let build2 = AffixPresetEditorPreset {
        name: "Boss".to_string(),
        entries: vec![
            // Same (id, type) as Build 1's first entry: duplicate.
            merge_entry(
                PresetEntryKind::Affix,
                "Affix_Shared",
                Some("Amulet"),
                "#00FF00",
            ),
            // Same id but a DIFFERENT item type: a new key.
            merge_entry(
                PresetEntryKind::Affix,
                "Affix_Shared",
                Some("Ring"),
                "#00FF00",
            ),
            merge_entry(PresetEntryKind::Aspect, "Aspect_Only2", None, "#00FF00"),
            merge_entry(PresetEntryKind::Sigil, "Sigil_Only2", None, "#00FF00"),
        ],
    };
    let options = merge_options(false, "", true, "#0000FF", true, "#FFFF00");
    let merged = merge_affix_presets(&build1, &build2, "Merged", &options);

    let find = |id: &str, item_type: Option<&str>| {
        merged
            .entries
            .iter()
            .find(|entry| entry.id == id && entry.item_type.as_deref() == item_type)
            .map(|entry| entry.color.as_str())
            .unwrap_or("missing")
    };
    // Duplicate: the EXISTING Build 1 entry is recolored to the
    // Build 1+2 color (ChangeColorBuild12 wins over Build 1's color).
    assert_eq!(find("Affix_Shared", Some("Amulet")), "#FFFF00");
    // New Build 2 keys: appended and recolored to Build 2's color.
    assert_eq!(find("Affix_Shared", Some("Ring")), "#0000FF");
    assert_eq!(find("Aspect_Only2", None), "#0000FF");
    assert_eq!(find("Sigil_Only2", None), "#0000FF");
    // Build-1-only entry untouched (no ChangeColorBuild1).
    assert_eq!(find("Affix_Only1", None), "#FF0000");
    assert_eq!(merged.entries.len(), 5);
}

#[test]
fn merge_excludes_uniques_and_runes_and_keeps_duplicate_color_without_checkbox() {
    let build1 = AffixPresetEditorPreset {
        name: "Speed".to_string(),
        entries: vec![
            merge_entry(
                PresetEntryKind::Unique,
                "Unique_1",
                Some("Amulet"),
                "#FF0000",
            ),
            merge_entry(PresetEntryKind::Rune, "Rune_1", None, "#FF0000"),
            merge_entry(PresetEntryKind::Affix, "Affix_Shared", None, "#FF0000"),
        ],
    };
    let build2 = AffixPresetEditorPreset {
        name: "Boss".to_string(),
        entries: vec![
            merge_entry(PresetEntryKind::Unique, "Unique_2", None, "#00FF00"),
            merge_entry(PresetEntryKind::Rune, "Rune_2", None, "#00FF00"),
            merge_entry(PresetEntryKind::Affix, "Affix_Shared", None, "#00FF00"),
        ],
    };
    // All checkboxes off: the duplicate keeps Build 1's color and no
    // unique/rune from either build reaches the merged preset.
    let options = merge_options(false, "", false, "", false, "");
    let merged = merge_affix_presets(&build1, &build2, "Merged", &options);
    assert_eq!(merged.entries.len(), 1, "only Build 1's affix survives");
    assert_eq!(merged.entries[0].id, "Affix_Shared");
    assert_eq!(merged.entries[0].color, "#FF0000");
}

#[test]
fn merge_name_falls_back_to_build1_and_trims_whitespace() {
    let build1 = AffixPresetEditorPreset {
        name: "Speed".to_string(),
        entries: Vec::new(),
    };
    let build2 = AffixPresetEditorPreset {
        name: "Boss".to_string(),
        entries: Vec::new(),
    };
    let options = MergeColorOptions::default();
    let merged = merge_affix_presets(&build1, &build2, "", &options);
    assert_eq!(merged.name, "Speed", "empty Build-1 name falls back");
    let merged = merge_affix_presets(&build1, &build2, "   ", &options);
    assert_eq!(merged.name, "Speed", "whitespace Build-1 name falls back");
    let merged = merge_affix_presets(&build1, &build2, "  Merged  ", &options);
    assert_eq!(merged.name, "Merged", "a typed name is trimmed");
}
