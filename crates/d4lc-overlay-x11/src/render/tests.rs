use super::*;
use crate::test_support::*;
use crate::*;

/// Preset and default colors arrive as hex strings; they must keep
/// their color instead of falling back to white.
#[test]
fn color_pixel_parses_stored_hex_colors() {
    use super::color_pixel;
    assert_eq!(color_pixel("#008000"), 0xFF008000);
    assert_eq!(color_pixel("#FF008000"), 0xFF008000);
    assert_eq!(color_pixel("#80FF0000"), 0x80FF0000);
    assert_eq!(color_pixel("#0f0"), 0xFF00FF00);
    assert_eq!(color_pixel("red"), 0xFFFF4040);
    assert_eq!(color_pixel("#12345"), 0xFFFFFFFF);
}

#[test]
fn render_plan_sorts_z_order_and_preserves_descriptor_colors() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    overlay
        .update(OverlayFrame {
            sequence: 8,
            tooltip_found: true,
            hotpath_ms: 12,
            cursor_window_relative: None,
            rule_descriptors: vec![
                d4lc_rules::RuleOverlayDescriptor {
                    target: d4lc_rules::RuleOverlayTarget::Trade,
                    marker_type: d4lc_rules::RuleOverlayMarkerType::Trade,
                    shape: d4lc_rules::RuleOverlayShape::TextPanel,
                    geometry: Rect {
                        x: 120,
                        y: 640,
                        width: 320,
                        height: 50,
                    },
                    color: "background".into(),
                    text: Some("5b".to_string()),
                    text_origin: None,
                    z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
                    geometry_source: "tooltip_bottom_trade_panel".into(),
                    source: d4lc_rules::RuleSourceNote::new(
                        "D4Companion.Services/OverlayHandler.cs",
                        "OverlayHandler.DrawGraphicsTrading",
                        "trade value text panel",
                    ),
                },
                d4lc_rules::RuleOverlayDescriptor {
                    target: d4lc_rules::RuleOverlayTarget::Power {
                        power_id: "Unique_HarlequinCrest".to_string(),
                        power_kind: d4lc_rules::RulePowerKind::Unique,
                    },
                    marker_type: d4lc_rules::RuleOverlayMarkerType::Unique,
                    shape: d4lc_rules::RuleOverlayShape::Circle,
                    geometry: Rect {
                        x: 140,
                        y: 260,
                        width: 20,
                        height: 20,
                    },
                    color: "Green".into(),
                    text: Some("Shako".to_string()),
                    text_origin: None,
                    z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
                    geometry_source: "full_window_power_marker_geometry".into(),
                    source: d4lc_rules::RuleSourceNote::new(
                        "D4Companion.Services/OverlayHandler.cs",
                        "OverlayHandler.DrawGraphicsAspects",
                        "aspect or unique marker",
                    ),
                },
            ]
            .into(),
            text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
        })
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));

    let render_ops = overlay.last_render_ops();
    assert_eq!(
        render_ops[0].marker_type,
        d4lc_rules::RuleOverlayMarkerType::Unique
    );
    assert_eq!(render_ops[0].color, "Green");
    assert_eq!(
        render_ops.last().map(|operation| operation.marker_type),
        Some(d4lc_rules::RuleOverlayMarkerType::Trade)
    );
    assert!(render_ops
        .iter()
        .any(|operation| operation.text.as_deref() == Some("5b")));
    let trade_panel = render_ops
        .iter()
        .find(|operation| operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Trade)
        .unwrap_or_else(|| panic!("trade render op missing"));
    assert_eq!(
        trade_panel.geometry,
        Rect {
            x: 120,
            y: 640,
            width: 320,
            height: 50,
        }
    );
    assert_eq!(
        trade_panel.geometry_source.as_deref(),
        Some("tooltip_bottom_trade_panel")
    );
    assert_eq!(
        trade_panel
            .source
            .as_ref()
            .map(|source| source.upstream_method.as_ref()),
        Some("OverlayHandler.DrawGraphicsTrading")
    );
}

#[test]
fn render_plan_keeps_window_relative_geometry_on_a_secondary_monitor() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    // The game window on a second monitor: the overlay surface starts at
    // the window's screen origin, so window-relative descriptor geometry
    // is already surface-local and must not be shifted by that origin
    // again; otherwise a marker at window x=1094 (a real tooltip crop)
    // lands off the surface and stays undrawn.
    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    overlay
        .update(OverlayFrame {
            sequence: 9,
            tooltip_found: false,
            hotpath_ms: 10,
            cursor_window_relative: None,
            rule_descriptors: vec![d4lc_rules::RuleOverlayDescriptor {
                target: d4lc_rules::RuleOverlayTarget::Affix {
                    affix_id: "Affix_AttackSpeed".to_string(),
                    row_id: Some(d4lc_core::AffixRowId(1)),
                },
                marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
                shape: d4lc_rules::RuleOverlayShape::Rectangle,
                geometry: Rect {
                    x: 1094,
                    y: 240,
                    width: 20,
                    height: 20,
                },
                color: "Gold".into(),
                text: Some("AS".to_string()),
                text_origin: None,
                z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
                geometry_source: "full_window_affix_marker_geometry".into(),
                source: d4lc_rules::RuleSourceNote::new(
                    "D4Companion.Services/OverlayHandler.cs",
                    "OverlayHandler.DrawGraphicsAffixes",
                    "affix marker mode and shape selection",
                ),
            }]
            .into(),
            text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
        })
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));

    let render_ops = overlay.last_render_ops();
    assert_eq!(
        render_ops[0].geometry,
        Rect {
            x: 1094,
            y: 240,
            width: 20,
            height: 20,
        }
    );
    assert_eq!(render_ops[0].color, "Gold");
}

#[test]
fn paragon_render_plan_consumes_descriptor_geometry_without_x11() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 100,
            y: 200,
            width: 1280,
            height: 720,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    let paragon_layout = d4lc_rules::ParagonOverlayLayout {
        window_geometry: Rect {
            x: 100,
            y: 200,
            width: 1280,
            height: 720,
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
        collapsed_left_offset: -50,
        collapsed_top_offset: 30,
    };
    let full_descriptors = d4lc_rules::build_paragon_overlay_descriptors(
        &d4lc_rules::ParagonOverlayState {
            build_name: "Whirlwind Arsenal".to_string(),
            routes: vec![d4lc_rules::ImportedParagonRoute {
                step_name: Some("Endgame".to_string()),
                boards: vec![
                    d4lc_rules::ImportedParagonBoard {
                        index: 0,
                        name: "Decimator".to_string(),
                        glyph: "Marshal".to_string(),
                        rotation: "0°".to_string(),
                        active_nodes: vec![0, 1, 2],
                    },
                    d4lc_rules::ImportedParagonBoard {
                        index: 0,
                        name: "Carnage".to_string(),
                        glyph: "Ambidextrous".to_string(),
                        rotation: "90°".to_string(),
                        active_nodes: vec![20, 39],
                    },
                ],
            }],
            current_route_index: 0,
            current_board_index: Some(1),
            current_board_name: None,
            mode: d4lc_rules::ParagonOverlayMode::Full,
        },
        &paragon_layout,
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
    );
    let full_ops = collect_render_ops(&mut overlay, 14, full_descriptors.clone());
    assert!(full_ops
        .iter()
        .any(|operation| operation.text.as_deref() == Some("Whirlwind Arsenal")));
    assert!(full_ops.iter().any(|operation| {
        operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Paragon
            && operation.geometry.x == 0
            && operation.geometry.y == 100
            && operation.geometry.height == 50
    }));

    let collapsed_descriptors = d4lc_rules::build_paragon_overlay_descriptors(
        &d4lc_rules::ParagonOverlayState {
            build_name: "Whirlwind Arsenal".to_string(),
            routes: vec![d4lc_rules::ImportedParagonRoute {
                step_name: None,
                boards: vec![d4lc_rules::ImportedParagonBoard {
                    index: 0,
                    name: "Decimator".to_string(),
                    glyph: "Marshal".to_string(),
                    rotation: "90°".to_string(),
                    active_nodes: vec![20, 39],
                }],
            }],
            current_route_index: 0,
            current_board_index: None,
            current_board_name: None,
            mode: d4lc_rules::ParagonOverlayMode::Collapsed,
        },
        &paragon_layout,
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
    );
    let collapsed_ops = collect_render_ops(&mut overlay, 15, collapsed_descriptors.clone());
    assert!(collapsed_ops.iter().any(|operation| {
        operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Paragon
            && operation.geometry.x == 1015
            && operation.geometry.y == 280
            && operation.geometry.width >= 10
            && operation.geometry.height >= 10
    }));

    let warning_descriptors = d4lc_rules::build_paragon_overlay_descriptors(
        &d4lc_rules::ParagonOverlayState {
            build_name: "Whirlwind Arsenal".to_string(),
            routes: Vec::new(),
            current_route_index: 0,
            current_board_index: None,
            current_board_name: None,
            mode: d4lc_rules::ParagonOverlayMode::Full,
        },
        &paragon_layout,
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
    );
    let warning_ops = collect_render_ops(&mut overlay, 16, warning_descriptors.clone());
    assert!(warning_ops
        .iter()
        .any(|operation| { operation.text.as_deref() == Some("No paragon boards available.") }));

    write_render_trace(
        "paragon-render-trace.json",
        &[
            ("full", &full_descriptors, &full_ops),
            ("collapsed", &collapsed_descriptors, &collapsed_ops),
            ("warning", &warning_descriptors, &warning_ops),
        ],
    );
}

#[test]
fn tuning_prism_descriptors_render_as_circle_ops() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    let prism_source = d4lc_rules::RuleSourceNote::new(
        "D4Companion.Services/OverlayHandler.cs",
        "OverlayHandler.DrawGraphicsTuningPrisms",
        "tuning prism hint circle for matched affix",
    );
    let prism_descriptor = |offset_x: i32, color: &str| d4lc_rules::RuleOverlayDescriptor {
        target: d4lc_rules::RuleOverlayTarget::Affix {
            affix_id: "Affix_AttackSpeed".to_string(),
            row_id: Some(d4lc_core::AffixRowId(0)),
        },
        marker_type: d4lc_rules::RuleOverlayMarkerType::TuningPrism,
        shape: d4lc_rules::RuleOverlayShape::Circle,
        geometry: Rect {
            x: offset_x,
            y: 240,
            width: 20,
            height: 20,
        },
        color: color.into(),
        text: None,
        text_origin: None,
        z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
        geometry_source: "affix_tuning_prism_circle_geometry".into(),
        source: prism_source.clone(),
    };
    let affix_marker = d4lc_rules::RuleOverlayDescriptor {
        target: d4lc_rules::RuleOverlayTarget::Affix {
            affix_id: "Affix_AttackSpeed".to_string(),
            row_id: Some(d4lc_core::AffixRowId(0)),
        },
        marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
        shape: d4lc_rules::RuleOverlayShape::Circle,
        geometry: Rect {
            x: 120,
            y: 240,
            width: 20,
            height: 20,
        },
        color: "Green".into(),
        text: None,
        text_origin: None,
        z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
        geometry_source: "full_window_affix_marker_geometry".into(),
        source: d4lc_rules::RuleSourceNote::new(
            "D4Companion.Services/OverlayHandler.cs",
            "OverlayHandler.DrawGraphicsAffixes",
            "normal affix circle marker",
        ),
    };
    let trade_panel = d4lc_rules::RuleOverlayDescriptor {
        target: d4lc_rules::RuleOverlayTarget::Trade,
        marker_type: d4lc_rules::RuleOverlayMarkerType::Trade,
        shape: d4lc_rules::RuleOverlayShape::TextPanel,
        geometry: Rect {
            x: 120,
            y: 640,
            width: 320,
            height: 50,
        },
        color: "background".into(),
        text: Some("5b".to_string()),
        text_origin: None,
        z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
        geometry_source: "tooltip_bottom_trade_panel".into(),
        source: d4lc_rules::RuleSourceNote::new(
            "D4Companion.Services/OverlayHandler.cs",
            "OverlayHandler.DrawGraphicsTrading",
            "trade value text panel",
        ),
    };

    let descriptors = vec![
        trade_panel,
        prism_descriptor(390, "Red"),
        prism_descriptor(400, "LightBlue"),
        affix_marker,
    ];
    let render_ops = collect_render_ops(&mut overlay, 17, descriptors.clone());

    // Prism hints render as real circle ops carrying the descriptor's
    // window-relative geometry verbatim. The regular affix marker adds a
    // third circle op (asserted below).
    let circle_ops: Vec<_> = render_ops
        .iter()
        .filter(|operation| operation.kind == OverlayRenderKind::Circle)
        .collect();
    assert_eq!(circle_ops.len(), 3);
    assert_eq!(circle_ops[0].color, "Red");
    assert_eq!(
        circle_ops[0].geometry,
        Rect {
            x: 390,
            y: 240,
            width: 20,
            height: 20
        }
    );
    assert_eq!(circle_ops[1].color, "LightBlue");
    assert_eq!(
        circle_ops[1].geometry,
        Rect {
            x: 400,
            y: 240,
            width: 20,
            height: 20
        }
    );
    // The two prism circles keep the prism provenance and z-order
    // discipline; the third circle op is the affix marker asserted
    // below.
    assert!(circle_ops[..2].iter().all(|operation| {
        operation.marker_type == d4lc_rules::RuleOverlayMarkerType::TuningPrism
            && operation.z_order_group == d4lc_rules::RuleOverlayZOrderGroup::Marker
            && operation.geometry_source.as_deref() == Some("affix_tuning_prism_circle_geometry")
            && operation
                .source
                .as_ref()
                .map(|source| source.upstream_method.as_ref())
                == Some("OverlayHandler.DrawGraphicsTuningPrisms")
    }));

    // The regular affix circle marker renders as a real outline+fill
    // circle over the descriptor's box, verbatim like every other
    // descriptor (the rules crate already derived the box from the row's
    // marker anchor).
    let affix_ops: Vec<_> = render_ops
        .iter()
        .filter(|operation| operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Affix)
        .collect();
    assert_eq!(affix_ops.len(), 1);
    assert_eq!(affix_ops[0].kind, OverlayRenderKind::Circle);
    assert_eq!(affix_ops[0].color, "Green");
    assert_eq!(
        affix_ops[0].geometry,
        Rect {
            x: 120,
            y: 240,
            width: 20,
            height: 20
        }
    );

    // Prism circles stay in the Marker z-order group ahead of text
    // panels, matching the upstream draw order.
    let first_panel_index = render_ops
        .iter()
        .position(|operation| {
            operation.z_order_group == d4lc_rules::RuleOverlayZOrderGroup::TextPanel
        })
        .unwrap_or_else(|| panic!("trade panel op missing"));
    let last_circle_index = render_ops
        .iter()
        .rposition(|operation| operation.kind == OverlayRenderKind::Circle)
        .unwrap_or_else(|| panic!("circle op missing"));
    assert!(last_circle_index < first_panel_index);

    write_render_trace(
        "tuning-prism-render-trace.json",
        &[("hints_enabled", &descriptors, &render_ops)],
    );
}

#[test]
fn affix_marker_shapes_render_upstream_geometry() {
    // One descriptor model for both backends: `d4lc-rules` turns the
    // row's marker anchor into the drawn box, the renderer draws it
    // verbatim. Upstream DrawGraphicsAffixes (OverlayHandler.cs
    // 251-319) centers a radius-10 circle on the marker column at the
    // row's vertical center, and hangs the 20x20 triangle and rectangle
    // (any-type, below the minimal value) from the row top — the text
    // width never enters a marker box.
    let menu = OverlayMenuState::from_config(&OverlayConfig {
        window_geometry: Rect {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    });
    let source = d4lc_rules::RuleSourceNote::new("test", "test", "test");
    let affix = |row: u32, color: &str, greater: bool, any_type: bool, below: bool| {
        d4lc_rules::RuleAffixMatch {
            affix_id: format!("Affix_{row}"),
            label: None,
            color: color.into(),
            value_text: None,
            source_text: String::new(),
            row_id: Some(d4lc_core::AffixRowId(row)),
            required_by: vec!["Preset".to_string()],
            is_greater_required: greater,
            is_any_type_required: any_type,
            value_status: if below {
                d4lc_rules::RuleCheck::failed(
                    d4lc_rules::RuleCheckKind::MinimalAffixValue,
                    Some(1.0),
                    Some(5.0),
                    source.clone(),
                )
            } else {
                d4lc_rules::RuleCheck::not_evaluated(
                    d4lc_rules::RuleCheckKind::MinimalAffixValue,
                    source.clone(),
                )
            },
            tuning_prisms: Vec::new(),
            column_offset: 0,
            source: source.clone(),
        }
    };
    let result = d4lc_rules::RuleEvaluationResult {
        final_decision: d4lc_rules::RuleDecision::Wanted,
        final_source: None,
        matched_affixes: vec![
            affix(0, "Green", false, false, false),
            affix(1, "Purple", true, false, false),
            affix(2, "Blue", false, true, false),
            affix(3, "Yellow", false, false, true),
        ],
        missing_affixes: Vec::new(),
        item_power_status: d4lc_rules::RuleCheck::not_evaluated(
            d4lc_rules::RuleCheckKind::ItemPowerLimit,
            source.clone(),
        ),
        minimal_value_status: d4lc_rules::RuleCheck::not_evaluated(
            d4lc_rules::RuleCheckKind::MinimalAffixValue,
            source.clone(),
        ),
        matched_power: None,
        matched_powers: Vec::new(),
        matched_runes: Vec::new(),
        matched_sigil: None,
        matched_sigils: Vec::new(),
        trade_marker: None,
        overlay_descriptors: Vec::new(),
        explanation: d4lc_rules::RuleExplanation {
            active_preset_names: Vec::new(),
            matched_build_names: Vec::new(),
            selected_build_name: None,
            item_type: None,
            rarity: None,
            branches: Vec::new(),
        },
        affix_traces: Vec::new(),
    };
    let anchor = |row: u32, y: i32| d4lc_rules::RuleAffixOverlayLocation {
        row_id: Some(d4lc_core::AffixRowId(row)),
        // Tooltip left edge 130, affix marker row `y`, 22 px high.
        geometry: Rect {
            x: 130,
            y,
            width: 18,
            height: 22,
        },
        text: None,
    };
    let layout = d4lc_rules::RuleOverlayLayout {
        tooltip_rect: Rect {
            x: 130,
            y: 200,
            width: 400,
            height: 400,
        },
        affix_locations: vec![
            anchor(0, 240),
            anchor(1, 300),
            anchor(2, 360),
            anchor(3, 420),
        ],
        power_location: None,
        power_locations: Vec::new(),
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: Vec::new(),
    };
    let settings = d4lc_rules::RuleOverlaySettings {
        marker_mode: d4lc_rules::RuleOverlayMarkerMode::ShowAll,
        is_minimal_affix_value_filter_enabled: true,
        is_trade_overlay_enabled: false,
        item_power_limit_check_ok: true,
        is_tuning_prism_hints_enabled: false,
    };
    let descriptors = d4lc_rules::build_overlay_descriptors(&result, &settings, &layout);

    let render_ops = build_render_ops(&descriptors, &menu, DEFAULT_OVERLAY_FONT_PX, Instant::now());
    let drawn: Vec<_> = render_ops
        .iter()
        .map(|operation| {
            (
                operation.kind,
                operation.color.name().unwrap_or(""),
                operation.geometry,
            )
        })
        .collect();
    let square = |x: i32, y: i32| Rect {
        x,
        y,
        width: 20,
        height: 20,
    };
    assert_eq!(
        drawn,
        vec![
            (OverlayRenderKind::Circle, "Green", square(120, 241)),
            (OverlayRenderKind::Triangle, "Purple", square(120, 300)),
            (
                OverlayRenderKind::OutlineFillRect { stroke: 1 },
                "Blue",
                square(120, 360)
            ),
            (
                OverlayRenderKind::OutlineFillRect { stroke: 1 },
                "Yellow",
                square(120, 420)
            ),
        ]
    );

    write_render_trace(
        "affix-marker-shape-render-trace.json",
        &[("normal_greater_anytype_minimal", &descriptors, &render_ops)],
    );
}

#[test]
fn shared_marker_forms_and_text_wrap_contract_match_upstream() {
    // The op expansion both backends consume verbatim: the upstream
    // marker forms (`OverlayHandler.cs` 251-318) and the text wrap
    // contract. Circle = normal affix marker, outline+fill rectangle with
    // stroke 1 = any-type / below-minimal entry, the same form with
    // stroke 2 = sigil dungeon marker, triangle = greater condition,
    // outline-only rectangle = paragon tile, filled panel = text panel.
    let descriptor = |shape: d4lc_rules::RuleOverlayShape,
                      marker_type: d4lc_rules::RuleOverlayMarkerType,
                      z_order_group: d4lc_rules::RuleOverlayZOrderGroup| {
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Affix {
                affix_id: "Affix_Contract".to_string(),
                row_id: None,
            },
            marker_type,
            shape,
            geometry: Rect {
                x: 40,
                y: 40,
                width: 20,
                height: 20,
            },
            color: "Gold".into(),
            text: Some("Tier 15".to_string()),
            text_origin: None,
            z_order_group,
            geometry_source: "contract_test".into(),
            source: d4lc_rules::RuleSourceNote::new("test", "test", "test"),
        }
    };
    use d4lc_rules::{RuleOverlayMarkerType as Marker, RuleOverlayShape as Shape};
    let descriptors = vec![
        descriptor(
            Shape::Circle,
            Marker::Affix,
            d4lc_rules::RuleOverlayZOrderGroup::Marker,
        ),
        descriptor(
            Shape::Triangle,
            Marker::Affix,
            d4lc_rules::RuleOverlayZOrderGroup::Marker,
        ),
        descriptor(
            Shape::Rectangle,
            Marker::Affix,
            d4lc_rules::RuleOverlayZOrderGroup::Marker,
        ),
        descriptor(
            Shape::Rectangle,
            Marker::Sigil,
            d4lc_rules::RuleOverlayZOrderGroup::Marker,
        ),
        descriptor(
            Shape::Rectangle,
            Marker::Paragon,
            d4lc_rules::RuleOverlayZOrderGroup::Marker,
        ),
        descriptor(
            Shape::TextPanel,
            Marker::Trade,
            d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
        ),
    ];
    let config = OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 200,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    };
    let menu = OverlayMenuState::from_config(&config);
    let render_ops = build_render_ops(&descriptors, &menu, DEFAULT_OVERLAY_FONT_PX, Instant::now());

    let shapes = render_ops
        .iter()
        .filter(|operation| operation.kind != OverlayRenderKind::Text)
        .map(|operation| (operation.marker_type, operation.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        shapes,
        vec![
            (Marker::Affix, OverlayRenderKind::Circle),
            (Marker::Affix, OverlayRenderKind::Triangle),
            (
                Marker::Affix,
                OverlayRenderKind::OutlineFillRect { stroke: 1 }
            ),
            (
                Marker::Sigil,
                OverlayRenderKind::OutlineFillRect { stroke: 2 }
            ),
            (Marker::Paragon, OverlayRenderKind::OutlineRect),
            (Marker::Trade, OverlayRenderKind::FilledRect),
        ]
    );

    // The wrap contract travels with the op: a marker caption is one
    // line, a panel text wraps inside its own box.
    let text_ops = render_ops
        .iter()
        .filter(|operation| operation.kind == OverlayRenderKind::Text)
        .collect::<Vec<_>>();
    assert_eq!(text_ops.len(), descriptors.len());
    for operation in text_ops {
        let expected = match operation.z_order_group {
            d4lc_rules::RuleOverlayZOrderGroup::Marker => text::TextWrapPolicy::SingleLine,
            d4lc_rules::RuleOverlayZOrderGroup::TextPanel => text::TextWrapPolicy::WrapWords,
        };
        assert_eq!(text_wrap_policy(operation), expected);
    }
}

#[test]
fn descriptors_render_while_menu_unlocked_with_descriptor_colors() {
    // Upstream draws the evaluation markers every frame from the current
    // tooltip (OverlayHandler.DrawGraphicsAffixes); the render-op seam
    // mirrors that by yielding descriptor ops regardless of menu lock.
    // Colors pass through verbatim — the "hide unwanted" red filtering
    // happens in d4lc-rules, never here — and the pre-existing
    // "background" text color mapping is preserved.
    let menu = OverlayMenuState::from_config(&OverlayConfig {
        window_geometry: Rect {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        },
        show_overlay_icon: true,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    });
    assert!(!menu.is_locked());

    let descriptors = vec![
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Affix {
                affix_id: "Affix_AttackSpeed".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
            shape: d4lc_rules::RuleOverlayShape::Rectangle,
            geometry: Rect {
                x: 130,
                y: 240,
                width: 20,
                height: 20,
            },
            color: "Red".into(),
            text: None,
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "full_window_affix_marker_geometry".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAffixes",
                "affix marker mode and shape selection",
            ),
        },
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Trade,
            marker_type: d4lc_rules::RuleOverlayMarkerType::Trade,
            shape: d4lc_rules::RuleOverlayShape::TextPanel,
            geometry: Rect {
                x: 120,
                y: 640,
                width: 320,
                height: 50,
            },
            color: "background".into(),
            text: Some("5b".to_string()),
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            geometry_source: "tooltip_bottom_trade_panel".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsTrading",
                "trade value text panel",
            ),
        },
    ];

    let render_ops = build_render_ops(&descriptors, &menu, DEFAULT_OVERLAY_FONT_PX, Instant::now());

    let affix = render_ops
        .iter()
        .find(|operation| operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Affix)
        .unwrap_or_else(|| panic!("affix marker op missing while menu unlocked"));
    assert_eq!(affix.color, "Red");
    assert_eq!(
        affix.geometry,
        Rect {
            x: 130,
            y: 240,
            width: 20,
            height: 20,
        }
    );
    let trade_ops: Vec<_> = render_ops
        .iter()
        .filter(|operation| operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Trade)
        .collect();
    assert_eq!(trade_ops.len(), 2);
    assert_eq!(trade_ops[0].color, "background");
    assert_eq!(trade_ops[1].kind, OverlayRenderKind::Text);
    assert_eq!(trade_ops[1].color, "White");
    assert_eq!(trade_ops[1].text.as_deref(), Some("5b"));
}

/// Upstream's paragon brushes (`OverlayHandler.cs:897-902`, Goldenrod
/// `Colors.Goldenrod`) resolve through the typed colour, never through the
/// name palette's white fallback.
#[test]
fn paragon_brushes_resolve_to_the_upstream_pixels() {
    use d4lc_rules::{OverlayColor, ParagonColor};
    let pixel = |brush: ParagonColor| overlay_color_pixel(&OverlayColor::Paragon(brush));
    assert_eq!(pixel(ParagonColor::Background), 0xC8191919);
    assert_eq!(pixel(ParagonColor::Border), 0xFF4B4B4B);
    assert_eq!(pixel(ParagonColor::BorderActive), 0xFF14DC50);
    assert_eq!(pixel(ParagonColor::Text), 0xFFC8C8C8);
    assert_eq!(pixel(ParagonColor::Goldenrod), 0xFFDAA520);
    assert_eq!(overlay_color_pixel(&"Gold".into()), 0xFFFFD700);
}

/// A paragon panel draws its text in the `text` brush on the
/// `backgroundTransparent` fill (`OverlayHandler.cs:622`), while the legacy
/// named background panel keeps white text.
#[test]
fn paragon_panel_text_uses_the_text_brush() {
    use d4lc_rules::{OverlayColor, ParagonColor};
    assert_eq!(
        text_color(&ParagonColor::Background.into()),
        OverlayColor::Paragon(ParagonColor::Text)
    );
    assert_eq!(text_color(&"background".into()), "White");
    assert_eq!(
        text_color(&ParagonColor::Goldenrod.into()),
        OverlayColor::Paragon(ParagonColor::Goldenrod)
    );
}

/// A paragon panel's text op starts its line at the descriptor's text
/// origin, unwrapped and at the frame's font size; other text keeps its area
/// placement at the same size.
#[test]
fn paragon_text_ops_draw_an_exact_line_at_the_frame_font_size() {
    let panel = Rect {
        x: 0,
        y: 100,
        width: 139,
        height: 50,
    };
    let descriptor = |origin: Option<d4lc_core::Point>, group| d4lc_rules::RuleOverlayDescriptor {
        target: d4lc_rules::RuleOverlayTarget::Trade,
        marker_type: d4lc_rules::RuleOverlayMarkerType::Paragon,
        shape: d4lc_rules::RuleOverlayShape::TextPanel,
        geometry: panel,
        color: d4lc_rules::ParagonColor::Background.into(),
        text: Some("Bench Build".to_string()),
        text_origin: origin,
        z_order_group: group,
        geometry_source: "test".into(),
        source: d4lc_rules::RuleSourceNote::new("file", "method", "branch"),
    };
    let menu = OverlayMenuState::from_config(&OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    });
    let ops = build_render_ops(
        &[
            descriptor(
                Some(d4lc_core::Point { x: 20, y: 114 }),
                d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            ),
            descriptor(None, d4lc_rules::RuleOverlayZOrderGroup::TextPanel),
        ],
        &menu,
        22,
        Instant::now(),
    );
    let texts: Vec<_> = ops
        .iter()
        .filter(|operation| operation.kind == OverlayRenderKind::Text)
        .collect();
    assert_eq!(texts.len(), 2);

    let exact = texts[0];
    assert_eq!(exact.text_placement, OverlayTextPlacement::Line);
    assert_eq!(exact.font_px, 22);
    assert_eq!(
        exact.geometry,
        Rect {
            x: 20,
            y: 114,
            width: 119,
            height: 22,
        }
    );
    assert_eq!(text_wrap_policy(exact), text::TextWrapPolicy::ExactLine);
    assert_eq!(
        exact.color,
        d4lc_rules::OverlayColor::Paragon(d4lc_rules::ParagonColor::Text)
    );

    let area = texts[1];
    assert_eq!(area.text_placement, OverlayTextPlacement::Area);
    assert_eq!(area.font_px, 22);
    assert_eq!(area.geometry, panel);
    assert_eq!(text_wrap_policy(area), text::TextWrapPolicy::WrapWords);
}

fn plan_descriptor(x: i32) -> d4lc_rules::RuleOverlayDescriptor {
    d4lc_rules::RuleOverlayDescriptor {
        target: d4lc_rules::RuleOverlayTarget::Trade,
        marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
        shape: d4lc_rules::RuleOverlayShape::Circle,
        geometry: Rect {
            x,
            y: 10,
            width: 20,
            height: 20,
        },
        color: "Green".into(),
        text: None,
        text_origin: None,
        z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
        geometry_source: "test".into(),
        source: d4lc_rules::RuleSourceNote::new("file", "method", "branch"),
    }
}

/// The plan reuses the descriptor ops for the same shared list and reports a
/// change only when the drawn ops differ.
#[test]
fn render_plan_reuses_ops_for_the_same_list_and_reports_changes() {
    let mut menu = OverlayMenuState::from_config(&OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        },
        show_overlay_icon: true,
        overlay_icon_pos_x: 500,
        overlay_icon_pos_y: 500,
        menu_locked: true,
    });
    let now = Instant::now();
    let list: Arc<[d4lc_rules::RuleOverlayDescriptor]> = vec![plan_descriptor(0)].into();
    let mut plan = RenderPlan::default();

    // First refresh builds everything: the marker plus the menu ops.
    assert!(plan.refresh(&list, &menu, DEFAULT_OVERLAY_FONT_PX, now));
    let first = plan.ops().to_vec();
    assert_eq!(first.len(), 1 + build_menu_render_ops(&menu, 18, now).len());

    // The same list, menu and size: nothing changed, same ops.
    assert!(!plan.refresh(&list, &menu, DEFAULT_OVERLAY_FONT_PX, now));
    assert_eq!(plan.ops(), first);

    // A new list with equal content draws identically: no change.
    let equal: Arc<[d4lc_rules::RuleOverlayDescriptor]> = vec![plan_descriptor(0)].into();
    assert!(!plan.refresh(&equal, &menu, DEFAULT_OVERLAY_FONT_PX, now));

    // A different list or font size changes the ops.
    let moved: Arc<[d4lc_rules::RuleOverlayDescriptor]> = vec![plan_descriptor(40)].into();
    assert!(plan.refresh(&moved, &menu, DEFAULT_OVERLAY_FONT_PX, now));
    assert_eq!(plan.ops()[0].geometry.x, 40);
    assert!(plan.refresh(&moved, &menu, 22, now));

    // With the same list only the menu ops follow the menu state, and they
    // match a full rebuild.
    menu.apply_settings(
        Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        },
        false,
        500,
        500,
        None,
    );
    let changed = plan.refresh(&moved, &menu, 22, now);
    assert_eq!(
        plan.ops(),
        build_render_ops(&moved, &menu, 22, now).as_slice()
    );
    assert!(changed);
}
