use crate::model::{
    RuleAffixMatch, RuleCheckStatus, RuleEvaluationResult, RulePowerKind, RulePowerMatch,
    RuleSourceNote,
};
use crate::paragon::{
    build_paragon_overlay_descriptors, ParagonColor, ParagonOverlayElementKind,
    ParagonOverlayLayout, ParagonOverlayState,
};
use d4lc_core::text_measure::TextMeasure;
use d4lc_core::{AffixRowId, Point, Rect};
use serde::Serialize;
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind")]
pub enum RuleOverlayTarget {
    Affix {
        affix_id: String,
        row_id: Option<AffixRowId>,
    },
    Power {
        power_id: String,
        power_kind: RulePowerKind,
    },
    Trade,
    ItemPower,
    Rune {
        rune_id: String,
    },
    Sigil {
        sigil_id: String,
    },
    Paragon {
        #[serde(rename = "element_kind")]
        kind: ParagonOverlayElementKind,
        route_index: Option<usize>,
        board_index: Option<usize>,
        /// Shared by every descriptor of the board.
        board_name: Option<Arc<str>>,
        node_index: Option<usize>,
        /// On the step and board-row panels: the board panel width truncated
        /// to whole pixels (`(int)panelWidthBoard`, OverlayHandler.cs:636),
        /// which bounds the mouse hover zone (:959, :964). The drawn width
        /// is rounded.
        panel_hit_width: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RuleOverlayShape {
    Circle,
    Rectangle,
    /// Outline-only rectangle with the configured active paragon node stroke.
    StrokedRectangle {
        stroke: u32,
    },
    Triangle,
    TextPanel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RuleOverlayMarkerType {
    Affix,
    Aspect,
    Unique,
    Rune,
    Sigil,
    Trade,
    ItemPower,
    Paragon,
    Menu,
    TuningPrism,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RuleOverlayZOrderGroup {
    Marker,
    TextPanel,
}

/// The colour of a descriptor: a preset or palette name (`"Green"`,
/// `"#RRGGBB"`, ...) resolved by the overlay palette, or one of the fixed
/// paragon brushes, which never go through a name lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum OverlayColor {
    /// Shared, so cloning a descriptor or render op does not allocate.
    Named(Arc<str>),
    Paragon(ParagonColor),
}

impl From<String> for OverlayColor {
    fn from(name: String) -> Self {
        Self::Named(name.into())
    }
}

impl From<&str> for OverlayColor {
    fn from(name: &str) -> Self {
        Self::Named(name.into())
    }
}

impl From<ParagonColor> for OverlayColor {
    fn from(color: ParagonColor) -> Self {
        Self::Paragon(color)
    }
}

impl PartialEq<&str> for OverlayColor {
    fn eq(&self, other: &&str) -> bool {
        matches!(self, Self::Named(name) if &**name == *other)
    }
}

impl OverlayColor {
    /// The palette name, `None` for the typed paragon brushes.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name),
            Self::Paragon(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleOverlayDescriptor {
    pub target: RuleOverlayTarget,
    pub marker_type: RuleOverlayMarkerType,
    pub shape: RuleOverlayShape,
    /// The drawn box in window-relative pixels: the space of the captured
    /// game window, which is also the overlay surface's own coordinate space
    /// (see `d4lc_overlay_x11::build_render_ops`). Every producer emits this
    /// space and the final box — markers come from [`upstream_marker_box`],
    /// panels are composed in window-local coordinates — so both overlay
    /// backends draw it verbatim, without deriving anything from it.
    pub geometry: Rect,
    pub color: OverlayColor,
    pub text: Option<String>,
    /// Where the text line starts (its top-left corner, window-relative),
    /// when the producer positions the line exactly. The text is then one
    /// unwrapped line drawn from this point, as upstream draws paragon text
    /// (`DrawText(font, size, brush, left, top, text)`). `None` leaves the
    /// placement to the overlay's text area rules.
    pub text_origin: Option<Point>,
    pub z_order_group: RuleOverlayZOrderGroup,
    /// Which upstream geometry decision placed the box; a literal for the
    /// common markers, so cloning a descriptor does not allocate.
    pub geometry_source: Cow<'static, str>,
    pub source: RuleSourceNote,
}

/// Upstream `Colors.Red` comparison: the draw hides a marker whose color
/// string equals red (OverlayHandler.cs:272-273, :350). Colors arrive as
/// names or `#RRGGBB`/`#AARRGGBB`, so the comparison is on the color value,
/// not on the spelling.
pub fn is_upstream_red(color: &str) -> bool {
    d4lc_config::PresetColor::try_parse(color)
        .is_ok_and(|color| color == d4lc_config::PresetColor::from_rgb(255, 0, 0))
}

/// Upstream affix marker shape (OverlayHandler.cs:296-319): any-type
/// entries draw a rectangle, greater entries a triangle, a row below its
/// active minimal value a rectangle, everything else a circle. The checks
/// run in this order, so an any-type or greater entry keeps its shape even
/// below the minimal value.
pub const fn upstream_affix_marker_shape(
    is_any_type: bool,
    is_greater: bool,
    below_active_minimal_value: bool,
) -> RuleOverlayShape {
    if is_any_type {
        RuleOverlayShape::Rectangle
    } else if is_greater {
        RuleOverlayShape::Triangle
    } else if below_active_minimal_value {
        RuleOverlayShape::Rectangle
    } else {
        RuleOverlayShape::Circle
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOverlayMarkerMode {
    ShowAll,
    HideUnwanted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleOverlaySettings {
    pub marker_mode: RuleOverlayMarkerMode,
    pub is_minimal_affix_value_filter_enabled: bool,
    pub is_trade_overlay_enabled: bool,
    pub item_power_limit_check_ok: bool,
    /// Upstream `Settings.IsTuningPrismHintsEnabled`
    /// (`D4Companion.Entities/SettingsD4.cs`): gates the tuning-prism
    /// hint circles next to matched affixes.
    pub is_tuning_prism_hints_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleAffixOverlayLocation {
    pub row_id: Option<AffixRowId>,
    /// The row's marker anchor in window-relative pixels: `x` is the marker
    /// column (tooltip left edge plus `OffsetX`), `y`/`height` the affix
    /// marker's row (OverlayHandler.cs:263-264). Never the text rect: the
    /// drawn box is derived from this anchor by [`upstream_marker_box`].
    pub geometry: Rect,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleIdOverlayLocation {
    pub id: String,
    /// Marker anchor in window-relative pixels (column in `x`, row in
    /// `y`/`height`), like [`RuleAffixOverlayLocation::geometry`].
    pub geometry: Rect,
    pub color: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOverlayLayout {
    /// Detected tooltip rect in window-relative pixels, from the runtime's
    /// `tooltip_window_relative` (the clamped upstream tooltip crop). Anchors
    /// the trade panel; a zero rect suppresses it.
    pub tooltip_rect: Rect,
    pub affix_locations: Vec<RuleAffixOverlayLocation>,
    pub power_location: Option<RuleIdOverlayLocation>,
    /// Per-build power-marker location copies for the multi-build draw:
    /// each copy already carries its build's column offset
    /// (`5 - (i * 20)`, OverlayHandler.cs:337/:487-522) and pairs with the
    /// `RuleEvaluationResult::matched_powers` entries in order.
    pub power_locations: Vec<RuleIdOverlayLocation>,
    pub rune_locations: Vec<RuleIdOverlayLocation>,
    pub sigil_location: Option<RuleIdOverlayLocation>,
    /// Per-build sigil-marker location copies for the multi-build draw,
    /// paired with the `RuleEvaluationResult::matched_sigils` entries in
    /// order.
    pub sigil_locations: Vec<RuleIdOverlayLocation>,
}

pub fn apply_overlay_descriptors(
    result: &mut RuleEvaluationResult,
    settings: &RuleOverlaySettings,
    layout: &RuleOverlayLayout,
) {
    result.overlay_descriptors = build_overlay_descriptors(result, settings, layout);
}

pub fn build_overlay_descriptors(
    result: &RuleEvaluationResult,
    settings: &RuleOverlaySettings,
    layout: &RuleOverlayLayout,
) -> Vec<RuleOverlayDescriptor> {
    let mut descriptors = Vec::new();

    if settings.item_power_limit_check_ok {
        push_affix_overlay_descriptors(result, settings, layout, &mut descriptors);
        push_power_overlay_descriptor(result, layout, &mut descriptors);
        push_rune_overlay_descriptors(result, layout, &mut descriptors);
        push_sigil_overlay_descriptor(result, layout, &mut descriptors);
    }
    // Upstream draws the prism hints from BOTH affix draw paths
    // (`DrawGraphicsAffixes` line 325 and `DrawGraphicsAffixesMulti`
    // line 415) *outside* the marker-mode and item-power gates, so the
    // prism push is deliberately not nested under
    // `item_power_limit_check_ok`.
    if settings.is_tuning_prism_hints_enabled {
        push_tuning_prism_overlay_descriptors(result, layout, &mut descriptors);
    }
    push_trade_overlay_descriptor(result, settings, layout, &mut descriptors);

    descriptors
}

pub fn build_live_overlay_descriptors(
    rule_state: Option<(
        &RuleEvaluationResult,
        &RuleOverlaySettings,
        &RuleOverlayLayout,
    )>,
    paragon_state: Option<(&ParagonOverlayState, &ParagonOverlayLayout)>,
    measure: &dyn TextMeasure,
) -> Vec<RuleOverlayDescriptor> {
    let mut descriptors = Vec::new();

    if let Some((result, settings, layout)) = rule_state {
        descriptors.extend(build_overlay_descriptors(result, settings, layout));
    }
    if let Some((state, layout)) = paragon_state {
        descriptors.extend(build_paragon_overlay_descriptors(state, layout, measure));
    }

    descriptors
}

fn push_affix_overlay_descriptors(
    result: &RuleEvaluationResult,
    settings: &RuleOverlaySettings,
    layout: &RuleOverlayLayout,
    descriptors: &mut Vec<RuleOverlayDescriptor>,
) {
    let source = RuleSourceNote::new(
        "D4Companion.Services/OverlayHandler.cs",
        "OverlayHandler.DrawGraphicsAffixes",
        "affix marker mode and shape selection",
    );
    for affix in &result.matched_affixes {
        if settings.marker_mode == RuleOverlayMarkerMode::HideUnwanted
            && is_upstream_red(&affix.color)
        {
            continue;
        }
        // The row's own marker anchor, never a neighbour's: rows join on
        // their tooltip-wide id, and a multi-build column is a shift of that
        // anchor, not a second location.
        let Some(anchor) = affix_marker_anchor(layout, affix) else {
            continue;
        };
        let shape = affix_overlay_shape(affix, settings);
        descriptors.push(RuleOverlayDescriptor {
            target: RuleOverlayTarget::Affix {
                affix_id: affix.affix_id.clone(),
                row_id: affix.row_id,
            },
            marker_type: RuleOverlayMarkerType::Affix,
            shape,
            geometry: upstream_marker_box(shape, anchor),
            color: affix.color.clone().into(),
            text: None,
            text_origin: None,
            z_order_group: RuleOverlayZOrderGroup::Marker,
            geometry_source: "affix_marker_anchor".into(),
            source: source.clone(),
        });
    }
}

/// The marker anchor of one affix entry: its row's anchor, shifted by the
/// entry's build column.
fn affix_marker_anchor(layout: &RuleOverlayLayout, affix: &RuleAffixMatch) -> Option<Rect> {
    let row_id = affix.row_id?;
    layout
        .affix_locations
        .iter()
        .find(|location| location.row_id == Some(row_id))
        .map(|location| Rect {
            x: location.geometry.x + affix.column_offset,
            ..location.geometry
        })
}

/// Upstream affix marker constants (`int radius = 10; int length = 20;`,
/// OverlayHandler.cs:255-256). The aspect circle uses `length = 10` as its
/// radius (OverlayHandler.cs:468, :481), the same size.
pub const UPSTREAM_MARKER_RADIUS: i32 = 10;

pub const UPSTREAM_MARKER_LENGTH: i32 = 20;

/// The drawn box of a marker at `anchor`, the one geometry both overlay
/// backends draw verbatim.
///
/// `anchor.x` is the marker column (tooltip left edge plus `OffsetX` and the
/// build column offset), `anchor.y`/`anchor.height` the marker row
/// (OverlayHandler.cs:263-264). A circle is centered on the column at the
/// row's vertical center (`OutlineFillCircle(left, top + height / 2,
/// radius)`, :318, :481); a rectangle and a triangle are the
/// `length`-sided square hanging from the row top, centered on the column
/// (`left - length / 2, top, left + length / 2, top + length`, :282, :300,
/// :305-307, :313). The row's text width never enters the box.
pub fn upstream_marker_box(shape: RuleOverlayShape, anchor: Rect) -> Rect {
    let side = UPSTREAM_MARKER_LENGTH as u32;
    match shape {
        RuleOverlayShape::Circle => Rect {
            x: anchor.x - UPSTREAM_MARKER_RADIUS,
            y: anchor.y + (anchor.height / 2) as i32 - UPSTREAM_MARKER_RADIUS,
            width: side,
            height: side,
        },
        RuleOverlayShape::Rectangle | RuleOverlayShape::Triangle => Rect {
            x: anchor.x - UPSTREAM_MARKER_LENGTH / 2,
            y: anchor.y,
            width: side,
            height: side,
        },
        RuleOverlayShape::TextPanel | RuleOverlayShape::StrokedRectangle { .. } => anchor,
    }
}

/// Tuning-prism hint circles beside matched affixes.
///
/// Upstream: `D4Companion.Services/OverlayHandler.cs::
/// DrawGraphicsTuningPrisms` — gated by `IsTuningPrismHintsEnabled`;
/// for every `ItemAffix.TuningPrisms` entry it draws an
/// `OutlineFillCircle` (outline `Colors.Black`, fill = prism color,
/// radius 10, stroke 2) centered at
/// `x = affix_left + TooltipWidth + (-20 + i * 10)`,
/// `y = affix_top + affix_location_height / 2`.
/// Unknown prism names keep the previously selected color (the
/// upstream `switch` default falls through without changing
/// `affixColor`, which starts at `Colors.Red`), and a circle is drawn
/// for every entry regardless.
fn push_tuning_prism_overlay_descriptors(
    result: &RuleEvaluationResult,
    layout: &RuleOverlayLayout,
    descriptors: &mut Vec<RuleOverlayDescriptor>,
) {
    let source = RuleSourceNote::new(
        "D4Companion.Services/OverlayHandler.cs",
        "OverlayHandler.DrawGraphicsTuningPrisms",
        "tuning prism hint circle for matched affix",
    );
    for affix in &result.matched_affixes {
        let Some(anchor) = affix_marker_anchor(layout, affix) else {
            continue;
        };

        // Upstream callers pass `left + TooltipWidth` as the prism
        // origin X, `left` including the build column offset
        // (OverlayHandler.cs:325, :415); the Rust seam derives the width
        // from the tooltip rect. The row's marker gives the vertical center.
        let prism_origin_x = layout.tooltip_rect.x
            + layout.tooltip_rect.width as i32
            + affix.column_offset
            + TUNING_PRISM_X_OFFSET;
        let center_y = anchor.y + (anchor.height / 2) as i32;

        let mut color = TUNING_PRISM_DEFAULT_COLOR.to_string();
        for (index, prism) in affix.tuning_prisms.iter().enumerate() {
            apply_tuning_prism_color(prism, &mut color);
            let center_x = prism_origin_x + (index as i32 * TUNING_PRISM_RADIUS);
            descriptors.push(RuleOverlayDescriptor {
                target: RuleOverlayTarget::Affix {
                    affix_id: affix.affix_id.clone(),
                    row_id: affix.row_id,
                },
                marker_type: RuleOverlayMarkerType::TuningPrism,
                shape: RuleOverlayShape::Circle,
                geometry: Rect {
                    x: center_x - TUNING_PRISM_RADIUS,
                    y: center_y - TUNING_PRISM_RADIUS,
                    width: (TUNING_PRISM_RADIUS * 2) as u32,
                    height: (TUNING_PRISM_RADIUS * 2) as u32,
                },
                color: color.clone().into(),
                text: None,
                text_origin: None,
                z_order_group: RuleOverlayZOrderGroup::Marker,
                geometry_source: "affix_tuning_prism_circle_geometry".into(),
                source: source.clone(),
            });
        }
    }
}

/// Upstream `DrawGraphicsTuningPrisms` constants: the first circle is
/// drawn 20 px left of the prism origin X and each next circle is
/// spaced by `radius` (10 px); centers use radius 10.
const TUNING_PRISM_X_OFFSET: i32 = -20;

const TUNING_PRISM_RADIUS: i32 = 10;

const TUNING_PRISM_DEFAULT_COLOR: &str = "Red";

/// Map a `TuningStone_N` prism identifier to its upstream marker
/// color (`OverlayHandler.DrawGraphicsTuningPrisms` switch). Unknown
/// identifiers leave the current color untouched, mirroring the
/// upstream `default:` fall-through.
fn apply_tuning_prism_color(prism: &str, color: &mut String) {
    let mapped = match prism {
        "TuningStone_1" => "Red",
        "TuningStone_2" => "Blue",
        "TuningStone_3" => "Green",
        "TuningStone_4" => "Purple",
        "TuningStone_5" => "LightBlue",
        "TuningStone_6" => "Yellow",
        "TuningStone_7" => "Orange",
        "TuningStone_8" => "Pink",
        _ => return,
    };
    *color = mapped.to_string();
}

fn push_power_overlay_descriptor(
    result: &RuleEvaluationResult,
    layout: &RuleOverlayLayout,
    descriptors: &mut Vec<RuleOverlayDescriptor>,
) {
    // Multi-build per-build columns (upstream `DrawGraphicsAspectsMulti`,
    // OverlayHandler.cs:487-522): every build that lists the item's
    // aspect/unique power draws its own marker at its build's column
    // offset (`5 - (i * 20)`, OH:337). The projection supplies one match
    // and one pre-shifted location copy per matching build, in column
    // order; they replace the single-slot draw for this frame so the
    // first build's unshifted claim cannot double-draw.
    if !result.matched_powers.is_empty() {
        let mut consumed_locations = vec![false; layout.power_locations.len()];
        for power in &result.matched_powers {
            let Some(location_index) = layout
                .power_locations
                .iter()
                .zip(&consumed_locations)
                .position(|(location, consumed)| !consumed && location.id == power.power_id)
            else {
                continue;
            };
            consumed_locations[location_index] = true;
            let location = &layout.power_locations[location_index];
            descriptors.push(power_marker_descriptor(
                power,
                location.geometry,
                location.color.clone(),
                location.text.clone(),
            ));
        }
        return;
    }

    let Some(power) = &result.matched_power else {
        return;
    };
    let Some(location) = &layout.power_location else {
        return;
    };
    if location.id != power.power_id {
        return;
    }
    descriptors.push(power_marker_descriptor(
        power,
        location.geometry,
        location.color.clone(),
        location.text.clone(),
    ));
}

fn power_marker_descriptor(
    power: &RulePowerMatch,
    geometry: Rect,
    color: String,
    text: Option<String>,
) -> RuleOverlayDescriptor {
    let (marker_type, method) = match power.power_kind {
        RulePowerKind::Aspect => (
            RuleOverlayMarkerType::Aspect,
            "OverlayHandler.DrawGraphicsAspects",
        ),
        RulePowerKind::Unique => (
            RuleOverlayMarkerType::Unique,
            "OverlayHandler.DrawGraphicsAspects",
        ),
    };
    RuleOverlayDescriptor {
        target: RuleOverlayTarget::Power {
            power_id: power.power_id.clone(),
            power_kind: power.power_kind,
        },
        marker_type,
        shape: RuleOverlayShape::Circle,
        geometry: upstream_marker_box(RuleOverlayShape::Circle, geometry),
        color: color.into(),
        text,
        text_origin: None,
        z_order_group: RuleOverlayZOrderGroup::Marker,
        geometry_source: "power_marker_anchor".into(),
        source: RuleSourceNote::new(
            "D4Companion.Services/OverlayHandler.cs",
            method,
            "aspect or unique marker",
        ),
    }
}

fn push_rune_overlay_descriptors(
    result: &RuleEvaluationResult,
    layout: &RuleOverlayLayout,
    descriptors: &mut Vec<RuleOverlayDescriptor>,
) {
    for rune_id in &result.matched_runes {
        let Some(location) = layout
            .rune_locations
            .iter()
            .find(|location| &location.id == rune_id)
        else {
            continue;
        };
        descriptors.push(RuleOverlayDescriptor {
            target: RuleOverlayTarget::Rune {
                rune_id: rune_id.clone(),
            },
            marker_type: RuleOverlayMarkerType::Rune,
            shape: RuleOverlayShape::Circle,
            geometry: upstream_marker_box(RuleOverlayShape::Circle, location.geometry),
            color: location.color.clone().into(),
            text: location.text.clone(),
            text_origin: None,
            z_order_group: RuleOverlayZOrderGroup::Marker,
            geometry_source: "rune_marker_anchor".into(),
            source: RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAffixes",
                "rune marker descriptor from parsed rule match",
            ),
        });
    }
}

fn push_sigil_overlay_descriptor(
    result: &RuleEvaluationResult,
    layout: &RuleOverlayLayout,
    descriptors: &mut Vec<RuleOverlayDescriptor>,
) {
    // Multi-build per-build columns: upstream draws each build's dungeon
    // square in its own offset column through `DrawGraphicsAffixesMulti`
    // (OverlayHandler.cs:331-419). One match and one pre-shifted location
    // copy per matching build, in column order, replace the single-slot
    // draw for this frame.
    if !result.matched_sigils.is_empty() {
        let mut consumed_locations = vec![false; layout.sigil_locations.len()];
        for sigil_id in &result.matched_sigils {
            let Some(location_index) = layout
                .sigil_locations
                .iter()
                .zip(&consumed_locations)
                .position(|(location, consumed)| !consumed && &location.id == sigil_id)
            else {
                continue;
            };
            consumed_locations[location_index] = true;
            let location = &layout.sigil_locations[location_index];
            descriptors.push(sigil_marker_descriptor(
                sigil_id,
                location.geometry,
                location.color.clone(),
                location.text.clone(),
            ));
        }
        return;
    }

    let Some(sigil_id) = &result.matched_sigil else {
        return;
    };
    let Some(location) = &layout.sigil_location else {
        return;
    };
    if &location.id != sigil_id {
        return;
    }
    descriptors.push(sigil_marker_descriptor(
        sigil_id,
        location.geometry,
        location.color.clone(),
        location.text.clone(),
    ));
}

fn sigil_marker_descriptor(
    sigil_id: &str,
    geometry: Rect,
    color: String,
    text: Option<String>,
) -> RuleOverlayDescriptor {
    RuleOverlayDescriptor {
        target: RuleOverlayTarget::Sigil {
            sigil_id: sigil_id.to_string(),
        },
        marker_type: RuleOverlayMarkerType::Sigil,
        shape: RuleOverlayShape::Rectangle,
        geometry: upstream_marker_box(RuleOverlayShape::Rectangle, geometry),
        color: color.into(),
        text,
        text_origin: None,
        z_order_group: RuleOverlayZOrderGroup::Marker,
        geometry_source: "sigil_marker_anchor".into(),
        source: RuleSourceNote::new(
            "D4Companion.Services/OverlayHandler.cs",
            "OverlayHandler.DrawGraphicsAffixes",
            "sigil dungeon rectangle marker",
        ),
    }
}

fn push_trade_overlay_descriptor(
    result: &RuleEvaluationResult,
    settings: &RuleOverlaySettings,
    layout: &RuleOverlayLayout,
    descriptors: &mut Vec<RuleOverlayDescriptor>,
) {
    if !settings.is_trade_overlay_enabled {
        return;
    }
    let Some(marker) = &result.trade_marker else {
        return;
    };
    // No live tooltip, no anchor: an empty tooltip rect would draw the
    // panel at the window origin (`trade_panel_geometry` of a zero
    // rect), a phantom upstream never draws — `DrawGraphicsTrading`
    // only renders while a parsed tooltip carries a matched trade item
    // (`_currentTooltip.TradeItem != null`, OH:524-566).
    if layout.tooltip_rect.width == 0 || layout.tooltip_rect.height == 0 {
        return;
    }
    descriptors.push(RuleOverlayDescriptor {
        target: RuleOverlayTarget::Trade,
        marker_type: RuleOverlayMarkerType::Trade,
        shape: RuleOverlayShape::TextPanel,
        geometry: trade_panel_geometry(layout.tooltip_rect),
        color: "background".into(),
        text: Some(marker.value_text.clone()),
        text_origin: None,
        z_order_group: RuleOverlayZOrderGroup::TextPanel,
        geometry_source: "tooltip_bottom_trade_panel".into(),
        source: RuleSourceNote::new(
            "D4Companion.Services/OverlayHandler.cs",
            "OverlayHandler.DrawGraphicsTrading",
            "trade value text panel",
        ),
    });
}

fn affix_overlay_shape(affix: &RuleAffixMatch, settings: &RuleOverlaySettings) -> RuleOverlayShape {
    upstream_affix_marker_shape(
        affix.is_any_type_required,
        affix.is_greater_required,
        settings.is_minimal_affix_value_filter_enabled
            && affix.value_status.status == RuleCheckStatus::Failed,
    )
}

fn trade_panel_geometry(tooltip_rect: Rect) -> Rect {
    Rect {
        x: tooltip_rect.x,
        y: tooltip_rect
            .y
            .saturating_add(tooltip_rect.height.saturating_sub(50) as i32),
        width: tooltip_rect.width,
        height: 50,
    }
}

/// Upstream's default `OverlayFontSize` (`SettingsD4.cs:67`): the size every
/// overlay text is drawn at until the settings say otherwise.
pub const DEFAULT_OVERLAY_FONT_PX: u32 = 18;

/// One frame handed to an overlay backend: the descriptors to draw and the
/// facts of the pipeline pass that produced them. The X11 and the Wayland
/// overlay draw the same frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFrame {
    pub sequence: u64,
    pub tooltip_found: bool,
    pub hotpath_ms: u128,
    pub cursor_window_relative: Option<Point>,
    /// Shared with the presentation that produced it: an update with the same
    /// list reuses the previous update's render ops.
    pub rule_descriptors: Arc<[RuleOverlayDescriptor]>,
    /// `OverlayFontSize`: the size of every text op of this frame (upstream
    /// creates its one `consolasBold` font at this size,
    /// `OverlayHandler.cs:884`, `:910`).
    pub text_font_px: u32,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
