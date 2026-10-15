use super::*;
use d4lc_core::text_measure::FixedTextMeasure;
use d4lc_core::{Point, Rect};
use d4lc_rules::{
    ImportedParagonBoard, ImportedParagonRoute, ParagonOverlayLayout, ParagonOverlayMode,
};
use std::time::Duration;

fn layout(font_px: u32) -> ParagonOverlayLayout {
    ParagonOverlayLayout {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        },
        panel_left: 0,
        panel_top: 100,
        panel_height: 50,
        panel_gap: 2,
        panel_text_offset: 20,
        font_px,
        tile_gap: 5,
        tile_count: 21,
        node_size_full: 40,
        node_size_collapsed: 23,
        node_border_size: 2,
        collapsed_left_offset: -200,
        collapsed_top_offset: -50,
    }
}

fn board(name: &str) -> ImportedParagonBoard {
    ImportedParagonBoard {
        index: 0,
        name: name.to_string(),
        glyph: "g".to_string(),
        rotation: "0".to_string(),
        active_nodes: vec![0],
    }
}

/// Two steps; the first has three boards. Board panels are `W_b` wide. The
/// live loop rebuilds the descriptors from the navigation after every change,
/// so the helper applies it the same way.
fn descriptors_for(font_px: u32, navigation: &ParagonNavigation) -> Vec<RuleOverlayDescriptor> {
    let mut state = ParagonOverlayState {
        build_name: "Build".to_string(),
        routes: vec![
            ImportedParagonRoute {
                step_name: None,
                boards: vec![board("A"), board("B"), board("C")],
            },
            ImportedParagonRoute {
                step_name: None,
                boards: vec![board("D")],
            },
        ],
        current_route_index: 0,
        current_board_index: Some(0),
        current_board_name: None,
        mode: ParagonOverlayMode::Full,
    };
    navigation.apply(&mut state);
    d4lc_rules::build_paragon_overlay_descriptors(
        &state,
        &layout(font_px),
        &FixedTextMeasure::HALF_EM,
    )
}

fn descriptors(font_px: u32) -> Vec<RuleOverlayDescriptor> {
    descriptors_for(font_px, &ParagonNavigation::default())
}

fn at(x: i32, y: i32) -> Option<Point> {
    Some(Point { x, y })
}

#[test]
fn rows_are_selected_by_the_upstream_integer_division() {
    let now = Instant::now();
    let mut navigation = ParagonNavigation::default();
    // Observes at `y` on the descriptors of the current selection and
    // reports whether the selection changed.
    let hover = |navigation: &mut ParagonNavigation, y: i32| {
        let descriptors = descriptors_for(18, navigation);
        navigation.observe(&descriptors, at(10, y), now)
    };
    // (Y - 202) / 52: Y 202 is no row, 203..=253 is row 0 (the gap belongs
    // to the row above), 254 starts row 1.
    assert!(!hover(&mut navigation, 202));
    assert!(!hover(&mut navigation, 203));
    assert!(!hover(&mut navigation, 253));
    assert!(hover(&mut navigation, 254));
    assert_eq!(navigation.board_index, 1);
    assert!(!hover(&mut navigation, 255));
    // The 2 px gap under row 1 still selects row 1.
    assert!(!hover(&mut navigation, 305));
    assert!(hover(&mut navigation, 306));
    assert_eq!(navigation.board_index, 2);
}

#[test]
fn an_index_past_the_last_row_keeps_the_selection() {
    let descriptors = descriptors(18);
    let now = Instant::now();
    let mut navigation = ParagonNavigation::default();
    assert!(navigation.observe(&descriptors, at(10, 254), now));
    let descriptors = descriptors_for(18, &navigation);
    assert!(!navigation.observe(&descriptors, at(10, 900), now));
    assert_eq!(navigation.board_index, 1);
}

#[test]
fn the_x_threshold_is_the_wider_of_the_board_panel_and_100() {
    // Font 4 px: 2 px per char, "A (0) (g)" is 9 chars, so W_b = 18 + 40 = 58.
    let narrow = descriptors(4);
    let now = Instant::now();
    let mut navigation = ParagonNavigation::default();
    assert!(navigation.observe(&narrow, at(99, 254), now));
    navigation = ParagonNavigation::default();
    assert!(!navigation.observe(&narrow, at(100, 254), now));
    // Font 18 px: W_b = 9 * 9 + 40 = 121 > 100.
    let wide = descriptors(18);
    navigation = ParagonNavigation::default();
    assert!(navigation.observe(&wide, at(120, 254), now));
    navigation = ParagonNavigation::default();
    assert!(!navigation.observe(&wide, at(121, 254), now));
}

#[test]
fn hovering_the_step_panel_advances_the_step_every_500_ms() {
    let descriptors = descriptors(18);
    let start = Instant::now();
    let mut navigation = ParagonNavigation::default();
    // 152 < Y < 202 is the step zone.
    assert!(!navigation.observe(&descriptors, at(10, 152), start));
    assert!(!navigation.observe(&descriptors, at(10, 153), start));
    assert!(!navigation.observe(
        &descriptors,
        at(10, 153),
        start + Duration::from_millis(499)
    ));
    assert!(navigation.observe(
        &descriptors,
        at(10, 153),
        start + Duration::from_millis(500)
    ));
    assert_eq!(navigation.route_index, 1);
    // The next tick is again 500 ms later, on the descriptors the live loop
    // rebuilt for the new step.
    let descriptors = descriptors_for(18, &navigation);
    assert!(!navigation.observe(
        &descriptors,
        at(10, 201),
        start + Duration::from_millis(900)
    ));
    assert!(navigation.observe(
        &descriptors,
        at(10, 201),
        start + Duration::from_millis(1000)
    ));
    assert_eq!(
        navigation.route_index, 2,
        "the rebuilt step panel is step 2"
    );
}

#[test]
fn leaving_the_step_panel_stops_the_timer() {
    let descriptors = descriptors(18);
    let start = Instant::now();
    let mut navigation = ParagonNavigation::default();
    assert!(!navigation.observe(&descriptors, at(10, 160), start));
    assert!(!navigation.observe(
        &descriptors,
        at(900, 160),
        start + Duration::from_millis(300)
    ));
    assert!(!navigation.observe(
        &descriptors,
        at(10, 160),
        start + Duration::from_millis(400)
    ));
    // The timer restarted at 400 ms, so 600 ms after the first sample is not
    // yet a tick.
    assert!(!navigation.observe(
        &descriptors,
        at(10, 160),
        start + Duration::from_millis(600)
    ));
    assert!(navigation.observe(
        &descriptors,
        at(10, 160),
        start + Duration::from_millis(900)
    ));
}

#[test]
fn a_missing_cursor_or_panel_changes_nothing() {
    let now = Instant::now();
    let mut navigation = ParagonNavigation::default();
    assert!(!navigation.observe(&descriptors(18), None, now));
    assert!(!navigation.observe(&[], at(10, 254), now));
}

#[test]
fn the_x_threshold_truncates_the_panel_width_like_upstream() {
    // Font 17 px: 8.5 px per char, "A (0) (g)" is 9 chars, so the panel is
    // 76.5 + 40 = 116.5 px: drawn as 117, `(int)` gives 116.
    let wide = descriptors(17);
    let now = Instant::now();
    let mut navigation = ParagonNavigation::default();
    assert!(!navigation.observe(&wide, at(116, 254), now));
    assert!(navigation.observe(&wide, at(115, 254), now));
}
