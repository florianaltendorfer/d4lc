use super::*;
use crate::overlay::OverlayColor;
use crate::test_support::{
    default_paragon_overlay_layout, rect, sample_paragon_board, write_overlay_debug_json,
};
use d4lc_core::text_measure::{FixedTextMeasure, TextExtent, TextMeasure};
use d4lc_core::AffixRowId;
use d4lc_core::Point;

#[test]
fn paragon_overlay_warning_uses_upstream_warning_panel() {
    let descriptors = build_paragon_overlay_descriptors(
        &ParagonOverlayState {
            build_name: "Whirlwind Arsenal".to_string(),
            routes: Vec::new(),
            current_route_index: 0,
            current_board_index: None,
            current_board_name: None,
            mode: ParagonOverlayMode::Full,
        },
        &default_paragon_overlay_layout(rect(100, 200, 1280, 720)),
        &FixedTextMeasure::HALF_EM,
    );

    assert_eq!(descriptors.len(), 2);
    assert!(descriptors.iter().any(|descriptor| {
        matches!(
            descriptor.target,
            RuleOverlayTarget::Paragon {
                kind: ParagonOverlayElementKind::WarningPanel,
                ..
            }
        ) && descriptor.text.as_deref() == Some("No paragon boards available.")
    }));
    assert!(descriptors.iter().any(|descriptor| {
        descriptor.shape == RuleOverlayShape::Rectangle
            && descriptor.color == OverlayColor::Paragon(ParagonColor::Border)
    }));

    write_paragon_debug_artifact("paragon-warning-descriptors.json", &descriptors);
}

#[test]
fn paragon_overlay_full_mode_keeps_upstream_step_and_board_order() {
    let descriptors = build_paragon_overlay_descriptors(
        &ParagonOverlayState {
            build_name: "Whirlwind Arsenal".to_string(),
            routes: vec![
                ImportedParagonRoute {
                    step_name: Some("Starter".to_string()),
                    boards: vec![sample_paragon_board("Start", "Marshal", "0°", &[0, 1])],
                },
                ImportedParagonRoute {
                    step_name: Some("Endgame".to_string()),
                    boards: vec![
                        sample_paragon_board("Decimator", "Marshal", "0°", &[0, 1, 2]),
                        sample_paragon_board("Carnage", "Ambidextrous", "90°", &[22, 23]),
                        sample_paragon_board("Blood Rage", "Crusher", "180°", &[440]),
                    ],
                },
            ],
            current_route_index: 1,
            current_board_index: None,
            current_board_name: None,
            mode: ParagonOverlayMode::Full,
        },
        &default_paragon_overlay_layout(rect(100, 200, 1280, 720)),
        &FixedTextMeasure::HALF_EM,
    );

    let board_labels = descriptors
        .iter()
        .filter_map(|descriptor| match &descriptor.target {
            RuleOverlayTarget::Paragon {
                kind: ParagonOverlayElementKind::BoardEntry,
                ..
            } => descriptor.text.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        board_labels,
        vec![
            "Decimator (0°) (Marshal)".to_string(),
            "Carnage (90°) (Ambidextrous)".to_string(),
            "Blood Rage (180°) (Crusher)".to_string(),
        ]
    );
    assert!(descriptors.iter().any(|descriptor| {
        matches!(
            descriptor.target,
            RuleOverlayTarget::Paragon {
                kind: ParagonOverlayElementKind::StepPanel,
                route_index: Some(1),
                ..
            }
        ) && descriptor.text.as_deref() == Some("Step 2 / 2")
    }));
    let active_board_outline = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                &descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::BoardEntry,
                    board_name: Some(board_name),
                    ..
                } if &**board_name == "Decimator"
            ) && descriptor.shape == RuleOverlayShape::StrokedRectangle { stroke: 2 }
        })
        .unwrap_or_else(|| panic!("active board outline missing"));
    assert_eq!(
        active_board_outline.color,
        OverlayColor::Paragon(ParagonColor::Goldenrod)
    );
    let board_border = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                &descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::BoardBorder,
                    board_name: Some(board_name),
                    ..
                } if &**board_name == "Decimator"
            )
        })
        .unwrap_or_else(|| panic!("board border missing"));
    assert_eq!(board_border.geometry, rect(425, 145, 530, 530));

    write_paragon_debug_artifact("paragon-full-descriptors.json", &descriptors);
}

#[test]
fn paragon_overlay_collapsed_mode_uses_collapsed_offsets() {
    let descriptors = build_paragon_overlay_descriptors(
        &ParagonOverlayState {
            build_name: "Whirlwind Arsenal".to_string(),
            routes: vec![ImportedParagonRoute {
                step_name: None,
                boards: vec![sample_paragon_board(
                    "Decimator",
                    "Marshal",
                    "90°",
                    &[20, 39],
                )],
            }],
            current_route_index: 0,
            current_board_index: None,
            current_board_name: None,
            mode: ParagonOverlayMode::Collapsed,
        },
        &ParagonOverlayLayout {
            window_geometry: rect(100, 200, 1000, 800),
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
        },
        &FixedTextMeasure::HALF_EM,
    );

    let first_node = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::Node,
                    node_index: Some(0),
                    ..
                }
            )
        })
        .unwrap_or_else(|| panic!("first node missing"));
    assert_eq!(first_node.geometry, rect(740, 325, 10, 10));
    let collapsed_border = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::BoardBorder,
                    ..
                }
            )
        })
        .unwrap_or_else(|| panic!("collapsed board border missing"));
    assert_eq!(collapsed_border.geometry, rect(735, 320, 320, 320));

    write_paragon_debug_artifact("paragon-collapsed-descriptors.json", &descriptors);
}

#[test]
fn paragon_overlay_prefers_board_index_and_keeps_glyph_rotation_text() {
    let descriptors = build_paragon_overlay_descriptors(
        &ParagonOverlayState {
            build_name: "Whirlwind Arsenal".to_string(),
            routes: vec![ImportedParagonRoute {
                step_name: Some("Endgame".to_string()),
                boards: vec![
                    sample_paragon_board("Decimator", "Marshal", "0°", &[0, 1]),
                    sample_paragon_board("Carnage", "Ambidextrous", "270°", &[440]),
                ],
            }],
            current_route_index: 0,
            current_board_index: Some(1),
            current_board_name: Some("Missing".to_string()),
            mode: ParagonOverlayMode::Full,
        },
        &default_paragon_overlay_layout(rect(100, 200, 1280, 720)),
        &FixedTextMeasure::HALF_EM,
    );

    let active_outline = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                &descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::BoardEntry,
                    board_name: Some(board_name),
                    ..
                } if &**board_name == "Carnage"
            ) && descriptor.shape == RuleOverlayShape::StrokedRectangle { stroke: 2 }
        })
        .unwrap_or_else(|| panic!("selected board outline missing"));
    assert_eq!(
        active_outline.color,
        OverlayColor::Paragon(ParagonColor::Goldenrod)
    );
    assert!(descriptors.iter().any(|descriptor| {
        matches!(
            &descriptor.target,
            RuleOverlayTarget::Paragon {
                kind: ParagonOverlayElementKind::BoardEntry,
                board_name: Some(board_name),
                ..
            } if &**board_name == "Carnage"
        ) && descriptor.text.as_deref() == Some("Carnage (270°) (Ambidextrous)")
    }));
    let active_node = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                &descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::BoardBorder,
                    board_name: Some(board_name),
                    ..
                } if &**board_name == "Carnage"
            )
        })
        .unwrap_or_else(|| panic!("selected board border missing"));
    assert_eq!(
        active_node.color,
        OverlayColor::Paragon(ParagonColor::Goldenrod)
    );
}

/// Golden coordinates hand-calculated from upstream's draw code
/// (OverlayHandler.cs:568-863) at the default settings: window 2560x1440,
/// `OverlayFontSize` 18, `ParagonNodeSize` 40, `ParagonNodeSizeCollapsed` 23,
/// collapsed offsets -200 / -50, `ParagonBorderSize` 2. Text is measured with
/// a 9 px advance and a 21.6 px line height (`FixedTextMeasure::HALF_EM` at
/// 18 px), so every expected value below follows from those two numbers.
fn golden_layout() -> ParagonOverlayLayout {
    ParagonOverlayLayout {
        window_geometry: rect(0, 0, 2560, 1440),
        panel_left: 0,
        panel_top: 100,
        panel_height: 50,
        panel_gap: 2,
        panel_text_offset: 20,
        font_px: 18,
        tile_gap: 5,
        tile_count: 21,
        node_size_full: 40,
        node_size_collapsed: 23,
        node_border_size: 2,
        collapsed_left_offset: -200,
        collapsed_top_offset: -50,
    }
}

fn golden_state(mode: ParagonOverlayMode) -> ParagonOverlayState {
    ParagonOverlayState {
        build_name: "Bench Build".to_string(),
        routes: vec![ImportedParagonRoute {
            step_name: Some("Endgame".to_string()),
            boards: vec![
                sample_paragon_board("Decimator", "Marshal", "0°", &[0, 1, 440]),
                sample_paragon_board("Carnage", "Ambidextrous", "270°", &[22, 23]),
            ],
        }],
        current_route_index: 0,
        current_board_index: Some(1),
        current_board_name: None,
        mode,
    }
}

fn golden_descriptors(mode: ParagonOverlayMode) -> Vec<RuleOverlayDescriptor> {
    build_paragon_overlay_descriptors(
        &golden_state(mode),
        &golden_layout(),
        &FixedTextMeasure::HALF_EM,
    )
}

/// The panel's fill (text-bearing) and outline descriptors.
fn panel_descriptors(
    descriptors: &[RuleOverlayDescriptor],
    wanted: ParagonOverlayElementKind,
    wanted_board: Option<usize>,
) -> (&RuleOverlayDescriptor, &RuleOverlayDescriptor) {
    let mut found = descriptors.iter().filter(|descriptor| {
        matches!(
            &descriptor.target,
            RuleOverlayTarget::Paragon { kind, board_index, .. }
                if *kind == wanted && (wanted_board.is_none() || *board_index == wanted_board)
        )
    });
    let fill = found
        .next()
        .unwrap_or_else(|| panic!("{wanted:?} fill missing"));
    let outline = found
        .next()
        .unwrap_or_else(|| panic!("{wanted:?} outline missing"));
    assert!(fill.text.is_some() && outline.text.is_none());
    (fill, outline)
}

fn node(descriptors: &[RuleOverlayDescriptor], index: usize) -> &RuleOverlayDescriptor {
    descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::Node,
                    node_index: Some(node_index),
                    ..
                } if node_index == index
            )
        })
        .unwrap_or_else(|| panic!("node {index} missing"))
}

fn board_frame(descriptors: &[RuleOverlayDescriptor]) -> &RuleOverlayDescriptor {
    descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::BoardBorder,
                    ..
                }
            )
        })
        .unwrap_or_else(|| panic!("board frame missing"))
}

fn brush(brush: ParagonColor) -> OverlayColor {
    OverlayColor::Paragon(brush)
}

#[test]
fn golden_panels_match_upstream_full_board() {
    let descriptors = golden_descriptors(ParagonOverlayMode::Full);

    // Build panel: (0,100)-(textW+40,150), text at x 20, top 100 + (50-h)/2
    // (OH:614-622). "Bench Build" is 11 chars: 99 + 40.
    let (fill, outline) =
        panel_descriptors(&descriptors, ParagonOverlayElementKind::BuildPanel, None);
    assert_eq!(fill.geometry, rect(0, 100, 139, 50));
    assert_eq!(outline.geometry, rect(0, 100, 139, 50));
    assert_eq!(fill.text.as_deref(), Some("Bench Build"));
    assert_eq!(fill.text_origin, Some(Point { x: 20, y: 114 }));
    assert_eq!(fill.color, brush(ParagonColor::Background));
    assert_eq!(outline.color, brush(ParagonColor::Border));
    assert_eq!(outline.shape, RuleOverlayShape::Rectangle);

    // Step panel: (0,152)-(W_b,202), W_b from the longest board row (OH:661-667).
    // "Carnage (270°) (Ambidextrous)" is 29 chars: 261 + 40.
    let (fill, outline) =
        panel_descriptors(&descriptors, ParagonOverlayElementKind::StepPanel, None);
    assert_eq!(fill.geometry, rect(0, 152, 301, 50));
    assert_eq!(outline.geometry, rect(0, 152, 301, 50));
    assert_eq!(fill.text.as_deref(), Some("Step 1 / 1"));
    assert_eq!(fill.text_origin, Some(Point { x: 20, y: 166 }));

    // Board rows: top 204 + 52i, height 48; the active row is outlined in
    // Goldenrod with stroke 2, the others in the border brush with stroke 1
    // (OH:626-657). Text top 204 + 52i + (50-h)/2.
    let (fill, outline) =
        panel_descriptors(&descriptors, ParagonOverlayElementKind::BoardEntry, Some(0));
    assert_eq!(fill.geometry, rect(0, 204, 301, 48));
    assert_eq!(fill.text.as_deref(), Some("Decimator (0°) (Marshal)"));
    assert_eq!(fill.text_origin, Some(Point { x: 20, y: 218 }));
    assert_eq!(outline.shape, RuleOverlayShape::Rectangle);
    assert_eq!(outline.color, brush(ParagonColor::Border));
    let (fill, outline) =
        panel_descriptors(&descriptors, ParagonOverlayElementKind::BoardEntry, Some(1));
    assert_eq!(fill.geometry, rect(0, 256, 301, 48));
    assert_eq!(fill.text_origin, Some(Point { x: 20, y: 270 }));
    assert_eq!(
        outline.shape,
        RuleOverlayShape::StrokedRectangle { stroke: 2 }
    );
    assert_eq!(outline.color, brush(ParagonColor::Goldenrod));
}

#[test]
fn golden_grid_and_frame_match_upstream_full_board() {
    let descriptors = golden_descriptors(ParagonOverlayMode::Full);
    assert_eq!(descriptors.len(), 441 + 1 + 2 * 4);

    // Origin ((W - 21 S) / 2, (H - 21 S) / 2) = (860, 300), pitch S + 5 = 45
    // (OH:670-693).
    assert_eq!(node(&descriptors, 0).geometry, rect(860, 300, 40, 40));
    assert_eq!(node(&descriptors, 1).geometry, rect(905, 300, 40, 40));
    assert_eq!(node(&descriptors, 21).geometry, rect(860, 345, 40, 40));
    assert_eq!(node(&descriptors, 440).geometry, rect(1760, 1200, 40, 40));

    // Active tiles outline with ParagonBorderSize in the active brush, the
    // others with stroke 1 in the border brush.
    let active = node(&descriptors, 22);
    assert_eq!(
        active.shape,
        RuleOverlayShape::StrokedRectangle { stroke: 2 }
    );
    assert_eq!(active.color, brush(ParagonColor::BorderActive));
    let inactive = node(&descriptors, 0);
    assert_eq!(inactive.shape, RuleOverlayShape::Rectangle);
    assert_eq!(inactive.color, brush(ParagonColor::Border));

    // Frame: origin - 5 .. origin + 21 (S + 5), Goldenrod, stroke 1 (OH:696-700).
    let frame = board_frame(&descriptors);
    assert_eq!(frame.geometry, rect(855, 295, 950, 950));
    assert_eq!(frame.shape, RuleOverlayShape::Rectangle);
    assert_eq!(frame.color, brush(ParagonColor::Goldenrod));
}

#[test]
fn golden_collapsed_board_rounds_upstream_half_pixels_to_nearest() {
    let descriptors = golden_descriptors(ParagonOverlayMode::Collapsed);

    // boardLeft = (W - 21 S) + offset = 2077 - 200 = 1877; boardTop =
    // (H - 21 S) / 2 + offset = 478.5 - 50 = 428.5 (OH:806-812), which
    // rounds to 429. Pitch S + 5 = 28.
    assert_eq!(node(&descriptors, 0).geometry, rect(1877, 429, 23, 23));
    assert_eq!(node(&descriptors, 440).geometry, rect(2437, 989, 23, 23));
    let frame = board_frame(&descriptors);
    assert_eq!(frame.geometry, rect(1872, 424, 593, 593));

    // The panels are the full board's (OH:741-803).
    let (fill, _) = panel_descriptors(&descriptors, ParagonOverlayElementKind::BuildPanel, None);
    assert_eq!(fill.geometry, rect(0, 100, 139, 50));
    // Active collapsed tiles use ParagonBorderSize too (OH:824).
    assert_eq!(
        node(&descriptors, 22).shape,
        RuleOverlayShape::StrokedRectangle { stroke: 2 }
    );
}

#[test]
fn golden_full_board_rounds_odd_node_sizes_to_nearest() {
    let mut layout = golden_layout();
    layout.node_size_full = 41;
    let descriptors = build_paragon_overlay_descriptors(
        &golden_state(ParagonOverlayMode::Full),
        &layout,
        &FixedTextMeasure::HALF_EM,
    );
    // (1440 - 861) / 2 = 289.5 -> 290; (2560 - 861) / 2 = 849.5 -> 850.
    assert_eq!(node(&descriptors, 0).geometry, rect(850, 290, 41, 41));
}

#[test]
fn board_panels_carry_the_truncated_width_for_the_hover_zone() {
    // Font 17 px: 8.5 px per char; "Carnage (270°) (Ambidextrous)" is 29
    // chars, 246.5 + 40 = 286.5 px: drawn as 287, `(int)` gives 286
    // (OverlayHandler.cs:636).
    let mut layout = golden_layout();
    layout.font_px = 17;
    let descriptors = build_paragon_overlay_descriptors(
        &golden_state(ParagonOverlayMode::Full),
        &layout,
        &FixedTextMeasure::HALF_EM,
    );
    let hit_width = |descriptor: &RuleOverlayDescriptor| match &descriptor.target {
        RuleOverlayTarget::Paragon {
            panel_hit_width, ..
        } => *panel_hit_width,
        _ => None,
    };
    let (step, _) = panel_descriptors(&descriptors, ParagonOverlayElementKind::StepPanel, None);
    assert_eq!(step.geometry.width, 287);
    assert_eq!(hit_width(step), Some(286));
    let (row, _) = panel_descriptors(&descriptors, ParagonOverlayElementKind::BoardEntry, Some(0));
    assert_eq!(hit_width(row), Some(286));
    let (build, _) = panel_descriptors(&descriptors, ParagonOverlayElementKind::BuildPanel, None);
    assert_eq!(hit_width(build), None);
}

#[test]
fn golden_warning_panel_matches_upstream() {
    let descriptors = build_paragon_overlay_descriptors(
        &ParagonOverlayState {
            routes: Vec::new(),
            ..golden_state(ParagonOverlayMode::Full)
        },
        &golden_layout(),
        &FixedTextMeasure::HALF_EM,
    );
    // "No paragon boards available." is 28 chars: 252 + 40 (OH:841-863).
    let (fill, outline) =
        panel_descriptors(&descriptors, ParagonOverlayElementKind::WarningPanel, None);
    assert_eq!(descriptors.len(), 2);
    assert_eq!(fill.geometry, rect(0, 100, 292, 50));
    assert_eq!(fill.text.as_deref(), Some("No paragon boards available."));
    assert_eq!(fill.text_origin, Some(Point { x: 20, y: 114 }));
    assert_eq!(outline.color, brush(ParagonColor::Border));
}

#[test]
fn an_empty_step_draws_nothing() {
    let mut state = golden_state(ParagonOverlayMode::Full);
    state.routes[0].boards.clear();
    assert!(build_paragon_overlay_descriptors(
        &state,
        &golden_layout(),
        &FixedTextMeasure::HALF_EM
    )
    .is_empty());
}

#[derive(Debug)]
struct NoFace;

impl TextMeasure for NoFace {
    fn measure(&self, _text: &str, _font_px: f32) -> Option<TextExtent> {
        None
    }
}

#[test]
fn without_a_font_face_no_text_panel_is_built_and_no_width_is_guessed() {
    let descriptors = build_paragon_overlay_descriptors(
        &golden_state(ParagonOverlayMode::Full),
        &golden_layout(),
        &NoFace,
    );
    // The grid and its frame need no text and are still built.
    assert_eq!(descriptors.len(), 441 + 1);
    assert!(descriptors
        .iter()
        .all(|descriptor| descriptor.text.is_none() && descriptor.text_origin.is_none()));

    let warning = build_paragon_overlay_descriptors(
        &ParagonOverlayState {
            routes: Vec::new(),
            ..golden_state(ParagonOverlayMode::Full)
        },
        &golden_layout(),
        &NoFace,
    );
    assert!(warning.is_empty());
}

#[test]
fn the_longest_board_is_picked_by_key_length_and_the_first_wins_ties() {
    let boards = [
        sample_paragon_board("Ab", "Gl", "0°", &[]),
        sample_paragon_board("Cd", "Xx", "0°", &[]),
        sample_paragon_board("Longer", "Gl", "0°", &[]),
        sample_paragon_board("Other", "Gla", "0°", &[]),
    ];
    // Keys: "Ab 0° Gl" (8), "Cd 0° Xx" (8), "Longer 0° Gl" (12), "Other 0° Gla" (12).
    let longest = longest_board(&boards).map(|board| board.name.as_str());
    assert_eq!(longest, Some("Longer"));
    assert_eq!(
        longest_board(&boards[..2]).map(|b| b.name.as_str()),
        Some("Ab")
    );
    assert!(longest_board(&[]).is_none());
}

fn write_paragon_debug_artifact(file_name: &str, descriptors: &[RuleOverlayDescriptor]) {
    let json = serde_json::Value::Array(
        descriptors
            .iter()
            .map(rule_overlay_descriptor_json)
            .collect::<Vec<_>>(),
    );
    write_overlay_debug_json(file_name, &json);
}

fn rule_overlay_descriptor_json(descriptor: &RuleOverlayDescriptor) -> serde_json::Value {
    serde_json::json!({
        "target": rule_overlay_target_json(&descriptor.target),
        "marker_type": format!("{:?}", descriptor.marker_type),
        "shape": format!("{:?}", descriptor.shape),
        "geometry": rect_json(descriptor.geometry),
        "color": descriptor.color,
        "text": descriptor.text,
        "z_order_group": format!("{:?}", descriptor.z_order_group),
        "geometry_source": descriptor.geometry_source,
        "source": {
            "upstream_file": descriptor.source.upstream_file,
            "upstream_method": descriptor.source.upstream_method,
            "branch": descriptor.source.branch,
        },
    })
}

fn rule_overlay_target_json(target: &RuleOverlayTarget) -> serde_json::Value {
    match target {
        RuleOverlayTarget::Affix { affix_id, row_id } => serde_json::json!({
            "kind": "Affix",
            "affix_id": affix_id,
            "row_id": row_id.map(AffixRowId::get),
        }),
        RuleOverlayTarget::Power {
            power_id,
            power_kind,
        } => serde_json::json!({
            "kind": "Power",
            "power_id": power_id,
            "power_kind": format!("{:?}", power_kind),
        }),
        RuleOverlayTarget::Trade => serde_json::json!({ "kind": "Trade" }),
        RuleOverlayTarget::ItemPower => serde_json::json!({ "kind": "ItemPower" }),
        RuleOverlayTarget::Rune { rune_id } => serde_json::json!({
            "kind": "Rune",
            "rune_id": rune_id,
        }),
        RuleOverlayTarget::Sigil { sigil_id } => serde_json::json!({
            "kind": "Sigil",
            "sigil_id": sigil_id,
        }),
        RuleOverlayTarget::Paragon {
            kind,
            route_index,
            board_index,
            board_name,
            node_index,
            ..
        } => serde_json::json!({
            "kind": "Paragon",
            "element_kind": format!("{:?}", kind),
            "route_index": route_index,
            "board_index": board_index,
            "board_name": board_name,
            "node_index": node_index,
        }),
    }
}

fn rect_json(rect: d4lc_core::Rect) -> serde_json::Value {
    serde_json::json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height,
    })
}
