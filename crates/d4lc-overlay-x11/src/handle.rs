#[cfg(feature = "real-x11")]
use crate::x11_backend;
use crate::{
    menu_interaction_zone, text, OverlayBackend, OverlayConfig, OverlayDrawTimings, OverlayFrame,
    OverlayMenuState, OverlayRenderOp, OverlayStatus, RenderPlan,
};
use d4lc_core::Rect;
use std::fmt;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Internal backend enum (private to this module).
// ---------------------------------------------------------------------------

enum Backend {
    Headless,
    #[cfg(feature = "real-x11")]
    RealX11(Box<x11_backend::RealX11Overlay>),
    Failed,
}

// ---------------------------------------------------------------------------
// Public OverlayHandle — stable API across all backends.
// ---------------------------------------------------------------------------

pub struct OverlayHandle {
    backend: Backend,
    menu: OverlayMenuState,
    status: OverlayStatus,
    last_frame: Option<OverlayFrame>,
    plan: RenderPlan,
    /// Whether at least one update executed a real draw: the
    /// skip path only engages after the window has real pixels.
    ever_drawn: bool,
    /// Set when pixels were invalidated outside a draw (window
    /// reposition/resize); the next update consumes it and forces a redraw.
    forced_redraw: bool,
    /// Whether the last update skipped the draw because nothing observable
    /// changed.
    last_update_skipped: bool,
    /// What the shared bounded text path could not draw on the last frame
    /// (unsupported codepoints, characters that did not fit their area) or
    /// why it is disabled (font unavailable, visual depth). Never silently
    /// empty when something was skipped: readable through
    /// [`OverlayHandle::text_diagnostics`].
    text_diagnostics: text::TextDiagnostics,
}

impl OverlayHandle {
    fn create_headless(config: OverlayConfig, limitation: &str) -> Self {
        let menu = OverlayMenuState::from_config(&config);
        Self {
            backend: Backend::Headless,
            menu,
            status: OverlayStatus {
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
            },
            last_frame: None,
            plan: RenderPlan::default(),
            ever_drawn: false,
            forced_redraw: false,
            last_update_skipped: false,
            text_diagnostics: text::TextDiagnostics::default(),
        }
    }

    #[cfg(feature = "real-x11")]
    fn create_failed(config: OverlayConfig, limitation: &str) -> Self {
        let menu = OverlayMenuState::from_config(&config);
        Self {
            backend: Backend::Failed,
            menu,
            status: OverlayStatus {
                backend: OverlayBackend::Failed,
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
            },
            last_frame: None,
            plan: RenderPlan::default(),
            ever_drawn: false,
            forced_redraw: false,
            last_update_skipped: false,
            text_diagnostics: text::TextDiagnostics::default(),
        }
    }

    pub fn update(&mut self, frame: OverlayFrame) -> Result<(), String> {
        self.update_at(frame, Instant::now())
    }

    pub(crate) fn update_at(&mut self, frame: OverlayFrame, now: Instant) -> Result<(), String> {
        if !self.status.enabled {
            return Err("overlay is disabled".to_string());
        }

        self.menu.observe_cursor(frame.cursor_window_relative, now);
        self.status.menu_locked = self.menu.is_locked;
        self.status.menu_visible = self.menu.is_visible;

        let ops_changed =
            self.plan
                .refresh(&frame.rule_descriptors, &self.menu, frame.text_font_px, now);

        // An unchanged frame on an unchanged window skips the
        // draw entirely. The first draw, a window geometry change, a pending
        // Expose (the server lost pixels) and any changed render-op list
        // always draw — menu hover and fade animation feed `now` into the
        // ops, so animated frames still differ and still draw. A skip is
        // visible through `last_update_skipped` and never looks like a draw.
        #[cfg(feature = "real-x11")]
        let expose_seen = match &self.backend {
            Backend::RealX11(overlay) => overlay.drain_events(),
            _ => false,
        };
        #[cfg(not(feature = "real-x11"))]
        let expose_seen = false;

        let skip_draw = self.ever_drawn && !self.forced_redraw && !expose_seen && !ops_changed;
        self.forced_redraw = false;
        self.last_update_skipped = skip_draw;
        if skip_draw {
            self.last_frame = Some(frame);
            return Ok(());
        }
        self.ever_drawn = true;

        #[cfg(feature = "real-x11")]
        let draw_error = match &mut self.backend {
            Backend::RealX11(overlay) => overlay.draw(&frame, self.plan.ops()).err(),
            _ => None,
        };

        #[cfg(feature = "real-x11")]
        if let Backend::RealX11(overlay) = &self.backend {
            self.text_diagnostics = overlay.text_diagnostics().clone();
        }

        #[cfg(not(feature = "real-x11"))]
        let draw_error: Option<String> = None;

        if let Some(error) = draw_error {
            self.status.backend = OverlayBackend::Failed;
            self.status.transparent = false;
            self.status.transparent_confirmed = false;
            self.status.always_on_top = false;
            self.status.always_on_top_confirmed = false;
            self.status.draw_error_count = self.status.draw_error_count.saturating_add(1);
            self.status.last_error = Some(error.clone());
            self.status.limitation = Some(format!(
                "overlay draw failed: {error}; disabled overlay drawing for remaining frames"
            ));
            self.backend = Backend::Failed;
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
            #[cfg(feature = "real-x11")]
            Backend::RealX11(overlay) => overlay.last_draw_timings,
            Backend::Headless | Backend::Failed => OverlayDrawTimings::default(),
        }
    }

    /// Follow the game window to a new on-screen geometry (moved, resized,
    /// other monitor). The overlay keeps drawing in window-relative
    /// coordinates, so descriptors need no second transformation; only the
    /// surface and the menu interaction zone move.
    pub fn set_window_geometry(&mut self, geometry: Rect) -> Result<(), String> {
        if geometry.width == 0 || geometry.height == 0 {
            return Err("overlay window geometry must be non-zero".to_string());
        }
        if self.status.window_geometry == geometry {
            return Ok(());
        }
        self.menu.set_window_geometry(geometry);
        let interaction_zone = self.menu.interaction_zone();

        #[cfg(feature = "real-x11")]
        if let Backend::RealX11(overlay) = &mut self.backend {
            overlay.reposition(geometry, interaction_zone)?;
            self.status.click_through = overlay.click_through_confirmed;
        }

        self.status.window_geometry = geometry;
        self.status.interaction_zone = interaction_zone;
        // The window moved or was resized: its previous pixels are gone.
        self.forced_redraw = true;
        Ok(())
    }

    /// Apply the overlay menu settings to the running surface: icon visibility + position and optionally the stored
    /// lock value. A changed setting forces the next update to redraw so
    /// the menu never keeps stale pixels.
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
        self.status.menu_locked = self.menu.is_locked;
        self.status.menu_visible = self.menu.is_visible;
        self.forced_redraw = true;
    }

    pub fn last_frame(&self) -> Option<&OverlayFrame> {
        self.last_frame.as_ref()
    }

    pub fn last_render_ops(&self) -> &[OverlayRenderOp] {
        self.plan.ops()
    }

    /// Whether the last update skipped the draw because the render ops, the
    /// window geometry and every pixel-invalidating event were unchanged
    /// The X server still shows the previous, identical frame.
    pub const fn last_update_skipped(&self) -> bool {
        self.last_update_skipped
    }

    /// What the shared bounded text path could not draw, or why it is
    /// disabled. Empty and clean while every laid-out text was drawn.
    pub fn text_diagnostics(&self) -> &text::TextDiagnostics {
        &self.text_diagnostics
    }

    #[cfg(all(test, feature = "real-x11"))]
    pub(crate) fn capture_real_x11_window_for_test(
        &self,
    ) -> Result<(u32, u32, usize, Vec<u8>), String> {
        match &self.backend {
            Backend::RealX11(overlay) => overlay.capture_window_pixels_for_test(),
            Backend::Headless => {
                Err("synthetic smoke requires a real X11 overlay backend".to_string())
            }
            Backend::Failed => {
                Err(self.status.limitation.clone().unwrap_or_else(|| {
                    "real X11 overlay backend failed before capture".to_string()
                }))
            }
        }
    }
}

// Prevent accidental Debug derives from leaking the x11rb connection details.
impl fmt::Debug for OverlayHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayHandle")
            .field("backend", &self.status.backend)
            .field("enabled", &self.status.enabled)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Create an overlay for the given window geometry.
///
/// When the `real-x11` feature is enabled and `DISPLAY` is set, this attempts
/// to open a real X11 diagnostic overlay window. On failure it falls back to a
/// `Failed` backend so the live loop continues headlessly with failure metadata.
/// When `DISPLAY` is absent or the feature is disabled, it falls back to a
/// `HeadlessNoop` backend.
pub fn create_overlay(config: OverlayConfig) -> Result<OverlayHandle, String> {
    if config.window_geometry.width == 0 || config.window_geometry.height == 0 {
        return Err("overlay window geometry must be non-zero".to_string());
    }

    #[cfg(feature = "real-x11")]
    {
        if std::env::var("DISPLAY").is_ok() {
            match x11_backend::RealX11Overlay::create(&config) {
                Ok(backend) => {
                    let always_on_top_requested = backend.always_on_top_requested;
                    let always_on_top_confirmed = backend.always_on_top_confirmed;
                    let transparent_requested = backend.transparent_requested;
                    let transparent_confirmed = backend.transparent_confirmed;
                    let click_through_confirmed = backend.click_through_confirmed;
                    let text_limitation = backend.text_limitation();
                    let text_diagnostics = backend.text_diagnostics().clone();
                    let mut limitations: Vec<String> = Vec::new();
                    if always_on_top_requested {
                        limitations.push(
                            "always_on_top requested via EWMH; whether it holds depends on the window manager"
                                .to_string(),
                        );
                    } else {
                        limitations.push("always_on_top EWMH request failed".to_string());
                    }
                    if transparent_requested {
                        limitations.push(
                            "ARGB visual requested; compositor-visible transparency is not confirmed"
                                .to_string(),
                        );
                    } else {
                        limitations.push("ARGB visual not available".to_string());
                    }
                    if !click_through_confirmed {
                        limitations.push(
                            "click_through input shape is unsupported or unavailable on this X11 server"
                                .to_string(),
                        );
                    }
                    limitations.push(text_limitation);
                    let menu = OverlayMenuState::from_config(&config);
                    return Ok(OverlayHandle {
                        backend: Backend::RealX11(Box::new(backend)),
                        menu,
                        status: OverlayStatus {
                            backend: OverlayBackend::RealX11,
                            enabled: true,
                            window_geometry: config.window_geometry,
                            interaction_zone: menu_interaction_zone(
                                config.window_geometry,
                                config.overlay_icon_pos_x,
                                config.overlay_icon_pos_y,
                            ),
                            menu_locked: config.menu_locked,
                            menu_visible: config.show_overlay_icon && config.menu_locked,
                            transparent: transparent_confirmed,
                            transparent_requested,
                            transparent_confirmed,
                            always_on_top: always_on_top_confirmed,
                            always_on_top_requested,
                            always_on_top_confirmed,
                            click_through: click_through_confirmed,
                            keyboard_interactive_requested: false,
                            keyboard_interactive_confirmed: false,
                            draw_error_count: 0,
                            last_error: None,
                            limitation: if limitations.is_empty() {
                                None
                            } else {
                                Some(limitations.join("; "))
                            },
                        },
                        last_frame: None,
                        plan: RenderPlan::default(),
                        ever_drawn: false,
                        forced_redraw: false,
                        last_update_skipped: false,
                        text_diagnostics,
                    });
                }
                Err(e) => {
                    eprintln!("d4lc-overlay-x11: X11 init failed: {e}; falling back to headless");
                    return Ok(OverlayHandle::create_failed(
                        config,
                        &format!("X11 init failed: {e}; using headless fallback"),
                    ));
                }
            }
        }
    }

    // No DISPLAY or feature not enabled — headless noop.
    Ok(OverlayHandle::create_headless(
        config,
        "real X11 overlay requires DISPLAY; compile with real-x11 feature and set DISPLAY to enable",
    ))
}

#[cfg(test)]
mod tests;
