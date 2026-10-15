//! d4lc-detect — ROI computation and tooltip detection pipeline.
//!
//! Upstream: D4Companion.Services/ScreenProcessHandler.cs, FindTooltips method.

use d4lc_core::{GrayImage, MatchCandidate, Rect};
use d4lc_vision::{match_templates, match_templates_below};

pub mod prepared;
pub use prepared::{
    clamp_upstream_tooltip_crop_rect, crop_frame_in_coordinate_space, derive_thresholded_crop_path,
    detect_roi_frame_prepared, raw_upstream_tooltip_crop_rect, rect_is_contained_in_frame,
    translate_candidate, CoordinateSpace, DetectFrameResult, DetectionTimings, PreparedDetector,
    RoiFrame, ThresholdSettings, TooltipDetectionTrace, UPSTREAM_TOOLTIP_CROP_RULE,
};

/// Purpose of a capture/search rectangle in the live pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoiRole {
    #[default]
    FullFrame,
    TooltipContent,
    /// The mouse-mode search band (upstream `ScreenProcessHandler.cs:562-566`,
    /// see [`upstream_mouse_search_roi`]).
    MouseSearchBand,
}

impl RoiRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullFrame => "full_frame",
            Self::TooltipContent => "tooltip_content",
            Self::MouseSearchBand => "mouse_search_band",
        }
    }
}

impl std::fmt::Display for RoiRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Structured ROI plan used by the live capture path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoiPlan {
    pub capture_rect: Option<Rect>,
    pub detection_rect: Option<Rect>,
    pub planned_tooltip_rect: Option<Rect>,
    pub role: RoiRole,
    pub fallback_reason: Option<String>,
}

pub fn rect_pixel_count(rect: Rect) -> u64 {
    u64::from(rect.width) * u64::from(rect.height)
}

/// Planned tooltip/content rectangle derived after the marker was found.
///
/// The tooltip command marker sits at the bottom-left command strip of the
/// active tooltip, not at the tooltip content's true left edge. The estimate
/// therefore expands left/up/right/down from the full-window marker rect and is
/// intentionally separate from the search ROI. It is used for debug/future OCR
/// planning, not for the hot-path marker scan.
pub fn compute_tooltip_content_roi(
    marker_rect: Rect,
    frame_width: u32,
    frame_height: u32,
    _tooltip_width: i32,
) -> Option<Rect> {
    const LEFT_SLACK: i64 = 64;
    const RIGHT_EXTENT: i64 = 456;
    const TOP_EXTENT: i64 = 940;
    const BOTTOM_SLACK: i64 = 36;

    if marker_rect.width == 0 || marker_rect.height == 0 || frame_width == 0 || frame_height == 0 {
        return None;
    }

    let left = i64::from(marker_rect.x).saturating_sub(LEFT_SLACK);
    let top = i64::from(marker_rect.y).saturating_sub(TOP_EXTENT);
    let right = i64::from(marker_rect.x).saturating_add(RIGHT_EXTENT);
    let bottom = i64::from(marker_rect.y)
        .saturating_add(i64::from(marker_rect.height))
        .saturating_add(BOTTOM_SLACK);

    let width = u32::try_from(right.saturating_sub(left)).ok()?;
    let height = u32::try_from(bottom.saturating_sub(top)).ok()?;
    let raw = Rect {
        x: i32::try_from(left).ok()?,
        y: i32::try_from(top).ok()?,
        width,
        height,
    };

    clamp_rect_to_frame(raw, frame_width, frame_height).ok()
}

pub fn clamp_rect_to_frame(
    rect: Rect,
    frame_width: u32,
    frame_height: u32,
) -> Result<Rect, String> {
    if rect.width == 0 || rect.height == 0 {
        return Err("ROI width and height must be non-zero".to_string());
    }
    if frame_width == 0 || frame_height == 0 {
        return Err("frame width and height must be non-zero".to_string());
    }

    let start_x = i64::from(rect.x).clamp(0, i64::from(frame_width));
    let start_y = i64::from(rect.y).clamp(0, i64::from(frame_height));
    let end_x = i64::from(rect.x)
        .saturating_add(i64::from(rect.width))
        .clamp(0, i64::from(frame_width));
    let end_y = i64::from(rect.y)
        .saturating_add(i64::from(rect.height))
        .clamp(0, i64::from(frame_height));

    if end_x <= start_x || end_y <= start_y {
        return Err("ROI does not intersect frame".to_string());
    }

    Ok(Rect {
        x: i32::try_from(start_x).map_err(|error| error.to_string())?,
        y: i32::try_from(start_y).map_err(|error| error.to_string())?,
        width: u32::try_from(end_x - start_x).map_err(|error| error.to_string())?,
        height: u32::try_from(end_y - start_y).map_err(|error| error.to_string())?,
    })
}

/// Search region for tooltip-marker detection: the whole captured frame.
///
/// Upstream parity: `ScreenProcessHandler.FindTooltips` template-matches over
/// the captured screen bitmap itself. The controller branch hands that whole
/// capture to the matcher (`currentScreenSource.Clone()`), and the tooltip
/// rectangle is derived only after the winning marker is known
/// (`Rectangle(location.X, 0, Settings.TooltipWidth, location.Y)`).
///
/// The mouse mode narrows its search to the band [`upstream_mouse_search_roi`]
/// computes (restoring upstream
/// `ScreenProcessHandler.cs:562-566`). The port searched the full frame in
/// both modes while the live detection depended on tooltips whose markers sit
/// outside a static band; with the band now cursor-centred per frame like
/// upstream, mouse mode applies it and controller mode keeps the full-frame
/// search.
///
/// Both modes therefore pass the captured frame, and a caller without a
/// cursor observation keeps [`RoiRole::FullFrame`] truthfully.
pub fn compute_roi(frame_width: u32, frame_height: u32) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: frame_width,
        height: frame_height,
    }
}

/// The upstream mouse-mode search band
/// (`ScreenProcessHandler.FindTooltips`, `ScreenProcessHandler.cs:562-566`):
/// the cursor-centred `TooltipWidth * 2.5` column over the bottom
/// `ScanHeight` percent of the capture.
///
/// Upstream, verbatim: `scanWidth = (int)(Settings.TooltipWidth * 2.5)`,
/// `scanHeigth = (int)(currentScreenBitmap.Height * (Settings.ScanHeight /
/// 100.0))`, `scanPosY = currentScreenBitmap.Height - scanHeigth`,
/// `scanPosX = Math.Max(0, _mouseCoordsX - (scanWidth / 2))`, and
/// `scanPosX = scanPosX + scanWidth >= Width ? Width - scanWidth : scanPosX`.
///
/// Returns `None` when no band exists — a degenerate width or height
/// (a `TooltipWidth`/`ScanHeight` that computes to zero rows or columns) or a
/// computed band wider than the frame (upstream would form a negative-x
/// rectangle there; the port falls back to the full-frame search instead of
/// inventing a clamp). The caller decides the `None` fallback; the pipeline
/// falls back to the full-frame search.
pub fn upstream_mouse_search_roi(
    frame_width: u32,
    frame_height: u32,
    cursor_x: i32,
    tooltip_width: i32,
    scan_height_percent: f64,
) -> Option<Rect> {
    let scan_width = (f64::from(tooltip_width) * 2.5) as i32;
    let scan_height = (f64::from(frame_height) * scan_height_percent / 100.0) as i32;
    if scan_width <= 0 || scan_height <= 0 {
        return None;
    }
    let scan_pos_y = i64::from(frame_height) - i64::from(scan_height);
    let mut scan_pos_x = i64::from(cursor_x) - i64::from(scan_width / 2);
    scan_pos_x = scan_pos_x.max(0);
    if scan_pos_x + i64::from(scan_width) >= i64::from(frame_width) {
        scan_pos_x = i64::from(frame_width) - i64::from(scan_width);
    }
    if scan_pos_x < 0 {
        return None;
    }
    Some(Rect {
        x: i32::try_from(scan_pos_x).ok()?,
        y: i32::try_from(scan_pos_y).ok()?,
        width: u32::try_from(scan_width).ok()?,
        height: u32::try_from(scan_height).ok()?,
    })
}

/// Run template matching for tooltip detection over a pre-cropped ROI image.
///
/// Upstream: ScreenProcessHandler.cs FindMatchTemplate — runs MatchTemplate then
/// MinMaxLoc and returns the best scoring position.
///
/// Returns all candidates (one per template) so callers can trace the full set.
pub fn run_tooltip_match(roi: &GrayImage, templates: &[(&str, &GrayImage)]) -> Vec<MatchCandidate> {
    match_templates(roi, templates)
}

/// Run template matching and keep only candidates below the upstream tooltip threshold.
///
/// This is the fast live path. It preserves upstream semantics because callers only
/// use templates whose best `SQDIFF_NORMED` score is below `ThresholdSimilarityTooltip`.
pub fn run_tooltip_match_below(
    roi: &GrayImage,
    templates: &[(&str, &GrayImage)],
    threshold: f64,
) -> Vec<MatchCandidate> {
    match_templates_below(roi, templates, threshold)
}

/// Select the best (lowest-scoring) candidate that passes the similarity threshold.
///
/// Upstream: ScreenProcessHandler.cs FindTooltips — picks the result with the
/// smallest `minVal` and discards results above the configured threshold.
///
/// Returns `None` if no candidates pass the threshold.
pub fn select_best_candidate(
    candidates: &[MatchCandidate],
    threshold: f64,
) -> Option<&MatchCandidate> {
    candidates
        .iter()
        .filter(|c| c.score < threshold)
        .min_by(|a, b| a.score.total_cmp(&b.score))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The default search region is the captured frame itself (upstream's
    // controller branch). The mouse-mode band is planned per frame by
    // `upstream_mouse_search_roi` and reaches the detection through the
    // caller's `planned_roi`, never through `compute_roi`.

    #[test]
    fn detection_roi_is_the_full_captured_frame_for_every_frame_size() {
        for (width, height) in [(1920, 1080), (2560, 1440), (3840, 2160), (117, 53)] {
            assert_eq!(
                compute_roi(width, height),
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                }
            );
        }
    }

    #[test]
    fn detection_roi_starts_at_the_capture_origin_and_covers_every_marker_pixel() {
        let roi = compute_roi(1920, 1080);

        assert_eq!(roi.x, 0);
        assert_eq!(roi.y, 0);
        assert_eq!(rect_pixel_count(roi), 1920 * 1080);
        // The live fixture markers the previous cursor-centred crop missed.
        assert!(point_inside_rect(1094, 843, roi));
        assert!(point_inside_rect(1038, 882, roi));
        assert!(point_inside_rect(825, 869, roi));
    }

    #[test]
    fn roi_role_display_uses_stable_snake_case() {
        assert_eq!(RoiRole::FullFrame.to_string(), "full_frame");
        assert_eq!(RoiRole::TooltipContent.to_string(), "tooltip_content");
        assert_eq!(RoiRole::MouseSearchBand.to_string(), "mouse_search_band");
    }

    // ---- upstream mouse-mode search band (ScreenProcessHandler.cs:562-566) ----

    #[test]
    fn mouse_band_is_cursor_centred_over_the_bottom_scan_height_strip() {
        // 1920x1080, TooltipWidth 375 (1080p preset), ScanHeight 50:
        // scanWidth = 937, scanHeight = 540, scanPosY = 540.
        let band = upstream_mouse_search_roi(1920, 1080, 960, 375, 50.0)
            .unwrap_or_else(|| panic!("a centred cursor yields the upstream band"));
        assert_eq!(
            band,
            Rect {
                x: 960 - 937 / 2,
                y: 540,
                width: 937,
                height: 540,
            }
        );
    }

    #[test]
    fn mouse_band_clamps_to_the_frame_edges_like_upstream() {
        // Left edge: Math.Max(0, cursorX - scanWidth / 2).
        let left =
            upstream_mouse_search_roi(1920, 1080, 10, 375, 50.0).unwrap_or_else(|| panic!("band"));
        assert_eq!(left.x, 0);
        // Right edge: scanPosX + scanWidth >= width forces width - scanWidth.
        let right = upstream_mouse_search_roi(1920, 1080, 1910, 375, 50.0)
            .unwrap_or_else(|| panic!("band"));
        assert_eq!(right.x, 1920 - 937);
        assert_eq!(right.x + right.width as i32, 1920);
        // ScanHeight 100 covers the full height (scanPosY = 0).
        let full = upstream_mouse_search_roi(1920, 1080, 960, 375, 100.0)
            .unwrap_or_else(|| panic!("band"));
        assert_eq!(full.y, 0);
        assert_eq!(full.height, 1080);
    }

    #[test]
    fn mouse_band_degenerates_to_none_without_a_rectangle() {
        // A band wider than the frame would form a negative-x rectangle
        // upstream; the port refuses instead of inventing a clamp.
        assert!(upstream_mouse_search_roi(100, 1080, 50, 375, 50.0).is_none());
        // A zero ScanHeight computes zero rows.
        assert!(upstream_mouse_search_roi(1920, 1080, 960, 375, 0.0).is_none());
        // A zero TooltipWidth computes zero columns.
        assert!(upstream_mouse_search_roi(1920, 1080, 960, 0, 50.0).is_none());
    }

    #[test]
    fn tooltip_content_roi_is_marker_anchored_for_observed_controller_markers() {
        for marker in [
            Rect {
                x: 825,
                y: 721,
                width: 26,
                height: 22,
            },
            Rect {
                x: 825,
                y: 749,
                width: 26,
                height: 22,
            },
            Rect {
                x: 825,
                y: 940,
                width: 26,
                height: 22,
            },
            Rect {
                x: 425,
                y: 749,
                width: 26,
                height: 22,
            },
            Rect {
                x: 425,
                y: 940,
                width: 26,
                height: 22,
            },
        ] {
            let tooltip = compute_tooltip_content_roi(marker, 1920, 1080, 375)
                .unwrap_or_else(|| panic!("expected planned tooltip ROI for {marker:?}"));

            assert!(tooltip.x < marker.x);
            assert_eq!(tooltip.width, 520);
            assert!(tooltip.height > marker.height);
            assert!(tooltip.width > marker.width);
            assert!(rect_contains_rect(tooltip, marker));
            assert!(rect_inside_frame(tooltip, 1920, 1080));
            assert_ne!(tooltip, marker);
            assert!(rect_pixel_count(tooltip) > rect_pixel_count(marker));
        }
    }

    #[test]
    fn tooltip_content_roi_uses_expected_1080p_marker_slack() {
        let marker = Rect {
            x: 825,
            y: 749,
            width: 26,
            height: 22,
        };
        let tooltip = compute_tooltip_content_roi(marker, 1920, 1080, 375)
            .unwrap_or_else(|| panic!("expected planned tooltip ROI"));

        assert_eq!(
            tooltip,
            Rect {
                x: 761,
                y: 0,
                width: 520,
                height: 807,
            }
        );
    }

    #[test]
    fn tooltip_content_roi_clamps_near_top_and_left_edges() {
        let marker = Rect {
            x: 20,
            y: 10,
            width: 26,
            height: 22,
        };
        let tooltip = compute_tooltip_content_roi(marker, 1920, 1080, 375)
            .unwrap_or_else(|| panic!("expected planned tooltip ROI"));

        assert_eq!(tooltip.x, 0);
        assert_eq!(tooltip.y, 0);
        assert!(rect_contains_rect(tooltip, marker));
        assert!(rect_inside_frame(tooltip, 1920, 1080));
    }

    #[test]
    fn tooltip_content_roi_clamps_near_bottom_and_right_edges() {
        let marker = Rect {
            x: 1880,
            y: 1020,
            width: 26,
            height: 22,
        };
        let tooltip = compute_tooltip_content_roi(marker, 1920, 1080, 375)
            .unwrap_or_else(|| panic!("expected planned tooltip ROI"));

        assert_eq!(tooltip.x, 1816);
        assert_eq!(tooltip.x as u32 + tooltip.width, 1920);
        assert_eq!(tooltip.y, 80);
        assert_eq!(tooltip.y as u32 + tooltip.height, 1078);
        assert!(rect_contains_rect(tooltip, marker));
        assert!(rect_inside_frame(tooltip, 1920, 1080));
    }

    #[test]
    fn tooltip_content_roi_is_separate_from_the_search_roi() {
        let marker_match = Rect {
            x: 825,
            y: 749,
            width: 26,
            height: 22,
        };
        let search_roi = compute_roi(1920, 1080);
        let tooltip = compute_tooltip_content_roi(marker_match, 1920, 1080, 375)
            .unwrap_or_else(|| panic!("expected planned tooltip ROI"));

        assert_ne!(tooltip, marker_match);
        assert_ne!(tooltip, search_roi);
        assert!(rects_intersect(tooltip, search_roi));
        assert!(rect_contains_rect(search_roi, tooltip));
        assert!(rect_pixel_count(tooltip) < rect_pixel_count(search_roi));
    }

    fn point_inside_rect(x: i32, y: i32, rect: Rect) -> bool {
        x >= rect.x
            && y >= rect.y
            && u32::try_from(x - rect.x)
                .ok()
                .is_some_and(|local_x| local_x < rect.width)
            && u32::try_from(y - rect.y)
                .ok()
                .is_some_and(|local_y| local_y < rect.height)
    }

    fn rect_contains_rect(container: Rect, rect: Rect) -> bool {
        let Some(container_right) = rect_right(container) else {
            return false;
        };
        let Some(container_bottom) = rect_bottom(container) else {
            return false;
        };
        let Some(rect_right) = rect_right(rect) else {
            return false;
        };
        let Some(rect_bottom) = rect_bottom(rect) else {
            return false;
        };

        rect.x >= container.x
            && rect.y >= container.y
            && rect_right <= container_right
            && rect_bottom <= container_bottom
    }

    fn rect_inside_frame(rect: Rect, frame_width: u32, frame_height: u32) -> bool {
        if rect.x < 0 || rect.y < 0 {
            return false;
        }
        let Some(right) = rect_right(rect) else {
            return false;
        };
        let Some(bottom) = rect_bottom(rect) else {
            return false;
        };

        right <= i64::from(frame_width) && bottom <= i64::from(frame_height)
    }

    fn rects_intersect(left: Rect, right: Rect) -> bool {
        let Some(left_right) = rect_right(left) else {
            return false;
        };
        let Some(left_bottom) = rect_bottom(left) else {
            return false;
        };
        let Some(right_right) = rect_right(right) else {
            return false;
        };
        let Some(right_bottom) = rect_bottom(right) else {
            return false;
        };

        i64::from(left.x) < right_right
            && left_right > i64::from(right.x)
            && i64::from(left.y) < right_bottom
            && left_bottom > i64::from(right.y)
    }

    fn rect_right(rect: Rect) -> Option<i64> {
        i64::from(rect.x).checked_add(i64::from(rect.width))
    }

    fn rect_bottom(rect: Rect) -> Option<i64> {
        i64::from(rect.y).checked_add(i64::from(rect.height))
    }

    // ---- matcher integration tests ----

    fn make_roi() -> GrayImage {
        let mut pixels = vec![100u8; 10 * 10];
        for row in 2usize..6 {
            for col in 3usize..7 {
                pixels[row * 10 + col] = 200;
            }
        }
        GrayImage {
            width: 10,
            height: 10,
            pixels,
        }
    }

    #[test]
    fn run_tooltip_match_returns_all_candidates() {
        let roi = make_roi();
        let exact = GrayImage {
            width: 4,
            height: 4,
            pixels: vec![200u8; 16],
        };
        let far = GrayImage {
            width: 4,
            height: 4,
            pixels: vec![50u8; 16],
        };
        let templates: &[(&str, &GrayImage)] = &[("tooltip_exact", &exact), ("tooltip_far", &far)];
        let candidates = run_tooltip_match(&roi, templates);
        assert_eq!(candidates.len(), 2);
        let names: Vec<&str> = candidates
            .iter()
            .map(|c| c.template_name.as_str())
            .collect();
        assert!(names.contains(&"tooltip_exact"));
        assert!(names.contains(&"tooltip_far"));
    }

    #[test]
    fn run_tooltip_match_below_keeps_only_threshold_passing_candidates() {
        let roi = make_roi();
        let exact = GrayImage {
            width: 4,
            height: 4,
            pixels: vec![200u8; 16],
        };
        let far = GrayImage {
            width: 4,
            height: 4,
            pixels: vec![50u8; 16],
        };
        let templates: &[(&str, &GrayImage)] = &[("tooltip_exact", &exact), ("tooltip_far", &far)];
        let candidates = run_tooltip_match_below(&roi, templates, 0.05);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].template_name, "tooltip_exact");
    }

    #[test]
    fn select_best_candidate_picks_lowest_score_under_threshold() {
        let c1 = MatchCandidate {
            template_name: "low".to_owned(),
            score: 0.05,
            rect: Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
        };
        let c2 = MatchCandidate {
            template_name: "high".to_owned(),
            score: 0.4,
            rect: Rect {
                x: 1,
                y: 1,
                width: 4,
                height: 4,
            },
        };
        let binding = [c1, c2];
        let result =
            select_best_candidate(&binding, 0.5).unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(result.template_name, "low");
    }

    #[test]
    fn select_best_candidate_rejects_above_threshold() {
        let c = MatchCandidate {
            template_name: "bad".to_owned(),
            score: 0.9,
            rect: Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
        };
        let binding = [c];
        assert!(select_best_candidate(&binding, 0.5).is_none());
    }
}
