//! X11 overlay boundary for d4lc live runtime.
//!
//! Descriptor-based affix/trade/paragon rendering, menu lock/unlock behavior
//! and the interaction-zone state go through this boundary; the real X11
//! backend sits behind the `real-x11` feature flag. When DISPLAY is available and `real-x11` is enabled, a top-level
//! X11 overlay window is created; on failure or when DISPLAY is absent, the
//! implementation falls back to HeadlessNoop.
//!
//! The overlay status and render-op types (`OverlayStatus`,
//! `OverlayRenderOp`) and the render-op expansion live here and are shared
//! verbatim with the Wayland backend (`d4lc-overlay-wayland`). The frame both
//! backends draw (`OverlayFrame`) is defined in `d4lc-rules`, so the
//! pipeline produces it without depending on a backend.
//!
//! Original reference: `upstream/Diablo4Companion/D4Companion.Services/OverlayHandler.cs`
//! uses Windows/GameOverlay.NET (`GraphicsWindow`, `FitTo`, `IsTopmost`, 60 FPS).
//! Whether transparency and always-on-top take effect depends on the
//! compositor and window manager; the backend reports what it could confirm.

use d4lc_core::Rect;
use std::fmt;

/// Antialiased marker shapes (upstream `OutlineFillCircle`) both backends
/// composite as premultiplied ARGB32 images.
pub mod shape;
pub mod text;

mod handle;
mod menu;
mod render;
#[cfg(test)]
mod test_support;
/// Real X11 backend — only compiled when the `real-x11` feature is active.
#[cfg(feature = "real-x11")]
mod x11_backend;

pub use handle::{create_overlay, OverlayHandle};
pub use menu::{menu_interaction_zone, OverlayMenuState};
pub use render::{
    build_descriptor_render_ops, build_render_ops, color_pixel, overlay_color_pixel,
    text_wrap_policy, RenderPlan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayConfig {
    pub window_geometry: Rect,
    pub show_overlay_icon: bool,
    pub overlay_icon_pos_x: u32,
    pub overlay_icon_pos_y: u32,
    pub menu_locked: bool,
}

/// The backend-neutral frame contract lives in `d4lc-rules`, next to the
/// descriptors it carries; it is re-exported here for the backends' callers.
pub use d4lc_rules::{OverlayFrame, DEFAULT_OVERLAY_FONT_PX};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayRenderKind {
    OutlineRect,
    StrokedRect {
        stroke: u32,
    },
    FilledRect,
    /// Upstream `Graphics.OutlineFillRectangle` (`OverlayHandler.cs` 282,
    /// 300, 313): a rectangle filled with the marker color and stroked in
    /// black with the marker's stroke width. `stroke` carries that width —
    /// 2 for a sigil's dungeon marker, 1 for an any-type entry or a row below
    /// its active minimal value. Both backends draw the identical form;
    /// outline-only rectangles (`OutlineRect`, upstream `DrawRectangle`)
    /// stay the paragon tiles' and the menu border's form.
    OutlineFillRect {
        stroke: u8,
    },
    Text,
    /// Upstream `Graphics.OutlineFillCircle` (black outline, fill color,
    /// stroke 2) used by `DrawGraphicsAffixes` for normal affixes,
    /// `DrawGraphicsAspects` for aspect/unique markers, and
    /// `DrawGraphicsTuningPrisms` for the prism hint circles. `geometry`
    /// carries the circle's bounding square.
    Circle,
    /// Upstream `gfx.FillTriangle` + `gfx.DrawTriangle` (black outline,
    /// stroke 2) used by `DrawGraphicsAffixes` for greater affixes
    /// (`OverlayHandler.cs` 305-310). `geometry` carries the triangle's
    /// bounding box: apex at the top-center, base on the bottom edge.
    Triangle,
}

/// Where a text op's geometry puts its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayTextPlacement {
    /// A text area: inset by the overlay's text padding, wrapped or single
    /// line by the op's group ([`text_wrap_policy`]).
    Area,
    /// One unwrapped line whose pen starts at the geometry's left edge and
    /// whose top is the geometry's top (upstream `DrawText(font, size, brush,
    /// left, top, text)`).
    Line,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayRenderOp {
    pub kind: OverlayRenderKind,
    pub marker_type: d4lc_rules::RuleOverlayMarkerType,
    pub z_order_group: d4lc_rules::RuleOverlayZOrderGroup,
    pub geometry: Rect,
    pub color: d4lc_rules::OverlayColor,
    pub text: Option<String>,
    /// Size of a text op's glyphs in whole pixels.
    pub font_px: u32,
    pub text_placement: OverlayTextPlacement,
    pub geometry_source: Option<std::borrow::Cow<'static, str>>,
    pub source: Option<d4lc_rules::RuleSourceNote>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayBackend {
    HeadlessNoop,
    RealX11,
    /// Real Wayland overlay via `zwlr_layer_shell_v1`
    /// (`crates/d4lc-overlay-wayland`). The Wayland backend reports the
    /// same requested/confirmed discipline.
    RealWayland,
    Failed,
}

impl OverlayBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HeadlessNoop => "headless_noop",
            Self::RealX11 => "real_x11",
            Self::RealWayland => "real_wayland",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for OverlayBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Raster and commit costs of a backend's last completed draw (the
/// `present_us` split). A `None` stage was not measured — no draw happened,
/// or the backend does not time it. Never a fabricated 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayDrawTimings {
    /// Render-op expansion plus rasterization into the surface buffer.
    pub raster_us: Option<u64>,
    /// Hand-off to the compositor: damage/attach/commit and the flush or
    /// roundtrip that follows it.
    pub commit_us: Option<u64>,
}

impl OverlayDrawTimings {
    /// Microseconds of a measured duration, saturating.
    pub fn micros(duration: std::time::Duration) -> u64 {
        u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayStatus {
    pub backend: OverlayBackend,
    pub enabled: bool,
    pub window_geometry: Rect,
    pub interaction_zone: Rect,
    pub menu_locked: bool,
    pub menu_visible: bool,
    /// Legacy compatibility flag: "confirmed transparent",
    /// not merely "an ARGB visual was requested/found".
    pub transparent: bool,
    pub transparent_requested: bool,
    pub transparent_confirmed: bool,
    /// Legacy compatibility flag: "confirmed always-on-top",
    /// not merely "an EWMH hint/property was sent".
    pub always_on_top: bool,
    pub always_on_top_requested: bool,
    pub always_on_top_confirmed: bool,
    pub click_through: bool,
    /// Keyboard interactivity requested from the host. The Wayland backend
    /// expresses this through the layer-shell `keyboard_interactivity` mode
    /// (`None` mirrors the X11 focus-free overlay, so this stays false unless
    /// a backend actually requests keyboard focus); the X11 backends never
    /// request it.
    pub keyboard_interactive_requested: bool,
    /// Keyboard interactivity confirmed by the host. Only an observed host
    /// signal (a `wl_keyboard` enter on the overlay surface for Wayland)
    /// may set this; a request alone never collapses into a confirmation.
    pub keyboard_interactive_confirmed: bool,
    pub draw_error_count: u64,
    pub last_error: Option<String>,
    pub limitation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_as_str_headless_noop() {
        assert_eq!(OverlayBackend::HeadlessNoop.as_str(), "headless_noop");
        assert_eq!(OverlayBackend::HeadlessNoop.to_string(), "headless_noop");
    }

    #[test]
    fn backend_as_str_real_x11() {
        assert_eq!(OverlayBackend::RealX11.as_str(), "real_x11");
        assert_eq!(OverlayBackend::RealX11.to_string(), "real_x11");
    }

    #[test]
    fn backend_as_str_failed() {
        assert_eq!(OverlayBackend::Failed.as_str(), "failed");
        assert_eq!(OverlayBackend::Failed.to_string(), "failed");
    }
}
