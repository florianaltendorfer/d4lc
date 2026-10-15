use super::*;
use crate::app_state::overlay_projection::evaluate_overlay_descriptor_state;
use crate::app_state::test_support::{
    apply_two_row_item_event, apply_two_row_item_event_with_type, seed_attack_speed_preset,
};
use d4lc_rules::RuleDecision;

/// A ring whose three affix rows come from three OCR segments, each at
/// segment-local line 0, with marker rows at y 300, 340 and 380 (marker
/// column 580). Row 0 is the Alpha affix, row 1 no preset lists, row 2
/// the affix every build lists.
fn apply_three_segment_ring(state: &mut AppState) {
    const CRIT: &str = "CritChance_To_LowLife;CritChance_To_LowLife_Jewelry";
    const SPEED: &str = "AttackSpeed;AttackSpeed_Jewelry";
    let rows = [(CRIT, 300), ("DamageHealthy", 340), (SPEED, 380)];
    crate::runtime_bridge::apply_runtime_event(
        state,
        &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
            item_name: Some("Band".to_string()),
            parse_status: "Parsed".to_string(),
            item_type: Some("ring".to_string()),
            base_type: Some("Ring".to_string()),
            item_power: Some(800),
            affixes: rows
                .iter()
                .enumerate()
                .map(|(index, (affix_id, _))| d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(index as u32),
                    affix_id: Some((*affix_id).to_string()),
                    label: (*affix_id).to_string(),
                    value_text: Some("10%".to_string()),
                    source_text: format!("+10% {affix_id}"),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                })
                .collect(),
            affix_row_geometry: rows
                .iter()
                .enumerate()
                .map(
                    |(index, (_, y))| d4lc_gui::CurrentItemAffixRowGeometryView {
                        row_id: d4lc_core::AffixRowId(index as u32),
                        source_segment_index: index + 1,
                        source_line_index: 0,
                        global_line_index: index + 1,
                        marker_ordinal: Some(index),
                        text_rect: Rect {
                            x: 604,
                            y: *y + 2,
                            width: 260,
                            height: 18,
                        },
                        affix_area: Rect {
                            x: 600,
                            y: *y,
                            width: 300,
                            height: 40,
                        },
                        marker_anchor: Some(Rect {
                            x: 580,
                            y: *y,
                            width: 18,
                            height: 22,
                        }),
                    },
                )
                .collect(),
            non_affix_rows: Vec::new(),
            tooltip_window_relative: Some(Rect {
                x: 580,
                y: 120,
                width: 400,
                height: 400,
            }),
        },
    );
}

fn ring_preset(state: &mut AppState, name: &str, affix_ids: &[&str], color: &str) {
    state.affix_preset_editor.create_preset(name).unwrap();
    for affix_id in affix_ids {
        state
            .affix_preset_editor
            .edit_entry(
                name,
                d4lc_gui::PresetEntry {
                    kind: d4lc_gui::PresetEntryKind::Affix,
                    id: (*affix_id).to_string(),
                    label: None,
                    item_type: Some("ring".to_string()),
                    color: color.to_string(),
                    is_any_type: false,
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                },
            )
            .unwrap();
    }
}

/// The live evaluation seam: the item event the accepted analysis
/// becomes, the rule projection and the descriptor conversion the live
/// loop draws.
fn live_affix_markers(state: &AppState) -> Vec<(u32, String, Rect)> {
    let projected = evaluate_overlay_descriptor_state(state);
    crate::runtime_bridge::convert_descriptors(projected.as_ref())
        .into_iter()
        .filter_map(|descriptor| match descriptor.target {
            d4lc_rules::RuleOverlayTarget::Affix {
                row_id: Some(row_id),
                ..
            } if descriptor.marker_type == d4lc_rules::RuleOverlayMarkerType::Affix => Some((
                row_id.0,
                descriptor.color.name().unwrap_or_default().to_string(),
                descriptor.geometry,
            )),
            _ => None,
        })
        .collect()
}

/// Rows sharing the local line index 0, Hide-Unwanted and
/// multiple builds. Every remaining marker stays on its own marker row;
/// hidden rows never shift another row's marker.
#[test]
fn hide_unwanted_and_multi_build_keep_every_marker_on_its_row() {
    const CRIT: &str = "CritChance_To_LowLife;CritChance_To_LowLife_Jewelry";
    const SPEED: &str = "AttackSpeed;AttackSpeed_Jewelry";
    let circle = |x: i32, row_top: i32| Rect {
        x: x - 10,
        y: row_top + 11 - 10,
        width: 20,
        height: 20,
    };
    let mut state = AppState::default_startup();
    state.shell.settings.overlay_enabled = true;
    ring_preset(&mut state, "Alpha", &[CRIT, SPEED], "#22C55E");
    ring_preset(&mut state, "Beta", &[SPEED], "#3B82F6");
    ring_preset(&mut state, "Gamma", &[CRIT], "Red");
    apply_three_segment_ring(&mut state);

    // Single build, Hide-Unwanted: the red middle row is hidden and the
    // two wanted rows keep their own marker rows (upstream
    // OverlayHandler.cs:272-273).
    state.affix_preset_editor.selected_preset_name = Some("Alpha".to_string());
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
    assert_eq!(
        live_affix_markers(&state),
        vec![
            (0, "#22C55E".to_string(), circle(580, 300)),
            (2, "#22C55E".to_string(), circle(580, 380)),
        ]
    );
    // Show All draws the red row on its own row as well.
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::ShowAll;
    assert_eq!(
        live_affix_markers(&state),
        vec![
            (0, "#22C55E".to_string(), circle(580, 300)),
            (1, "Red".to_string(), circle(580, 340)),
            (2, "#22C55E".to_string(), circle(580, 380)),
        ]
    );

    // Multi-build (any marker mode): build i draws at 580 + 5 - 20 i on
    // the rows its preset lists; the red Gamma build and unlisted rows
    // draw nothing (OverlayHandler.cs:337, :350).
    state.build_import.multi_builds = ["Alpha", "Beta", "Gamma"]
        .iter()
        .enumerate()
        .map(|(index, name)| MultiBuildRow {
            name: (*name).to_string(),
            index,
            color: ["#22C55E", "#3B82F6", "Red"][index].to_string(),
        })
        .collect();
    state.shell.settings.multi_build_mode_enabled = true;
    let mut markers = live_affix_markers(&state);
    markers.sort_by_key(|(row, _, geometry)| (*row, -geometry.x));
    assert_eq!(
        markers,
        vec![
            (0, "#22C55E".to_string(), circle(585, 300)),
            (2, "#22C55E".to_string(), circle(585, 380)),
            (2, "#3B82F6".to_string(), circle(565, 380)),
        ]
    );

    // Every (build, row) evaluation is traced, red builds included.
    let projected = evaluate_overlay_descriptor_state(&state).expect("rule state");
    let traces = &projected.rule.expect("rule").result.affix_traces;
    assert_eq!(traces.len(), 9);
    let gamma_crit = traces
        .iter()
        .find(|trace| trace.preset_name == "Gamma" && trace.affix_id == CRIT)
        .expect("Gamma trace");
    assert_eq!(gamma_crit.build_column, Some(2));
    assert_eq!(gamma_crit.color, "Red");
    assert!(gamma_crit.is_match());
    let unlisted = traces
        .iter()
        .find(|trace| trace.preset_name == "Beta" && trace.row_id == Some(d4lc_core::AffixRowId(1)))
        .expect("Beta trace of the unlisted row");
    assert_eq!(
        unlisted.rejection,
        Some(d4lc_rules::RuleRejection::NoEntryForAffix)
    );
}

/// Two rows read from two OCR segments both carry the
/// segment-local line index 0. The unwanted first row is hidden; the
/// wanted second row must keep its own geometry instead of taking the
/// hidden row's (the old line-index join handed it the first rect).
#[test]
fn rows_sharing_a_local_line_index_keep_their_own_geometry() {
    let row = |row_id: u32, affix_id: &str, label: &str| d4lc_gui::CurrentItemAffixView {
        row_id: d4lc_core::AffixRowId(row_id),
        affix_id: Some(affix_id.to_string()),
        label: label.to_string(),
        value_text: None,
        source_text: label.to_string(),
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        matched_by_rule: false,
    };
    let geometry =
        |row_id: u32, segment: usize, y: i32| d4lc_gui::CurrentItemAffixRowGeometryView {
            row_id: d4lc_core::AffixRowId(row_id),
            source_segment_index: segment,
            source_line_index: 0,
            global_line_index: segment,
            marker_ordinal: None,
            text_rect: Rect {
                x: 600,
                y,
                width: 240,
                height: 22,
            },
            affix_area: Rect {
                x: 600,
                y,
                width: 240,
                height: 22,
            },
            // Tooltip left edge (580), the affix marker's row.
            marker_anchor: Some(Rect {
                x: 580,
                y,
                width: 18,
                height: 22,
            }),
        };
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
    crate::runtime_bridge::apply_runtime_event(
        &mut state,
        &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
            item_name: Some("Storm Helm".to_string()),
            parse_status: "Parsed".to_string(),
            item_type: Some("helm".to_string()),
            base_type: Some("Helm".to_string()),
            item_power: Some(925),
            affixes: vec![
                row(0, "MovementSpeed", "Movement Speed"),
                row(1, "AttackSpeed", "Attack Speed"),
            ],
            affix_row_geometry: vec![geometry(0, 1, 300), geometry(1, 2, 322)],
            non_affix_rows: Vec::new(),
            tooltip_window_relative: Some(Rect {
                x: 580,
                y: 120,
                width: 400,
                height: 320,
            }),
        },
    );

    state.rebuild_overlay_descriptor_state();
    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("evaluated rows must project rule state");
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let affix_descriptors: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Affix { .. }
            )
        })
        .collect();
    assert_eq!(affix_descriptors.len(), 1, "HideUnwanted hides the red row");
    assert_eq!(affix_descriptors[0].color, "Green");
    assert_eq!(
        affix_descriptors[0].target,
        d4lc_rules::RuleOverlayTarget::Affix {
            affix_id: "AttackSpeed".to_string(),
            row_id: Some(d4lc_core::AffixRowId(1)),
        }
    );
    // The wanted row's own marker: centered on its marker row
    // (322 + 22 / 2), not on the hidden row at 300.
    assert_eq!(affix_descriptors[0].geometry.y, 323);
}

#[test]
fn rebuild_overlay_descriptor_state_evaluates_current_item_rows_against_selected_preset() {
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    apply_two_row_item_event(&mut state);
    // HideUnwanted must filter the unmatched row's red marker.
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::HideUnwanted;

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("evaluated rows must project rule state");
    // One entry per evaluated item row: the matched row keeps the
    // preset color, the unmatched row carries the upstream red default.
    assert_eq!(rule.result.matched_affixes.len(), 2);
    let matched = &rule.result.matched_affixes[0];
    assert_eq!(matched.affix_id, "AttackSpeed");
    assert_eq!(matched.color, "Green");
    assert_eq!(matched.row_id, Some(d4lc_core::AffixRowId(0)));
    assert!(!matched.required_by.is_empty());
    let unmatched = &rule.result.matched_affixes[1];
    assert_eq!(unmatched.affix_id, "MovementSpeed");
    assert_eq!(unmatched.color, "Red");
    assert!(unmatched.required_by.is_empty());

    // The single descriptor source (frozen rules builder) draws the
    // matched marker at the transported row rect and hides the red row.
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let affix_descriptors: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Affix { .. }
            )
        })
        .collect();
    assert_eq!(affix_descriptors.len(), 1, "HideUnwanted hides the red row");
    assert_eq!(affix_descriptors[0].color, "Green");
    // Marker-sized circle on the marker column (tooltip left edge 580)
    // at the marker row's center (300 + 22 / 2), not the text rect.
    assert_eq!(
        affix_descriptors[0].geometry,
        Rect {
            x: 570,
            y: 301,
            width: 20,
            height: 20,
        }
    );
    assert_eq!(
        affix_descriptors[0].shape,
        d4lc_rules::RuleOverlayShape::Circle
    );

    // ShowAll keeps the red marker, on its own marker row.
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::ShowAll;
    state.rebuild_overlay_descriptor_state();
    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("rule state still projected");
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let mut affix_descriptors: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Affix { .. }
            )
        })
        .collect();
    affix_descriptors.sort_by_key(|descriptor| descriptor.geometry.y);
    assert_eq!(affix_descriptors.len(), 2);
    assert_eq!(affix_descriptors[0].color, "Green");
    assert_eq!(affix_descriptors[1].color, "Red");
    assert_eq!(
        affix_descriptors[1].geometry,
        Rect {
            x: 570,
            y: 323,
            width: 20,
            height: 20,
        }
    );

    // The row evaluation compares the transported canonical type id
    // only. The same item transported with its display derivation ("Helm")
    // must not match the preset's canonical `helm` requirement.
    let mut display_typed = AppState::default_startup();
    seed_attack_speed_preset(&mut display_typed);
    apply_two_row_item_event_with_type(&mut display_typed, "Helm");
    display_typed.rebuild_overlay_descriptor_state();
    let rule = display_typed
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("evaluated rows still project");
    assert_eq!(
        rule.result.matched_affixes[0].color, "Red",
        "the display derivation never matches the canonical helm requirement"
    );
}

#[test]
fn rebuild_overlay_descriptor_state_anchors_trade_panel_to_transported_tooltip_rect() {
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    apply_two_row_item_event(&mut state);
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
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("rule state projected");
    assert_eq!(
        rule.layout.tooltip_rect,
        state.current_item().tooltip_window_relative.unwrap()
    );
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let trade = descriptors
        .iter()
        .find(|descriptor| matches!(descriptor.target, d4lc_rules::RuleOverlayTarget::Trade))
        .expect("trade panel descriptor");
    // The trade panel anchors to the bottom of the real tooltip rect
    // (upstream `DrawGraphicsTrading` tooltip-bottom panel, height 50).
    assert_eq!(
        trade.geometry,
        Rect {
            x: 580,
            y: 390,
            width: 400,
            height: 50,
        }
    );
}

#[test]
fn rebuild_overlay_descriptor_state_yields_no_affix_descriptors_without_tooltip_geometry() {
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    // Parsed rows but no transported geometry: an event whose snapshot
    // carries no row rects and no tooltip (the tooltip vanished).
    crate::runtime_bridge::apply_runtime_event(
        &mut state,
        &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
            item_name: Some("Storm Helm".to_string()),
            parse_status: "Parsed".to_string(),
            item_type: Some("helm".to_string()),
            base_type: Some("Helm".to_string()),
            item_power: Some(925),
            affixes: vec![d4lc_gui::CurrentItemAffixView {
                row_id: d4lc_core::AffixRowId(0),
                affix_id: Some("AttackSpeed".to_string()),
                label: "Attack Speed".to_string(),
                value_text: Some("+18.0%".to_string()),
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
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("the trade marker still projects rule state");
    assert!(rule.layout.affix_locations.is_empty());
    assert_eq!(rule.layout.tooltip_rect, Rect::default());
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    assert!(
        descriptors.iter().all(|descriptor| !matches!(
            descriptor.target,
            d4lc_rules::RuleOverlayTarget::Affix { .. }
        )),
        "no tooltip geometry: no affix marker may be fabricated"
    );
}

#[test]
fn rebuild_overlay_descriptor_state_marks_all_unmatched_rows_unwanted() {
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    // Only the non-matching row: every evaluated row unmatched.
    crate::runtime_bridge::apply_runtime_event(
        &mut state,
        &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
            item_name: Some("Storm Helm".to_string()),
            parse_status: "Parsed".to_string(),
            item_type: Some("helm".to_string()),
            base_type: Some("Helm".to_string()),
            item_power: Some(925),
            affixes: vec![d4lc_gui::CurrentItemAffixView {
                row_id: d4lc_core::AffixRowId(0),
                affix_id: Some("MovementSpeed".to_string()),
                label: "Movement Speed".to_string(),
                value_text: Some("+12.0%".to_string()),
                source_text: "+12.0% Movement Speed".to_string(),
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                matched_by_rule: false,
            }],
            affix_row_geometry: vec![d4lc_gui::CurrentItemAffixRowGeometryView {
                row_id: d4lc_core::AffixRowId(0),
                source_segment_index: 0,
                source_line_index: 5,
                global_line_index: 6,
                marker_ordinal: None,
                text_rect: Rect {
                    x: 600,
                    y: 322,
                    width: 260,
                    height: 22,
                },
                affix_area: Rect {
                    x: 600,
                    y: 322,
                    width: 260,
                    height: 22,
                },
                // Tooltip left edge (580), the affix marker's row.
                marker_anchor: Some(Rect {
                    x: 580,
                    y: 322,
                    width: 18,
                    height: 22,
                }),
            }],
            non_affix_rows: Vec::new(),
            tooltip_window_relative: Some(Rect {
                x: 580,
                y: 120,
                width: 400,
                height: 320,
            }),
        },
    );

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("evaluated rows project rule state");
    assert_eq!(rule.result.final_decision, RuleDecision::Unwanted);
    assert_eq!(rule.result.matched_affixes.len(), 1);
    assert_eq!(rule.result.matched_affixes[0].color, "Red");
}

/// Transport a parsed item carrying classified non-affix rows (the
/// f620ad6 transport shape): one aspect row the preset lists and one
/// rune row it does not, each with canonical id, label and
/// window-relative row rect.
fn apply_aspect_and_rune_item_event(state: &mut AppState) {
    crate::runtime_bridge::apply_runtime_event(
        state,
        &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
            item_name: Some("Storm Helm".to_string()),
            parse_status: "Parsed".to_string(),
            item_type: Some("helm".to_string()),
            base_type: Some("Helm".to_string()),
            item_power: Some(925),
            affixes: Vec::new(),
            affix_row_geometry: Vec::new(),
            non_affix_rows: vec![
                d4lc_gui::CurrentItemNonAffixRowView {
                    kind: d4lc_gui::CurrentItemMatchKind::Aspect,
                    id: Some("AspectOfWhirlwind".to_string()),
                    label: Some("Whirlwind Aspect".to_string()),
                    source_line_index: 3,
                    global_line_index: 4,
                    rect: Some(Rect {
                        x: 600,
                        y: 280,
                        width: 300,
                        height: 22,
                    }),
                    marker_anchor: Some(Rect {
                        x: 580,
                        y: 280,
                        width: 18,
                        height: 22,
                    }),
                },
                d4lc_gui::CurrentItemNonAffixRowView {
                    kind: d4lc_gui::CurrentItemMatchKind::Rune,
                    id: Some("Rune_Yul".to_string()),
                    label: Some("Yul".to_string()),
                    source_line_index: 5,
                    global_line_index: 6,
                    rect: Some(Rect {
                        x: 600,
                        y: 302,
                        width: 120,
                        height: 22,
                    }),
                    marker_anchor: Some(Rect {
                        x: 580,
                        y: 302,
                        width: 18,
                        height: 22,
                    }),
                },
            ],
            tooltip_window_relative: Some(Rect {
                x: 580,
                y: 120,
                width: 400,
                height: 320,
            }),
        },
    );
}

#[test]
fn rebuild_overlay_descriptor_state_evaluates_non_affix_rows_against_selected_preset() {
    let mut state = AppState::default_startup();
    state.shell.settings.overlay_enabled = true;
    state
        .affix_preset_editor
        .create_preset("Aspect Build")
        .unwrap();
    // The preset lists the transported aspect (its entry color wins)
    // and a rune this tooltip does not carry (the red default wins).
    state
        .affix_preset_editor
        .edit_entry(
            "Aspect Build",
            d4lc_gui::PresetEntry {
                kind: d4lc_gui::PresetEntryKind::Aspect,
                id: "AspectOfWhirlwind".to_string(),
                label: Some("Whirlwind Aspect".to_string()),
                item_type: Some("helm".to_string()),
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
        .affix_preset_editor
        .edit_entry(
            "Aspect Build",
            d4lc_gui::PresetEntry {
                kind: d4lc_gui::PresetEntryKind::Rune,
                id: "Rune_Eom".to_string(),
                label: None,
                item_type: None,
                color: "Blue".to_string(),
                is_any_type: true,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
        )
        .unwrap();
    state.affix_preset_editor.selected_preset_name = Some("Aspect Build".to_string());
    apply_aspect_and_rune_item_event(&mut state);
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::ShowAll;

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("evaluated non-affix rows project rule state");
    // Matched aspect: the preset entry's color on the transported row
    // rect, with the row label as the marker label.
    let power = rule
        .result
        .matched_power
        .as_ref()
        .expect("the aspect verdict projects the power marker slot");
    assert_eq!(power.power_id, "AspectOfWhirlwind");
    assert_eq!(power.power_kind, d4lc_rules::RulePowerKind::Aspect);
    let power_location = rule
        .layout
        .power_location
        .as_ref()
        .expect("the aspect marker anchors at its row");
    assert_eq!(power_location.color, "Green");
    assert_eq!(power_location.id, "AspectOfWhirlwind");
    // Upstream aspect markers carry no text (OverlayHandler.cs:463-485).
    assert_eq!(power_location.text, None);
    // The aspect marker's anchor (tooltip left edge, aspect marker
    // row), not the aspect text rect.
    assert_eq!(
        power_location.geometry,
        Rect {
            x: 580,
            y: 280,
            width: 18,
            height: 22,
        }
    );
    // Unmatched rune: the upstream red default at its own row rect.
    assert_eq!(rule.result.matched_runes, vec!["Rune_Yul".to_string()]);
    assert_eq!(rule.layout.rune_locations.len(), 1);
    assert_eq!(rule.layout.rune_locations[0].color, "Red");
    assert_eq!(rule.layout.rune_locations[0].id, "Rune_Yul");
    // One of the two evaluated rows matched: the item is wanted.
    assert_eq!(rule.result.final_decision, RuleDecision::Wanted);

    // The single descriptor source draws both markers (ShowAll).
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let aspect = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Power { .. }
            )
        })
        .expect("aspect marker descriptor");
    assert_eq!(aspect.color, "Green");
    assert_eq!(
        aspect.marker_type,
        d4lc_rules::RuleOverlayMarkerType::Aspect
    );
    let rune = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Rune { .. }
            )
        })
        .expect("rune marker descriptor");
    assert_eq!(rune.color, "Red");

    // HideUnwanted hides the red rune marker and keeps the matched
    // aspect — the same filter semantics the affix markers get from
    // the frozen descriptor builder.
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
    state.rebuild_overlay_descriptor_state();
    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("rule state still projected");
    assert!(
        rule.layout.rune_locations.is_empty(),
        "HideUnwanted withholds the red rune location"
    );
    assert_eq!(
        rule.result.matched_runes,
        vec!["Rune_Yul".to_string()],
        "the verdict itself stays in the result"
    );
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    assert!(
        descriptors.iter().all(|descriptor| !matches!(
            descriptor.target,
            d4lc_rules::RuleOverlayTarget::Rune { .. }
        )),
        "HideUnwanted hides the red rune marker"
    );
    assert!(
        descriptors.iter().any(|descriptor| matches!(
            descriptor.target,
            d4lc_rules::RuleOverlayTarget::Power { .. }
        ) && descriptor.color == "Green"),
        "the matched aspect marker survives HideUnwanted"
    );
}

#[test]
fn rebuild_overlay_descriptor_state_multi_build_evaluates_rows_per_build() {
    let mut state = AppState::default_startup();
    state.shell.settings.overlay_enabled = true;
    // Two build presets, each listing a different row of the SAME
    // tooltip (upstream SetMultiBuildMode re-evaluates every build).
    for (name, affix_id, color) in [
        ("Alpha", "AttackSpeed", "#CC3333"),
        ("Beta", "MovementSpeed", "#22C55E"),
    ] {
        state.affix_preset_editor.create_preset(name).unwrap();
        state
            .affix_preset_editor
            .edit_entry(
                name,
                d4lc_gui::PresetEntry {
                    kind: d4lc_gui::PresetEntryKind::Affix,
                    id: affix_id.to_string(),
                    label: None,
                    item_type: Some("helm".to_string()),
                    color: color.to_string(),
                    is_any_type: false,
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                },
            )
            .unwrap();
    }
    // Both builds also list the transported aspect: each build draws
    // its own aspect marker in its own column (upstream
    // `DrawGraphicsAspectsMulti`, OH:487-522), colored with the
    // build's color.
    for name in ["Alpha", "Beta"] {
        state
            .affix_preset_editor
            .edit_entry(
                name,
                d4lc_gui::PresetEntry {
                    kind: d4lc_gui::PresetEntryKind::Aspect,
                    id: "AspectOfWhirlwind".to_string(),
                    label: None,
                    item_type: Some("helm".to_string()),
                    color: "Purple".to_string(),
                    is_any_type: false,
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                },
            )
            .unwrap();
    }
    // Column order comes from the build index, not the stored order.
    state.build_import.multi_builds = vec![
        MultiBuildRow {
            name: "Beta".to_string(),
            index: 1,
            color: "#22C55E".to_string(),
        },
        MultiBuildRow {
            name: "Alpha".to_string(),
            index: 0,
            color: "#CC3333".to_string(),
        },
    ];
    state.shell.settings.multi_build_mode_enabled = true;
    // The same two-row item as the single-build tests plus a third
    // row no build lists.
    crate::runtime_bridge::apply_runtime_event(
        &mut state,
        &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
            item_name: Some("Storm Helm".to_string()),
            parse_status: "Parsed".to_string(),
            item_type: Some("helm".to_string()),
            base_type: Some("Helm".to_string()),
            item_power: Some(925),
            affixes: vec![
                d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(0),
                    affix_id: Some("AttackSpeed".to_string()),
                    label: "Attack Speed".to_string(),
                    value_text: Some("+18.0%".to_string()),
                    source_text: "+18.0% Attack Speed".to_string(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                },
                d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(1),
                    affix_id: Some("MovementSpeed".to_string()),
                    label: "Movement Speed".to_string(),
                    value_text: Some("+12.0%".to_string()),
                    source_text: "+12.0% Movement Speed".to_string(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                },
                d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(2),
                    affix_id: Some("Unlisted".to_string()),
                    label: "Unlisted".to_string(),
                    value_text: None,
                    source_text: "+5.0% Unlisted".to_string(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                },
            ],
            affix_row_geometry: vec![
                d4lc_gui::CurrentItemAffixRowGeometryView {
                    row_id: d4lc_core::AffixRowId(0),
                    source_segment_index: 0,
                    source_line_index: 4,
                    global_line_index: 5,
                    marker_ordinal: None,
                    text_rect: Rect {
                        x: 600,
                        y: 300,
                        width: 240,
                        height: 22,
                    },
                    affix_area: Rect {
                        x: 600,
                        y: 300,
                        width: 240,
                        height: 22,
                    },
                    // Tooltip left edge (580), the affix marker's row.
                    marker_anchor: Some(Rect {
                        x: 580,
                        y: 300,
                        width: 18,
                        height: 22,
                    }),
                },
                d4lc_gui::CurrentItemAffixRowGeometryView {
                    row_id: d4lc_core::AffixRowId(1),
                    source_segment_index: 0,
                    source_line_index: 5,
                    global_line_index: 6,
                    marker_ordinal: None,
                    text_rect: Rect {
                        x: 600,
                        y: 322,
                        width: 260,
                        height: 22,
                    },
                    affix_area: Rect {
                        x: 600,
                        y: 322,
                        width: 260,
                        height: 22,
                    },
                    // Tooltip left edge (580), the affix marker's row.
                    marker_anchor: Some(Rect {
                        x: 580,
                        y: 322,
                        width: 18,
                        height: 22,
                    }),
                },
                d4lc_gui::CurrentItemAffixRowGeometryView {
                    row_id: d4lc_core::AffixRowId(2),
                    source_segment_index: 0,
                    source_line_index: 6,
                    global_line_index: 7,
                    marker_ordinal: None,
                    text_rect: Rect {
                        x: 600,
                        y: 344,
                        width: 200,
                        height: 22,
                    },
                    affix_area: Rect {
                        x: 600,
                        y: 344,
                        width: 200,
                        height: 22,
                    },
                    // Tooltip left edge (580), the affix marker's row.
                    marker_anchor: Some(Rect {
                        x: 580,
                        y: 344,
                        width: 18,
                        height: 22,
                    }),
                },
            ],
            non_affix_rows: vec![d4lc_gui::CurrentItemNonAffixRowView {
                kind: d4lc_gui::CurrentItemMatchKind::Aspect,
                id: Some("AspectOfWhirlwind".to_string()),
                label: Some("Whirlwind Aspect".to_string()),
                source_line_index: 3,
                global_line_index: 4,
                rect: Some(Rect {
                    x: 600,
                    y: 280,
                    width: 300,
                    height: 22,
                }),
                marker_anchor: Some(Rect {
                    x: 580,
                    y: 280,
                    width: 18,
                    height: 22,
                }),
            }],
            tooltip_window_relative: Some(Rect {
                x: 580,
                y: 120,
                width: 400,
                height: 320,
            }),
        },
    );

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("multi-build evaluation projects rule state");
    // Matched rows carry their BUILD's color, not the preset entry
    // color; the row no build lists draws nothing (upstream's multi
    // draw hides the red default in every column).
    assert_eq!(rule.result.matched_affixes.len(), 2);
    let alpha = rule
        .result
        .matched_affixes
        .iter()
        .find(|affix| affix.affix_id == "AttackSpeed")
        .expect("Alpha's matched row");
    assert_eq!(alpha.color, "#CC3333");
    assert_eq!(alpha.required_by, vec!["Alpha".to_string()]);
    let beta = rule
        .result
        .matched_affixes
        .iter()
        .find(|affix| affix.affix_id == "MovementSpeed")
        .expect("Beta's matched row");
    assert_eq!(beta.color, "#22C55E");
    assert_eq!(beta.required_by, vec!["Beta".to_string()]);
    // The aspect is listed by both builds: the single slot keeps the
    // first column's claim (Alpha), and the per-build copies carry one
    // shifted location per build (overriding the Purple entry color,
    // exactly like `evaluate_non_affix_multi_build` recolors).
    let power_location = rule
        .layout
        .power_location
        .as_ref()
        .expect("first claim's aspect marker");
    assert_eq!(power_location.color, "#CC3333");
    assert_eq!(rule.result.matched_powers.len(), 2);
    assert_eq!(rule.layout.power_locations.len(), 2);
    assert_eq!(rule.layout.power_locations[0].geometry.x, 585);
    assert_eq!(rule.layout.power_locations[1].geometry.x, 565);
    // Three of the four evaluated rows matched: the item is wanted.
    assert_eq!(rule.result.final_decision, RuleDecision::Wanted);

    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let mut affix_descriptors: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Affix { .. }
            )
        })
        .collect();
    affix_descriptors.sort_by_key(|descriptor| descriptor.geometry.y);
    assert_eq!(
        affix_descriptors.len(),
        2,
        "the unlisted row draws no marker in multi-build mode"
    );
    assert_eq!(affix_descriptors[0].color, "#CC3333");
    assert_eq!(affix_descriptors[1].color, "#22C55E");
    // Per-build column offsets (upstream `5 - (i * 20)`, OH:337):
    // Alpha holds column 0 (marker column 580 + 5), Beta column 1
    // (580 - 15); each circle is centered on its column at its own
    // marker row.
    assert_eq!(
        affix_descriptors[0].geometry,
        Rect {
            x: 575,
            y: 301,
            width: 20,
            height: 20,
        }
    );
    assert_eq!(
        affix_descriptors[1].geometry,
        Rect {
            x: 555,
            y: 323,
            width: 20,
            height: 20,
        }
    );
    // Per-build power markers (upstream `DrawGraphicsAspectsMulti`,
    // OH:487-522): both builds list the aspect, so two distinct aspect
    // markers draw at the builds' column offsets — Alpha column 0
    // (marker column 580 + 5) in Alpha's color, Beta column 1 (580 - 15) in
    // Beta's color — instead of fighting over one shared slot.
    let power_descriptors: Vec<_> = descriptors
        .iter()
        .filter(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Power { .. }
            )
        })
        .collect();
    assert_eq!(
        power_descriptors.len(),
        2,
        "one aspect marker per matching build column"
    );
    assert_eq!(power_descriptors[0].color, "#CC3333");
    assert_eq!(
        power_descriptors[0].geometry,
        Rect {
            x: 575,
            y: 281,
            width: 20,
            height: 20,
        }
    );
    assert_eq!(power_descriptors[1].color, "#22C55E");
    assert_eq!(
        power_descriptors[1].geometry,
        Rect {
            x: 555,
            y: 281,
            width: 20,
            height: 20,
        }
    );
}

/// Seed a wanted row plus a parsed current item whose affixes the
/// lookup evaluates.
fn seed_live_trade_lookup(state: &mut AppState, wanted_value: &str, current_type: &str) {
    state.shell.settings.overlay_enabled = true;
    state.shell.settings.trade_overlay_enabled = true;
    state
        .trade_imports_mut()
        .upsert_trade_item(d4lc_gui::TradeItemRow {
            trade_item_id: "Trade_Amulet".to_string(),
            item_type: "Amulet".to_string(),
            value_text: wanted_value.to_string(),
            affixes: vec![d4lc_gui::TradeItemAffixView {
                affix_id: "Affix_AttackSpeed".to_string(),
                is_greater: false,
            }],
        });
    let current = state.current_item_mut();
    current.item_type = Some(current_type.to_string());
    current.affixes = vec![d4lc_gui::CurrentItemAffixView {
        row_id: d4lc_core::AffixRowId(0),
        affix_id: Some("Affix_AttackSpeed".to_string()),
        label: "Attack Speed".to_string(),
        value_text: Some("18.0%".to_string()),
        source_text: "+18.0% Attack Speed".to_string(),
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        matched_by_rule: false,
    }];
}

#[test]
fn live_parsed_item_matching_wanted_row_projects_lookup_marker() {
    let mut state = AppState::default_startup();
    seed_live_trade_lookup(&mut state, "10M", "Amulet");
    // A live tooltip: the looked-up marker's panel anchors to it.
    state.current_item_mut().tooltip_window_relative = Some(Rect {
        x: 580,
        y: 120,
        width: 400,
        height: 320,
    });

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("a live trade match must project rule state");
    let marker = rule.result.trade_marker.as_ref().expect("lookup marker");
    assert_eq!(marker.trade_item_id, "Trade_Amulet");
    assert_eq!(marker.value_text, "10M");
    // The marker's overlay descriptor carries the wanted value
    // (the same per-frame build the live loop runs).
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let trade_descriptor = descriptors
        .iter()
        .find(|descriptor| matches!(descriptor.target, d4lc_rules::RuleOverlayTarget::Trade))
        .expect("trade overlay descriptor");
    assert_eq!(
        trade_descriptor.text.as_deref(),
        Some("10M"),
        "the overlay panel text derives from the looked-up wanted row"
    );
}

#[test]
fn live_parsed_item_without_match_projects_no_trade_marker() {
    let mut state = AppState::default_startup();
    seed_live_trade_lookup(&mut state, "10M", "Ring");

    state.rebuild_overlay_descriptor_state();

    // No wanted row matches the parsed Ring: no trade marker.
    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("the parsed affix row is still evaluated");
    assert!(
        rule.result.trade_marker.is_none(),
        "a parsed item that matches no wanted row must show no marker"
    );
    // No preset is selected: upstream `GetAffix` returns the red default
    // for every row (`preset == null`, AffixManager.cs:540-541).
    assert_eq!(rule.result.matched_affixes.len(), 1);
    assert_eq!(rule.result.matched_affixes[0].color, "Red");
    assert_eq!(
        rule.result.affix_traces[0].rejection,
        Some(d4lc_rules::RuleRejection::PresetNotFound)
    );
}

#[test]
fn partially_parsed_item_with_affixes_but_no_type_projects_lookup_marker() {
    let mut state = AppState::default_startup();
    state.shell.settings.overlay_enabled = true;
    state.shell.settings.trade_overlay_enabled = true;
    // The typeless wanted row matches the partially parsed tooltip
    // (an empty parsed type only compares equal to a typeless row,
    // upstream `itemType.Equals(t.Type.Type)`); the typed selected
    // row must not win through the no-parse fallback.
    state
        .trade_imports_mut()
        .upsert_trade_item(d4lc_gui::TradeItemRow {
            trade_item_id: "Trade_Item".to_string(),
            item_type: String::new(),
            value_text: "10M".to_string(),
            affixes: vec![d4lc_gui::TradeItemAffixView {
                affix_id: "Affix_AttackSpeed".to_string(),
                is_greater: false,
            }],
        });
    state
        .trade_imports_mut()
        .upsert_trade_item(d4lc_gui::TradeItemRow {
            trade_item_id: "Trade_Ring".to_string(),
            item_type: "Ring".to_string(),
            value_text: "5b".to_string(),
            affixes: vec![],
        });
    let current = state.current_item_mut();
    current.item_type = None;
    // A live tooltip anchors the looked-up marker's panel.
    current.tooltip_window_relative = Some(Rect {
        x: 580,
        y: 120,
        width: 400,
        height: 320,
    });
    current.affixes = vec![d4lc_gui::CurrentItemAffixView {
        row_id: d4lc_core::AffixRowId(0),
        affix_id: Some("Affix_AttackSpeed".to_string()),
        label: "Attack Speed".to_string(),
        value_text: Some("18.0%".to_string()),
        source_text: "+18.0% Attack Speed".to_string(),
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        matched_by_rule: false,
    }];

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref())
        .expect("a live trade match must project rule state");
    let marker = rule.result.trade_marker.as_ref().expect("lookup marker");
    assert_eq!(marker.trade_item_id, "Trade_Item");
    assert_eq!(marker.value_text, "10M");
}

#[test]
fn partially_parsed_item_with_affixes_but_no_type_matching_nothing_projects_no_marker() {
    let mut state = AppState::default_startup();
    state.shell.settings.trade_overlay_enabled = true;
    state
        .trade_imports_mut()
        .upsert_trade_item(d4lc_gui::TradeItemRow {
            trade_item_id: "Trade_Ring".to_string(),
            item_type: "Ring".to_string(),
            value_text: "5b".to_string(),
            affixes: vec![d4lc_gui::TradeItemAffixView {
                affix_id: "Affix_AttackSpeed".to_string(),
                is_greater: false,
            }],
        });
    // Parsed affixes but no type line: the lookup still runs
    // (upstream gates `FindTradeItem` on affixes only) and matches
    // no wanted row, so no marker may be synthesized.
    let current = state.current_item_mut();
    current.item_type = None;
    current.affixes = vec![d4lc_gui::CurrentItemAffixView {
        row_id: d4lc_core::AffixRowId(0),
        affix_id: Some("Affix_AttackSpeed".to_string()),
        label: "Attack Speed".to_string(),
        value_text: Some("18.0%".to_string()),
        source_text: "+18.0% Attack Speed".to_string(),
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        matched_by_rule: false,
    }];

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|descriptor| descriptor.rule.as_ref());
    let has_trade_marker = rule
        .map(|rule| rule.result.trade_marker.is_some())
        .unwrap_or(false);
    assert!(
        !has_trade_marker,
        "a partially parsed item that matches no wanted row must show no marker"
    );
}

#[test]
fn minimal_value_verdict_draws_the_rectangle_below_the_threshold() {
    // Upstream OH:311-315: `AffixValue < AffixThresholdValue` draws
    // the rectangle instead of the circle while the minimal-value
    // filter is on. The verdict now carries the parsed OCR value vs
    // the persisted per-affix minimal value (`GetAffixMinimalValue`,
    // AffixManager.cs:610-613) instead of staying not_evaluated.
    let mut state = AppState::default_startup();
    seed_attack_speed_preset(&mut state);
    apply_two_row_item_event(&mut state);
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::ShowAll;
    state.shell.settings.minimal_affix_value_filter_enabled = true;
    state
        .minimal_affix_values
        .insert("AttackSpeed".to_string(), 12.0);
    // Drag the matched row's parsed OCR value below the threshold.
    state.current_item_mut().affixes[0].value_text = Some("+3.0%".to_string());

    state.rebuild_overlay_descriptor_state();

    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.rule.as_ref())
        .expect("rule state projects");
    let matched = &rule.result.matched_affixes[0];
    assert_eq!(
        matched.value_status.status,
        d4lc_rules::RuleCheckStatus::Failed,
        "the below-minimal row carries a real failed verdict"
    );
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let below = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Affix { .. }
            )
        })
        .expect("the below-minimal marker still draws");
    assert_eq!(
        below.shape,
        d4lc_rules::RuleOverlayShape::Rectangle,
        "below minimal: the upstream rectangle"
    );
    // The marker-sized square hanging from the marker row, never the
    // text width (OverlayHandler.cs:311-315).
    assert_eq!(
        (below.geometry.width, below.geometry.height),
        (20, 20),
        "the minimal-value rectangle is a marker, not the text line"
    );
    assert_eq!(below.geometry.x, 580 - 10);

    // Above the threshold the marker is a circle with a passed verdict.
    state.current_item_mut().affixes[0].value_text = Some("+18.0%".to_string());
    state.rebuild_overlay_descriptor_state();
    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.rule.as_ref())
        .expect("rule state projects");
    assert_eq!(
        rule.result.matched_affixes[0].value_status.status,
        d4lc_rules::RuleCheckStatus::Passed
    );
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let above = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Affix { .. }
            )
        })
        .expect("the marker draws");
    assert_eq!(above.shape, d4lc_rules::RuleOverlayShape::Circle);
}

/// The vendored enUS rules catalog the app's startup projection loads
/// (`actions::affix::project_affix_entries`); the overlay's dungeon-type
/// gate reads the sigil `Type` token from it. Loaded once per test
/// process.
fn enus_catalog() -> &'static d4lc_rules::AffixCatalog {
    static CATALOG: std::sync::OnceLock<d4lc_rules::AffixCatalog> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest_dir
            .parent()
            .and_then(|crates| crates.parent())
            .expect("workspace root");
        d4lc_rules::AffixCatalog::load(&d4lc_assets::AssetRepository::new(root), "enUS")
            .unwrap_or_else(|error| panic!("load the vendored enUS catalog: {error}"))
    })
}

#[test]
fn dungeon_tiers_draw_the_tier_text_on_the_sigil_marker() {
    // Upstream OH:279-293: a dungeon square on a sigil tooltip draws
    // the dungeon tier (`GetSigilDungeonTier`, AffixManager.cs:744-747)
    // while the persisted `DungeonTiers` setting is on — the stored
    // `Config/DungeonTiers.json` tier or the upstream `F` default. The
    // gate is the sigil's game-data `Type` token (`GetSigilType`), here
    // read from the loaded rules catalog, so non-dungeon sigils draw no
    // tier text.
    const DUNGEON_SIGIL: &str = "World_DGN_Kehj_AbandonedMineworks";
    const MINOR_SIGIL: &str = "DungeonAffix_Minor_Monster_SunderArmor";
    let catalog = enus_catalog();
    assert_eq!(
        catalog
            .entry(d4lc_rules::CatalogKind::Sigil, DUNGEON_SIGIL)
            .and_then(|entry| entry.sigil_type.as_deref()),
        Some("Dungeon"),
        "the vendored enUS data must type the seeded sigil as a dungeon sigil"
    );
    assert_eq!(
        catalog
            .entry(d4lc_rules::CatalogKind::Sigil, MINOR_SIGIL)
            .and_then(|entry| entry.sigil_type.as_deref()),
        Some("Minor"),
        "the vendored enUS data must type the seeded sigil as a non-dungeon sigil"
    );

    let seed = |state: &mut AppState, sigil_id: &str, label: &str, stored_tier: Option<&str>| {
        state.affix_catalog = Some(catalog.clone());
        state.shell.settings.overlay_enabled = true;
        state.shell.settings.dungeon_tiers_enabled = true;
        state.affix_preset_editor.create_preset("Sigils").unwrap();
        state
            .affix_preset_editor
            .edit_entry(
                "Sigils",
                d4lc_gui::PresetEntry {
                    kind: d4lc_gui::PresetEntryKind::Sigil,
                    id: sigil_id.to_string(),
                    label: None,
                    item_type: None,
                    color: "Green".to_string(),
                    is_any_type: true,
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                },
            )
            .unwrap();
        state.affix_preset_editor.selected_preset_name = Some("Sigils".to_string());
        crate::runtime_bridge::apply_runtime_event(
            state,
            &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
                item_name: Some(label.to_string()),
                parse_status: "Parsed".to_string(),
                item_type: Some("sigil".to_string()),
                base_type: None,
                item_power: None,
                affixes: Vec::new(),
                affix_row_geometry: Vec::new(),
                non_affix_rows: vec![d4lc_gui::CurrentItemNonAffixRowView {
                    kind: d4lc_gui::CurrentItemMatchKind::Sigil,
                    id: Some(sigil_id.to_string()),
                    label: Some(label.to_string()),
                    source_line_index: 0,
                    global_line_index: 1,
                    rect: Some(Rect {
                        x: 600,
                        y: 200,
                        width: 300,
                        height: 22,
                    }),
                    marker_anchor: Some(Rect {
                        x: 580,
                        y: 200,
                        width: 18,
                        height: 22,
                    }),
                }],
                tooltip_window_relative: Some(Rect {
                    x: 580,
                    y: 120,
                    width: 400,
                    height: 320,
                }),
            },
        );
        if let Some(tier) = stored_tier {
            state
                .dungeon_tiers
                .insert(sigil_id.to_string(), tier.to_string());
        }
    };

    let marker_text = |state: &mut AppState| -> Option<String> {
        state.rebuild_overlay_descriptor_state();
        state
            .overlay_descriptor_state
            .as_ref()
            .and_then(|projected| projected.rule.as_ref())
            .and_then(|rule| rule.layout.sigil_location.as_ref())
            .and_then(|location| location.text.clone())
    };

    // The stored tier reaches the sigil marker and the descriptor the
    // frozen builder draws.
    let mut state = AppState::default_startup();
    seed(&mut state, DUNGEON_SIGIL, "Abandoned Mineworks", Some("T3"));
    state.rebuild_overlay_descriptor_state();
    let rule = state
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.rule.as_ref())
        .expect("sigil state projects");
    assert_eq!(
        rule.layout
            .sigil_location
            .as_ref()
            .and_then(|location| location.text.as_deref()),
        Some("T3"),
        "the dungeon tier replaces the row label on the marker"
    );
    let descriptors =
        d4lc_rules::build_overlay_descriptors(&rule.result, &rule.settings, &rule.layout);
    let sigil = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Sigil { .. }
            )
        })
        .expect("sigil marker descriptor");
    assert_eq!(sigil.text.as_deref(), Some("T3"));

    // An unstored dungeon sigil draws the upstream `F` default
    // (`GetSigilDungeonTier` never returns an empty tier).
    let mut unstored = AppState::default_startup();
    seed(&mut unstored, DUNGEON_SIGIL, "Abandoned Mineworks", None);
    assert_eq!(
        marker_text(&mut unstored).as_deref(),
        Some(d4lc_gui::affix::SIGIL_DUNGEON_TIER_DEFAULT),
        "an unstored dungeon sigil draws the upstream F default"
    );

    // A non-dungeon sigil stays silent even with a stored tier entry.
    let mut minor = AppState::default_startup();
    seed(&mut minor, MINOR_SIGIL, "Armor Breakers", Some("T3"));
    assert_eq!(
        marker_text(&mut minor),
        None,
        "only dungeon sigils draw the tier text"
    );

    // The setting off draws no text: the tier text did not run and the
    // dungeon square never shows the sigil name (OverlayHandler.cs:284-292).
    let mut off_state = AppState::default_startup();
    seed(
        &mut off_state,
        DUNGEON_SIGIL,
        "Abandoned Mineworks",
        Some("T3"),
    );
    off_state.shell.settings.dungeon_tiers_enabled = false;
    assert_eq!(marker_text(&mut off_state), None);
}
