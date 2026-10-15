//! Wayland overlay boundary for d4lc live runtime.
//!
//! This crate renders the same descriptor-driven overlay as
//! `d4lc-overlay-x11` on compositors that advertise `zwlr_layer_shell_v1`
//! (wlroots family, KDE Plasma/KWin, Hyprland). It follows the X11 crate's
//! model:
//!
//! - the descriptor pipeline is unchanged: descriptors from `d4lc-rules`
//!   are expanded into render ops by the shared `build_render_ops` in
//!   `d4lc-overlay-x11` — this crate never calls OCR, parsing, imports, or
//!   rule evaluation during draw;
//! - text goes through the same shared bounded text path as the X11 backend
//!   (`d4lc_overlay_x11::text`): one explicit font assignment, font size,
//!   text area and line-wrap contract, rasterized client-side with `swash`
//!   (this backend has no server-side text drawing at all) and blitted with
//!   the same coverage-to-pixel rule the X11 `put_image` payload uses;
//! - the layer surface requests the `Overlay` layer, so z-order is
//!   compositor-managed; this replaces the X11 always-on-top hint and is
//!   reported as requested-but-not-client-provable;
//! - the surface is output-anchored: the config's `window_geometry` is the
//!   capture stream rect in stream space (origin (0,0), size = stream
//!   size), and the backend anchors the surface to the advertised
//!   `wl_output` whose current-mode pixel size matches that stream size,
//!   all edges, zero margins, with the compositor's configure driving the
//!   surface size. The explicit coordinate chain is
//!   stream-pixel == output-pixel == surface-pixel (identity transform,
//!   scale 1), so window/stream-relative descriptors need no extra
//!   mapping. Every deviation from that identity — output scale != 1, a
//!   configure size that differs from the stream size, a non-zero stream
//!   origin, a vanished anchored output, or no output matching at all —
//!   is surfaced through the `OverlayStatus.limitation` string
//!   (no match is a structured `failed` state, never a guessed anchor);
//! - transparency is a client-side premultiplied ARGB SHM buffer, requested
//!   but not client-provably confirmed;
//! - click-through is an input region covering only the menu interaction
//!   zone (or an empty region when the menu icon is disabled), so the rest
//!   of the surface passes pointer events through to the game;
//! - global cursor position has no Wayland equivalent: pointer position is
//!   known only from pointer events on the overlay surface's interaction
//!   zone, which feeds the shared menu hover state machine;
//! - keyboard interactivity is not requested (`None` mode, mirroring the
//!   focus-free X11 overlay) and reported requested-vs-confirmed;
//! - no layer-shell global (GNOME/Mutter sessions, for example) or Wayland
//!   connection failure degrades to an `failed`/`headless_noop`
//!   state with a `limitation` string — the live loop never crashes.
//!
//! Original reference: `upstream/Diablo4Companion/D4Companion.Services/OverlayHandler.cs`
//! uses Windows/GameOverlay.NET. Wayland is new Linux-native scope, not
//! upstream parity. Real-desktop validation against Diablo IV on a Wayland
//! session remains a separate manual check.

use std::fmt;
use std::time::Instant;

use d4lc_core::{Point, Rect};
use d4lc_overlay_x11::{menu_interaction_zone, overlay_color_pixel, OverlayMenuState, RenderPlan};

// The shared descriptor model (`OverlayFrame`, `OverlayStatus`,
// render ops) is re-exported so consumers of this backend need no X11-named
// dependency edge of their own.
pub use d4lc_overlay_x11::text::{
    TextDiagnostics, TextError, TextLayout, TextLineMask, TextRasterizer, TextWrapPolicy,
};
pub use d4lc_overlay_x11::{
    text_wrap_policy, OverlayBackend, OverlayConfig, OverlayDrawTimings, OverlayFrame,
    OverlayRenderKind, OverlayRenderOp, OverlayStatus,
};

const OVERLAY_NAMESPACE: &str = "d4lc-overlay";

/// Straight-alpha ARGB to premultiplied-alpha ARGB conversion. `wl_shm`
/// ARGB8888 buffers are premultiplied; the shared `color_pixel` palette is
/// straight-alpha (X11 semantics), so e.g. the 0xCC background color must be
/// converted before it reaches a Wayland buffer.
pub fn premultiply_argb(argb: u32) -> u32 {
    let a = (argb >> 24) & 0xFF;
    let r = (argb >> 16) & 0xFF;
    let g = (argb >> 8) & 0xFF;
    let b = argb & 0xFF;
    (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255)
}

// ---------------------------------------------------------------------------
// Public handle — mirrors the d4lc-overlay-x11 OverlayHandle API.
// ---------------------------------------------------------------------------

enum Backend {
    Headless,
    Real(
        Box<wayland_backend::OverlayState>,
        wayland_client::EventQueue<wayland_backend::OverlayState>,
    ),
    Failed,
}

pub struct OverlayHandle {
    backend: Backend,
    menu: OverlayMenuState,
    status: OverlayStatus,
    last_frame: Option<OverlayFrame>,
    plan: RenderPlan,
    /// The paragon panels' input regions of the last descriptor list, so a
    /// frame with the same list does not scan it again.
    paragon_regions: Option<(
        std::sync::Arc<[d4lc_rules::RuleOverlayDescriptor]>,
        Vec<Rect>,
    )>,
    /// Whether at least one update executed a real draw: the
    /// skip path only engages after the surface has real pixels.
    ever_drawn: bool,
    /// Whether the last update skipped the draw because nothing observable
    /// changed. Read through
    /// [`OverlayHandle::last_update_skipped`].
    last_update_skipped: bool,
    /// What the shared bounded text path could not draw (unsupported
    /// codepoints, characters that did not fit their area) or why it is
    /// disabled. The X11 handle reports the identical diagnostics shape.
    text_diagnostics: TextDiagnostics,
}

impl OverlayHandle {
    fn create_headless(config: OverlayConfig, limitation: &str) -> Self {
        let menu = OverlayMenuState::from_config(&config);
        Self {
            backend: Backend::Headless,
            menu,
            status: headless_status(&config, limitation),
            last_frame: None,
            plan: RenderPlan::default(),
            paragon_regions: None,
            ever_drawn: false,
            last_update_skipped: false,
            text_diagnostics: TextDiagnostics::default(),
        }
    }

    fn create_failed(config: OverlayConfig, limitation: &str) -> Self {
        let menu = OverlayMenuState::from_config(&config);
        let mut status = headless_status(&config, limitation);
        status.backend = OverlayBackend::Failed;
        Self {
            backend: Backend::Failed,
            menu,
            status,
            last_frame: None,
            plan: RenderPlan::default(),
            paragon_regions: None,
            ever_drawn: false,
            last_update_skipped: false,
            text_diagnostics: TextDiagnostics::default(),
        }
    }

    /// Flip the backend to `Failed` with the status fields a failed
    /// draw sets. Used by the draw path and by a dispatch error surfacing a
    /// protocol error of an earlier flushed commit.
    fn fail_drawing(&mut self, error: &str) {
        self.status.backend = OverlayBackend::Failed;
        self.status.transparent = false;
        self.status.transparent_confirmed = false;
        self.status.always_on_top = false;
        self.status.always_on_top_confirmed = false;
        self.status.click_through = false;
        self.status.keyboard_interactive_confirmed = false;
        self.status.draw_error_count = self.status.draw_error_count.saturating_add(1);
        self.status.last_error = Some(error.to_string());
        self.status.limitation = Some(format!(
            "overlay draw failed: {error}; disabled overlay drawing for remaining frames"
        ));
        self.backend = Backend::Failed;
    }

    /// Drain surface-local pointer events before the app evaluates navigation.
    /// Protocol errors use the same failed status as drawing errors.
    pub fn poll_cursor_window_relative(&mut self) -> Option<d4lc_core::Point> {
        let dispatch_error = if let Backend::Real(state, queue) = &mut self.backend {
            wayland_backend::dispatch_available(queue, state).err()
        } else {
            None
        };
        if let Some(error) = dispatch_error {
            self.fail_drawing(&error);
        }
        match &self.backend {
            Backend::Real(state, _) => state.pointer_window_relative,
            Backend::Headless | Backend::Failed => None,
        }
    }

    /// Push one overlay frame: expand descriptors into render ops (shared
    /// pipeline), rasterize them into a premultiplied ARGB SHM buffer and
    /// commit the layer surface. A draw failure flips the backend to
    /// `failed` with metadata; the caller's live loop continues.
    pub fn update(&mut self, frame: OverlayFrame) -> Result<(), String> {
        self.update_at(frame, Instant::now())
    }

    fn update_at(&mut self, frame: OverlayFrame, now: Instant) -> Result<(), String> {
        if !self.status.enabled {
            return Err("overlay is disabled".to_string());
        }

        let cursor = self
            .poll_cursor_window_relative()
            .or(frame.cursor_window_relative);
        self.menu.observe_cursor(cursor, now);
        self.status.menu_locked = self.menu.is_locked();
        self.status.menu_visible = self.menu.is_visible();

        let ops_changed =
            self.plan
                .refresh(&frame.rule_descriptors, &self.menu, frame.text_font_px, now);
        if let Backend::Real(state, queue) = &mut self.backend {
            let cached = self.paragon_regions.take().filter(|(descriptors, _)| {
                std::sync::Arc::ptr_eq(descriptors, &frame.rule_descriptors)
            });
            let (descriptors, panel_regions) = cached.unwrap_or_else(|| {
                (
                    std::sync::Arc::clone(&frame.rule_descriptors),
                    paragon_input_regions(&frame.rule_descriptors),
                )
            });
            let mut regions = panel_regions.clone();
            self.paragon_regions = Some((descriptors, panel_regions));
            if self.menu.is_icon_enabled() {
                regions.push(self.menu.interaction_zone());
            }
            state.set_input_regions(&queue.handle(), regions);
        }

        // An unchanged frame on an unchanged surface skips the
        // draw entirely. The first frame and every forced-redraw event (a
        // configure with a size change, a lost anchored output) always
        // draw, and so does any changed render-op list — menu hover and
        // fade animation feed `now` into the ops, so animated frames still
        // differ and still draw. A skip is visible through
        // `last_update_skipped` and never looks like a draw.
        let mut state_forced_redraw = false;
        let mut first_frame = false;
        if let Backend::Real(state, _) = &mut self.backend {
            state_forced_redraw = state.take_redraw_required();
            first_frame = !state.first_frame_committed;
        }
        let skip_draw = !first_frame && !state_forced_redraw && self.ever_drawn && !ops_changed;
        self.last_update_skipped = skip_draw;
        if skip_draw {
            self.last_frame = Some(frame);
            return Ok(());
        }
        self.ever_drawn = true;

        let draw_error = match &mut self.backend {
            Backend::Real(state, queue) => {
                let error = state.draw(queue, self.plan.ops()).err();
                self.text_diagnostics = state.text_diagnostics().clone();
                if error.is_none() {
                    // Keep the capability record current: the
                    // output-anchored mapping line reflects the anchored
                    // output's live geometry and the last ignored geometry
                    // update.
                    self.status.limitation = Some(state.limitation());
                }
                error
            }
            Backend::Headless | Backend::Failed => None,
        };

        if let Some(error) = &draw_error {
            self.fail_drawing(error);
        }

        self.last_frame = Some(frame);
        Ok(())
    }

    pub fn status(&self) -> &OverlayStatus {
        &self.status
    }

    /// Raster/commit costs of the last completed draw. Stages the
    /// backend did not measure stay `None` — a headless or failed backend
    /// reports both absent.
    pub fn draw_timings(&self) -> OverlayDrawTimings {
        match &self.backend {
            Backend::Real(state, _) => state.last_draw_timings,
            Backend::Headless | Backend::Failed => OverlayDrawTimings::default(),
        }
    }

    pub fn last_frame(&self) -> Option<&OverlayFrame> {
        self.last_frame.as_ref()
    }

    pub fn last_render_ops(&self) -> &[OverlayRenderOp] {
        self.plan.ops()
    }

    /// Whether the last update skipped the draw because the render ops, the
    /// surface size and every forced-redraw source were unchanged.
    /// The compositor still shows the previous, identical frame.
    pub const fn last_update_skipped(&self) -> bool {
        self.last_update_skipped
    }

    /// What the shared bounded text path could not draw, or why it is
    /// disabled. The X11 handle reports the identical diagnostics shape.
    pub fn text_diagnostics(&self) -> &TextDiagnostics {
        &self.text_diagnostics
    }

    /// Enable a copy of the last committed SHM canvas on every draw. The
    /// live loop must never enable this (it copies the full frame buffer);
    /// it exists for synthetic smoke evidence and debug tooling.
    #[doc(hidden)]
    pub fn enable_buffer_snapshot(&mut self) {
        if let Backend::Real(state, _) = &mut self.backend {
            state.snapshot_enabled = true;
        }
    }

    /// Client-side snapshot `(width, height, premultiplied_argb_bytes)` of
    /// the last committed buffer — the exact pixels the compositor received.
    /// Requires [`OverlayHandle::enable_buffer_snapshot`] first. This proves
    /// what the client rendered and committed; it is not a compositor
    /// screenshot.
    #[doc(hidden)]
    pub fn committed_buffer_snapshot(&self) -> Option<(u32, u32, Vec<u8>)> {
        match &self.backend {
            Backend::Real(state, _) => state.last_committed.clone(),
            _ => None,
        }
    }

    /// The compositor source label recorded at creation (for smoke
    /// evidence: `"WAYLAND_DISPLAY session"` vs a synthetic double/nested
    /// compositor label).
    #[doc(hidden)]
    pub fn compositor_source(&self) -> Option<&str> {
        match &self.backend {
            Backend::Real(state, _) => Some(state.compositor_source()),
            _ => None,
        }
    }

    /// Follow the game window to a new on-screen geometry.
    ///
    /// The Wayland surface is output-anchored: it fills the output carrying
    /// the capture stream, and the portal stream stays bound to that output
    /// for the whole session. A game-window update therefore never moves or
    /// resizes the surface — re-anchoring to the window would desync the
    /// overlay from the capture it must draw over. The update stays
    /// explicit instead of a hard error: a zero geometry is an error, an
    /// identical geometry is a no-op, a changed geometry is recorded as not
    /// applied in the status limitation on the real backend (the surface's
    /// geometry record stays the stream rect it maps 1:1), and on the
    /// headless/failed backends — where no surface exists and nothing can
    /// be mispositioned — the record and the per-mille menu zone follow the
    /// reported reference like the X11 headless bookkeeping.
    pub fn set_window_geometry(&mut self, geometry: Rect) -> Result<(), String> {
        if geometry.width == 0 || geometry.height == 0 {
            return Err("overlay window geometry must be non-zero".to_string());
        }
        if self.status.window_geometry == geometry {
            return Ok(());
        }
        match &mut self.backend {
            Backend::Real(state, _) => {
                state.note_ignored_geometry(geometry);
                self.status.limitation = Some(state.limitation());
            }
            Backend::Headless | Backend::Failed => {
                self.menu.set_window_geometry(geometry);
                self.status.interaction_zone = self.menu.interaction_zone();
                self.status.window_geometry = geometry;
            }
        }
        Ok(())
    }

    /// Apply the overlay menu settings to the running surface: icon visibility + position and optionally the stored
    /// lock value. On the real layer-shell backend the menu is part of the
    /// per-frame render ops, so the state-machine update is the whole
    /// change; the status mirrors it like the X11 handle does.
    pub fn apply_menu_settings(
        &mut self,
        show_overlay_icon: bool,
        overlay_icon_pos_x: u32,
        overlay_icon_pos_y: u32,
        menu_locked: Option<bool>,
    ) {
        let geometry = self.status.window_geometry;
        self.menu.apply_settings(
            geometry,
            show_overlay_icon,
            overlay_icon_pos_x,
            overlay_icon_pos_y,
            menu_locked,
        );
        self.status.interaction_zone = self.menu.interaction_zone();
        self.status.menu_locked = self.menu.is_locked();
        self.status.menu_visible = self.menu.is_visible();
    }
}

impl fmt::Debug for OverlayHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayHandle")
            .field("backend", &self.status.backend)
            .field("enabled", &self.status.enabled)
            .finish()
    }
}

fn headless_status(config: &OverlayConfig, limitation: &str) -> OverlayStatus {
    OverlayStatus {
        backend: OverlayBackend::HeadlessNoop,
        enabled: true,
        window_geometry: config.window_geometry,
        interaction_zone: menu_interaction_zone(
            config.window_geometry,
            config.overlay_icon_pos_x,
            config.overlay_icon_pos_y,
        ),
        menu_locked: config.menu_locked,
        menu_visible: config.show_overlay_icon && config.menu_locked,
        transparent: false,
        transparent_requested: false,
        transparent_confirmed: false,
        always_on_top: false,
        always_on_top_requested: false,
        always_on_top_confirmed: false,
        click_through: false,
        keyboard_interactive_requested: false,
        keyboard_interactive_confirmed: false,
        draw_error_count: 0,
        last_error: None,
        limitation: Some(limitation.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Create a Wayland overlay for the given capture-stream geometry.
///
/// `config.window_geometry` is the capture stream rect in stream space:
/// its origin is the output origin (0,0) and its size is the stream size.
/// With a Wayland session advertised (`WAYLAND_DISPLAY`), this attempts
/// the real layer-shell backend, anchored to the advertised `wl_output`
/// whose current-mode pixel size matches the stream size (see the module
/// docs for the coordinate chain). Compositors without
/// `zwlr_layer_shell_v1` (GNOME/Mutter, for example) yield the structured
/// `failed` state with a `limitation` naming the missing global; a failed
/// connection yields the same state; and when no advertised output
/// matches the stream size the surface is never created on a guessed
/// output — that too is the structured `failed` state naming the sizes.
/// Without a Wayland session the backend falls back to `headless_noop`.
/// No path crashes the caller's live loop and none claims an X11 backend.
pub fn create_overlay(config: OverlayConfig) -> Result<OverlayHandle, String> {
    create_overlay_anchored(config, None)
}

/// [`create_overlay`] with a global logical point that lies on the output
/// carrying the capture stream (the portal's monitor stream position, or
/// the game window's center for window streams). Among the outputs whose
/// pixel size matches the stream, the one containing the anchor wins; the
/// name tie-break only applies without a usable anchor. Two identical
/// monitors are otherwise indistinguishable by size alone.
pub fn create_overlay_anchored(
    config: OverlayConfig,
    anchor: Option<(i32, i32)>,
) -> Result<OverlayHandle, String> {
    if config.window_geometry.width == 0 || config.window_geometry.height == 0 {
        return Err("overlay window geometry must be non-zero".to_string());
    }

    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Ok(OverlayHandle::create_headless(
            config,
            "real Wayland overlay requires a Wayland session (WAYLAND_DISPLAY); \
             falling back to headless noop",
        ));
    }

    match wayland_client::Connection::connect_to_env() {
        Ok(conn) => {
            create_overlay_on_connection_anchored(conn, "WAYLAND_DISPLAY session", config, anchor)
        }
        Err(error) => Ok(OverlayHandle::create_failed(
            config,
            &format!("Wayland connection failed: {error}; using headless fallback"),
        )),
    }
}

/// Create the overlay on an explicit connection. The `compositor_source`
/// label travels into the smoke evidence so synthetic runs are never
/// mistaken for real-desktop proof.
///
/// # Errors
///
/// Only a zero geometry is a hard error; every environment failure —
/// including no advertised output matching the capture stream size — is
/// reported through the returned handle's `failed`/`headless_noop`
/// status instead.
#[doc(hidden)]
pub fn create_overlay_on_connection(
    conn: wayland_client::Connection,
    compositor_source: &str,
    config: OverlayConfig,
) -> Result<OverlayHandle, String> {
    create_overlay_on_connection_anchored(conn, compositor_source, config, None)
}

/// [`create_overlay_on_connection`] with an output anchor (see
/// [`create_overlay_anchored`]).
#[doc(hidden)]
pub fn create_overlay_on_connection_anchored(
    conn: wayland_client::Connection,
    compositor_source: &str,
    config: OverlayConfig,
    anchor: Option<(i32, i32)>,
) -> Result<OverlayHandle, String> {
    if config.window_geometry.width == 0 || config.window_geometry.height == 0 {
        return Err("overlay window geometry must be non-zero".to_string());
    }

    match wayland_backend::OverlayState::create(conn, compositor_source, &config, anchor) {
        Ok((state, queue)) => {
            let menu = OverlayMenuState::from_config(&config);
            let status = state.real_status(&menu);
            let text_diagnostics = state.text_diagnostics().clone();
            Ok(OverlayHandle {
                backend: Backend::Real(Box::new(state), queue),
                menu,
                status,
                last_frame: None,
                plan: RenderPlan::default(),
                paragon_regions: None,
                ever_drawn: false,
                last_update_skipped: false,
                text_diagnostics,
            })
        }
        Err(limitation) => Ok(OverlayHandle::create_failed(config, &limitation)),
    }
}

// ---------------------------------------------------------------------------
// Real Wayland backend — layer shell + SHM rendering.
// ---------------------------------------------------------------------------

fn paragon_input_regions(descriptors: &[d4lc_rules::RuleOverlayDescriptor]) -> Vec<Rect> {
    use d4lc_rules::{ParagonOverlayElementKind, RuleOverlayShape, RuleOverlayTarget};
    descriptors
        .iter()
        .filter(|descriptor| {
            descriptor.shape == RuleOverlayShape::TextPanel
                && matches!(
                    descriptor.target,
                    RuleOverlayTarget::Paragon {
                        kind: ParagonOverlayElementKind::StepPanel
                            | ParagonOverlayElementKind::BoardEntry,
                        ..
                    }
                )
        })
        .map(|descriptor| descriptor.geometry)
        .collect()
}

mod wayland_backend {
    use super::{
        menu_interaction_zone, overlay_color_pixel, premultiply_argb, text_wrap_policy,
        OverlayBackend, OverlayConfig, OverlayDrawTimings, OverlayMenuState, OverlayRenderKind,
        OverlayRenderOp, OverlayStatus, Point, Rect, TextDiagnostics, TextLineMask, TextRasterizer,
        OVERLAY_NAMESPACE,
    };
    use embedded_graphics::draw_target::DrawTarget;
    use embedded_graphics::geometry::Point as EgPoint;
    use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
    use embedded_graphics::prelude::{
        Drawable, OriginDimensions, Pixel, Primitive, Size as EgSize,
    };
    use embedded_graphics::primitives::{PrimitiveStyleBuilder, Rectangle, Triangle};
    use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
    use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
    use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
    use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
    use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
    use smithay_client_toolkit::shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    };
    use smithay_client_toolkit::shell::WaylandSurface;
    use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
    use smithay_client_toolkit::shm::{Shm, ShmHandler};
    use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};
    use std::cell::OnceCell;
    use std::time::Instant;
    use wayland_client::backend::WaylandError;
    use wayland_client::globals::registry_queue_init;
    use wayland_client::protocol::{wl_output, wl_pointer, wl_region, wl_seat, wl_shm, wl_surface};
    use wayland_client::{Connection, EventQueue, QueueHandle};

    /// Geometry snapshot of one advertised output, in the output's own
    /// pixel space. Built from the toolkit's `OutputInfo` (current
    /// `wl_output` mode); the pure matching and mapping logic below works
    /// on these snapshots so it is unit-testable without a connection.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct OutputSnapshot {
        /// The advertised output name (`HDMI-A-1`, ...); `None` when the
        /// compositor does not name outputs (pre-v4 `wl_output`, no
        /// xdg-output).
        pub name: Option<String>,
        /// The output's advertised scale factor. The identity mapping
        /// assumes scale 1; anything else is surfaced as a limitation.
        pub scale_factor: i32,
        /// Current-mode pixel size `(width, height)` — the output's real
        /// pixel grid. Falls back to the logical size (equal to the pixel
        /// grid at scale 1) when no modes are advertised, and to (0, 0)
        /// (which never matches a non-empty stream) when neither exists.
        pub pixel_size: (u32, u32),
        /// The output's rect in the compositor's global logical space
        /// (xdg-output position and size); `None` when not advertised.
        pub logical_rect: Option<Rect>,
    }

    impl OutputSnapshot {
        /// Whether the global logical point lies on this output.
        pub fn contains(&self, (x, y): (i32, i32)) -> bool {
            self.logical_rect.is_some_and(|rect| {
                x >= rect.x
                    && y >= rect.y
                    && i64::from(x) < i64::from(rect.x) + i64::from(rect.width)
                    && i64::from(y) < i64::from(rect.y) + i64::from(rect.height)
            })
        }

        pub fn from_info(info: &OutputInfo) -> Self {
            let pixel_size = info
                .modes
                .iter()
                .find(|mode| mode.current)
                .map(|mode| mode.dimensions)
                .or(info.logical_size)
                .map(|(w, h)| (w.max(0) as u32, h.max(0) as u32))
                .unwrap_or((0, 0));
            let logical_rect = match (info.logical_position, info.logical_size) {
                (Some((x, y)), Some((w, h))) if w > 0 && h > 0 => Some(Rect {
                    x,
                    y,
                    width: w as u32,
                    height: h as u32,
                }),
                _ => None,
            };
            Self {
                name: info.name.clone(),
                scale_factor: info.scale_factor,
                pixel_size,
                logical_rect,
            }
        }
    }

    /// Select the output that carries the capture stream (index only; the
    /// tests' view of [`select_output`]).
    ///
    /// The portal stream covers exactly one output, and its pixel grid is
    /// that output's pixel grid: stream-pixel == output-pixel, and the
    /// stream origin is the output origin (0,0) — the stream rect carries
    /// no global offset to match. A candidate therefore matches when its
    /// current-mode pixel size equals the stream size. When several
    /// outputs share that size (a size collision), the choice is
    /// resolved by `anchor` — a global logical point on the carrying
    /// output (monitor stream position, or the game window's center) —
    /// and only without a containing candidate deterministically:
    /// lexicographically first by advertised name, unnamed outputs last,
    /// then advertise order. A no-match is a failure — the
    /// surface is never anchored to an output of the wrong size.
    #[cfg(test)]
    pub fn select_output_index(
        outputs: &[OutputSnapshot],
        stream: &Rect,
        anchor: Option<(i32, i32)>,
    ) -> Option<usize> {
        select_output(outputs, stream, anchor).map(|selection| selection.index)
    }

    /// How [`select_output`] chose the output carrying the capture stream.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum OutputSelectionBasis {
        /// Exactly one advertised output has the stream's pixel size.
        UniqueSize,
        /// Several outputs share the stream size; the anchor point lies on
        /// the selected one.
        Anchor,
        /// Several outputs share the stream size and no anchor point lies
        /// on any of them: the name tie-break picked one of `candidates`
        /// (advertised names), which may not be the output the stream
        /// shows.
        TieBreak { candidates: Vec<String> },
    }

    /// The selected output index plus the basis of the choice.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct OutputSelection {
        pub index: usize,
        pub basis: OutputSelectionBasis,
    }

    /// The output selection described above, plus how the output was
    /// chosen, so a tie-break guess is announced in the status instead of
    /// looking like a proven match.
    pub fn select_output(
        outputs: &[OutputSnapshot],
        stream: &Rect,
        anchor: Option<(i32, i32)>,
    ) -> Option<OutputSelection> {
        let mut candidates = outputs
            .iter()
            .enumerate()
            .filter(|(_, output)| output.pixel_size == (stream.width, stream.height))
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return candidates.first().map(|(index, _)| OutputSelection {
                index: *index,
                basis: OutputSelectionBasis::UniqueSize,
            });
        }
        if let Some(point) = anchor {
            if let Some((index, _)) = candidates.iter().find(|(_, output)| output.contains(point)) {
                return Some(OutputSelection {
                    index: *index,
                    basis: OutputSelectionBasis::Anchor,
                });
            }
        }
        candidates.sort_by(|(a_index, a), (b_index, b)| match (&a.name, &b.name) {
            (Some(a_name), Some(b_name)) => a_name.cmp(b_name).then(a_index.cmp(b_index)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a_index.cmp(b_index),
        });
        let names = candidates
            .iter()
            .map(|(_, output)| output.name.as_deref().unwrap_or("<unnamed>").to_string())
            .collect();
        candidates.first().map(|(index, _)| OutputSelection {
            index: *index,
            basis: OutputSelectionBasis::TieBreak { candidates: names },
        })
    }

    /// The status line for a selection that is a guess, `None` for a
    /// proven one (unique size or anchor match).
    pub fn output_selection_line(
        basis: &OutputSelectionBasis,
        anchor: Option<(i32, i32)>,
    ) -> Option<String> {
        match basis {
            OutputSelectionBasis::UniqueSize | OutputSelectionBasis::Anchor => None,
            OutputSelectionBasis::TieBreak { candidates } => {
                let why = match anchor {
                    Some((x, y)) => format!("the stream position ({x}, {y}) lies on none of them"),
                    None => "the portal reported no stream position and no game-window \
                             position was known"
                        .to_string(),
                };
                Some(format!(
                    "output selection is a guess: outputs {} all match the capture stream size \
                     and {why}; the surface was anchored to the first by name, which may not \
                     be the monitor the stream shows",
                    candidates.join(", ")
                ))
            }
        }
    }

    /// The explicit coordinate-chain lines for the status limitation.
    ///
    /// The surface is anchored to the output carrying the capture stream,
    /// and the mapping is stream-pixel == output-pixel == surface-pixel
    /// (identity transform): window/stream-relative descriptors reach the
    /// surface unchanged. Every deviation from that identity — output
    /// scale != 1, a configure size that differs from the stream size, a
    /// non-zero stream origin, or a vanished anchored output — gets its
    /// own line so the status never silently mispositions.
    pub fn output_mapping_lines(
        anchored: Option<&OutputSnapshot>,
        anchored_gone: bool,
        configured_size: Option<(u32, u32)>,
        stream: &Rect,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        match anchored {
            Some(output) => {
                lines.push(format!(
                    "output-anchored placement: the surface fills output {} ({}x{} px, scale \
                     {}); stream-pixel == output-pixel == surface-pixel (identity transform) — \
                     descriptors stay stream-relative and need no extra mapping",
                    output.name.as_deref().unwrap_or("<unnamed>"),
                    output.pixel_size.0,
                    output.pixel_size.1,
                    output.scale_factor,
                ));
                if output.scale_factor != 1 {
                    lines.push(format!(
                        "output scale {}: the stream/overlay pixel mapping is not identity at \
                         this scale; overlay placement is not guaranteed",
                        output.scale_factor
                    ));
                }
            }
            None if anchored_gone => {
                lines.push(
                    "the output carrying the capture stream is gone; the compositor will close \
                     or reposition the surface and the next draw surfaces it"
                        .to_string(),
                );
            }
            None => {
                lines.push(
                    "the anchored output's geometry is unavailable; overlay placement cannot \
                     be described"
                        .to_string(),
                );
            }
        }
        if stream.x != 0 || stream.y != 0 {
            lines.push(format!(
                "stream rect origin ({}, {}) is ignored: the capture stream's origin is the \
                 output origin (0, 0)",
                stream.x, stream.y
            ));
        }
        if let Some((w, h)) = configured_size {
            if (w, h) != (stream.width, stream.height) {
                lines.push(format!(
                    "compositor configured the surface at {w}x{h} but the capture stream is \
                     {}x{}; descriptors are stream-relative, so overlay placement is shifted",
                    stream.width, stream.height
                ));
            }
        }
        lines
    }

    /// The failure reason when no advertised output matches the
    /// stream size: it names the stream size and every observed output so
    /// the log explains the mismatch instead of hiding it.
    pub fn no_match_limitation(outputs: &[OutputSnapshot], stream: &Rect) -> String {
        let observed = if outputs.is_empty() {
            "the compositor advertises no outputs".to_string()
        } else {
            format!(
                "observed outputs: {}",
                outputs
                    .iter()
                    .map(|output| format!(
                        "{} {}x{}",
                        output.name.as_deref().unwrap_or("<unnamed>"),
                        output.pixel_size.0,
                        output.pixel_size.1
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!(
            "no advertised output matches the capture stream size {}x{}; the overlay would be \
             mispositioned on a guessed output ({observed})",
            stream.width, stream.height
        )
    }

    /// Read what the compositor sent since the last update without blocking
    /// and dispatch it. `dispatch_pending` alone never reads the socket:
    /// after the setup round trips, buffer releases, configures and pointer
    /// events stayed unread. No release ever arrived, so every draw took a
    /// fresh slot and the SHM pool grew by one full-output buffer per
    /// changed frame, and the unread events piled up in the compositor's
    /// send buffer for this client.
    pub(super) fn dispatch_available(
        queue: &mut EventQueue<OverlayState>,
        state: &mut OverlayState,
    ) -> Result<usize, String> {
        queue
            .flush()
            .map_err(|e| format!("wayland flush failed: {e}"))?;
        if let Some(guard) = queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("wayland read failed: {error}")),
            }
        }
        queue
            .dispatch_pending(state)
            .map_err(|error| format!("wayland dispatch failed: {error}"))
    }

    /// Client-side rendering state. Owns the layer surface, the SHM pool,
    /// the capability record, and the surface pointer tracking.
    /// The menu state machine lives in the handle; this state only records
    /// the pointer observations the handle feeds into it.
    pub struct OverlayState {
        /// Retained so the bound `wl_compositor` global outlives the
        /// surface (protocol objects are reference-counted by proxy).
        _compositor_state: CompositorState,
        shm: Shm,
        pool: SlotPool,
        /// The previous frame's buffer, kept alive until the compositor's
        /// release event proves it is no longer scanned (buffer
        /// ownership). Reused for the next draw only when the size matches
        /// and sctk's active-buffer tracking says the canvas is writable
        /// again; otherwise the pool allocates a fresh slot and sctk
        /// destroys the old buffer on release.
        spare: Option<Buffer>,
        /// The output-anchored layer surface. Held in a cell because the
        /// surface is created only after the capture-stream output has
        /// been selected from the first roundtrip's output events.
        layer: OnceCell<LayerSurface>,
        registry_state: RegistryState,
        seat_state: SeatState,
        output_state: OutputState,
        window_geometry: Rect,
        width: u32,
        height: u32,
        /// The size the compositor configured the surface to (both
        /// components non-zero). A value differing from the stream size is
        /// surfaced as a limitation: descriptors are stream-relative.
        configured_size: Option<(u32, u32)>,
        /// The `wl_output` the surface is anchored to (the capture
        /// stream's output). The mapping lines read its live geometry from
        /// `output_state.info`, so scale/geometry changes surface without
        /// extra bookkeeping.
        anchored_output: Option<wl_output::WlOutput>,
        /// Whether the anchored output has been destroyed or its geometry
        /// has vanished; the compositor closes or repositions the surface
        /// and the next draw surfaces it.
        anchored_output_gone: bool,
        /// The announced status line when the output selection was a
        /// name tie-break guess (see [`output_selection_line`]).
        output_selection_note: Option<String>,
        /// The last game-window geometry update the handle did not apply
        /// (the surface stays output-anchored to the capture stream).
        ignored_geometry: Option<Rect>,
        pub(super) pointer_window_relative: Option<Point>,
        /// Set when the surface changed in a way that invalidates painted
        /// pixels (configure with a size change, lost anchored output); the
        /// next update consumes it and forces a redraw.
        pub(crate) redraw_required: bool,
        /// The shared render-op rasterizer: same text contract and glyph
        /// rasterization as the X11 backend, painted into the SHM canvas.
        renderer: CanvasRenderer,
        input_region_applied: bool,
        input_regions: Vec<Rect>,
        pub(crate) first_frame_committed: bool,
        compositor_source: String,
        pub(super) snapshot_enabled: bool,
        pub(super) last_committed: Option<(u32, u32, Vec<u8>)>,
        /// Raster/commit costs of the last completed draw.
        pub(crate) last_draw_timings: OverlayDrawTimings,
        /// The ink area of the frame the compositor currently shows, used as
        /// the clear and damage anchor of the next draw. `None`
        /// before the first commit.
        last_ink_rect: Option<Rect>,
    }

    impl OverlayState {
        /// Connect protocol globals and create the output-anchored layer
        /// surface. `Err(limitation)` carries the unavailability
        /// reason — including a capture stream no advertised output
        /// matches.
        pub fn create(
            conn: Connection,
            compositor_source: &str,
            config: &OverlayConfig,
            anchor: Option<(i32, i32)>,
        ) -> Result<(Self, EventQueue<Self>), String> {
            let (globals, mut event_queue) = registry_queue_init(&conn)
                .map_err(|e| format!("Wayland registry init failed: {e}"))?;
            let qh = event_queue.handle();

            let compositor_state = CompositorState::bind(&globals, &qh)
                .map_err(|e| format!("wl_compositor global unavailable: {e}"))?;

            // The decisive capability: without zwlr_layer_shell_v1 there is
            // no layer-shell overlay path at all (GNOME/Mutter sessions,
            // for example). Report the structured unavailability instead of
            // any fallback claim.
            let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| {
                "compositor does not advertise zwlr_layer_shell_v1; Wayland layer-shell \
                 overlay unavailable (GNOME/Mutter sessions, for example)"
                    .to_string()
            })?;

            let shm =
                Shm::bind(&globals, &qh).map_err(|e| format!("wl_shm global unavailable: {e}"))?;

            // The capture stream rect: origin (0,0) is the output origin,
            // width/height are the stream pixels the identity mapping
            // equates with output pixels. The x/y offsets are stream-space
            // metadata and never shift the surface.
            let width = config.window_geometry.width;
            let height = config.window_geometry.height;
            let pool = SlotPool::new((width * height * 4) as usize, &shm)
                .map_err(|e| format!("wl_shm pool allocation failed: {e}"))?;

            let surface = compositor_state.create_surface(&qh);

            // Click-through input region: only the menu interaction zone
            // accepts pointer input (empty region entirely when the menu
            // icon is disabled). Applied before the first commit so the
            // compositor sees it for the mapped surface.
            let interaction_zone = menu_interaction_zone(
                config.window_geometry,
                config.overlay_icon_pos_x,
                config.overlay_icon_pos_y,
            );
            let region = compositor_state.wl_compositor().create_region(&qh, ());
            if config.show_overlay_icon {
                region.add(
                    interaction_zone.x,
                    interaction_zone.y,
                    interaction_zone.width as i32,
                    interaction_zone.height as i32,
                );
            }
            surface.set_input_region(Some(&region));

            let mut state = Self {
                _compositor_state: compositor_state,
                shm,
                pool,
                spare: None,
                layer: OnceCell::new(),
                registry_state: RegistryState::new(&globals),
                seat_state: SeatState::new(&globals, &qh),
                output_state: OutputState::new(&globals, &qh),
                window_geometry: config.window_geometry,
                width,
                height,
                configured_size: None,
                anchored_output: None,
                anchored_output_gone: false,
                output_selection_note: None,
                ignored_geometry: None,
                renderer: CanvasRenderer::new(),
                pointer_window_relative: None,
                redraw_required: false,
                input_region_applied: true,
                input_regions: if config.show_overlay_icon {
                    vec![interaction_zone]
                } else {
                    Vec::new()
                },
                first_frame_committed: false,
                compositor_source: compositor_source.to_string(),
                snapshot_enabled: false,
                last_committed: None,
                last_draw_timings: OverlayDrawTimings::default(),
                last_ink_rect: None,
            };

            // Discovery roundtrip: collect the advertised outputs'
            // geometry/mode/scale/name events before selecting the anchor
            // target. No layer surface exists yet, so nothing else can
            // fire.
            event_queue
                .roundtrip(&mut state)
                .map_err(|e| format!("output discovery roundtrip failed: {e}"))?;

            // Select the output carrying the capture stream: match by
            // current-mode pixel size, then by the anchor point, then a
            // deterministic tie-break by name that the status announces as
            // a guess. No size match is a failure — the surface is
            // never anchored to an output of the wrong size.
            let mut candidates: Vec<(wl_output::WlOutput, OutputSnapshot)> = state
                .output_state
                .outputs()
                .map(|output| {
                    let snapshot = state
                        .output_state
                        .info(&output)
                        .map(|info| OutputSnapshot::from_info(&info))
                        .unwrap_or(OutputSnapshot {
                            name: None,
                            scale_factor: 1,
                            pixel_size: (0, 0),
                            logical_rect: None,
                        });
                    (output, snapshot)
                })
                .collect();
            let snapshots: Vec<OutputSnapshot> = candidates
                .iter()
                .map(|(_, snapshot)| snapshot.clone())
                .collect();
            let selection = select_output(&snapshots, &config.window_geometry, anchor)
                .ok_or_else(|| no_match_limitation(&snapshots, &config.window_geometry))?;
            state.output_selection_note = output_selection_line(&selection.basis, anchor);
            let matched_output = candidates.remove(selection.index).0;
            state.anchored_output = Some(matched_output);

            // Anchor the surface to that output: all edges, zero margins,
            // fill request (0x0) — the compositor assigns the output size
            // and reports it in the configure event, which drives the
            // surface size from here on.
            let layer = layer_shell.create_layer_surface(
                &qh,
                surface,
                Layer::Overlay,
                Some(OVERLAY_NAMESPACE),
                state.anchored_output.as_ref(),
            );
            layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            layer.set_margin(0, 0, 0, 0);
            layer.set_size(0, 0);
            // Mirror the focus-free X11 overlay: no keyboard interactivity.
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.commit();
            state
                .layer
                .set(layer)
                .map_err(|_| "layer surface already created".to_string())?;

            // Wait for the compositor's first configure (it names the
            // assigned output size), then map the surface with a
            // transparent initial frame.
            event_queue
                .roundtrip(&mut state)
                .map_err(|e| format!("layer surface configure roundtrip failed: {e}"))?;
            state
                .draw(&mut event_queue, &[])
                .map_err(|e| format!("initial layer surface frame failed: {e}"))?;
            if !state.first_frame_committed {
                return Err(
                    "compositor did not accept the initial d4lc layer surface frame; \
                     overlay unusable"
                        .to_string(),
                );
            }

            Ok((state, event_queue))
        }

        /// The real-Wayland status at creation time. Menu and error
        /// fields are refreshed by the handle on every update.
        pub fn real_status(&self, menu: &OverlayMenuState) -> OverlayStatus {
            OverlayStatus {
                backend: OverlayBackend::RealWayland,
                enabled: true,
                window_geometry: self.window_geometry,
                interaction_zone: menu.interaction_zone(),
                menu_locked: menu.is_locked(),
                menu_visible: menu.is_visible(),
                // ARGB SHM buffer requested; compositor-visible
                // transparency is not client-provable (same
                // requested-vs-confirmed split as the X11 ARGB path).
                transparent: false,
                transparent_requested: true,
                transparent_confirmed: false,
                // Overlay layer requested; compositor-managed z-order means
                // the client cannot prove above-game placement.
                always_on_top: false,
                always_on_top_requested: true,
                always_on_top_confirmed: false,
                // The input region excludes everything but the interaction
                // zone; it was applied on the live surface before the first
                // commit and the surface survived that frame without a
                // protocol error.
                click_through: self.input_region_applied && self.first_frame_committed,
                keyboard_interactive_requested: false,
                keyboard_interactive_confirmed: false,
                draw_error_count: 0,
                last_error: None,
                limitation: Some(self.limitation()),
            }
        }

        pub fn limitation(&self) -> String {
            let mut limitations = vec![
                "compositor-managed z-order: overlay layer requested, above-game placement \
                 not client-provable"
                    .to_string(),
                "ARGB SHM transparency requested; compositor-visible alpha blending not \
                 confirmed"
                    .to_string(),
                "global cursor position has no Wayland equivalent; pointer position known \
                 only from overlay-surface pointer events within the interaction zone"
                    .to_string(),
                "keyboard interactivity not requested (layer-shell None mode, mirroring the \
                 focus-free X11 overlay); menu hover toggle requires pointer entry into the \
                 interaction zone"
                    .to_string(),
            ];
            limitations.extend(self.output_selection_note.clone());
            limitations.extend(output_mapping_lines(
                self.anchored_snapshot().as_ref(),
                self.anchored_output_gone,
                self.configured_size,
                &self.window_geometry,
            ));
            if let Some(ignored) = self.ignored_geometry {
                limitations.push(format!(
                    "geometry update to {}x{} at ({}, {}) not applied: the surface stays \
                     output-anchored to the capture stream's output",
                    ignored.width, ignored.height, ignored.x, ignored.y
                ));
            }
            format!("{}; {}", limitations.join("; "), self.renderer.limitation())
        }

        /// The anchored output's current geometry snapshot, read live from
        /// the toolkit's output tracking (`OutputState::info`), so scale or
        /// geometry changes on that output surface in the status without
        /// duplicated bookkeeping.
        fn anchored_snapshot(&self) -> Option<OutputSnapshot> {
            let output = self.anchored_output.as_ref()?;
            self.output_state
                .info(output)
                .map(|info| OutputSnapshot::from_info(&info))
        }

        /// Whether `output` is the output the surface is anchored to.
        fn is_anchored(&self, output: &wl_output::WlOutput) -> bool {
            self.anchored_output.as_ref() == Some(output)
        }

        /// Record a game-window geometry update the handle did not apply
        /// (the surface stays output-anchored to the capture stream); the
        /// status limitation names it.
        pub fn note_ignored_geometry(&mut self, geometry: Rect) {
            self.ignored_geometry = Some(geometry);
        }

        /// What the shared bounded text path could not draw, or why it is
        /// disabled.
        pub fn text_diagnostics(&self) -> &TextDiagnostics {
            self.renderer.diagnostics()
        }

        /// Rasterize the render ops into an SHM buffer and commit it.
        pub fn draw(
            &mut self,
            queue: &mut EventQueue<Self>,
            render_ops: &[OverlayRenderOp],
        ) -> Result<(), String> {
            let draw_start = Instant::now();
            let Some(layer) = self.layer.get() else {
                return Err("overlay draw before the layer surface was created".to_string());
            };
            let width = self.width;
            let height = self.height;
            let stride = width as i32 * 4;

            // Buffer ownership: never write into a buffer the
            // compositor may still hold. The previous frame's buffer is
            // reused only once its release event arrived (sctk's active
            // tracking marks its canvas writable again); otherwise a fresh
            // slot is allocated and sctk destroys the old buffer on
            // release. The slot memory therefore never outlives the
            // compositor's use of it.
            let reusable = self
                .spare
                .take()
                .filter(|buffer| buffer.height() == height as i32 && buffer.stride() == stride);
            let (buffer, canvas, reused_last_frame) = match reusable {
                Some(spare) => match spare.canvas(&mut self.pool) {
                    Some(canvas) => (spare, canvas, true),
                    None => {
                        let fresh = self
                            .pool
                            .create_buffer(
                                width as i32,
                                height as i32,
                                stride,
                                wl_shm::Format::Argb8888,
                            )
                            .map_err(|e| format!("shm buffer allocation failed: {e}"))?;
                        (fresh.0, fresh.1, false)
                    }
                },
                None => {
                    let fresh = self
                        .pool
                        .create_buffer(
                            width as i32,
                            height as i32,
                            stride,
                            wl_shm::Format::Argb8888,
                        )
                        .map_err(|e| format!("shm buffer allocation failed: {e}"))?;
                    (fresh.0, fresh.1, false)
                }
            };

            // Clear only the previous frame's ink when this is the
            // buffer the compositor last showed (its remaining pixels are
            // identical to the screen); a fresh slot's recycled memory is
            // unknown and is fully cleared.
            let clear_rect = if reused_last_frame {
                self.last_ink_rect
            } else {
                None
            };
            let ink = self
                .renderer
                .render_tracked(canvas, width, height, render_ops, clear_rect);

            if self.snapshot_enabled {
                self.last_committed = Some((width, height, canvas.to_vec()));
            }

            let raster_us = OverlayDrawTimings::micros(draw_start.elapsed());
            // Damage the screen area this commit can change: the union of
            // the ink the compositor currently shows and this frame's ink.
            // The first commit on a surface damages everything.
            let damage = match self.last_ink_rect {
                None => Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                Some(previous) => {
                    let mut damage = Some(previous);
                    union_in(&mut damage, ink);
                    clamp_rect(damage, width, height)
                }
            };
            layer.wl_surface().damage_buffer(
                damage.x,
                damage.y,
                damage.width as i32,
                damage.height as i32,
            );
            buffer
                .attach_to(layer.wl_surface())
                .map_err(|e| format!("buffer attach failed: {e}"))?;
            layer.commit();

            // The commit is flushed, not awaited. A blocking
            // round trip per draw put the compositor's latency on the live
            // loop's critical path; after the first frame the loop only
            // flushes, and a protocol error the compositor produces
            // surfaces on the next update's dispatch (which fails the
            // overlay). The first commit keeps the round trip: it
            // must be live before the surface reports click-through
            // confirmation, and the frame-callback pacing then
            // bounds how far the loop can run ahead.
            if self.first_frame_committed {
                queue
                    .flush()
                    .map_err(|e| format!("wayland flush failed: {e}"))?;
            } else {
                queue
                    .roundtrip(self)
                    .map_err(|e| format!("wayland roundtrip failed: {e}"))?;
            }

            // Keep the committed buffer alive until its release event
            // arrives (see the buffer-ownership note above).
            self.spare = Some(buffer);
            self.last_ink_rect = Some(ink);
            self.first_frame_committed = true;
            self.last_draw_timings = OverlayDrawTimings {
                raster_us: Some(raster_us),
                commit_us: Some(
                    OverlayDrawTimings::micros(draw_start.elapsed()).saturating_sub(raster_us),
                ),
            };
            Ok(())
        }

        pub(crate) fn set_input_regions(&mut self, qh: &QueueHandle<Self>, regions: Vec<Rect>) {
            if self.input_regions == regions {
                return;
            }
            let Some(layer) = self.layer.get() else {
                return;
            };
            let region = self._compositor_state.wl_compositor().create_region(qh, ());
            for rect in &regions {
                region.add(rect.x, rect.y, rect.width as i32, rect.height as i32);
            }
            layer.wl_surface().set_input_region(Some(&region));
            region.destroy();
            self.input_regions = regions;
            self.redraw_required = true;
        }

        pub fn compositor_source(&self) -> &str {
            &self.compositor_source
        }

        /// Consume the forced-redraw flag: set by a configure
        /// with a size change or a lost anchored output, read and cleared by
        /// the next update.
        pub(crate) fn take_redraw_required(&mut self) -> bool {
            std::mem::take(&mut self.redraw_required)
        }
    }

    // -----------------------------------------------------------------------
    // Client-side rasterization of the shared render ops.
    // -----------------------------------------------------------------------

    fn pixel_bytes(premultiplied: u32) -> [u8; 4] {
        premultiplied.to_le_bytes()
    }

    /// Writes premultiplied ARGB8888 pixels (little-endian: b,g,r,a in
    /// memory) for one render op color.
    struct ArgbCanvas<'a> {
        buf: &'a mut [u8],
        width: u32,
        height: u32,
        /// Current write color in little-endian buffer bytes.
        color: [u8; 4],
        /// The op's alpha, applied to text glyph pixels.
        alpha: u8,
    }

    impl ArgbCanvas<'_> {
        fn new(buf: &mut [u8], width: u32, height: u32, premultiplied: u32) -> ArgbCanvas<'_> {
            ArgbCanvas {
                buf,
                width,
                height,
                color: pixel_bytes(premultiplied),
                alpha: (premultiplied >> 24) as u8,
            }
        }

        fn write_pixel(&mut self, x: i32, y: i32) {
            if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                return;
            }
            let idx = ((y as usize) * (self.width as usize) + x as usize) * 4;
            self.buf[idx..idx + 4].copy_from_slice(&self.color);
        }

        fn fill_rect(&mut self, rect: Rect) {
            let x0 = rect.x.max(0);
            let y0 = rect.y.max(0);
            let x1 = (rect.x + rect.width as i32).min(self.width as i32);
            let y1 = (rect.y + rect.height as i32).min(self.height as i32);
            for y in y0..y1 {
                for x in x0..x1 {
                    self.write_pixel(x, y);
                }
            }
        }

        fn outline_rect(&mut self, rect: Rect) {
            // X11 `poly_rectangle` strokes a 1px border inside the geometry.
            if rect.width == 0 || rect.height == 0 {
                return;
            }
            let right = rect.x + rect.width as i32 - 1;
            let bottom = rect.y + rect.height as i32 - 1;
            for offset in 0..rect.width as i32 {
                self.write_pixel(rect.x + offset, rect.y);
                self.write_pixel(rect.x + offset, bottom);
            }
            for offset in 0..rect.height as i32 {
                self.write_pixel(rect.x, rect.y + offset);
                self.write_pixel(right, rect.y + offset);
            }
        }

        /// Upstream `Graphics.OutlineFillRectangle(black, fill, ..., stroke)`
        /// (`OverlayHandler.cs` 282/:300/:313): the marker color fills the
        /// rectangle and a black border of `stroke` pixels sits inside its
        /// edge. The X11 backend paints the same form with
        /// `poly_fill_rectangle` + `poly_rectangle` at the same GC line
        /// width.
        fn outline_fill_rect(&mut self, rect: Rect, stroke: u8) {
            let fill_color = self.color;
            self.fill_rect(rect);
            if rect.width == 0 || rect.height == 0 {
                return;
            }
            let widest = (rect.width.min(rect.height) as i32 + 1) / 2;
            let rings = i32::from(stroke).clamp(0, widest.max(0));
            self.color = pixel_bytes(premultiply_argb(0xFF000000));
            for ring in 0..rings {
                let left = rect.x + ring;
                let right = rect.x + rect.width as i32 - 1 - ring;
                let top = rect.y + ring;
                let bottom = rect.y + rect.height as i32 - 1 - ring;
                if left > right || top > bottom {
                    break;
                }
                for x in left..=right {
                    self.write_pixel(x, top);
                    self.write_pixel(x, bottom);
                }
                for y in top..=bottom {
                    self.write_pixel(left, y);
                    self.write_pixel(right, y);
                }
            }
            self.color = fill_color;
        }

        /// Upstream `Graphics.OutlineFillCircle`: the shared antialiased
        /// raster (fill inside, 2px black outline along the inside of the
        /// bounding square) blended OVER the canvas, pixel for pixel what
        /// the X11 RENDER composite produces.
        fn outline_fill_circle(&mut self, rect: Rect) {
            let fill = u32::from_le_bytes(self.color);
            let Some(image) = d4lc_overlay_x11::shape::outline_fill_circle(rect, fill) else {
                return;
            };
            for row in 0..image.height {
                let y = image.y + row as i32;
                if y < 0 || y >= self.height as i32 {
                    continue;
                }
                for column in 0..image.width {
                    let x = image.x + column as i32;
                    if x < 0 || x >= self.width as i32 {
                        continue;
                    }
                    let source = image.pixels[(row * image.width + column) as usize];
                    if source == 0 {
                        continue;
                    }
                    let index = ((y as usize) * (self.width as usize) + x as usize) * 4;
                    let target = &mut self.buf[index..index + 4];
                    let beneath = u32::from_le_bytes([target[0], target[1], target[2], target[3]]);
                    let blended = d4lc_overlay_x11::text::blend_over(source, beneath);
                    target.copy_from_slice(&blended.to_le_bytes());
                }
            }
        }

        /// Upstream greater-affix marker: `gfx.FillTriangle` +
        /// `gfx.DrawTriangle(Colors.Black, 2)` (`OverlayHandler.cs`
        /// 305-310) from the marker's bounding box — apex at the
        /// top-center, base on the bottom edge. The Wayland counterpart
        /// of the X11 `fill_poly` + closed `poly_line` pair.
        fn outline_fill_triangle(&mut self, rect: Rect, fill: Rgb888) {
            let apex = EgPoint::new(rect.x + rect.width as i32 / 2, rect.y);
            let base_left = EgPoint::new(rect.x, rect.y + rect.height as i32);
            let base_right = EgPoint::new(rect.x + rect.width as i32, rect.y + rect.height as i32);
            let style = PrimitiveStyleBuilder::new()
                .fill_color(fill)
                .stroke_color(Rgb888::BLACK)
                .stroke_width(2)
                .build();
            Triangle::new(base_left, base_right, apex)
                .into_styled(style)
                .draw(self)
                .ok();
        }
    }

    /// Client-side rasterizer for the shared render ops: owns the reusable
    /// bounded-text engine and the reused glyph mask, so one frame allocates
    /// only the canvas the caller passes. Pure drawing; no protocol, no
    /// descriptor logic.
    /// Union `rect` into `dirty`. A zero-size rect is "nothing".
    pub(crate) fn union_in(dirty: &mut Option<Rect>, rect: Rect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        *dirty = Some(match *dirty {
            Some(existing) => {
                let x = existing.x.min(rect.x);
                let y = existing.y.min(rect.y);
                let right = (existing.x + existing.width as i32).max(rect.x + rect.width as i32);
                let bottom = (existing.y + existing.height as i32).max(rect.y + rect.height as i32);
                Rect {
                    x,
                    y,
                    width: (right - x).max(0) as u32,
                    height: (bottom - y).max(0) as u32,
                }
            }
            None => rect,
        });
    }

    /// Clamp a rect to the canvas bounds; an empty or fully out-of-bounds
    /// rect collapses to the zero-size rect (no damage).
    pub(crate) fn clamp_rect(rect: Option<Rect>, width: u32, height: u32) -> Rect {
        let Some(rect) = rect else {
            return Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        };
        let x = rect.x.clamp(0, width as i32);
        let y = rect.y.clamp(0, height as i32);
        let right = (rect.x + rect.width as i32).clamp(0, width as i32);
        let bottom = (rect.y + rect.height as i32).clamp(0, height as i32);
        Rect {
            x,
            y,
            width: (right - x).max(0) as u32,
            height: (bottom - y).max(0) as u32,
        }
    }

    /// Zero the pixels of `rect` (already canvas-clamped) in a tightly
    /// packed width*height*4 canvas.
    fn fill_rect_zero(canvas: &mut [u8], width: u32, rect: Rect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let stride = width as usize * 4;
        let row_len = rect.width as usize * 4;
        for y in rect.y..rect.y + rect.height as i32 {
            let row = y as usize * stride + rect.x as usize * 4;
            canvas[row..row + row_len].fill(0);
        }
    }

    pub struct CanvasRenderer {
        text: Option<TextRasterizer>,
        mask: TextLineMask,
        diagnostics: TextDiagnostics,
    }

    impl CanvasRenderer {
        /// Resolves the shared text path's font assignment. A failure is
        /// reported through [`CanvasRenderer::diagnostics`] and text ops draw
        /// nothing — no substitute face is ever used.
        pub fn new() -> Self {
            let mut diagnostics = TextDiagnostics::default();
            let text = match TextRasterizer::load_system() {
                Ok(rasterizer) => Some(rasterizer),
                Err(error) => {
                    diagnostics.record_font_unavailable(&error);
                    eprintln!("d4lc-overlay-wayland: overlay text disabled: {error}");
                    None
                }
            };
            Self {
                text,
                mask: TextLineMask::default(),
                diagnostics,
            }
        }

        /// What the text path could not draw, or why it is disabled.
        pub fn diagnostics(&self) -> &TextDiagnostics {
            &self.diagnostics
        }

        /// One limitation line for the status.
        pub fn limitation(&self) -> String {
            if let Some(font_unavailable) = &self.diagnostics.font_unavailable {
                return format!("overlay text disabled: {font_unavailable}");
            }
            match &self.text {
                Some(rasterizer) => format!(
                    "overlay text: {} at OverlayFontSize, wrapped panels, single-line \
                     markers and exact paragon lines (Consolas Bold, else bundled \
                     Inconsolata Bold)",
                    rasterizer.assignment()
                ),
                None => "overlay text disabled: no font assignment".to_string(),
            }
        }

        /// Rasterizes the shared, backend-agnostic render ops into a
        /// premultiplied ARGB8888 canvas.
        /// Render the ops into the canvas, clearing only `clear_rect` (the
        /// previously presented ink area of this buffer) when given, or the
        /// whole canvas when `None`. Returns the bounding box of
        /// the pixels this frame's ops touched, clamped to the canvas — the
        /// damage candidate for this frame.
        pub fn render_tracked(
            &mut self,
            canvas: &mut [u8],
            width: u32,
            height: u32,
            render_ops: &[OverlayRenderOp],
            clear_rect: Option<Rect>,
        ) -> Rect {
            match clear_rect {
                Some(rect) => fill_rect_zero(canvas, width, rect),
                None => canvas.fill(0),
            }
            let mut dirty: Option<Rect> = None;

            for operation in render_ops {
                let straight = overlay_color_pixel(&operation.color);
                let premultiplied = premultiply_argb(straight);
                match operation.kind {
                    OverlayRenderKind::FilledRect => {
                        ArgbCanvas::new(canvas, width, height, premultiplied)
                            .fill_rect(operation.geometry);
                        union_in(&mut dirty, operation.geometry);
                    }
                    OverlayRenderKind::OutlineRect => {
                        ArgbCanvas::new(canvas, width, height, premultiplied)
                            .outline_rect(operation.geometry);
                        union_in(&mut dirty, operation.geometry);
                    }
                    OverlayRenderKind::StrokedRect { stroke } => {
                        let stroke = stroke.clamp(1, u32::from(u16::MAX));
                        let color = Rgb888::new(
                            (straight >> 16) as u8,
                            (straight >> 8) as u8,
                            straight as u8,
                        );
                        let style = PrimitiveStyleBuilder::new()
                            .stroke_color(color)
                            .stroke_width(stroke)
                            .build();
                        let _ = Rectangle::new(
                            EgPoint::new(operation.geometry.x, operation.geometry.y),
                            EgSize::new(operation.geometry.width, operation.geometry.height),
                        )
                        .into_styled(style)
                        .draw(&mut ArgbCanvas::new(
                            canvas,
                            width,
                            height,
                            premultiplied,
                        ));
                        let margin = (stroke / 2) as i32;
                        union_in(
                            &mut dirty,
                            Rect {
                                x: operation.geometry.x - margin,
                                y: operation.geometry.y - margin,
                                width: operation.geometry.width.saturating_add(stroke),
                                height: operation.geometry.height.saturating_add(stroke),
                            },
                        );
                    }
                    OverlayRenderKind::OutlineFillRect { stroke } => {
                        // Upstream `OutlineFillRectangle(black, affixColor,
                        // ..., stroke)`: the marker color fills the rectangle
                        // and a black border of the marker's stroke width
                        // (2 for a sigil dungeon marker, 1 for an any-type or
                        // below-minimal affix) sits inside its edge.
                        ArgbCanvas::new(canvas, width, height, premultiplied)
                            .outline_fill_rect(operation.geometry, stroke);
                        union_in(&mut dirty, operation.geometry);
                    }
                    OverlayRenderKind::Circle => {
                        ArgbCanvas::new(canvas, width, height, premultiplied)
                            .outline_fill_circle(operation.geometry);
                        union_in(&mut dirty, operation.geometry);
                    }
                    OverlayRenderKind::Triangle => {
                        // Straight-alpha fill channels; `draw_iter` applies the
                        // op alpha (and premultiplication) per pixel exactly
                        // like the text path.
                        ArgbCanvas::new(canvas, width, height, premultiplied)
                            .outline_fill_triangle(
                                operation.geometry,
                                Rgb888::new(
                                    (straight >> 16) as u8,
                                    (straight >> 8) as u8,
                                    straight as u8,
                                ),
                            );
                        union_in(&mut dirty, operation.geometry);
                    }
                    OverlayRenderKind::Text => {
                        if let Some(ink) =
                            self.draw_text_op(canvas, width, height, operation, straight)
                        {
                            union_in(&mut dirty, ink);
                        }
                    }
                }
            }

            clamp_rect(dirty, width, height)
        }

        /// Paints one text op through the shared bounded text path: layout
        /// inside the op's area under the op's wrap contract, then blit each
        /// line's glyph mask with the same coverage-to-pixel rule the X11
        /// `put_image` payload uses — identical render ops produce identical
        /// text pixels on both backends. Returns the union of the line
        /// masks' pixel areas (the text ink), when any line was painted.
        fn draw_text_op(
            &mut self,
            canvas: &mut [u8],
            width: u32,
            height: u32,
            operation: &OverlayRenderOp,
            straight_color: u32,
        ) -> Option<Rect> {
            let mut ink: Option<Rect> = None;
            let Some(text) = operation.text.as_deref() else {
                return ink;
            };
            let Some(rasterizer) = self.text.as_mut() else {
                return ink;
            };
            let opacity = (straight_color >> 24) as u8;
            let layout = rasterizer.layout(
                operation.geometry,
                text_wrap_policy(operation),
                text,
                operation.font_px,
            );
            self.diagnostics.record(&layout);
            for line in &layout.lines {
                rasterizer.rasterize_line_into(line, &mut self.mask);
                self.mask
                    .paint_into(canvas, width, height, straight_color, opacity);
                union_in(
                    &mut ink,
                    Rect {
                        x: self.mask.x,
                        y: self.mask.y,
                        width: self.mask.width,
                        height: self.mask.height,
                    },
                );
            }
            ink
        }
    }

    impl Default for CanvasRenderer {
        fn default() -> Self {
            Self::new()
        }
    }

    impl DrawTarget for ArgbCanvas<'_> {
        type Color = Rgb888;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Rgb888>>,
        {
            let alpha = self.alpha;
            for Pixel(point, color) in pixels {
                if point.x < 0 || point.y < 0 {
                    continue;
                }
                // Premultiply the glyph color with the op alpha before the
                // little-endian b,g,r,a write (wl_shm ARGB8888 contract).
                let a = u32::from(alpha);
                let r = (u32::from(color.r()) * a / 255) as u8;
                let g = (u32::from(color.g()) * a / 255) as u8;
                let b = (u32::from(color.b()) * a / 255) as u8;
                self.color = [b, g, r, alpha];
                self.write_pixel(point.x, point.y);
            }
            Ok(())
        }
    }

    impl OriginDimensions for ArgbCanvas<'_> {
        fn size(&self) -> EgSize {
            EgSize::new(self.width, self.height)
        }
    }

    // -----------------------------------------------------------------------
    // Toolkit event handlers.
    // -----------------------------------------------------------------------

    impl CompositorHandler for OverlayState {
        fn scale_factor_changed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            _new_factor: i32,
        ) {
        }

        fn transform_changed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            _new_transform: wayland_client::protocol::wl_output::Transform,
        ) {
        }

        fn frame(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            _time: u32,
        ) {
        }

        fn surface_enter(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            _output: &wayland_client::protocol::wl_output::WlOutput,
        ) {
        }

        fn surface_leave(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            _output: &wayland_client::protocol::wl_output::WlOutput,
        ) {
        }
    }

    impl LayerShellHandler for OverlayState {
        fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
            // The compositor closed the overlay surface. The next draw's
            // roundtrip surfaces the dead connection.
        }

        fn configure(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _layer: &LayerSurface,
            configure: LayerSurfaceConfigure,
            _serial: u32,
        ) {
            let (w, h) = configure.new_size;
            if w == 0 || h == 0 {
                // A zero component means the compositor did not assign
                // that dimension; keep the current size (the capture
                // stream's size on the pre-configure surface).
                return;
            }
            self.configured_size = Some((w, h));
            if w != self.width || h != self.height {
                self.width = w;
                self.height = h;
                // The painted pixels of the previous size are gone.
                self.redraw_required = true;
                // The surface size is compositor-driven; rebuild the SHM
                // pool so the next draw allocates its buffer at the
                // configured size. If the rebuild fails, the next draw
                // fails on the old pool instead of here.
                if let Ok(pool) = SlotPool::new((w * h * 4) as usize, &self.shm) {
                    self.pool = pool;
                }
            }
        }
    }

    impl OutputHandler for OverlayState {
        fn output_state(&mut self) -> &mut OutputState {
            &mut self.output_state
        }

        fn new_output(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _output: wayland_client::protocol::wl_output::WlOutput,
        ) {
            // The toolkit's OutputState records the new output's geometry
            // (OutputState::info) and the creation-time matching reads it;
            // a newly advertised output never changes an already-anchored
            // surface, so there is nothing further to track here.
        }

        fn update_output(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            output: wayland_client::protocol::wl_output::WlOutput,
        ) {
            // Geometry/scale changes on the anchored output are read live
            // from OutputState::info by the status mapping lines; if that
            // info has vanished (output destroyed mid-update), record the
            // loss so the status stops describing a dead anchor.
            if !self.anchored_output_gone && self.is_anchored(&output) {
                self.anchored_output_gone = self.anchored_snapshot().is_none();
            }
        }

        fn output_destroyed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            output: wayland_client::protocol::wl_output::WlOutput,
        ) {
            if self.is_anchored(&output) {
                self.anchored_output_gone = true;
                self.redraw_required = true;
            }
        }
    }

    impl ShmHandler for OverlayState {
        fn shm_state(&mut self) -> &mut Shm {
            &mut self.shm
        }
    }

    impl SeatHandler for OverlayState {
        fn seat_state(&mut self) -> &mut SeatState {
            &mut self.seat_state
        }

        fn new_seat(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _seat: wl_seat::WlSeat,
        ) {
        }

        fn new_capability(
            &mut self,
            _conn: &Connection,
            qh: &QueueHandle<Self>,
            seat: wl_seat::WlSeat,
            capability: Capability,
        ) {
            if capability == Capability::Pointer {
                let _ = self.seat_state.get_pointer(qh, &seat);
            }
        }

        fn remove_capability(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _seat: wl_seat::WlSeat,
            capability: Capability,
        ) {
            if capability == Capability::Pointer {
                self.pointer_window_relative = None;
            }
        }

        fn remove_seat(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _seat: wl_seat::WlSeat,
        ) {
        }
    }

    impl PointerHandler for OverlayState {
        fn pointer_frame(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _pointer: &wl_pointer::WlPointer,
            events: &[PointerEvent],
        ) {
            for event in events {
                let Some(layer) = self.layer.get() else {
                    continue;
                };
                if &event.surface != layer.wl_surface() {
                    continue;
                }
                match event.kind {
                    PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                        self.pointer_window_relative = Some(Point {
                            x: event.position.0 as i32,
                            y: event.position.1 as i32,
                        });
                    }
                    PointerEventKind::Leave { .. } => {
                        self.pointer_window_relative = None;
                    }
                    _ => {}
                }
            }
        }
    }

    impl wayland_client::Dispatch<wl_region::WlRegion, (), Self> for OverlayState {
        fn event(
            _state: &mut Self,
            _region: &wl_region::WlRegion,
            _event: wl_region::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
        }
    }

    delegate_registry!(OverlayState);

    impl ProvidesRegistryState for OverlayState {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry_state
        }
        registry_handlers![OutputState, SeatState];
    }

    delegate_dispatch2!(OverlayState);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::wayland_backend::{
        clamp_rect, no_match_limitation, output_mapping_lines, output_selection_line,
        select_output, select_output_index, union_in, CanvasRenderer, OutputSelectionBasis,
        OutputSnapshot,
    };
    use super::{premultiply_argb, OverlayRenderKind, OverlayRenderOp, Rect};

    fn output(name: Option<&str>, scale: i32, width: u32, height: u32) -> OutputSnapshot {
        OutputSnapshot {
            name: name.map(str::to_string),
            scale_factor: scale,
            pixel_size: (width, height),
            logical_rect: None,
        }
    }

    fn placed(name: &str, x: i32, width: u32, height: u32) -> OutputSnapshot {
        OutputSnapshot {
            logical_rect: Some(Rect {
                x,
                y: 0,
                width,
                height,
            }),
            ..output(Some(name), 1, width, height)
        }
    }

    fn stream(width: u32, height: u32) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn headless_config() -> super::OverlayConfig {
        super::OverlayConfig {
            window_geometry: Rect {
                x: 0,
                y: 0,
                width: 960,
                height: 540,
            },
            show_overlay_icon: false,
            overlay_icon_pos_x: 0,
            overlay_icon_pos_y: 0,
            menu_locked: false,
        }
    }

    fn empty_frame(sequence: u64) -> super::OverlayFrame {
        super::OverlayFrame {
            sequence,
            tooltip_found: false,
            hotpath_ms: 0,
            cursor_window_relative: None,
            rule_descriptors: std::sync::Arc::from([]),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        }
    }

    #[test]
    fn paragon_input_regions_accept_panels_but_leave_the_board_click_through() {
        use d4lc_rules::{
            ParagonOverlayElementKind as Kind, RuleOverlayShape as Shape, RuleOverlayTarget,
        };
        let descriptor = |kind, shape, y| d4lc_rules::RuleOverlayDescriptor {
            target: RuleOverlayTarget::Paragon {
                kind,
                route_index: Some(0),
                board_index: Some(0),
                board_name: None,
                node_index: None,
                panel_hit_width: None,
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Paragon,
            shape,
            geometry: Rect {
                x: 0,
                y,
                width: 200,
                height: 50,
            },
            color: "White".into(),
            text: None,
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            geometry_source: "test".into(),
            source: d4lc_rules::RuleSourceNote::new("test", "test", "test"),
        };
        let step = descriptor(Kind::StepPanel, Shape::TextPanel, 152);
        let board = descriptor(Kind::BoardEntry, Shape::TextPanel, 204);
        assert_eq!(
            super::paragon_input_regions(&[
                step.clone(),
                board.clone(),
                descriptor(Kind::BoardEntry, Shape::Rectangle, 204),
                descriptor(Kind::Node, Shape::Rectangle, 500),
                descriptor(Kind::BuildPanel, Shape::TextPanel, 100),
            ]),
            vec![step.geometry, board.geometry]
        );
        assert!(super::paragon_input_regions(&[]).is_empty());
    }

    /// Headless backend: the first update draws, an identical
    /// frame skips, and a changed frame draws again. The skip is observable
    /// through `last_update_skipped` and never masquerades as a draw.
    #[test]
    fn headless_backend_draws_once_and_skips_identical_frames() {
        let mut handle =
            super::OverlayHandle::create_headless(headless_config(), "headless test surface");
        handle.update(empty_frame(1)).expect("first update");
        assert!(!handle.last_update_skipped(), "the first update draws");

        handle.update(empty_frame(2)).expect("identical update");
        assert!(
            handle.last_update_skipped(),
            "an identical frame must skip the draw"
        );

        let affix_descriptor = |y: i32| d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Affix {
                affix_id: "Affix_Test".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
            shape: d4lc_rules::RuleOverlayShape::Rectangle,
            geometry: Rect {
                x: 10,
                y,
                width: 20,
                height: 20,
            },
            color: "red".into(),
            text: None,
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "headless skip test".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "test",
                "headless skip test",
                "synthetic marker",
            ),
        };
        handle
            .update(super::OverlayFrame {
                rule_descriptors: vec![affix_descriptor(10)].into(),
                text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
                ..empty_frame(3)
            })
            .expect("changed update");
        assert!(!handle.last_update_skipped(), "a changed frame must redraw");

        handle
            .update(super::OverlayFrame {
                rule_descriptors: std::sync::Arc::from([]),
                text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
                ..empty_frame(4)
            })
            .expect("empty update");
        assert!(
            !handle.last_update_skipped(),
            "an empty frame after content must redraw (the hide stays immediate)"
        );

        handle.update(empty_frame(5)).expect("second empty update");
        assert!(
            handle.last_update_skipped(),
            "an identical empty frame skips"
        );
    }

    /// A protocol error that only surfaces on a later update's
    /// dispatch (a flushed commit's error) flips the backend to `Failed`
    /// with exactly the fields a failed draw sets, so a broken
    /// overlay is never reported as healthy.
    #[test]
    fn dispatch_failure_flips_the_backend_to_failed_with_failure_fields() {
        let mut handle =
            super::OverlayHandle::create_headless(headless_config(), "headless test surface");
        assert_eq!(handle.status().draw_error_count, 0);

        handle.fail_drawing("wayland dispatch failed: protocol error");

        assert_eq!(handle.status().backend, super::OverlayBackend::Failed);
        assert_eq!(handle.status().draw_error_count, 1);
        assert_eq!(
            handle.status().last_error.as_deref(),
            Some("wayland dispatch failed: protocol error")
        );
        let limitation = handle
            .status()
            .limitation
            .clone()
            .unwrap_or_else(|| panic!("a failed backend must carry a limitation"));
        assert!(limitation.contains("overlay draw failed"));
        assert!(
            limitation.contains("disabled overlay drawing"),
            "the limitation must name that drawing stopped: {limitation}"
        );
        assert!(!handle.status().click_through);
        assert!(!handle.status().transparent_confirmed);
        assert!(!handle.status().always_on_top_confirmed);

        // Further updates cannot un-fail the backend: they draw nothing
        // and report no new error.
        handle.update(empty_frame(9)).expect("update after failure");
        assert_eq!(handle.status().draw_error_count, 1);
    }

    /// A small op on a large canvas reports a damage rect equal to
    /// the op bounds, and rendering a second frame into a reused canvas with
    /// a partial clear produces exactly the same pixels as a full clear
    /// (today's path), so partial damage never changes what is displayed.
    #[test]
    fn render_tracked_clears_and_damages_only_the_touched_area() {
        let (width, height) = (200_u32, 100_u32);
        let first_ops = vec![op(
            OverlayRenderKind::FilledRect,
            Rect {
                x: 50,
                y: 20,
                width: 30,
                height: 10,
            },
            None,
        )];
        let second_ops = vec![op(
            OverlayRenderKind::FilledRect,
            Rect {
                x: 120,
                y: 60,
                width: 10,
                height: 10,
            },
            None,
        )];

        // Frame 1: full clear (a fresh buffer) — ink equals the op bounds.
        let mut reused_canvas = vec![0_u8; (width * height * 4) as usize];
        let ink1 = CanvasRenderer::new().render_tracked(
            &mut reused_canvas,
            width,
            height,
            &first_ops,
            None,
        );
        assert_eq!(
            ink1,
            Rect {
                x: 50,
                y: 20,
                width: 30,
                height: 10,
            }
        );

        // Frame 2 into the same buffer: clear only frame 1's ink.
        let ink2 = CanvasRenderer::new().render_tracked(
            &mut reused_canvas,
            width,
            height,
            &second_ops,
            Some(ink1),
        );
        assert_eq!(
            ink2,
            Rect {
                x: 120,
                y: 60,
                width: 10,
                height: 10,
            }
        );

        // The partial clear reproduces the full-clear pixels exactly.
        let fresh = render(&second_ops, width, height);
        assert_eq!(reused_canvas, fresh, "partial clear must be pixel-exact");

        // The damage is the union of both ink areas.
        let mut damage = Some(ink1);
        union_in(&mut damage, ink2);
        assert_eq!(
            clamp_rect(damage, width, height),
            Rect {
                x: 50,
                y: 20,
                width: 80,
                height: 50,
            }
        );
    }

    /// A text op's damage covers the laid-out glyph mask, not the
    /// whole text area; an empty frame damages nothing.
    #[test]
    fn render_tracked_tracks_text_ink_and_empty_frames() {
        let (width, height) = (400_u32, 300_u32);
        let text_ops = vec![op(
            OverlayRenderKind::Text,
            Rect {
                x: 20,
                y: 20,
                width: 100,
                height: 30,
            },
            Some("Sigil"),
        )];
        let mut canvas = vec![0_u8; (width * height * 4) as usize];
        let ink = CanvasRenderer::new().render_tracked(&mut canvas, width, height, &text_ops, None);
        assert!(ink.width > 0 && ink.height > 0, "text ink must be measured");
        assert!(
            ink.x >= 20
                && ink.y >= 20
                && ink.x + ink.width as i32 <= 124
                && ink.y + ink.height as i32 <= 54,
            "text ink must stay within a small margin of the op area, got {ink:?}"
        );

        // An empty op list paints nothing and damages nothing.
        let mut canvas = vec![0_u8; (width * height * 4) as usize];
        let ink = CanvasRenderer::new().render_tracked(&mut canvas, width, height, &[], None);
        assert_eq!(
            ink,
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }
        );
    }

    #[test]
    fn output_matching_selects_by_stream_size_with_deterministic_tie_break() {
        let outputs = [
            output(Some("DP-1"), 1, 2560, 1440),
            output(Some("HDMI-A-1"), 1, 1920, 1080),
            output(Some("DP-2"), 1, 1920, 1080),
        ];

        // Exact size match on the stream's pixel size.
        assert_eq!(
            select_output_index(&outputs, &stream(2560, 1440), None),
            Some(0),
            "a single exact match wins"
        );

        // A size collision resolves deterministically: lexicographically
        // first advertised name (DP-2 before HDMI-A-1).
        assert_eq!(
            select_output_index(&outputs, &stream(1920, 1080), None),
            Some(2),
            "size collision must resolve to the first name, not advertise order"
        );

        // Unnamed outputs lose the tie-break against named ones.
        let mixed = [
            output(None, 1, 1920, 1080),
            output(Some("eDP-1"), 1, 1920, 1080),
        ];
        assert_eq!(
            select_output_index(&mixed, &stream(1920, 1080), None),
            Some(1),
            "named outputs win the tie-break over unnamed ones"
        );

        // No size match is a None — the caller fails the create.
        assert_eq!(select_output_index(&outputs, &stream(960, 540), None), None);
        assert_eq!(select_output_index(&[], &stream(960, 540), None), None);
    }

    #[test]
    fn output_mapping_lines_describe_the_identity_chain_and_its_deviations() {
        let anchored = output(Some("DP-1"), 1, 2560, 1440);
        let lines = output_mapping_lines(
            Some(&anchored),
            false,
            Some((2560, 1440)),
            &stream(2560, 1440),
        );

        // The identity chain is stated explicitly...
        let identity = lines.join("; ");
        assert!(
            identity.contains("output-anchored placement")
                && identity.contains("DP-1")
                && identity.contains("2560x1440")
                && identity.contains("identity transform"),
            "the mapping line must name the output and the identity chain: {identity}"
        );
        // ...and no deviation applies in the matched scale-1 case.
        assert!(
            !identity.contains("not identity")
                && !identity.contains("configured the surface at")
                && !identity.contains("origin ("),
            "a matched scale-1 stream must not carry deviation lines: {identity}"
        );

        // Scale != 1 gets its own line.
        let scaled = output(Some("DP-1"), 2, 2560, 1440);
        let lines = output_mapping_lines(
            Some(&scaled),
            false,
            Some((2560, 1440)),
            &stream(2560, 1440),
        );
        assert!(
            lines.iter().any(|line| line.contains("output scale 2")
                && line.contains("not identity")
                && line.contains("placement is not guaranteed")),
            "scale != 1 must surface the mapping deviation: {lines:?}"
        );

        // A configure size differing from the stream size gets its own
        // line.
        let lines = output_mapping_lines(
            Some(&anchored),
            false,
            Some((1920, 1080)),
            &stream(2560, 1440),
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("configured the surface at 1920x1080")
                    && line.contains("capture stream is 2560x1440")
                    && line.contains("placement is shifted")),
            "a configure/stream size mismatch must surface: {lines:?}"
        );
        // The same size stays silent.
        let lines = output_mapping_lines(
            Some(&anchored),
            false,
            Some((2560, 1440)),
            &stream(2560, 1440),
        );
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("configured the surface at")),
            "a matching configure must not produce a mismatch line: {lines:?}"
        );

        // A non-zero stream origin is explicitly ignored, never silent.
        let offset_stream = Rect {
            x: 40,
            y: 60,
            ..stream(2560, 1440)
        };
        let lines = output_mapping_lines(Some(&anchored), false, None, &offset_stream);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("origin (40, 60) is ignored")),
            "a non-zero stream origin must be named as ignored: {lines:?}"
        );

        // A vanished anchored output is about the lost mapping.
        let lines = output_mapping_lines(None, true, Some((2560, 1440)), &stream(2560, 1440));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("output carrying the capture stream is gone")),
            "a destroyed anchored output must surface its loss: {lines:?}"
        );
    }

    #[test]
    fn output_selection_announces_a_tie_break_guess() {
        let outputs = [
            placed("HDMI-A-2", 0, 1920, 1080),
            placed("DP-2", 1920, 1920, 1080),
            placed("DP-1", 3840, 2560, 1440),
        ];
        let unique = select_output(&outputs, &stream(2560, 1440), None).unwrap();
        assert_eq!(unique.basis, OutputSelectionBasis::UniqueSize);
        assert_eq!(output_selection_line(&unique.basis, None), None);

        let anchored = select_output(&outputs, &stream(1920, 1080), Some((2000, 10))).unwrap();
        assert_eq!(anchored.index, 1);
        assert_eq!(anchored.basis, OutputSelectionBasis::Anchor);
        assert_eq!(
            output_selection_line(&anchored.basis, Some((2000, 10))),
            None
        );

        for anchor in [None, Some((-5000, 0))] {
            let guessed = select_output(&outputs, &stream(1920, 1080), anchor).unwrap();
            assert_eq!(guessed.index, 1, "tie-break stays deterministic");
            assert_eq!(
                guessed.basis,
                OutputSelectionBasis::TieBreak {
                    candidates: vec!["DP-2".to_string(), "HDMI-A-2".to_string()]
                }
            );
            let line = output_selection_line(&guessed.basis, anchor)
                .expect("a tie-break must be announced");
            assert!(
                line.contains("guess") && line.contains("DP-2, HDMI-A-2"),
                "{line}"
            );
        }
    }

    #[test]
    fn output_matching_prefers_the_output_containing_the_anchor() {
        // Two identical monitors: HDMI-A-2 at 0,0 and DP-2 at 1920,0.
        let outputs = [
            placed("HDMI-A-2", 0, 1920, 1080),
            placed("DP-2", 1920, 1920, 1080),
        ];
        assert_eq!(
            select_output_index(&outputs, &stream(1920, 1080), Some((0, 0))),
            Some(0),
            "the monitor stream position selects its own output"
        );
        assert_eq!(
            select_output_index(&outputs, &stream(1920, 1080), Some((2880, 540))),
            Some(1),
            "the game window center selects the output it lies on"
        );
        assert_eq!(
            select_output_index(&outputs, &stream(1920, 1080), Some((-5000, 0))),
            Some(1),
            "an anchor on no candidate falls back to the name tie-break"
        );
        assert_eq!(
            select_output_index(&outputs, &stream(2560, 1440), Some((0, 0))),
            None,
            "the anchor never overrides the size match"
        );
    }

    #[test]
    fn no_match_limitation_names_the_stream_and_every_observed_output() {
        let outputs = [
            output(Some("DP-1"), 1, 2560, 1440),
            output(None, 1, 1920, 1080),
        ];
        let limitation = no_match_limitation(&outputs, &stream(960, 540));
        assert!(
            limitation.contains("no advertised output matches the capture stream size 960x540")
                && limitation.contains("mispositioned on a guessed output")
                && limitation.contains("DP-1 2560x1440")
                && limitation.contains("<unnamed> 1920x1080"),
            "the no-match limitation must explain the mismatch: {limitation}"
        );

        let empty = no_match_limitation(&[], &stream(960, 540));
        assert!(
            empty.contains("advertises no outputs"),
            "the empty-output case must be named: {empty}"
        );
    }

    /// Renders ops through the same shared renderer the backend uses.
    fn render(render_ops: &[OverlayRenderOp], width: u32, height: u32) -> Vec<u8> {
        let mut canvas = vec![0u8; (width * height * 4) as usize];
        CanvasRenderer::new().render_tracked(&mut canvas, width, height, render_ops, None);
        canvas
    }

    fn op(kind: OverlayRenderKind, geometry: Rect, text: Option<&str>) -> OverlayRenderOp {
        OverlayRenderOp {
            kind,
            marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry,
            color: "Gold".into(),
            text: text.map(str::to_string),
            font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
            text_placement: d4lc_overlay_x11::OverlayTextPlacement::Area,
            geometry_source: None,
            source: None,
        }
    }

    fn pixel_at(canvas: &[u8], width: u32, x: i32, y: i32) -> [u8; 4] {
        let idx = ((y as usize) * (width as usize) + x as usize) * 4;
        [
            canvas[idx],
            canvas[idx + 1],
            canvas[idx + 2],
            canvas[idx + 3],
        ]
    }

    fn nonblank_in(canvas: &[u8], width: u32, rect: Rect) -> usize {
        let mut count = 0;
        for y in rect.y.max(0)..(rect.y + rect.height as i32) {
            for x in rect.x.max(0)..(rect.x + rect.width as i32) {
                let px = pixel_at(canvas, width, x, y);
                if px.iter().any(|b| *b != 0) {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn premultiply_converts_straight_alpha_palette_pixels() {
        // Fully opaque colors are unchanged.
        assert_eq!(premultiply_argb(0xFF00FF80), 0xFF00FF80);
        // The background color 0xCC1A1A1A premultiplies its channels.
        assert_eq!(premultiply_argb(0xCC1A1A1A), 0xCC141414);
        // Fully transparent colors stay zero.
        assert_eq!(premultiply_argb(0x00FFFFFF), 0);
    }

    #[test]
    fn panel_text_wraps_inside_its_area_and_paints_glyphs() {
        // The shared bounded text path: a panel text op lays out inside its
        // own area, wraps there, and paints glyph coverage — never a filled
        // rectangle, and never ink in the padding that bounds the area.
        let width = 128;
        let height = 64;
        let panel = Rect {
            x: 4,
            y: 4,
            width: 100,
            height: 50,
        };
        let canvas = render(
            &[op(
                OverlayRenderKind::Text,
                panel,
                Some("Trade match for a very long item name that wraps"),
            )],
            width,
            height,
        );

        let glyph_pixels = nonblank_in(&canvas, width, panel);
        assert!(
            glyph_pixels > 0,
            "a text op must produce rasterized glyph pixels"
        );
        let rect_area = (panel.width * panel.height) as usize;
        assert!(
            glyph_pixels < rect_area,
            "glyph pixels must be sparse, not a filled rectangle: {glyph_pixels}/{rect_area}"
        );
        // The wrap keeps the ink inside the text area: the leftmost and
        // topmost padding columns/rows of the panel stay transparent.
        let left_pad = Rect {
            x: panel.x,
            y: panel.y,
            width: 1,
            height: panel.height,
        };
        let below_last_line = Rect {
            x: panel.x,
            y: panel.y + 30,
            width: panel.width,
            height: panel.height - 30,
        };
        assert_eq!(nonblank_in(&canvas, width, left_pad), 0);
        assert_eq!(nonblank_in(&canvas, width, below_last_line), 0);
    }

    #[test]
    fn marker_rectangles_are_outline_filled_with_their_stroke_width() {
        // Both backends draw the identical marker form from the shared op
        // kind: the marker color fills the rectangle and a black border of
        // the marker's stroke width sits inside its edge (upstream
        // `OutlineFillRectangle`, 2 for a sigil dungeon marker, 1 for an
        // any-type or below-minimal affix). A paragon tile keeps the
        // outline-only form with a transparent interior.
        let width = 64;
        let height = 64;
        let canvas = render(
            &[
                op(
                    OverlayRenderKind::OutlineFillRect { stroke: 1 },
                    Rect {
                        x: 4,
                        y: 4,
                        width: 20,
                        height: 20,
                    },
                    None,
                ),
                op(
                    OverlayRenderKind::OutlineFillRect { stroke: 2 },
                    Rect {
                        x: 40,
                        y: 4,
                        width: 20,
                        height: 20,
                    },
                    None,
                ),
                op(
                    OverlayRenderKind::OutlineRect,
                    Rect {
                        x: 4,
                        y: 30,
                        width: 20,
                        height: 20,
                    },
                    None,
                ),
            ],
            width,
            height,
        );

        // Gold 0xFFFFD700 premultiplied, little-endian b,g,r,a.
        const GOLD: [u8; 4] = [0x00, 0xD7, 0xFF, 0xFF];
        const BLACK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];
        // Stroke 1: one black ring at the edge, the rest is the fill color.
        assert_eq!(pixel_at(&canvas, width, 4, 4), BLACK);
        assert_eq!(pixel_at(&canvas, width, 5, 5), GOLD);
        assert_eq!(pixel_at(&canvas, width, 14, 14), GOLD);
        // Stroke 2: two black rings, fill from the second ring inward.
        assert_eq!(pixel_at(&canvas, width, 40, 4), BLACK);
        assert_eq!(pixel_at(&canvas, width, 41, 5), BLACK);
        assert_eq!(pixel_at(&canvas, width, 42, 6), GOLD);
        // Outline-only: border painted, interior transparent.
        assert_eq!(pixel_at(&canvas, width, 4, 30), GOLD);
        assert_eq!(pixel_at(&canvas, width, 14, 40), [0, 0, 0, 0]);
    }

    #[test]
    fn paragon_active_node_stroke_has_configured_thickness_and_transparent_interior() {
        let width = 64;
        let rect = Rect {
            x: 16,
            y: 16,
            width: 24,
            height: 24,
        };
        let mut operation = op(OverlayRenderKind::StrokedRect { stroke: 4 }, rect, None);
        operation.color = "#14DC50".into();
        let thick = render(&[operation.clone()], width, width);
        assert_eq!(pixel_at(&thick, width, 17, 28), [80, 220, 20, 255]);
        assert_eq!(pixel_at(&thick, width, 28, 28), [0, 0, 0, 0]);
        operation.kind = OverlayRenderKind::StrokedRect { stroke: 1 };
        let thin = render(&[operation], width, width);
        assert_eq!(pixel_at(&thin, width, 16, 28), [80, 220, 20, 255]);
        assert_eq!(pixel_at(&thin, width, 17, 28), [0, 0, 0, 0]);
        assert!(
            thick.iter().filter(|v| **v != 0).count() > thin.iter().filter(|v| **v != 0).count()
        );
    }

    #[test]
    fn filled_and_outline_rects_paint_their_own_pixels() {
        let width = 64;
        let height = 64;
        let mut canvas = vec![0u8; (width * height * 4) as usize];

        CanvasRenderer::new().render_tracked(
            &mut canvas,
            width,
            height,
            &[
                op(
                    OverlayRenderKind::FilledRect,
                    Rect {
                        x: 4,
                        y: 4,
                        width: 8,
                        height: 8,
                    },
                    None,
                ),
                op(
                    OverlayRenderKind::OutlineRect,
                    Rect {
                        x: 20,
                        y: 20,
                        width: 10,
                        height: 10,
                    },
                    None,
                ),
            ],
            None,
        );

        // Gold 0xFFFFD700 premultiplied, little-endian b,g,r,a.
        assert_eq!(pixel_at(&canvas, width, 6, 6), [0x00, 0xD7, 0xFF, 0xFF]);
        // Outline rect: border painted, interior untouched.
        assert_eq!(pixel_at(&canvas, width, 20, 20), [0x00, 0xD7, 0xFF, 0xFF]);
        assert_eq!(pixel_at(&canvas, width, 24, 24), [0, 0, 0, 0]);
        // Everything outside the ops stays transparent.
        assert_eq!(
            nonblank_in(
                &canvas,
                width,
                Rect {
                    x: 40,
                    y: 40,
                    width: 20,
                    height: 20
                }
            ),
            0
        );
    }

    #[test]
    fn triangle_ops_paint_fill_with_black_outline() {
        // Named decision point: greater-affix markers must rasterize as
        // filled triangles with a black outline (upstream FillTriangle +
        // DrawTriangle stroke 2), not as rect or circle approximations.
        let width = 64;
        let height = 64;
        let mut canvas = vec![0u8; (width * height * 4) as usize];

        CanvasRenderer::new().render_tracked(
            &mut canvas,
            width,
            height,
            &[op(
                OverlayRenderKind::Triangle,
                Rect {
                    x: 8,
                    y: 8,
                    width: 20,
                    height: 20,
                },
                None,
            )],
            None,
        );

        // Interior fill is Gold (the op color), little-endian b,g,r,a.
        assert_eq!(pixel_at(&canvas, width, 18, 25), [0x00, 0xD7, 0xFF, 0xFF]);
        // The outline strokes the edges in black: the apex column carries
        // black stroke pixels (exact stroke phase is the rasterizer's
        // choice, so scan instead of pinning one pixel).
        assert!(
            (8..=28).any(|y| pixel_at(&canvas, width, 18, y) == [0, 0, 0, 255]),
            "no black outline pixel along the apex column x=18"
        );
        // The bounding-box corner above the hypotenuse stays transparent.
        assert_eq!(pixel_at(&canvas, width, 9, 9), [0, 0, 0, 0]);
    }
}
