use super::*;
use crate::affix::{
    apply_item_power_and_value_filters, evaluate_affix_preset, evaluate_multi_build_affix_presets,
};
use crate::import::ImportedParagonRoute;
use crate::model::{
    AffixPreset, AffixRequirement, AffixValueThreshold, MultiBuildEntry, MultiBuildSettings,
    ParsedAffixKind, ParsedItemForRules, RuleCheck, RuleCheckKind, RuleDecision, RuleExplanation,
    RuleFilterSettings, RuleTradeMarker,
};
use crate::paragon::ParagonOverlayMode;
use crate::test_support::{
    affix_requirement, default_paragon_overlay_layout, parsed_affix_with_value, prism_requirement,
    prism_test_item, rect, sample_paragon_board,
};

fn tuning_overlay_settings(
    is_tuning_prism_hints_enabled: bool,
    item_power_limit_check_ok: bool,
    marker_mode: RuleOverlayMarkerMode,
) -> RuleOverlaySettings {
    RuleOverlaySettings {
        marker_mode,
        is_minimal_affix_value_filter_enabled: false,
        is_trade_overlay_enabled: false,
        item_power_limit_check_ok,
        is_tuning_prism_hints_enabled,
    }
}

fn tuning_overlay_layout() -> RuleOverlayLayout {
    RuleOverlayLayout {
        tooltip_rect: rect(100, 200, 320, 500),
        affix_locations: vec![affix_location(0, rect(120, 240, 20, 20), None)],
        power_location: None,
        power_locations: Vec::new(),
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: Vec::new(),
    }
}

#[test]
fn tuning_prism_descriptors_use_upstream_geometry_and_colors() {
    let mut result = empty_rule_result();
    result.matched_affixes.push(RuleAffixMatch {
        affix_id: "Affix_AttackSpeed".to_string(),
        label: Some("Attack Speed".to_string()),
        color: "Green".to_string(),
        value_text: Some("+32.0%".to_string()),
        source_text: "+32.0% Attack Speed".to_string(),
        row_id: Some(AffixRowId(0)),
        required_by: vec!["Speed Farm".to_string()],
        is_greater_required: false,
        is_any_type_required: false,
        value_status: RuleCheck::not_evaluated(
            RuleCheckKind::MinimalAffixValue,
            RuleSourceNote::new("test", "test", "test"),
        ),
        tuning_prisms: vec!["TuningStone_1".to_string(), "TuningStone_5".to_string()],
        source: RuleSourceNote::new("test", "test", "test"),
        column_offset: 0,
    });

    let descriptors = build_overlay_descriptors(
        &result,
        &tuning_overlay_settings(true, true, RuleOverlayMarkerMode::ShowAll),
        &tuning_overlay_layout(),
    );

    // Upstream `DrawGraphicsTuningPrisms`: origin X = tooltip left +
    // TooltipWidth + (-20); circle i center X = origin + i * radius(10);
    // center Y = affix top + location height / 2 = 240 + 10 = 250.
    // So circle 0 is centered at (100 + 320 - 20, 250) = (400, 250) and
    // circle 1 at (410, 250), each with the 20x20 bounding square of a
    // radius-10 circle.
    let prisms: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::TuningPrism)
        .collect();
    assert_eq!(prisms.len(), 2);
    assert_eq!(prisms[0].shape, RuleOverlayShape::Circle);
    assert_eq!(prisms[0].color, "Red");
    assert_eq!(prisms[0].geometry, rect(390, 240, 20, 20));
    assert_eq!(prisms[1].color, "LightBlue");
    assert_eq!(prisms[1].geometry, rect(400, 240, 20, 20));
    assert!(prisms.iter().all(|descriptor| {
        descriptor.z_order_group == RuleOverlayZOrderGroup::Marker
            && descriptor.text.is_none()
            && descriptor.geometry_source == "affix_tuning_prism_circle_geometry"
            && descriptor.source.upstream_file == "D4Companion.Services/OverlayHandler.cs"
            && descriptor.source.upstream_method == "OverlayHandler.DrawGraphicsTuningPrisms"
            && matches!(
                descriptor.target,
                RuleOverlayTarget::Affix {
                    ref affix_id,
                    row_id: Some(AffixRowId(0)),
                } if affix_id == "Affix_AttackSpeed"
            )
    }));
    // The regular affix marker is still generated next to the hints.
    assert!(descriptors
        .iter()
        .any(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Affix));
}

#[test]
fn tuning_prism_descriptors_are_gated_by_the_hints_setting() {
    let mut result = empty_rule_result();
    result.matched_affixes.push(RuleAffixMatch {
        affix_id: "Affix_AttackSpeed".to_string(),
        label: Some("Attack Speed".to_string()),
        color: "Green".to_string(),
        value_text: Some("+32.0%".to_string()),
        source_text: "+32.0% Attack Speed".to_string(),
        row_id: Some(AffixRowId(0)),
        required_by: vec!["Speed Farm".to_string()],
        is_greater_required: false,
        is_any_type_required: false,
        value_status: RuleCheck::not_evaluated(
            RuleCheckKind::MinimalAffixValue,
            RuleSourceNote::new("test", "test", "test"),
        ),
        tuning_prisms: vec!["TuningStone_1".to_string()],
        source: RuleSourceNote::new("test", "test", "test"),
        column_offset: 0,
    });

    let descriptors = build_overlay_descriptors(
        &result,
        &tuning_overlay_settings(false, true, RuleOverlayMarkerMode::ShowAll),
        &tuning_overlay_layout(),
    );

    assert!(descriptors
        .iter()
        .all(|descriptor| descriptor.marker_type != RuleOverlayMarkerType::TuningPrism));
    assert!(descriptors
        .iter()
        .any(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Affix));
}

#[test]
fn tuning_prism_descriptors_draw_outside_marker_mode_and_power_gates() {
    // Upstream calls `DrawGraphicsTuningPrisms` from both affix draw
    // paths outside the `!Red` marker gate and the item-power gate:
    // red (unwanted) affixes still get their prism hints, and the
    // hints survive `itemPowerLimitCheckOk == false`.
    let mut result = empty_rule_result();
    result.matched_affixes.push(RuleAffixMatch {
        affix_id: "Affix_AttackSpeed".to_string(),
        label: Some("Attack Speed".to_string()),
        color: "Red".to_string(),
        value_text: Some("+32.0%".to_string()),
        source_text: "+32.0% Attack Speed".to_string(),
        row_id: Some(AffixRowId(0)),
        required_by: vec!["Speed Farm".to_string()],
        is_greater_required: false,
        is_any_type_required: false,
        value_status: RuleCheck::not_evaluated(
            RuleCheckKind::MinimalAffixValue,
            RuleSourceNote::new("test", "test", "test"),
        ),
        // Unknown prism names keep the previous circle color (upstream
        // switch default falls through): Blue then Blue again.
        tuning_prisms: vec!["TuningStone_2".to_string(), "UnknownStone".to_string()],
        source: RuleSourceNote::new("test", "test", "test"),
        column_offset: 0,
    });

    let descriptors = build_overlay_descriptors(
        &result,
        &tuning_overlay_settings(true, false, RuleOverlayMarkerMode::HideUnwanted),
        &tuning_overlay_layout(),
    );

    assert!(descriptors
        .iter()
        .all(|descriptor| descriptor.marker_type != RuleOverlayMarkerType::Affix));
    let prisms: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::TuningPrism)
        .collect();
    assert_eq!(prisms.len(), 2);
    assert_eq!(prisms[0].color, "Blue");
    assert_eq!(prisms[1].color, "Blue");
}

#[test]
fn multi_build_matches_project_tuning_prism_descriptors() {
    // Upstream draws prism hints from BOTH affix draw paths
    // (`DrawGraphicsAffixes` and `DrawGraphicsAffixesMulti`). The Rust
    // seam has one descriptor generator serving both flows: the
    // multi-build evaluation reuses `evaluate_affix_preset`, so
    // winning multi-build matches must carry prism descriptors too.
    let item = prism_test_item("Affix_AttackSpeed");
    let presets = vec![AffixPreset {
        name: "Speed".to_string(),
        affixes: vec![prism_requirement("Affix_AttackSpeed", &["TuningStone_1"])],
    }];
    let settings = MultiBuildSettings {
        enabled: true,
        selected_preset: "Speed".to_string(),
        builds: vec![MultiBuildEntry {
            build_name: "Whirlwind".to_string(),
            preset_name: "Speed".to_string(),
            color: "Blue".to_string(),
        }],
    };

    let result = evaluate_multi_build_affix_presets(&item, &settings, &presets);
    assert_eq!(
        result.matched_affixes[0].tuning_prisms,
        vec!["TuningStone_1"]
    );

    let descriptors = build_overlay_descriptors(
        &result,
        &tuning_overlay_settings(true, true, RuleOverlayMarkerMode::ShowAll),
        &tuning_overlay_layout(),
    );
    let prisms: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::TuningPrism)
        .collect();
    assert_eq!(prisms.len(), 1);
    assert_eq!(prisms[0].color, "Red");
    assert_eq!(prisms[0].geometry, rect(390, 240, 20, 20));
}

#[test]
fn build_overlay_descriptors_shift_each_build_column_from_the_row_anchor() {
    // `OverlayHandler.DrawGraphicsAffixesMulti` draws build i's marker
    // at the row's anchor shifted by `5 - (i * 20)` (OH:337, :357). Every
    // build entry of a row keeps that row's anchor; only its own column
    // offset moves it.
    let row = |color: &str, column_offset: i32| RuleAffixMatch {
        affix_id: "AttackSpeed".to_string(),
        label: None,
        color: color.to_string(),
        value_text: None,
        source_text: "+32.0% Attack Speed".to_string(),
        row_id: Some(AffixRowId(4)),
        required_by: vec!["Build".to_string()],
        is_greater_required: false,
        is_any_type_required: false,
        value_status: RuleCheck::not_evaluated(
            RuleCheckKind::MinimalAffixValue,
            RuleSourceNote::new("test", "test", "test"),
        ),
        tuning_prisms: Vec::new(),
        source: RuleSourceNote::new("test", "test", "test"),
        column_offset,
    };
    let result = RuleEvaluationResult {
        final_decision: RuleDecision::Wanted,
        final_source: None,
        matched_affixes: vec![row("Blue", 5), row("Green", -15)],
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
            active_preset_names: Vec::new(),
            matched_build_names: Vec::new(),
            selected_build_name: None,
            item_type: None,
            rarity: None,
            branches: Vec::new(),
        },
    };
    let settings = RuleOverlaySettings {
        marker_mode: RuleOverlayMarkerMode::ShowAll,
        is_minimal_affix_value_filter_enabled: false,
        is_trade_overlay_enabled: false,
        item_power_limit_check_ok: true,
        is_tuning_prism_hints_enabled: false,
    };
    let anchor = RuleAffixOverlayLocation {
        row_id: Some(AffixRowId(4)),
        geometry: Rect {
            x: 100,
            y: 300,
            width: 18,
            height: 22,
        },
        text: None,
    };
    let layout = RuleOverlayLayout {
        tooltip_rect: Rect {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        },
        affix_locations: vec![anchor],
        power_location: None,
        power_locations: Vec::new(),
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: Vec::new(),
    };

    let descriptors = build_overlay_descriptors(&result, &settings, &layout);

    assert_eq!(descriptors.len(), 2);
    assert_eq!(descriptors[0].color, "Blue");
    assert_eq!(descriptors[0].geometry, rect(95, 301, 20, 20));
    assert_eq!(descriptors[1].color, "Green");
    assert_eq!(descriptors[1].geometry, rect(75, 301, 20, 20));
}

#[test]
fn build_overlay_descriptors_emits_per_build_power_and_sigil_columns() {
    // Upstream `DrawGraphicsAspectsMulti` (OH:487-522) draws one aspect
    // circle per build at the shared column offset `5 - (i * 20)`
    // (OH:337), and the multi affix path draws each build's dungeon
    // square in its own column. Two builds listing the same tooltip row
    // produce two distinct markers, and the per-build vectors replace
    // the single-slot draw so the first build's claim cannot
    // double-draw.
    let power = RulePowerMatch {
        power_id: "Aspect_BoldChieftain".to_string(),
        power_kind: RulePowerKind::Aspect,
        source: RuleSourceNote::new("test", "test", "test"),
    };
    let mut result = empty_rule_result();
    result.matched_powers = vec![power.clone(), power];
    result.matched_sigils = vec!["Sigil_Pit".to_string(), "Sigil_Pit".to_string()];
    let settings = tuning_overlay_settings(false, true, RuleOverlayMarkerMode::ShowAll);
    let layout = RuleOverlayLayout {
        tooltip_rect: rect(100, 200, 320, 500),
        affix_locations: Vec::new(),
        power_location: Some(RuleIdOverlayLocation {
            id: "Aspect_BoldChieftain".to_string(),
            geometry: rect(999, 650, 300, 22),
            color: "Gold".to_string(),
            text: None,
        }),
        power_locations: vec![
            RuleIdOverlayLocation {
                id: "Aspect_BoldChieftain".to_string(),
                geometry: rect(605, 650, 300, 22),
                color: "Blue".to_string(),
                text: None,
            },
            RuleIdOverlayLocation {
                id: "Aspect_BoldChieftain".to_string(),
                geometry: rect(585, 650, 300, 22),
                color: "Green".to_string(),
                text: None,
            },
        ],
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: vec![
            RuleIdOverlayLocation {
                id: "Sigil_Pit".to_string(),
                geometry: rect(605, 700, 20, 20),
                color: "Teal".to_string(),
                text: None,
            },
            RuleIdOverlayLocation {
                id: "Sigil_Pit".to_string(),
                geometry: rect(585, 700, 20, 20),
                color: "Purple".to_string(),
                text: None,
            },
        ],
    };

    let descriptors = build_overlay_descriptors(&result, &settings, &layout);

    let power_descriptors: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Aspect)
        .collect();
    assert_eq!(power_descriptors.len(), 2, "one marker per matching build");
    // Circles centered on each build's column: box left = column - 10.
    assert_eq!(power_descriptors[0].geometry.x, 595);
    assert_eq!(power_descriptors[0].color, "Blue");
    assert_eq!(power_descriptors[1].geometry.x, 575);
    assert_eq!(power_descriptors[1].color, "Green");
    let sigil_descriptors: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Sigil)
        .collect();
    assert_eq!(sigil_descriptors.len(), 2, "one square per matching build");
    assert_eq!(sigil_descriptors[0].shape, RuleOverlayShape::Rectangle);
    assert_eq!(sigil_descriptors[0].geometry, rect(595, 700, 20, 20));
    assert_eq!(sigil_descriptors[1].geometry, rect(575, 700, 20, 20));

    // A row only one build lists keeps exactly its own marker: the
    // single-column build still draws, none of its markers dropped.
    result.matched_powers.truncate(1);
    result.matched_sigils.truncate(1);
    let descriptors = build_overlay_descriptors(&result, &settings, &layout);
    assert_eq!(
        descriptors
            .iter()
            .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Aspect)
            .count(),
        1
    );
    assert_eq!(
        descriptors
            .iter()
            .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Sigil)
            .count(),
        1
    );
}

#[test]
fn build_overlay_descriptors_suppresses_trade_panel_on_empty_tooltip_rect() {
    // Upstream draws the trade panel only while a parsed tooltip is
    // live (OH:524-566); a zero tooltip rect would put the panel at
    // the window origin, so the descriptor is suppressed.
    let result = RuleEvaluationResult {
        final_decision: RuleDecision::Wanted,
        final_source: None,
        matched_affixes: Vec::new(),
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
        trade_marker: Some(RuleTradeMarker {
            trade_item_id: "Trade_Helm".to_string(),
            value_text: "10M".to_string(),
            source: RuleSourceNote::new("test", "test", "test"),
        }),
        overlay_descriptors: Vec::new(),
        affix_traces: Vec::new(),
        explanation: RuleExplanation {
            active_preset_names: Vec::new(),
            matched_build_names: Vec::new(),
            selected_build_name: None,
            item_type: None,
            rarity: None,
            branches: Vec::new(),
        },
    };
    let settings = RuleOverlaySettings {
        marker_mode: RuleOverlayMarkerMode::ShowAll,
        is_minimal_affix_value_filter_enabled: false,
        is_trade_overlay_enabled: true,
        item_power_limit_check_ok: true,
        is_tuning_prism_hints_enabled: false,
    };
    let layout = RuleOverlayLayout {
        tooltip_rect: Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        affix_locations: Vec::new(),
        power_location: None,
        power_locations: Vec::new(),
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: Vec::new(),
    };

    let descriptors = build_overlay_descriptors(&result, &settings, &layout);

    assert!(
        !descriptors
            .iter()
            .any(|descriptor| matches!(descriptor.target, RuleOverlayTarget::Trade)),
        "empty tooltip rect must suppress the trade panel"
    );

    // A real tooltip rect anchors the panel as before.
    let layout = RuleOverlayLayout {
        tooltip_rect: Rect {
            x: 580,
            y: 100,
            width: 400,
            height: 340,
        },
        ..layout
    };
    let descriptors = build_overlay_descriptors(&result, &settings, &layout);
    let trade = descriptors
        .iter()
        .find(|descriptor| matches!(descriptor.target, RuleOverlayTarget::Trade))
        .expect("live tooltip anchors the trade panel");
    assert_eq!(trade.geometry, trade_panel_geometry(layout.tooltip_rect));
}

#[test]
fn overlay_descriptors_cover_affix_shapes_and_unwanted_visibility() {
    let item = ParsedItemForRules {
        item_type: "amulet".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![
            parsed_affix_with_value(
                "Affix_AttackSpeed",
                ParsedAffixKind::Normal,
                Some("+32.0%"),
                Some(AffixRowId(1)),
            ),
            parsed_affix_with_value(
                "Affix_CriticalStrikeChance",
                ParsedAffixKind::Normal,
                Some("+8.0%"),
                Some(AffixRowId(2)),
            ),
            parsed_affix_with_value(
                "Affix_Damage",
                ParsedAffixKind::Normal,
                Some("+10.0%"),
                Some(AffixRowId(3)),
            ),
            parsed_affix_with_value(
                "Affix_ColdResistance",
                ParsedAffixKind::Normal,
                Some("+4.0%"),
                Some(AffixRowId(4)),
            ),
        ],
    };
    let preset = AffixPreset {
        name: "Overlay".to_string(),
        affixes: vec![
            affix_requirement("Affix_AttackSpeed", "amulet", "Green"),
            AffixRequirement {
                affix_id: "Affix_CriticalStrikeChance".to_string(),
                label: None,
                item_type: "amulet".to_string(),
                color: "Gold".to_string(),
                is_any_type: false,
                is_greater: true,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
            AffixRequirement {
                affix_id: "Affix_Damage".to_string(),
                label: None,
                item_type: "ring".to_string(),
                color: "Blue".to_string(),
                is_any_type: true,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
            affix_requirement("Affix_ColdResistance", "amulet", "Red"),
        ],
    };
    let mut result = evaluate_affix_preset(&item, "Overlay", &[preset]);
    apply_item_power_and_value_filters(
        &mut result,
        Some(925),
        "amulet",
        &RuleFilterSettings {
            is_item_power_limit_enabled: false,
            item_power_limit: 800,
            is_minimal_affix_value_filter_enabled: true,
        },
        &[AffixValueThreshold {
            affix_id: "Affix_Damage".to_string(),
            threshold: 20.0,
        }],
    );

    apply_overlay_descriptors(
        &mut result,
        &RuleOverlaySettings {
            marker_mode: RuleOverlayMarkerMode::HideUnwanted,
            is_minimal_affix_value_filter_enabled: true,
            is_trade_overlay_enabled: false,
            item_power_limit_check_ok: true,
            is_tuning_prism_hints_enabled: false,
        },
        &RuleOverlayLayout {
            tooltip_rect: rect(100, 200, 320, 500),
            affix_locations: vec![
                affix_location(1, rect(120, 240, 20, 20), None),
                affix_location(2, rect(120, 280, 20, 20), None),
                affix_location(3, rect(120, 320, 20, 20), None),
                affix_location(4, rect(120, 360, 20, 20), None),
            ],
            power_location: None,
            power_locations: Vec::new(),
            rune_locations: Vec::new(),
            sigil_location: None,
            sigil_locations: Vec::new(),
        },
    );

    assert_eq!(
        result
            .overlay_descriptors
            .iter()
            .filter(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Affix)
            .map(|descriptor| descriptor.shape)
            .collect::<Vec<_>>(),
        vec![
            RuleOverlayShape::Circle,
            RuleOverlayShape::Triangle,
            RuleOverlayShape::Rectangle
        ]
    );
    assert!(result
        .overlay_descriptors
        .iter()
        .all(|descriptor| descriptor.color != "Red"));
    assert!(result.overlay_descriptors.iter().all(|descriptor| {
        descriptor.z_order_group == RuleOverlayZOrderGroup::Marker
            && descriptor.geometry.x >= 100
            && descriptor.source.upstream_method == "OverlayHandler.DrawGraphicsAffixes"
    }));
}

#[test]
fn overlay_descriptors_cover_power_rune_sigil_and_trade_markers() {
    let source = RuleSourceNote::new(
        "D4Companion.Services/AffixManager.cs",
        "AffixManager.GetAspect",
        "aspect match",
    );
    let mut result = empty_rule_result();
    result.matched_power = Some(RulePowerMatch {
        power_id: "Aspect_BoldChieftain".to_string(),
        power_kind: RulePowerKind::Aspect,
        source,
    });
    result.matched_runes = vec!["Rune_Yul".to_string()];
    result.matched_sigil = Some("Sigil_Pit".to_string());
    result.trade_marker = Some(RuleTradeMarker {
        trade_item_id: "trade-amulet-speed".to_string(),
        value_text: "5b".to_string(),
        source: RuleSourceNote::new(
            "D4Companion.Services/TradeItemManager.cs",
            "TradeItemManager.FindTradeItem",
            "trade item type and affix match",
        ),
    });

    apply_overlay_descriptors(
        &mut result,
        &RuleOverlaySettings {
            marker_mode: RuleOverlayMarkerMode::ShowAll,
            is_minimal_affix_value_filter_enabled: false,
            is_trade_overlay_enabled: true,
            item_power_limit_check_ok: true,
            is_tuning_prism_hints_enabled: false,
        },
        &RuleOverlayLayout {
            tooltip_rect: rect(100, 200, 320, 500),
            affix_locations: Vec::new(),
            power_location: Some(RuleIdOverlayLocation {
                id: "Aspect_BoldChieftain".to_string(),
                geometry: rect(120, 650, 20, 20),
                color: "Gold".to_string(),
                text: None,
            }),
            power_locations: Vec::new(),
            rune_locations: vec![RuleIdOverlayLocation {
                id: "Rune_Yul".to_string(),
                geometry: rect(140, 700, 20, 20),
                color: "Purple".to_string(),
                text: Some("Yul".to_string()),
            }],
            sigil_location: Some(RuleIdOverlayLocation {
                id: "Sigil_Pit".to_string(),
                geometry: rect(160, 700, 20, 20),
                color: "Teal".to_string(),
                text: Some("Pit".to_string()),
            }),
            sigil_locations: Vec::new(),
        },
    );

    assert_eq!(
        result
            .overlay_descriptors
            .iter()
            .map(|descriptor| descriptor.marker_type)
            .collect::<Vec<_>>(),
        vec![
            RuleOverlayMarkerType::Aspect,
            RuleOverlayMarkerType::Rune,
            RuleOverlayMarkerType::Sigil,
            RuleOverlayMarkerType::Trade
        ]
    );
    let trade = result
        .overlay_descriptors
        .iter()
        .find(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Trade);
    assert_eq!(
        trade.map(|descriptor| descriptor.shape),
        Some(RuleOverlayShape::TextPanel)
    );
    assert_eq!(
        trade.and_then(|descriptor| descriptor.text.as_deref()),
        Some("5b")
    );
    assert_eq!(
        trade.map(|descriptor| descriptor.geometry),
        Some(rect(100, 650, 320, 50))
    );
}

#[test]
fn affix_overlay_color_is_not_overridden_by_unique_power_marker() {
    let mut result = empty_rule_result();
    result.matched_affixes.push(RuleAffixMatch {
        affix_id: "AttackSpeed".to_string(),
        label: Some("Attack Speed".to_string()),
        color: "Green".to_string(),
        value_text: Some("12.0%".to_string()),
        source_text: "Attack Speed +12.0%".to_string(),
        row_id: Some(AffixRowId(0)),
        required_by: vec!["Pinned baseline".to_string()],
        is_greater_required: false,
        is_any_type_required: false,
        value_status: RuleCheck::passed(
            RuleCheckKind::MinimalAffixValue,
            Some(12.0),
            Some(10.0),
            RuleSourceNote::new(
                "D4Companion.Services/ScreenProcessHandler.cs",
                "ScreenProcessHandler.UpdateItemAffixAreasWithOcrResults",
                "affix value ok",
            ),
        ),
        tuning_prisms: Vec::new(),
        source: RuleSourceNote::new(
            "D4Companion.Services/AffixManager.cs",
            "AffixManager.GetAffix",
            "matched affix",
        ),
        column_offset: 0,
    });
    result.matched_power = Some(RulePowerMatch {
        power_id: "Unique_HarlequinCrest".to_string(),
        power_kind: RulePowerKind::Unique,
        source: RuleSourceNote::new(
            "D4Companion.Services/AffixManager.cs",
            "AffixManager.GetUnique",
            "matched unique",
        ),
    });

    apply_overlay_descriptors(
        &mut result,
        &RuleOverlaySettings {
            marker_mode: RuleOverlayMarkerMode::ShowAll,
            is_minimal_affix_value_filter_enabled: true,
            is_trade_overlay_enabled: false,
            item_power_limit_check_ok: true,
            is_tuning_prism_hints_enabled: false,
        },
        &RuleOverlayLayout {
            tooltip_rect: rect(100, 200, 320, 500),
            affix_locations: vec![affix_location(0, rect(120, 240, 20, 20), Some("AS"))],
            power_location: Some(RuleIdOverlayLocation {
                id: "Unique_HarlequinCrest".to_string(),
                geometry: rect(120, 650, 20, 20),
                color: "Orange".to_string(),
                text: Some("Shako".to_string()),
            }),
            power_locations: Vec::new(),
            rune_locations: Vec::new(),
            sigil_location: None,
            sigil_locations: Vec::new(),
        },
    );

    let affix = result
        .overlay_descriptors
        .iter()
        .find(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Affix)
        .unwrap_or_else(|| panic!("affix descriptor missing"));
    let unique = result
        .overlay_descriptors
        .iter()
        .find(|descriptor| descriptor.marker_type == RuleOverlayMarkerType::Unique)
        .unwrap_or_else(|| panic!("unique descriptor missing"));
    assert_eq!(affix.color, "Green");
    assert_eq!(unique.color, "Orange");
}

#[test]
fn overlay_descriptors_cover_unique_and_block_non_trade_when_item_power_fails() {
    let mut result = empty_rule_result();
    result.matched_power = Some(RulePowerMatch {
        power_id: "Unique_HarlequinCrest".to_string(),
        power_kind: RulePowerKind::Unique,
        source: RuleSourceNote::new(
            "D4Companion.Services/AffixManager.cs",
            "AffixManager.GetUnique",
            "unique match",
        ),
    });
    result.trade_marker = Some(RuleTradeMarker {
        trade_item_id: "trade-unique".to_string(),
        value_text: "10b".to_string(),
        source: RuleSourceNote::new(
            "D4Companion.Services/TradeItemManager.cs",
            "TradeItemManager.FindTradeItem",
            "trade item type and affix match",
        ),
    });

    apply_overlay_descriptors(
        &mut result,
        &RuleOverlaySettings {
            marker_mode: RuleOverlayMarkerMode::ShowAll,
            is_minimal_affix_value_filter_enabled: false,
            is_trade_overlay_enabled: true,
            item_power_limit_check_ok: false,
            is_tuning_prism_hints_enabled: false,
        },
        &RuleOverlayLayout {
            tooltip_rect: rect(100, 200, 320, 500),
            affix_locations: Vec::new(),
            power_location: Some(RuleIdOverlayLocation {
                id: "Unique_HarlequinCrest".to_string(),
                geometry: rect(120, 650, 20, 20),
                color: "Orange".to_string(),
                text: None,
            }),
            power_locations: Vec::new(),
            rune_locations: Vec::new(),
            sigil_location: None,
            sigil_locations: Vec::new(),
        },
    );

    assert_eq!(
        result
            .overlay_descriptors
            .iter()
            .map(|descriptor| descriptor.marker_type)
            .collect::<Vec<_>>(),
        vec![RuleOverlayMarkerType::Trade]
    );
}

#[test]
fn overlay_descriptor_preserves_sigil_dungeon_tier_text() {
    let mut result = empty_rule_result();
    result.matched_sigil = Some("Sigil_Pit".to_string());

    apply_overlay_descriptors(
        &mut result,
        &RuleOverlaySettings {
            marker_mode: RuleOverlayMarkerMode::ShowAll,
            is_minimal_affix_value_filter_enabled: false,
            is_trade_overlay_enabled: false,
            item_power_limit_check_ok: true,
            is_tuning_prism_hints_enabled: false,
        },
        &RuleOverlayLayout {
            tooltip_rect: rect(100, 200, 320, 500),
            affix_locations: Vec::new(),
            power_location: None,
            power_locations: Vec::new(),
            rune_locations: Vec::new(),
            sigil_location: Some(RuleIdOverlayLocation {
                id: "Sigil_Pit".to_string(),
                geometry: rect(160, 700, 20, 20),
                color: "Teal".to_string(),
                text: Some("S".to_string()),
            }),
            sigil_locations: Vec::new(),
        },
    );

    assert_eq!(
        result.overlay_descriptors[0].marker_type,
        RuleOverlayMarkerType::Sigil
    );
    assert_eq!(result.overlay_descriptors[0].text.as_deref(), Some("S"));
}

#[test]
fn live_overlay_descriptor_builder_combines_rule_and_paragon_sources() {
    let mut rule_result = empty_rule_result();
    rule_result.matched_affixes.push(RuleAffixMatch {
        affix_id: "Affix_AttackSpeed".to_string(),
        label: Some("Attack Speed".to_string()),
        color: "Green".to_string(),
        value_text: Some("+10.0%".to_string()),
        source_text: "+10.0% Attack Speed".to_string(),
        row_id: Some(AffixRowId(0)),
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
    });

    let descriptors = build_live_overlay_descriptors(
        Some((
            &rule_result,
            &RuleOverlaySettings {
                marker_mode: RuleOverlayMarkerMode::ShowAll,
                is_minimal_affix_value_filter_enabled: false,
                is_trade_overlay_enabled: false,
                item_power_limit_check_ok: true,
                is_tuning_prism_hints_enabled: false,
            },
            &RuleOverlayLayout {
                tooltip_rect: rect(100, 200, 320, 500),
                affix_locations: vec![RuleAffixOverlayLocation {
                    row_id: Some(AffixRowId(0)),
                    geometry: rect(120, 240, 20, 20),
                    text: Some("AS".to_string()),
                }],
                power_location: None,
                power_locations: Vec::new(),
                rune_locations: Vec::new(),
                sigil_location: None,
                sigil_locations: Vec::new(),
            },
        )),
        Some((
            &ParagonOverlayState {
                build_name: "Whirlwind".to_string(),
                routes: vec![ImportedParagonRoute {
                    step_name: Some("Step 1".to_string()),
                    boards: vec![sample_paragon_board(
                        "Starting Board",
                        "Marshal",
                        "0°",
                        &[0, 1, 2],
                    )],
                }],
                current_route_index: 0,
                current_board_index: Some(0),
                current_board_name: Some("Starting Board".to_string()),
                mode: ParagonOverlayMode::Full,
            },
            &default_paragon_overlay_layout(rect(0, 0, 1920, 1080)),
        )),
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
    );

    assert!(descriptors
        .iter()
        .any(|descriptor| { matches!(descriptor.target, RuleOverlayTarget::Affix { .. }) }));
    assert!(descriptors.iter().any(|descriptor| {
        matches!(
            descriptor.target,
            RuleOverlayTarget::Paragon {
                kind: ParagonOverlayElementKind::BuildPanel,
                ..
            }
        )
    }));
}

#[test]
fn live_overlay_descriptors_stay_inside_a_secondary_monitor_window() {
    // A game window on a second monitor, captured live at
    // `1920x1080+1920+0`: the overlay surface starts at that window's
    // screen origin and covers exactly the window frame, so descriptor
    // geometry must stay window-relative. Adding the screen origin to it
    // drew every rule marker 1920px left of the surface and left the
    // overlay empty while a tooltip was up.
    let window = rect(1920, 0, 1920, 1080);
    let mut rule_result = empty_rule_result();
    rule_result.matched_affixes.push(RuleAffixMatch {
        affix_id: "Affix_AttackSpeed".to_string(),
        label: Some("Attack Speed".to_string()),
        color: "Green".to_string(),
        value_text: Some("+10.0%".to_string()),
        source_text: "+10.0% Attack Speed".to_string(),
        row_id: Some(AffixRowId(0)),
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
    });
    // The live app ran with the trade overlay on and a matched trade
    // item, so the tooltip-anchored panel is part of the surface check.
    rule_result.trade_marker = Some(RuleTradeMarker {
        trade_item_id: "Trade_Helm".to_string(),
        value_text: "10M".to_string(),
        source: RuleSourceNote::new("test", "test", "test"),
    });

    let descriptors = build_live_overlay_descriptors(
        Some((
            &rule_result,
            &RuleOverlaySettings {
                marker_mode: RuleOverlayMarkerMode::ShowAll,
                is_minimal_affix_value_filter_enabled: false,
                is_trade_overlay_enabled: true,
                item_power_limit_check_ok: true,
                is_tuning_prism_hints_enabled: false,
            },
            &RuleOverlayLayout {
                // The live values measured on the running game: the
                // detected tooltip crop and an OCR row inside it.
                tooltip_rect: rect(1094, 0, 375, 717),
                affix_locations: vec![RuleAffixOverlayLocation {
                    row_id: Some(AffixRowId(0)),
                    geometry: rect(1114, 240, 20, 20),
                    text: Some("AS".to_string()),
                }],
                power_location: None,
                power_locations: Vec::new(),
                rune_locations: Vec::new(),
                sigil_location: None,
                sigil_locations: Vec::new(),
            },
        )),
        Some((
            &ParagonOverlayState {
                build_name: "Whirlwind".to_string(),
                routes: vec![ImportedParagonRoute {
                    step_name: Some("Step 1".to_string()),
                    boards: vec![sample_paragon_board(
                        "Starting Board",
                        "Marshal",
                        "0°",
                        &[0, 1, 2],
                    )],
                }],
                current_route_index: 0,
                current_board_index: Some(0),
                current_board_name: Some("Starting Board".to_string()),
                mode: ParagonOverlayMode::Full,
            },
            &default_paragon_overlay_layout(window),
        )),
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
    );

    let affix_marker = descriptors
        .iter()
        .find(|descriptor| matches!(descriptor.target, RuleOverlayTarget::Affix { .. }))
        .unwrap_or_else(|| panic!("affix marker missing"));
    // The radius-10 circle centered on the anchor column at the row's
    // vertical center.
    assert_eq!(affix_marker.geometry, rect(1104, 240, 20, 20));
    let trade_panel = descriptors
        .iter()
        .find(|descriptor| matches!(descriptor.target, RuleOverlayTarget::Trade))
        .unwrap_or_else(|| panic!("trade panel missing"));
    assert!(window_contains(window, trade_panel.geometry));

    // Both producers — the runtime's window-relative rule rects and the
    // paragon panels the layout composes in window-local coordinates —
    // land inside the surface the overlay window covers.
    for descriptor in &descriptors {
        assert!(
            window_contains(window, descriptor.geometry),
            "{:?} escapes the overlay surface: {:?}",
            descriptor.marker_type,
            descriptor.geometry,
        );
    }
}

fn affix_location(
    row_id: u32,
    geometry: d4lc_core::Rect,
    text: Option<&str>,
) -> RuleAffixOverlayLocation {
    RuleAffixOverlayLocation {
        row_id: Some(AffixRowId(row_id)),
        geometry,
        text: text.map(ToOwned::to_owned),
    }
}

fn empty_rule_result() -> RuleEvaluationResult {
    let source = RuleSourceNote::new("test", "test", "test");
    RuleEvaluationResult {
        final_decision: RuleDecision::Neutral,
        final_source: None,
        matched_affixes: Vec::new(),
        missing_affixes: Vec::new(),
        item_power_status: RuleCheck::not_evaluated(RuleCheckKind::ItemPowerLimit, source.clone()),
        minimal_value_status: RuleCheck::not_evaluated(RuleCheckKind::MinimalAffixValue, source),
        matched_power: None,
        matched_powers: Vec::new(),
        matched_runes: Vec::new(),
        matched_sigil: None,
        matched_sigils: Vec::new(),
        trade_marker: None,
        overlay_descriptors: Vec::new(),
        affix_traces: Vec::new(),
        explanation: RuleExplanation {
            active_preset_names: Vec::new(),
            matched_build_names: Vec::new(),
            selected_build_name: None,
            item_type: None,
            rarity: None,
            branches: Vec::new(),
        },
    }
}

/// Whether a window-relative rect fits inside the captured window frame
/// the overlay surface covers.
fn window_contains(window: d4lc_core::Rect, geometry: d4lc_core::Rect) -> bool {
    geometry.x >= 0
        && geometry.y >= 0
        && geometry.x.saturating_add(geometry.width as i32) <= window.width as i32
        && geometry.y.saturating_add(geometry.height as i32) <= window.height as i32
}

#[test]
fn upstream_red_is_compared_by_color_value() {
    for red in ["Red", "red", "#FF0000", "#ff0000", "#FFFF0000", "#F00"] {
        assert!(is_upstream_red(red), "{red}");
    }
    for other in ["Green", "#80FF0000", "#FE0000", "Maroon", "", "not a color"] {
        assert!(!is_upstream_red(other), "{other}");
    }
}
