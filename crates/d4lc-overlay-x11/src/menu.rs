use crate::{OverlayConfig, OverlayRenderKind, OverlayRenderOp, OverlayTextPlacement};
use d4lc_core::{Point, Rect};
use std::time::Instant;

pub(crate) const OVERLAY_MENU_WIDTH: u32 = 50;
pub(crate) const OVERLAY_MENU_HEIGHT: u32 = 50;
const OVERLAY_MENU_PROGRESS_HEIGHT: u32 = 5;
const OVERLAY_MENU_LOCK_TIMER_MS: u128 = 1000;
const OVERLAY_MENU_LABEL: &str = "D4";

/// Menu lock/unlock and interaction-zone state machine shared by the
/// descriptor-driven backends (X11 and Wayland). The state is backend
/// agnostic: it is driven by cursor positions and time only, and the
/// render-op expansion consumes it unchanged.
#[derive(Debug)]
pub struct OverlayMenuState {
    show_overlay_icon: bool,
    overlay_icon_pos_x: u32,
    overlay_icon_pos_y: u32,
    interaction_zone: Rect,
    pub(crate) is_locked: bool,
    pub(crate) is_visible: bool,
    hover_started_at: Option<Instant>,
    hover_consumed: bool,
}

impl OverlayMenuState {
    pub fn from_config(config: &OverlayConfig) -> Self {
        let interaction_zone = menu_interaction_zone(
            config.window_geometry,
            config.overlay_icon_pos_x,
            config.overlay_icon_pos_y,
        );
        let is_visible = config.show_overlay_icon && config.menu_locked;
        Self {
            show_overlay_icon: config.show_overlay_icon,
            overlay_icon_pos_x: config.overlay_icon_pos_x,
            overlay_icon_pos_y: config.overlay_icon_pos_y,
            interaction_zone,
            is_locked: config.menu_locked,
            is_visible,
            hover_started_at: None,
            hover_consumed: false,
        }
    }

    /// Feed one cursor observation (window-relative coordinates or `None`)
    /// into the hover lock/unlock state machine.
    pub fn observe_cursor(&mut self, cursor_window_relative: Option<Point>, now: Instant) {
        let hovered = cursor_window_relative
            .map(|cursor| point_hits_rect(cursor, self.interaction_zone))
            .unwrap_or(false);

        if hovered {
            if self.hover_consumed {
                // Stay in the post-toggle state until the pointer leaves.
            } else if self.hover_started_at.is_none() {
                self.hover_started_at = Some(now);
            } else if self.hover_elapsed_ms(now) >= OVERLAY_MENU_LOCK_TIMER_MS {
                self.is_locked = !self.is_locked;
                self.hover_started_at = None;
                self.hover_consumed = true;
            }
        } else {
            self.hover_started_at = None;
            self.hover_consumed = false;
        }

        self.is_visible = (self.show_overlay_icon && self.is_locked) || hovered;
    }

    /// Recompute the interaction zone for a moved or resized window. The
    /// icon position is a per-mille share of the window size.
    pub fn set_window_geometry(&mut self, window_geometry: Rect) {
        self.interaction_zone = menu_interaction_zone(
            window_geometry,
            self.overlay_icon_pos_x,
            self.overlay_icon_pos_y,
        );
    }

    /// Apply new menu settings to the running state machine: icon visibility and position, and optionally the stored lock
    /// value (`None` keeps the running machine's lock — an observed hover
    /// transition may have flipped it). The hover state machine continues
    /// from the updated state; a re-computed `is_visible` without a hover
    /// observation is corrected by the next cursor feed.
    pub fn apply_settings(
        &mut self,
        window_geometry: Rect,
        show_overlay_icon: bool,
        overlay_icon_pos_x: u32,
        overlay_icon_pos_y: u32,
        menu_locked: Option<bool>,
    ) {
        self.show_overlay_icon = show_overlay_icon;
        self.overlay_icon_pos_x = overlay_icon_pos_x;
        self.overlay_icon_pos_y = overlay_icon_pos_y;
        if let Some(locked) = menu_locked {
            self.is_locked = locked;
            self.hover_started_at = None;
            self.hover_consumed = false;
        }
        self.interaction_zone = menu_interaction_zone(
            window_geometry,
            self.overlay_icon_pos_x,
            self.overlay_icon_pos_y,
        );
        self.is_visible = self.show_overlay_icon && self.is_locked;
    }

    /// Whether the menu is currently locked (rendered persistently).
    pub fn is_locked(&self) -> bool {
        self.is_locked
    }

    pub fn is_icon_enabled(&self) -> bool {
        self.show_overlay_icon
    }

    /// Whether the menu is currently visible (locked or hovered).
    pub fn is_visible(&self) -> bool {
        self.is_visible
    }

    /// The interaction-zone rectangle in window-relative coordinates.
    pub fn interaction_zone(&self) -> Rect {
        self.interaction_zone
    }

    fn hover_elapsed_ms(&self, now: Instant) -> u128 {
        self.hover_started_at
            .map(|start| now.duration_since(start).as_millis())
            .unwrap_or(0)
    }

    fn progress_width(&self, now: Instant) -> u32 {
        if self.hover_consumed {
            return 0;
        }

        let elapsed = self.hover_elapsed_ms(now).min(OVERLAY_MENU_LOCK_TIMER_MS);
        ((elapsed * u128::from(self.interaction_zone.width)) / OVERLAY_MENU_LOCK_TIMER_MS) as u32
    }
}

pub(crate) fn build_menu_render_ops(
    menu: &OverlayMenuState,
    font_px: u32,
    now: Instant,
) -> Vec<OverlayRenderOp> {
    if !menu.is_visible {
        return Vec::new();
    }

    let mut render_ops = vec![
        OverlayRenderOp {
            kind: OverlayRenderKind::FilledRect,
            marker_type: d4lc_rules::RuleOverlayMarkerType::Menu,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            geometry: menu.interaction_zone,
            color: "background".into(),
            text: None,
            font_px,
            text_placement: OverlayTextPlacement::Area,
            geometry_source: None,
            source: None,
        },
        OverlayRenderOp {
            kind: OverlayRenderKind::OutlineRect,
            marker_type: d4lc_rules::RuleOverlayMarkerType::Menu,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            geometry: menu.interaction_zone,
            color: if menu.is_locked {
                "Gold".into()
            } else {
                "White".into()
            },
            text: None,
            font_px,
            text_placement: OverlayTextPlacement::Area,
            geometry_source: None,
            source: None,
        },
        OverlayRenderOp {
            kind: OverlayRenderKind::Text,
            marker_type: d4lc_rules::RuleOverlayMarkerType::Menu,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            geometry: menu.interaction_zone,
            color: if menu.is_locked {
                "Gold".into()
            } else {
                "White".into()
            },
            text: Some(OVERLAY_MENU_LABEL.to_string()),
            font_px,
            text_placement: OverlayTextPlacement::Area,
            geometry_source: None,
            source: None,
        },
    ];

    let progress_width = menu.progress_width(now);
    if progress_width > 0 {
        render_ops.push(OverlayRenderOp {
            kind: OverlayRenderKind::FilledRect,
            marker_type: d4lc_rules::RuleOverlayMarkerType::Menu,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            geometry: Rect {
                x: menu.interaction_zone.x,
                y: menu.interaction_zone.y
                    + menu
                        .interaction_zone
                        .height
                        .saturating_sub(OVERLAY_MENU_PROGRESS_HEIGHT) as i32,
                width: progress_width,
                height: OVERLAY_MENU_PROGRESS_HEIGHT,
            },
            color: "Gold".into(),
            text: None,
            font_px,
            text_placement: OverlayTextPlacement::Area,
            geometry_source: None,
            source: None,
        });
    }

    render_ops
}

/// Compute the menu interaction zone in window-relative coordinates from the
/// percentage-based icon position. Shared by the X11 and Wayland backends.
pub fn menu_interaction_zone(
    window_geometry: Rect,
    overlay_icon_pos_x: u32,
    overlay_icon_pos_y: u32,
) -> Rect {
    Rect {
        x: ((u64::from(window_geometry.width) * u64::from(overlay_icon_pos_x)) / 1000) as i32,
        y: ((u64::from(window_geometry.height) * u64::from(overlay_icon_pos_y)) / 1000) as i32,
        width: OVERLAY_MENU_WIDTH,
        height: OVERLAY_MENU_HEIGHT,
    }
}

fn point_hits_rect(point: Point, rect: Rect) -> bool {
    let right = rect.x.saturating_add(rect.width as i32);
    let bottom = rect.y.saturating_add(rect.height as i32);
    point.x >= rect.x && point.x <= right && point.y >= rect.y && point.y <= bottom
}

#[cfg(test)]
mod tests;
