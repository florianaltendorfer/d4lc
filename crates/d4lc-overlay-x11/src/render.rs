use crate::menu::build_menu_render_ops;
use crate::{text, OverlayMenuState, OverlayRenderKind, OverlayRenderOp, OverlayTextPlacement};
use d4lc_core::Rect;
use d4lc_rules::{OverlayColor, ParagonColor};
use std::sync::Arc;
use std::time::Instant;

/// Expand `d4lc-rules` overlay descriptors into ordered render ops, shared
/// verbatim by the X11 and Wayland backends so both consume the identical
/// descriptor pipeline (z-order sort, menu render ops). Must stay free of
/// backend-specific drawing.
///
/// Descriptor geometry is window-relative — the space of the captured game
/// window, which is also the overlay surface (`OverlayConfig::window_geometry`
/// positions the surface on that same rect) and the space
/// `cursor_window_relative` and `menu_interaction_zone` already use. Render
/// ops therefore carry it verbatim; upstream draws the markers into the
/// window bitmap the same way. Running a descriptor rect through a window
/// origin again would push every marker off the surface whenever the game
/// window does not start at the screen's top-left corner.
///
/// Descriptor-derived ops are produced on every call while the overlay is
/// enabled, regardless of the menu lock state — upstream draws the evaluation
/// markers every frame from the current tooltip
/// (`OverlayHandler.DrawGraphicsAffixes`). The lock/hover interaction gates
/// only the menu icon ops appended below.
pub fn build_render_ops(
    descriptors: &[d4lc_rules::RuleOverlayDescriptor],
    menu: &OverlayMenuState,
    font_px: u32,
    now: Instant,
) -> Vec<OverlayRenderOp> {
    let mut render_ops = build_descriptor_render_ops(descriptors, font_px);
    render_ops.extend(build_menu_render_ops(menu, font_px, now));
    render_ops
}

/// The descriptor-derived ops of [`build_render_ops`]: markers first, then
/// text panels (a stable z-order by group), each descriptor's shape followed
/// by its text. Borrows the descriptors; the ops share their strings.
pub fn build_descriptor_render_ops(
    descriptors: &[d4lc_rules::RuleOverlayDescriptor],
    font_px: u32,
) -> Vec<OverlayRenderOp> {
    let mut render_ops = Vec::with_capacity(descriptors.len() + descriptors.len() / 16);

    let groups = [
        d4lc_rules::RuleOverlayZOrderGroup::Marker,
        d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
    ];
    let ordered = groups.into_iter().flat_map(|group| {
        descriptors
            .iter()
            .filter(move |descriptor| descriptor.z_order_group == group)
    });

    for descriptor in ordered {
        // Shape selection mirrors the upstream draw dispatch
        // (`OverlayHandler.cs` 279-319): circles for normal
        // affixes/aspects/runes/prism hints, triangles for greater
        // affixes, outline+fill rectangles for any-type entries, rows below
        // their minimal value and sigil dungeon markers, outline-only
        // rectangles for paragon tiles, filled panels for text panels. The
        // descriptor geometry already is the drawn box
        // (`d4lc_rules::upstream_marker_box` for markers), so both backends
        // draw it verbatim.
        let kind = match descriptor.shape {
            d4lc_rules::RuleOverlayShape::Circle => OverlayRenderKind::Circle,
            d4lc_rules::RuleOverlayShape::Triangle => OverlayRenderKind::Triangle,
            d4lc_rules::RuleOverlayShape::TextPanel => OverlayRenderKind::FilledRect,
            d4lc_rules::RuleOverlayShape::StrokedRectangle { stroke } => {
                OverlayRenderKind::StrokedRect { stroke }
            }
            d4lc_rules::RuleOverlayShape::Rectangle => match descriptor.marker_type {
                // Upstream draws a sigil's dungeon marker with stroke 2 and
                // an any-type / below-minimal affix marker with stroke 1
                // (`OutlineFillRectangle(..., 2)` at :282 vs `..., 1)` at
                // :300/:313). Paragon tiles keep the outline-only form
                // (`DrawRectangle`, :684/:693).
                d4lc_rules::RuleOverlayMarkerType::Sigil => {
                    OverlayRenderKind::OutlineFillRect { stroke: 2 }
                }
                d4lc_rules::RuleOverlayMarkerType::Affix => {
                    OverlayRenderKind::OutlineFillRect { stroke: 1 }
                }
                _ => OverlayRenderKind::OutlineRect,
            },
        };
        render_ops.push(OverlayRenderOp {
            kind,
            marker_type: descriptor.marker_type,
            z_order_group: descriptor.z_order_group,
            geometry: descriptor.geometry,
            color: descriptor.color.clone(),
            text: None,
            font_px,
            text_placement: OverlayTextPlacement::Area,
            geometry_source: Some(descriptor.geometry_source.clone()),
            source: Some(descriptor.source.clone()),
        });
        if let Some(text) = descriptor.text.as_ref() {
            render_ops.push(OverlayRenderOp {
                kind: OverlayRenderKind::Text,
                marker_type: descriptor.marker_type,
                z_order_group: descriptor.z_order_group,
                geometry: text_geometry(descriptor, font_px),
                color: text_color(&descriptor.color),
                text: Some(text.clone()),
                font_px,
                text_placement: if descriptor.text_origin.is_some() {
                    OverlayTextPlacement::Line
                } else {
                    OverlayTextPlacement::Area
                },
                geometry_source: Some(descriptor.geometry_source.clone()),
                source: Some(descriptor.source.clone()),
            });
        }
    }

    render_ops
}

/// The ops of the last update, kept between frames. The descriptor-derived
/// ops are rebuilt only when the descriptor list (the shared `Arc` the live
/// loop hands over) or the font size changed; a frame with the same list
/// only rebuilds the few menu ops. Draw-skipping reads [`Self::refresh`]'s
/// answer instead of comparing hundreds of ops.
#[derive(Debug, Default)]
pub struct RenderPlan {
    ops: Vec<OverlayRenderOp>,
    source: Option<(Arc<[d4lc_rules::RuleOverlayDescriptor]>, u32)>,
    descriptor_ops: usize,
}

impl RenderPlan {
    /// Brings the plan up to date for one frame and returns whether the ops
    /// differ from the previous plan's.
    pub fn refresh(
        &mut self,
        descriptors: &Arc<[d4lc_rules::RuleOverlayDescriptor]>,
        menu: &OverlayMenuState,
        font_px: u32,
        now: Instant,
    ) -> bool {
        let menu_ops = build_menu_render_ops(menu, font_px, now);
        let same_source = self.source.as_ref().is_some_and(|(cached, cached_px)| {
            Arc::ptr_eq(cached, descriptors) && *cached_px == font_px
        });
        if same_source {
            if self.ops[self.descriptor_ops..] == menu_ops[..] {
                return false;
            }
            self.ops.truncate(self.descriptor_ops);
            self.ops.extend(menu_ops);
            return true;
        }
        let mut ops = build_descriptor_render_ops(descriptors, font_px);
        let descriptor_ops = ops.len();
        ops.extend(menu_ops);
        // A rebuilt list that draws identically (an evaluation that produced
        // equal descriptors) must still skip the draw.
        let changed = ops != self.ops;
        self.ops = ops;
        self.source = Some((Arc::clone(descriptors), font_px));
        self.descriptor_ops = descriptor_ops;
        changed
    }

    /// The ops of the last refresh.
    pub fn ops(&self) -> &[OverlayRenderOp] {
        &self.ops
    }
}

/// The colour a panel's text is drawn in. The legacy `"background"` panel
/// keeps its white text; the paragon panel fill draws upstream's `text`
/// brush on it (`OverlayHandler.cs:622`, `:653`, `:666`). Any other
/// descriptor draws its text in its own colour.
fn text_color(panel: &OverlayColor) -> OverlayColor {
    match panel {
        OverlayColor::Named(name) if &**name == "background" => "White".into(),
        OverlayColor::Paragon(ParagonColor::Background) => ParagonColor::Text.into(),
        other => other.clone(),
    }
}

/// Where a descriptor's text starts. A descriptor that positions its line
/// exactly (`text_origin`, the paragon panels) puts the line box at that
/// point, spanning the rest of the panel; panel text uses the panel box; marker
/// text (the sigil dungeon tier) starts a quarter marker length into the
/// marker box, upstream's `left - length / 4` for a box spanning
/// `left - length / 2 .. left + length / 2` (OverlayHandler.cs:290).
fn text_geometry(descriptor: &d4lc_rules::RuleOverlayDescriptor, font_px: u32) -> Rect {
    if let Some(origin) = descriptor.text_origin {
        let panel_right = descriptor.geometry.x + descriptor.geometry.width as i32;
        return Rect {
            x: origin.x,
            y: origin.y,
            width: (panel_right - origin.x).max(0) as u32,
            height: font_px,
        };
    }
    match descriptor.z_order_group {
        d4lc_rules::RuleOverlayZOrderGroup::Marker => Rect {
            x: descriptor.geometry.x + (descriptor.geometry.width / 4) as i32,
            ..descriptor.geometry
        },
        d4lc_rules::RuleOverlayZOrderGroup::TextPanel => descriptor.geometry,
    }
}

/// The wrap contract of one render op's text area, shared by both backends.
///
/// A marker caption (the sigil dungeon tier) is a single line: its intended
/// area is the overlay surface row, and the surface clip bounds it. A text
/// panel's box is its own area, so panel text wraps inside the box.
pub fn text_wrap_policy(operation: &OverlayRenderOp) -> text::TextWrapPolicy {
    if operation.text_placement == OverlayTextPlacement::Line {
        return text::TextWrapPolicy::ExactLine;
    }
    match operation.z_order_group {
        d4lc_rules::RuleOverlayZOrderGroup::Marker => text::TextWrapPolicy::SingleLine,
        d4lc_rules::RuleOverlayZOrderGroup::TextPanel => text::TextWrapPolicy::WrapWords,
    }
}

/// The straight-alpha ARGB pixel of a descriptor colour: a paragon brush
/// carries its own value, a name goes through [`color_pixel`].
pub fn overlay_color_pixel(color: &OverlayColor) -> u32 {
    match color {
        OverlayColor::Named(name) => color_pixel(name),
        OverlayColor::Paragon(brush) => brush.argb(),
    }
}

/// Map a marker color to a straight-alpha ARGB pixel: an upstream palette
/// name, or the `#RGB` / `#RRGGBB` / `#AARRGGBB` form preset and default
/// colors are stored in. Shared by the X11 and Wayland draw paths (the
/// Wayland backend converts to premultiplied alpha itself, as `wl_shm`
/// requires).
pub fn color_pixel(color: &str) -> u32 {
    if let Some(pixel) = color.trim().strip_prefix('#').and_then(hex_color_pixel) {
        return pixel;
    }
    // Case-insensitive lookup identical to the previous
    // `to_ascii_lowercase`-based table, but allocation-free: this runs once
    // per render op per overlay update on the live loop.
    if color.eq_ignore_ascii_case("green") {
        0xFF00FF00
    } else if color.eq_ignore_ascii_case("gold") {
        0xFFFFD700
    } else if color.eq_ignore_ascii_case("blue") {
        0xFF1E90FF
    } else if color.eq_ignore_ascii_case("orange") {
        0xFFFF8C00
    } else if color.eq_ignore_ascii_case("purple") {
        0xFF9370DB
    } else if color.eq_ignore_ascii_case("teal") {
        0xFF20B2AA
    } else if color.eq_ignore_ascii_case("red") {
        0xFFFF4040
    } else if color.eq_ignore_ascii_case("yellow") {
        0xFFFFFF00
    } else if color.eq_ignore_ascii_case("cyan") {
        0xFF00FFFF
    } else if color.eq_ignore_ascii_case("white") {
        0xFFFFFFFF
    } else if color.eq_ignore_ascii_case("background") {
        0xCC1A1A1A
    } else if color.eq_ignore_ascii_case("black") {
        // Upstream outline brush (`Colors.Black`).
        0xFF000000
    } else if color.eq_ignore_ascii_case("lightblue") || color.eq_ignore_ascii_case("pink") {
        // Tuning-prism palette additions mirroring
        // `System.Windows.Media.Colors` exactly (0xFFADD8E6 / 0xFFFFC0CB),
        // matching the already-light upstream values.
        if color.eq_ignore_ascii_case("lightblue") {
            0xFFADD8E6
        } else {
            0xFFFFC0CB
        }
    } else {
        0xFFFFFFFF
    }
}

/// `RGB`, `RRGGBB` or `AARRGGBB` hex digits as an ARGB pixel.
fn hex_color_pixel(hex: &str) -> Option<u32> {
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        3 => {
            let expand = |nibble: u32| (nibble & 0xF) * 0x11;
            Some(0xFF00_0000 | expand(value >> 8) << 16 | expand(value >> 4) << 8 | expand(value))
        }
        6 => Some(0xFF00_0000 | value),
        8 => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
