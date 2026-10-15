//! Coordinate spaces of the capture → detection → OCR → overlay chain.
//!
//! Every rect on that chain lives in exactly one of these spaces, and the
//! space is part of its type so a rect cannot silently be read in another
//! space:
//!
//! | Space        | Origin                                                      |
//! | ------------ | ----------------------------------------------------------- |
//! | [`DesktopRect`] | virtual desktop; negative on monitors left of/above the primary |
//! | [`WindowRect`]  | the captured game window, which is also the overlay surface |
//! | [`FrameRect`]   | the pixel grid of one captured frame                       |
//! | [`TooltipRect`] | the tooltip image an OCR job received (the detected crop)  |
//! | [`SegmentRect`] | one OCR segment image cut out of the tooltip image         |
//!
//! Conversions go through [`FramePlacement`] and [`TooltipPlacement`], which
//! carry the capture-time geometry. A composed rect is converted exactly
//! once from the innermost space it was measured in to the space its
//! consumer draws in; there is no second translation step further down the
//! chain.

use crate::{Point, Rect};

macro_rules! space_rect {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
        pub struct $name(pub Rect);

        impl $name {
            /// The plain rect, for consumers that document the space they
            /// read it in.
            pub const fn rect(self) -> Rect {
                self.0
            }

            /// Top-left corner in this space.
            pub const fn origin(self) -> Point {
                Point {
                    x: self.0.x,
                    y: self.0.y,
                }
            }
        }
    };
}

space_rect!(
    /// Rect in virtual-desktop pixels. Multi-monitor layouts place monitors
    /// left of or above the primary at negative origins.
    DesktopRect
);
space_rect!(
    /// Rect relative to the captured game window. The overlay surface is
    /// positioned on that window, so this is also the overlay drawing space.
    WindowRect
);
space_rect!(
    /// Rect in the pixel grid of one captured frame.
    FrameRect
);
space_rect!(
    /// Rect relative to the tooltip image an OCR job received.
    TooltipRect
);
space_rect!(
    /// Rect relative to one OCR segment image.
    SegmentRect
);

/// Frame pixels per window pixel, in thousandths (`1000` is 1:1).
///
/// Every capture backend reports 1:1 today. A backend that captures at a
/// different pixel density than the window it observes reports its ratio
/// here, and [`FramePlacement::frame_to_window`] divides it out once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameScale {
    frame_px_per_window_px_milli: u32,
}

impl FrameScale {
    pub const ONE: Self = Self {
        frame_px_per_window_px_milli: 1000,
    };

    /// A ratio in thousandths; `0` is not a scale and is treated as 1:1.
    pub const fn from_milli(frame_px_per_window_px_milli: u32) -> Self {
        if frame_px_per_window_px_milli == 0 {
            Self::ONE
        } else {
            Self {
                frame_px_per_window_px_milli,
            }
        }
    }

    pub const fn milli(self) -> u32 {
        self.frame_px_per_window_px_milli
    }

    /// Frame span `[start, start + len)` in window pixels: the start rounds
    /// down and the end rounds up, so the window span covers every window
    /// pixel the frame span touches.
    fn frame_span_to_window(self, start: i32, len: u32) -> (i32, u32) {
        let milli = i64::from(self.frame_px_per_window_px_milli);
        let start = i64::from(start);
        let end = start + i64::from(len);
        let window_start = (start * 1000).div_euclid(milli);
        let window_end = -((-end * 1000).div_euclid(milli));
        let window_len = (window_end - window_start).max(0);
        (
            i32::try_from(window_start).unwrap_or(if window_start < 0 {
                i32::MIN
            } else {
                i32::MAX
            }),
            u32::try_from(window_len).unwrap_or(u32::MAX),
        )
    }
}

impl Default for FrameScale {
    fn default() -> Self {
        Self::ONE
    }
}

/// Where one captured frame sits: frame → window → desktop.
///
/// Recorded at capture time and carried with everything derived from the
/// frame, so a later window move cannot re-place rects measured in an
/// earlier frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FramePlacement {
    /// The captured window in desktop coordinates at capture time.
    pub window_in_desktop: DesktopRect,
    /// Window-space position of the frame's top-left pixel. Zero for a
    /// capture of the whole window.
    pub frame_origin_in_window: Point,
    pub scale: FrameScale,
}

impl FramePlacement {
    /// A frame that is the whole window at 1:1.
    pub const fn whole_window(window_in_desktop: DesktopRect) -> Self {
        Self {
            window_in_desktop,
            frame_origin_in_window: Point { x: 0, y: 0 },
            scale: FrameScale::ONE,
        }
    }

    pub fn frame_to_window(&self, rect: FrameRect) -> WindowRect {
        let (x, width) = self.scale.frame_span_to_window(rect.0.x, rect.0.width);
        let (y, height) = self.scale.frame_span_to_window(rect.0.y, rect.0.height);
        WindowRect(Rect {
            x: x.saturating_add(self.frame_origin_in_window.x),
            y: y.saturating_add(self.frame_origin_in_window.y),
            width,
            height,
        })
    }

    /// Desktop position of a window rect. The overlay does not need this —
    /// its surface already sits on the window — but diagnostics and
    /// desktop-global inputs (the pointer) meet the window here.
    pub fn window_to_desktop(&self, rect: WindowRect) -> DesktopRect {
        DesktopRect(Rect {
            x: rect.0.x.saturating_add(self.window_in_desktop.0.x),
            y: rect.0.y.saturating_add(self.window_in_desktop.0.y),
            ..rect.0
        })
    }
}

/// Where one tooltip image sits: its crop in the frame plus that frame's
/// placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TooltipPlacement {
    pub frame: FramePlacement,
    /// The tooltip image's rect in the frame it was cut from.
    pub crop_in_frame: FrameRect,
}

impl TooltipPlacement {
    /// A tooltip image that is the whole frame (whole-window callers such
    /// as the offline `read-tooltip` path hand the full capture to OCR).
    pub const fn whole_frame(frame: FramePlacement, width: u32, height: u32) -> Self {
        Self {
            frame,
            crop_in_frame: FrameRect(Rect {
                x: 0,
                y: 0,
                width,
                height,
            }),
        }
    }

    /// The single tooltip → window conversion.
    pub fn tooltip_to_window(&self, rect: TooltipRect) -> WindowRect {
        self.frame.frame_to_window(FrameRect(Rect {
            x: rect.0.x.saturating_add(self.crop_in_frame.0.x),
            y: rect.0.y.saturating_add(self.crop_in_frame.0.y),
            ..rect.0
        }))
    }

    /// The tooltip image's own rect in window space.
    pub fn crop_in_window(&self) -> WindowRect {
        self.frame.frame_to_window(self.crop_in_frame)
    }
}

/// Place a rect measured in a segment image into the tooltip image, given
/// the segment's rect in the tooltip image. Pure nesting in the same pixel
/// grid — no scale is involved below the frame.
pub fn segment_to_tooltip(segment_in_tooltip: TooltipRect, rect: SegmentRect) -> TooltipRect {
    TooltipRect(Rect {
        x: rect.0.x.saturating_add(segment_in_tooltip.0.x),
        y: rect.0.y.saturating_add(segment_in_tooltip.0.y),
        ..rect.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn whole_window_frame_maps_one_to_one() {
        let placement = FramePlacement::whole_window(DesktopRect(rect(-1920, 0, 1920, 1080)));
        assert_eq!(
            placement.frame_to_window(FrameRect(rect(873, 12, 375, 883))),
            WindowRect(rect(873, 12, 375, 883))
        );
    }

    #[test]
    fn scaled_frame_is_divided_out_once_and_covers_the_touched_pixels() {
        let placement = FramePlacement {
            window_in_desktop: DesktopRect(rect(0, 0, 1280, 720)),
            frame_origin_in_window: Point { x: 0, y: 0 },
            scale: FrameScale::from_milli(1500),
        };
        // 1.5 frame px per window px: [300, 450) -> [200, 300).
        assert_eq!(
            placement.frame_to_window(FrameRect(rect(300, 150, 150, 3))),
            WindowRect(rect(200, 100, 100, 2))
        );
    }

    #[test]
    fn window_to_desktop_keeps_negative_monitor_origins() {
        let placement = FramePlacement::whole_window(DesktopRect(rect(-2560, -300, 1920, 1080)));
        assert_eq!(
            placement.window_to_desktop(WindowRect(rect(10, 20, 30, 40))),
            DesktopRect(rect(-2550, -280, 30, 40))
        );
    }

    #[test]
    fn zero_scale_is_not_a_scale() {
        assert_eq!(FrameScale::from_milli(0), FrameScale::ONE);
    }
}
