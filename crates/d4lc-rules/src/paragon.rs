use crate::import::{ImportedParagonBoard, ImportedParagonRoute};
use crate::model::RuleSourceNote;
use crate::overlay::{
    RuleOverlayDescriptor, RuleOverlayMarkerType, RuleOverlayShape, RuleOverlayTarget,
    RuleOverlayZOrderGroup,
};
use d4lc_core::text_measure::TextMeasure;
use d4lc_core::{Point, Rect};
use serde::Serialize;
use std::borrow::Cow;
use std::sync::Arc;

/// The brushes upstream draws the paragon overlay with
/// (`OverlayHandler.cs:897-902`, `Colors.Goldenrod` at :644, :696).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ParagonColor {
    /// `backgroundTransparent`, ARGB (200, 25, 25, 25), the panel fill.
    Background,
    /// `border`, RGB (75, 75, 75): panel outlines and inactive tiles.
    Border,
    /// `borderactive`, RGB (20, 220, 80): active tiles.
    BorderActive,
    /// `text`, RGB (200, 200, 200): all paragon text.
    Text,
    /// `Colors.Goldenrod`: the active board row and the board frame.
    Goldenrod,
}

impl ParagonColor {
    /// The straight-alpha ARGB pixel of the brush.
    pub const fn argb(self) -> u32 {
        match self {
            Self::Background => 0xC819_1919,
            Self::Border => 0xFF4B_4B4B,
            Self::BorderActive => 0xFF14_DC50,
            Self::Text => 0xFFC8_C8C8,
            Self::Goldenrod => 0xFFDA_A520,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParagonOverlayMode {
    Full,
    Collapsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ParagonOverlayElementKind {
    WarningPanel,
    BuildPanel,
    StepPanel,
    BoardEntry,
    Node,
    BoardBorder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParagonOverlayState {
    pub build_name: String,
    pub routes: Vec<ImportedParagonRoute>,
    pub current_route_index: usize,
    pub current_board_index: Option<usize>,
    pub current_board_name: Option<String>,
    pub mode: ParagonOverlayMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParagonOverlayLayout {
    /// The game window rect the overlay surface covers. Used to size and
    /// center the board and its panels inside the window — never to shift a
    /// descriptor: `ParagonOverlayLayout` panel and node rects are already
    /// window-relative, like every other `RuleOverlayDescriptor::geometry`.
    pub window_geometry: Rect,
    pub panel_left: i32,
    pub panel_top: i32,
    pub panel_height: u32,
    pub panel_gap: u32,
    pub panel_text_offset: u32,
    /// The overlay font size every paragon text is measured and drawn at
    /// (`OverlayFontSize`, OverlayHandler.cs:606).
    pub font_px: u32,
    pub tile_gap: u32,
    pub tile_count: u32,
    pub node_size_full: u32,
    pub node_size_collapsed: u32,
    pub node_border_size: u32,
    pub collapsed_left_offset: i32,
    pub collapsed_top_offset: i32,
}

/// Stroke of every panel outline and inactive tile (`strokeBuild`,
/// `strokeBoard` = 1, OverlayHandler.cs:612, :628, :686).
const PANEL_STROKE: u32 = 1;

/// Stroke of the active board row's outline (`strokeBoard + 1`,
/// OverlayHandler.cs:644, :779).
const ACTIVE_ROW_STROKE: u32 = PANEL_STROKE + 1;

/// Builds the paragon overlay of `state` as upstream draws it
/// (`DrawGraphicsParagon`, OverlayHandler.cs:568-702;
/// `DrawGraphicsParagonCollapsed`, :704-839; `DrawGraphicsParagonWarning`,
/// :841-863).
///
/// Panel widths come from `measure` on the overlay font
/// (`MeasureString(...).X + 2 * textOffset`). While the face is unavailable
/// the measurement is `None`: the text panels are then not built (no width is
/// estimated) and only the board grid and frame, which need no text, are.
/// Upstream's float coordinates are rounded to the nearest pixel.
pub fn build_paragon_overlay_descriptors(
    state: &ParagonOverlayState,
    layout: &ParagonOverlayLayout,
    measure: &dyn TextMeasure,
) -> Vec<RuleOverlayDescriptor> {
    if state.routes.is_empty() {
        return paragon_warning_panel(layout, measure);
    }

    let route_index = normalized_route_index(state.current_route_index, state.routes.len());
    let route = &state.routes[route_index];
    if route.boards.is_empty() {
        return Vec::new();
    }

    let board_index = selected_paragon_board_index(
        route,
        state.current_board_index,
        state.current_board_name.as_deref(),
    );
    let board = &route.boards[board_index];

    let mut descriptors = Vec::with_capacity(
        (layout.tile_count * layout.tile_count) as usize + route.boards.len() * 2 + 5,
    );
    if let Some(panels) = ParagonPanels::measure(state, route, layout, measure) {
        panels.push_descriptors(state, route_index, board_index, layout, &mut descriptors);
    }
    push_paragon_board(
        &mut descriptors,
        state.mode,
        layout,
        route_index,
        board_index,
        board,
    );
    descriptors
}

/// Measured widths and text heights of the build, step and board-row panels.
struct ParagonPanels {
    build_width: u32,
    build_text_height: f32,
    board_width: u32,
    /// The board width truncated, for the hover zone.
    board_hit_width: u32,
    board_text_height: f32,
}

impl ParagonPanels {
    /// `None` while the font face cannot measure.
    fn measure(
        state: &ParagonOverlayState,
        route: &ImportedParagonRoute,
        layout: &ParagonOverlayLayout,
        measure: &dyn TextMeasure,
    ) -> Option<Self> {
        let font_px = layout.font_px as f32;
        let build = measure.measure(&state.build_name, font_px)?;
        let longest = longest_board(&route.boards)?;
        let board = measure.measure(&paragon_board_label(longest), font_px)?;
        let text_offset = layout.panel_text_offset as f32;
        Some(Self {
            build_width: panel_width(build.width, text_offset),
            build_text_height: build.height,
            board_width: panel_width(board.width, text_offset),
            board_hit_width: (board.width + 2.0 * text_offset).max(0.0) as u32,
            board_text_height: board.height,
        })
    }

    fn push_descriptors(
        &self,
        state: &ParagonOverlayState,
        route_index: usize,
        board_index: usize,
        layout: &ParagonOverlayLayout,
        descriptors: &mut Vec<RuleOverlayDescriptor>,
    ) {
        let route = &state.routes[route_index];
        let build_rect = Rect {
            x: layout.panel_left,
            y: layout.panel_top,
            width: self.build_width,
            height: layout.panel_height,
        };
        descriptors.extend(paragon_panel_descriptors(ParagonPanelDescriptorSpec {
            geometry: build_rect,
            text: state.build_name.clone(),
            text_origin: text_origin(build_rect, layout, self.build_text_height),
            border_color: ParagonColor::Border,
            border_stroke: PANEL_STROKE,
            kind: ParagonOverlayElementKind::BuildPanel,
            route_index: None,
            board_index: None,
            board_name: None,
            node_index: None,
            hit_width: None,
            source: paragon_source_note(state.mode, "current build panel"),
        }));

        let step_rect = Rect {
            x: layout.panel_left,
            y: layout.panel_top + layout.panel_height as i32 + layout.panel_gap as i32,
            width: self.board_width,
            height: layout.panel_height,
        };
        descriptors.extend(paragon_panel_descriptors(ParagonPanelDescriptorSpec {
            geometry: step_rect,
            text: format!("Step {} / {}", route_index + 1, state.routes.len()),
            text_origin: text_origin(step_rect, layout, self.build_text_height),
            border_color: ParagonColor::Border,
            border_stroke: PANEL_STROKE,
            kind: ParagonOverlayElementKind::StepPanel,
            route_index: Some(route_index),
            board_index: None,
            board_name: None,
            node_index: None,
            hit_width: Some(self.board_hit_width),
            source: paragon_step_source_note(
                state.mode,
                route_index,
                state.routes.len(),
                route.step_name.as_deref(),
            ),
        }));

        // The board rows start one gap below the step panel; every row is
        // drawn from `panelTopBoard + 2` to `panelTopBoard + 50`
        // (OverlayHandler.cs:626-657), so a row is the panel height less
        // the gap and the pitch is panel height plus gap.
        let list_top =
            layout.panel_top + (layout.panel_height * 2) as i32 + (layout.panel_gap * 2) as i32;
        let pitch = (layout.panel_height + layout.panel_gap) as i32;
        let row_height = layout.panel_height.saturating_sub(layout.panel_gap);
        for (index, route_board) in route.boards.iter().enumerate() {
            let row_rect = Rect {
                x: layout.panel_left,
                y: list_top + index as i32 * pitch,
                width: self.board_width,
                height: row_height,
            };
            let (border_color, border_stroke) = if index == board_index {
                (ParagonColor::Goldenrod, ACTIVE_ROW_STROKE)
            } else {
                (ParagonColor::Border, PANEL_STROKE)
            };
            descriptors.extend(paragon_panel_descriptors(ParagonPanelDescriptorSpec {
                geometry: row_rect,
                text: paragon_board_label(route_board),
                // The row text is centred in the 50 px slot, not the 48 px
                // outline (`panelTopBoard + 2 + (panelHeightBoard - h) / 2`).
                text_origin: text_origin(row_rect, layout, self.board_text_height),
                border_color,
                border_stroke,
                kind: ParagonOverlayElementKind::BoardEntry,
                route_index: Some(route_index),
                board_index: Some(index),
                board_name: Some(Arc::from(route_board.name.as_str())),
                node_index: None,
                hit_width: Some(self.board_hit_width),
                source: paragon_source_note(state.mode, "board entry"),
            }));
        }
    }
}

/// Upstream's `MaxBy(t => "{Name} {Rotation} {Glyph}".Length)`: the board with
/// the longest key, the first one on ties. `String.Length` counts UTF-16 code
/// units.
fn longest_board(boards: &[ImportedParagonBoard]) -> Option<&ImportedParagonBoard> {
    let mut longest: Option<(&ImportedParagonBoard, usize)> = None;
    for board in boards {
        let key_length = format!("{} {} {}", board.name, board.rotation, board.glyph)
            .encode_utf16()
            .count();
        if longest.is_none_or(|(_, length)| key_length > length) {
            longest = Some((board, key_length));
        }
    }
    longest.map(|(board, _)| board)
}

/// `textWidth + 2 * textOffset`, rounded to the nearest pixel.
fn panel_width(text_width: f32, text_offset: f32) -> u32 {
    round_px(text_width + 2.0 * text_offset).max(0) as u32
}

/// Where the panel's text line starts: `textOffset` right of the panel and
/// `panelTop + (panelHeight - textHeight) / 2` down (OverlayHandler.cs:621-622,
/// :652-653, :665-666), where `panelHeight` is the full panel height.
fn text_origin(panel: Rect, layout: &ParagonOverlayLayout, text_height: f32) -> Point {
    Point {
        x: panel.x + layout.panel_text_offset as i32,
        y: round_px(panel.y as f32 + (layout.panel_height as f32 - text_height) / 2.0),
    }
}

fn round_px(value: f32) -> i32 {
    value.round() as i32
}

/// The 21x21 tile grid and its frame (OverlayHandler.cs:669-701,
/// :805-838).
fn push_paragon_board(
    descriptors: &mut Vec<RuleOverlayDescriptor>,
    mode: ParagonOverlayMode,
    layout: &ParagonOverlayLayout,
    route_index: usize,
    board_index: usize,
    board: &ImportedParagonBoard,
) {
    let node_size = match mode {
        ParagonOverlayMode::Full => layout.node_size_full,
        ParagonOverlayMode::Collapsed => layout.node_size_collapsed,
    };
    let tile_stride = node_size + layout.tile_gap;
    let window_width = layout.window_geometry.width as f32;
    let window_height = layout.window_geometry.height as f32;
    let span = (node_size * layout.tile_count) as f32;
    let (board_left, board_top) = match mode {
        ParagonOverlayMode::Full => ((window_width - span) / 2.0, (window_height - span) / 2.0),
        ParagonOverlayMode::Collapsed => (
            (window_width - span) + layout.collapsed_left_offset as f32,
            ((window_height - span) / 2.0) + layout.collapsed_top_offset as f32,
        ),
    };

    let board_name: Arc<str> = Arc::from(board.name.as_str());
    // Shared by all 441 nodes: literals and one `Arc`, so a node costs no
    // allocation.
    let node_geometry_source: Cow<'static, str> = match mode {
        ParagonOverlayMode::Full => "paragon_full_node",
        ParagonOverlayMode::Collapsed => "paragon_collapsed_node",
    }
    .into();
    let node_source = paragon_source_note(mode, "board node");
    let mut active = vec![false; (layout.tile_count * layout.tile_count) as usize];
    for &node in &board.active_nodes {
        if let Some(slot) = active.get_mut(node) {
            *slot = true;
        }
    }
    for y in 0..layout.tile_count {
        for x in 0..layout.tile_count {
            let node_index = (y * layout.tile_count + x) as usize;
            let is_active = active[node_index];
            descriptors.push(RuleOverlayDescriptor {
                target: RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::Node,
                    route_index: Some(route_index),
                    board_index: Some(board_index),
                    board_name: Some(Arc::clone(&board_name)),
                    node_index: Some(node_index),
                    panel_hit_width: None,
                },
                marker_type: RuleOverlayMarkerType::Paragon,
                shape: if is_active {
                    RuleOverlayShape::StrokedRectangle {
                        stroke: layout.node_border_size,
                    }
                } else {
                    RuleOverlayShape::Rectangle
                },
                geometry: Rect {
                    x: round_px(board_left + (x * tile_stride) as f32),
                    y: round_px(board_top + (y * tile_stride) as f32),
                    width: node_size,
                    height: node_size,
                },
                color: if is_active {
                    ParagonColor::BorderActive
                } else {
                    ParagonColor::Border
                }
                .into(),
                text: None,
                text_origin: None,
                z_order_group: RuleOverlayZOrderGroup::Marker,
                geometry_source: node_geometry_source.clone(),
                source: node_source.clone(),
            });
        }
    }

    let board_span = tile_stride * layout.tile_count;
    descriptors.push(RuleOverlayDescriptor {
        target: RuleOverlayTarget::Paragon {
            kind: ParagonOverlayElementKind::BoardBorder,
            route_index: Some(route_index),
            board_index: Some(board_index),
            board_name: Some(board_name),
            node_index: None,
            panel_hit_width: None,
        },
        marker_type: RuleOverlayMarkerType::Paragon,
        shape: RuleOverlayShape::Rectangle,
        geometry: Rect {
            x: round_px(board_left - layout.tile_gap as f32),
            y: round_px(board_top - layout.tile_gap as f32),
            width: board_span + layout.tile_gap,
            height: board_span + layout.tile_gap,
        },
        color: ParagonColor::Goldenrod.into(),
        text: None,
        text_origin: None,
        z_order_group: RuleOverlayZOrderGroup::Marker,
        geometry_source: match mode {
            ParagonOverlayMode::Full => "paragon_full_board_border",
            ParagonOverlayMode::Collapsed => "paragon_collapsed_board_border",
        }
        .into(),
        source: paragon_source_note(mode, "board border"),
    });
}

fn paragon_warning_panel(
    layout: &ParagonOverlayLayout,
    measure: &dyn TextMeasure,
) -> Vec<RuleOverlayDescriptor> {
    const WARNING_TEXT: &str = "No paragon boards available.";
    let Some(extent) = measure.measure(WARNING_TEXT, layout.font_px as f32) else {
        return Vec::new();
    };
    let warning_rect = Rect {
        x: layout.panel_left,
        y: layout.panel_top,
        width: panel_width(extent.width, layout.panel_text_offset as f32),
        height: layout.panel_height,
    };
    paragon_panel_descriptors(ParagonPanelDescriptorSpec {
        geometry: warning_rect,
        text: WARNING_TEXT.to_string(),
        text_origin: text_origin(warning_rect, layout, extent.height),
        border_color: ParagonColor::Border,
        border_stroke: PANEL_STROKE,
        kind: ParagonOverlayElementKind::WarningPanel,
        route_index: None,
        board_index: None,
        board_name: None,
        node_index: None,
        hit_width: None,
        source: RuleSourceNote::new(
            "D4Companion.Services/OverlayHandler.cs",
            "OverlayHandler.DrawGraphicsParagonWarning",
            "no paragon boards available",
        ),
    })
}

struct ParagonPanelDescriptorSpec {
    geometry: Rect,
    text: String,
    text_origin: Point,
    border_color: ParagonColor,
    border_stroke: u32,
    kind: ParagonOverlayElementKind,
    route_index: Option<usize>,
    board_index: Option<usize>,
    board_name: Option<Arc<str>>,
    node_index: Option<usize>,
    /// See `RuleOverlayTarget::Paragon::panel_hit_width`.
    hit_width: Option<u32>,
    source: RuleSourceNote,
}

fn paragon_panel_descriptors(spec: ParagonPanelDescriptorSpec) -> Vec<RuleOverlayDescriptor> {
    vec![
        RuleOverlayDescriptor {
            target: RuleOverlayTarget::Paragon {
                kind: spec.kind,
                route_index: spec.route_index,
                board_index: spec.board_index,
                board_name: spec.board_name.clone(),
                node_index: spec.node_index,
                panel_hit_width: spec.hit_width,
            },
            marker_type: RuleOverlayMarkerType::Paragon,
            shape: RuleOverlayShape::TextPanel,
            geometry: spec.geometry,
            color: ParagonColor::Background.into(),
            text: Some(spec.text),
            text_origin: Some(spec.text_origin),
            z_order_group: RuleOverlayZOrderGroup::TextPanel,
            geometry_source: paragon_panel_geometry_source(
                spec.kind,
                spec.route_index,
                spec.board_index,
                spec.node_index,
            )
            .into(),
            source: spec.source.clone(),
        },
        RuleOverlayDescriptor {
            target: RuleOverlayTarget::Paragon {
                kind: spec.kind,
                route_index: spec.route_index,
                board_index: spec.board_index,
                board_name: spec.board_name,
                node_index: spec.node_index,
                panel_hit_width: spec.hit_width,
            },
            marker_type: RuleOverlayMarkerType::Paragon,
            shape: if spec.border_stroke == PANEL_STROKE {
                RuleOverlayShape::Rectangle
            } else {
                RuleOverlayShape::StrokedRectangle {
                    stroke: spec.border_stroke,
                }
            },
            geometry: spec.geometry,
            color: spec.border_color.into(),
            text: None,
            text_origin: None,
            z_order_group: RuleOverlayZOrderGroup::TextPanel,
            geometry_source: paragon_panel_geometry_source(
                spec.kind,
                spec.route_index,
                spec.board_index,
                spec.node_index,
            )
            .into(),
            source: spec.source,
        },
    ]
}

fn paragon_panel_geometry_source(
    kind: ParagonOverlayElementKind,
    route_index: Option<usize>,
    board_index: Option<usize>,
    node_index: Option<usize>,
) -> String {
    let base = match kind {
        ParagonOverlayElementKind::WarningPanel => "paragon_warning_panel".to_string(),
        ParagonOverlayElementKind::BuildPanel => "paragon_build_panel".to_string(),
        ParagonOverlayElementKind::StepPanel => "paragon_step_panel".to_string(),
        ParagonOverlayElementKind::BoardEntry => "paragon_board_entry".to_string(),
        ParagonOverlayElementKind::Node => "paragon_node".to_string(),
        ParagonOverlayElementKind::BoardBorder => "paragon_board_border".to_string(),
    };
    match (route_index, board_index, node_index) {
        (Some(route), Some(board), Some(node)) => format!("{base}[{route}][{board}][{node}]"),
        (Some(route), Some(board), None) => format!("{base}[{route}][{board}]"),
        (Some(route), None, None) => format!("{base}[{route}]"),
        _ => base,
    }
}

fn paragon_source_note(
    mode: ParagonOverlayMode,
    branch: impl Into<Cow<'static, str>>,
) -> RuleSourceNote {
    RuleSourceNote::new(
        "D4Companion.Services/OverlayHandler.cs",
        match mode {
            ParagonOverlayMode::Full => "OverlayHandler.DrawGraphicsParagon",
            ParagonOverlayMode::Collapsed => "OverlayHandler.DrawGraphicsParagonCollapsed",
        },
        branch,
    )
}

fn paragon_step_source_note(
    mode: ParagonOverlayMode,
    route_index: usize,
    route_count: usize,
    step_name: Option<&str>,
) -> RuleSourceNote {
    let branch = match step_name.filter(|value| !value.trim().is_empty()) {
        Some(step_name) => format!("step {} / {} ({step_name})", route_index + 1, route_count),
        None => format!("step {} / {}", route_index + 1, route_count),
    };
    paragon_source_note(mode, branch)
}

fn paragon_board_label(board: &ImportedParagonBoard) -> String {
    format!("{} ({}) ({})", board.name, board.rotation, board.glyph)
}

fn normalized_route_index(index: usize, route_count: usize) -> usize {
    if index >= route_count {
        0
    } else {
        index
    }
}

fn selected_paragon_board_index(
    route: &ImportedParagonRoute,
    current_board_index: Option<usize>,
    current_board_name: Option<&str>,
) -> usize {
    if let Some(index) = current_board_index {
        if index < route.boards.len() {
            return index;
        }
    }
    if let Some(name) = current_board_name {
        if let Some(index) = route.boards.iter().position(|board| board.name == name) {
            return index;
        }
    }
    0
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
