use super::*;
use d4lc_rules::ImportedParagonBoard;

/// Transport a parsed two-row item through the real runtime event path:
/// one row the preset wants and one row it does not list, each with OCR
/// row geometry, plus the detected tooltip rect. Canonical IdName ids
/// (maxroll expected-fixture style).
pub(super) fn apply_two_row_item_event(state: &mut AppState) {
    apply_two_row_item_event_with_type(state, "helm");
}

/// The same transport with an explicit transported item type, so a test
/// can compare the canonical id against the display derivation.
pub(super) fn apply_two_row_item_event_with_type(state: &mut AppState, item_type: &str) {
    crate::runtime_bridge::apply_runtime_event(
        state,
        &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
            item_name: Some("Storm Helm".to_string()),
            parse_status: "Parsed".to_string(),
            item_type: Some(item_type.to_string()),
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
            ],
            non_affix_rows: Vec::new(),
            tooltip_window_relative: Some(Rect {
                x: 580,
                y: 120,
                width: 400,
                height: 320,
            }),
        },
    );
}

/// Seed the "Speed" preset with the canonical `AttackSpeed` helm entry.
/// The overlay power switch is on: the marker tests exercise the
/// enabled overlay (the off state is the gate test's subject).
pub(super) fn seed_attack_speed_preset(state: &mut AppState) {
    state.shell.settings.overlay_enabled = true;
    state.affix_preset_editor.create_preset("Speed").unwrap();
    state
        .affix_preset_editor
        .edit_entry(
            "Speed",
            d4lc_gui::PresetEntry {
                kind: d4lc_gui::PresetEntryKind::Affix,
                id: "AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
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
    state.affix_preset_editor.selected_preset_name = Some("Speed".to_string());
}

/// Paragon routes with the maxroll expected-fixture oracle values
/// (starter board plus one 90°-rotated board with localized
/// labels and transformed node positions).
pub(super) fn sample_paragon_routes() -> Vec<ImportedParagonRoute> {
    vec![
        ImportedParagonRoute {
            step_name: Some("Starter".to_string()),
            boards: vec![ImportedParagonBoard {
                index: 0,
                name: "Start".to_string(),
                glyph: String::new(),
                rotation: "0°".to_string(),
                active_nodes: vec![105],
            }],
        },
        ImportedParagonRoute {
            step_name: Some("Endgame".to_string()),
            boards: vec![ImportedParagonBoard {
                index: 0,
                name: "Hemorrhage".to_string(),
                glyph: "Enchanter".to_string(),
                rotation: "90°".to_string(),
                active_nodes: vec![36, 57],
            }],
        },
    ]
}
