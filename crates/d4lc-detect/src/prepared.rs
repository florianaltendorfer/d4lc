//! Prepared detection pipeline — types and functions that can run once the
//! detector is configured (thresholds, templates, matcher backend).
//!
//! These are extracted from `d4lc-cli/src/detect.rs` for reuse by the live
//! pipeline.  No CLI entry-point logic, asset loading, or debug file I/O lives
//! here.

use d4lc_capture::crop_frame;
use d4lc_core::{CaptureFrame, GrayImage, InputMode, MatchCandidate, Point, Rect};
use d4lc_vision::{
    match_templates_below_parallel_timed, scan_threads_for_host, to_grayscale_into, MatchScratch,
};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, TryLockError};
use std::time::Instant;

use crate::{compute_roi, compute_tooltip_content_roi, select_best_candidate, RoiRole};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThresholdSettings {
    pub min: u8,
    pub max: u8,
    pub similarity_tooltip: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DetectionTimings {
    pub asset_load_ms: u128,
    pub roi_crop_ms: u128,
    pub grayscale_ms: u128,
    pub template_select_ms: u128,
    pub template_load_ms: u128,
    pub template_prepare_ms: u128,
    /// The haystack preparation: the squared integral image build. The scan cost is `match_ms − integral_ms`.
    pub integral_ms: u128,
    pub match_ms: u128,
    /// Threads that ran the template scan (1 = sequential).
    pub scan_threads: usize,
    pub coordinate_translate_ms: u128,
    pub trace_write_ms: u128,
    pub crop_write_ms: u128,
    pub total_ms: u128,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectFrameResult {
    pub preset: String,
    pub mode: InputMode,
    pub thresholds: ThresholdSettings,
    pub roi: Rect,
    pub roi_role: RoiRole,
    pub planned_tooltip_roi: Option<Rect>,
    pub tooltip_detection: TooltipDetectionTrace,
    pub candidates: Vec<MatchCandidate>,
    pub selected: Option<MatchCandidate>,
    pub crop: Option<CaptureFrame>,
    pub trace_output_path: Option<PathBuf>,
    pub crop_output_path: Option<PathBuf>,
    pub thresholded_crop_output_path: Option<PathBuf>,
    pub timings: DetectionTimings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipDetectionTrace {
    pub search_roi: Rect,
    pub detected_tooltip_rect: Option<Rect>,
    pub final_tooltip_crop_rect: Option<Rect>,
    pub crop_source: &'static str,
    pub upstream_rule: &'static str,
    pub final_tooltip_size: Option<(u32, u32)>,
}

pub const UPSTREAM_TOOLTIP_CROP_RULE: &str = "ScreenProcessHandler.FindTooltip/FindTooltips: tooltip template match returns Rectangle(location.X, 0, Settings.TooltipWidth, location.Y); FindTooltips bounds width to currentScreenSource then copies currentScreenSource.Copy(location) into _currentScreenTooltip and thresholds it as _currentScreenTooltipFilter";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateSpace {
    FullWindow,
    WindowRelativeRoi,
}

#[derive(Debug, Clone, Copy)]
pub struct RoiFrame<'a> {
    pub frame: &'a CaptureFrame,
    pub origin: Point,
    pub full_width: u32,
    pub full_height: u32,
    pub coordinate_space: CoordinateSpace,
    pub planned_roi: Option<Rect>,
    pub roi_role: RoiRole,
}

impl<'a> RoiFrame<'a> {
    pub fn full_window(frame: &'a CaptureFrame) -> Self {
        Self {
            frame,
            origin: Point { x: 0, y: 0 },
            full_width: frame.width,
            full_height: frame.height,
            coordinate_space: CoordinateSpace::FullWindow,
            planned_roi: None,
            roi_role: RoiRole::FullFrame,
        }
    }

    pub fn window_relative_roi(
        frame: &'a CaptureFrame,
        origin: Point,
        full_width: u32,
        full_height: u32,
        planned_roi: Rect,
        roi_role: RoiRole,
    ) -> Self {
        Self {
            frame,
            origin,
            full_width,
            full_height,
            coordinate_space: CoordinateSpace::WindowRelativeRoi,
            planned_roi: Some(planned_roi),
            roi_role,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedDetector {
    pub preset: String,
    pub mode: InputMode,
    /// `ScanHeight` from the active settings. In mouse mode it sizes the
    /// bottom search strip the caller plans via `planned_roi`
    /// (`d4lc_detect::upstream_mouse_search_roi`,
    /// `ScreenProcessHandler.cs:563-566`); controller mode and band-less
    /// frames search the whole captured frame (see `compute_roi`).
    pub scan_height_percent: f64,
    pub thresholds: ThresholdSettings,
    pub tooltip_width: i32,
    pub templates: Vec<(String, GrayImage)>,
    pub scan: DetectScan,
}

/// Per-detector scan resources: the thread budget of the template scan and
/// the buffers one detection reuses for the next (grayscale haystack, squared
/// integral image).
///
/// Neither changes a detection result: the scan is bit-identical for every
/// thread count, and the buffers are only a capacity cache that is fully
/// rewritten before it is read. A clone keeps the thread budget and starts
/// with empty buffers; scans compare by thread budget only.
pub struct DetectScan {
    threads: usize,
    scratch: Mutex<DetectScratch>,
}

struct DetectScratch {
    gray: GrayImage,
    matcher: MatchScratch,
}

impl Default for DetectScratch {
    fn default() -> Self {
        Self {
            gray: GrayImage {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            },
            matcher: MatchScratch::default(),
        }
    }
}

/// The host's scan budget (see
/// [`d4lc_vision::scan_threads_for_host`]).
impl Default for DetectScan {
    fn default() -> Self {
        Self::with_threads(scan_threads_for_host())
    }
}

impl Clone for DetectScan {
    fn clone(&self) -> Self {
        Self::with_threads(self.threads)
    }
}

impl PartialEq for DetectScan {
    fn eq(&self, other: &Self) -> bool {
        self.threads == other.threads
    }
}

impl fmt::Debug for DetectScan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DetectScan")
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

impl DetectScan {
    /// A scan budget of `threads` (at least 1, the sequential scan).
    pub fn with_threads(threads: usize) -> Self {
        Self {
            threads: threads.max(1),
            scratch: Mutex::new(DetectScratch::default()),
        }
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Run `scan` with this detector's buffers. When another detection holds
    /// them (a concurrent debug export), it runs with fresh buffers instead:
    /// the result is the same, only the allocation is not reused.
    fn with_scratch<T>(&self, scan: impl FnOnce(&mut DetectScratch) -> T) -> T {
        match self.scratch.try_lock() {
            Ok(mut scratch) => scan(&mut scratch),
            // A panic mid-scan leaves no state that matters: every buffer
            // is rewritten before it is read.
            Err(TryLockError::Poisoned(poisoned)) => scan(&mut poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => scan(&mut DetectScratch::default()),
        }
    }
}

impl PreparedDetector {
    pub fn template_count(&self) -> usize {
        self.templates.len()
    }

    pub fn template_refs(&self) -> Vec<(&str, &GrayImage)> {
        self.templates
            .iter()
            .map(|(name, image)| (name.as_str(), image))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Core detection logic
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn detect_roi_frame_prepared(
    roi_frame: RoiFrame<'_>,
    prepared: &PreparedDetector,
    _mouse_x: i32,
    trace_output_path: Option<PathBuf>,
    crop_output_path: Option<PathBuf>,
) -> Result<DetectFrameResult, String> {
    let total_start = Instant::now();
    let mut timings = DetectionTimings::default();

    let thresholds = prepared.thresholds;
    let _coordinate_space = roi_frame.coordinate_space;
    let roi_start = Instant::now();
    // The ROI comes from the caller: mouse mode plans upstream's
    // cursor-centred `TooltipWidth * 2.5` / `ScanHeight` band
    // (`d4lc_detect::upstream_mouse_search_roi`,
    // ScreenProcessHandler.cs:562-566), controller mode and band-less
    // frames keep the full capture (`compute_roi`). When the caller passes
    // a planned ROI over a full-window frame, only the search is narrowed —
    // the tooltip crop after a match still reads the full-frame pixels.
    let roi = roi_frame
        .planned_roi
        .unwrap_or_else(|| compute_roi(roi_frame.full_width, roi_frame.full_height));
    let local_roi = Rect {
        x: roi.x - roi_frame.origin.x,
        y: roi.y - roi_frame.origin.y,
        width: roi.width,
        height: roi.height,
    };
    let roi_capture = if roi_needs_crop(roi_frame.frame, local_roi) {
        Some(crop_frame(roi_frame.frame, local_roi)?)
    } else {
        None
    };
    timings.roi_crop_ms = elapsed_ms(roi_start);

    let roi_candidates = prepared.scan.with_scratch(|scratch| {
        let grayscale_start = Instant::now();
        let roi_source = roi_capture.as_ref().unwrap_or(roi_frame.frame);
        to_grayscale_into(roi_source, &mut scratch.gray);
        timings.grayscale_ms = elapsed_ms(grayscale_start);

        let match_start = Instant::now();
        let template_refs = prepared.template_refs();
        let scan = match_templates_below_parallel_timed(
            &scratch.gray,
            &template_refs,
            thresholds.similarity_tooltip,
            prepared.scan.threads(),
            &mut scratch.matcher,
        );
        timings.integral_ms = scan.integral_build_ms;
        timings.match_ms = elapsed_ms(match_start);
        timings.scan_threads = scan.threads_used;
        Ok::<_, String>(scan.candidates)
    })?;

    let translate_start = Instant::now();
    let candidates = roi_candidates
        .into_iter()
        .map(|candidate| translate_candidate(candidate, roi.x, roi.y))
        .collect::<Vec<_>>();
    let selected = select_best_candidate(&candidates, thresholds.similarity_tooltip).cloned();
    timings.coordinate_translate_ms = elapsed_ms(translate_start);

    let planned_tooltip_roi = selected.as_ref().and_then(|marker| {
        compute_tooltip_content_roi(
            marker.rect,
            roi_frame.full_width,
            roi_frame.full_height,
            prepared.tooltip_width,
        )
    });
    let detected_tooltip_rect = selected.as_ref().and_then(|candidate| {
        raw_upstream_tooltip_crop_rect(candidate.rect, prepared.tooltip_width)
    });
    let final_tooltip_crop_rect = detected_tooltip_rect.and_then(|rect| {
        clamp_upstream_tooltip_crop_rect(rect, roi_frame.full_width, roi_frame.full_height)
    });
    let tooltip_detection = TooltipDetectionTrace {
        search_roi: roi,
        detected_tooltip_rect,
        final_tooltip_crop_rect,
        crop_source: if final_tooltip_crop_rect.is_some() {
            "upstream_tooltip_template_match"
        } else {
            "none"
        },
        upstream_rule: UPSTREAM_TOOLTIP_CROP_RULE,
        final_tooltip_size: final_tooltip_crop_rect.map(|rect| (rect.width, rect.height)),
    };
    let crop_candidate = final_tooltip_crop_rect
        .map(|rect| MatchCandidate {
            template_name: selected
                .as_ref()
                .map(|candidate| candidate.template_name.clone())
                .unwrap_or_default(),
            score: selected
                .as_ref()
                .map(|candidate| candidate.score)
                .unwrap_or_default(),
            rect,
        })
        .filter(|candidate| {
            rect_is_contained_in_frame(candidate.rect, roi_frame.origin, roi_frame.frame)
        });

    // No debug file writes in the prepared pipeline — the caller handles
    // trace JSON / crop PNG output.  We only compute paths and crop data.
    let (crop, thresholded_crop_result_path) = match (&crop_candidate, &crop_output_path) {
        (Some(best), Some(path)) => {
            let crop =
                crop_frame_in_coordinate_space(roi_frame.frame, best.rect, roi_frame.origin)?;
            let thresholded_path = derive_thresholded_crop_path(path);
            (Some(crop), Some(thresholded_path))
        }
        (Some(best), None) => {
            let crop =
                crop_frame_in_coordinate_space(roi_frame.frame, best.rect, roi_frame.origin)?;
            (Some(crop), None)
        }
        (None, _) => (None, None),
    };
    timings.total_ms = elapsed_ms(total_start);

    Ok(DetectFrameResult {
        preset: prepared.preset.clone(),
        mode: prepared.mode,
        thresholds,
        roi,
        roi_role: roi_frame.roi_role,
        planned_tooltip_roi,
        tooltip_detection,
        candidates,
        selected,
        crop,
        trace_output_path,
        crop_output_path,
        thresholded_crop_output_path: thresholded_crop_result_path,
        timings,
    })
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Whether the search ROI must be copied out of the capture before grayscale.
///
/// When the ROI covers the whole capture (the true-ROI hot path and every
/// full-window frame), grayscale reads the capture directly instead of a copy
/// of it; the gray pixels are identical either way. A zero-sized ROI still
/// goes through `crop_frame`, which rejects it by name.
fn roi_needs_crop(frame: &CaptureFrame, local_roi: Rect) -> bool {
    !(local_roi.x == 0
        && local_roi.y == 0
        && local_roi.width == frame.width
        && local_roi.height == frame.height
        && local_roi.width > 0
        && local_roi.height > 0)
}

pub fn raw_upstream_tooltip_crop_rect(marker_rect: Rect, tooltip_width: i32) -> Option<Rect> {
    let width = u32::try_from(tooltip_width.max(1)).ok()?;
    if marker_rect.y <= 0 || width == 0 {
        return None;
    }
    Some(Rect {
        x: marker_rect.x,
        y: 0,
        width,
        height: u32::try_from(marker_rect.y).ok()?,
    })
}

pub fn clamp_upstream_tooltip_crop_rect(
    rect: Rect,
    frame_width: u32,
    frame_height: u32,
) -> Option<Rect> {
    if rect.x < 0 || rect.y != 0 || frame_width == 0 || frame_height == 0 {
        return None;
    }
    let start_x = u32::try_from(rect.x).ok()?;
    if start_x >= frame_width {
        return None;
    }
    let width = rect.width.min(frame_width.saturating_sub(start_x));
    let height = rect.height.min(frame_height);
    if width == 0 || height == 0 {
        return None;
    }
    Some(Rect {
        x: rect.x,
        y: 0,
        width,
        height,
    })
}

pub fn translate_candidate(
    candidate: MatchCandidate,
    offset_x: i32,
    offset_y: i32,
) -> MatchCandidate {
    MatchCandidate {
        template_name: candidate.template_name,
        score: candidate.score,
        rect: Rect {
            x: candidate.rect.x + offset_x,
            y: candidate.rect.y + offset_y,
            width: candidate.rect.width,
            height: candidate.rect.height,
        },
    }
}

pub fn rect_is_contained_in_frame(rect: Rect, origin: Point, frame: &CaptureFrame) -> bool {
    let local_x = rect.x - origin.x;
    let local_y = rect.y - origin.y;
    if local_x < 0 || local_y < 0 {
        return false;
    }
    let Ok(local_x) = u32::try_from(local_x) else {
        return false;
    };
    let Ok(local_y) = u32::try_from(local_y) else {
        return false;
    };
    local_x
        .checked_add(rect.width)
        .is_some_and(|end_x| end_x <= frame.width)
        && local_y
            .checked_add(rect.height)
            .is_some_and(|end_y| end_y <= frame.height)
}

pub fn crop_frame_in_coordinate_space(
    frame: &CaptureFrame,
    rect: Rect,
    origin: Point,
) -> Result<CaptureFrame, String> {
    crop_frame(
        frame,
        Rect {
            x: rect.x - origin.x,
            y: rect.y - origin.y,
            width: rect.width,
            height: rect.height,
        },
    )
}

pub fn derive_thresholded_crop_path(crop_output_path: &Path) -> PathBuf {
    let parent = crop_output_path.parent().map(Path::to_path_buf);
    let stem = crop_output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("detect-crop");
    let thresholded_name = format!("{stem}-thresholded.png");
    match parent {
        Some(parent) => parent.join(thresholded_name),
        None => PathBuf::from(thresholded_name),
    }
}

fn elapsed_ms(start: Instant) -> u128 {
    start.elapsed().as_micros().div_ceil(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32) -> CaptureFrame {
        CaptureFrame {
            width,
            height,
            stride: width as usize * 4,
            pixels_bgra: vec![0; (width * height * 4) as usize],
        }
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn roi_covering_the_whole_capture_skips_the_copy() {
        let capture = frame(8, 6);
        assert!(!roi_needs_crop(&capture, rect(0, 0, 8, 6)));
    }

    #[test]
    fn roi_inside_a_larger_capture_is_copied() {
        let capture = frame(8, 6);
        assert!(roi_needs_crop(&capture, rect(1, 0, 7, 6)));
        assert!(roi_needs_crop(&capture, rect(0, 1, 8, 5)));
        assert!(roi_needs_crop(&capture, rect(0, 0, 7, 6)));
        assert!(roi_needs_crop(&capture, rect(0, 0, 8, 5)));
    }

    #[test]
    fn empty_capture_still_goes_through_the_named_crop_refusal() {
        let capture = frame(0, 0);
        assert!(roi_needs_crop(&capture, rect(0, 0, 0, 0)));
    }

    #[test]
    fn full_window_detection_takes_the_copy_free_path() {
        let capture = frame(8, 6);
        let roi_frame = RoiFrame::full_window(&capture);
        let roi = compute_roi(roi_frame.full_width, roi_frame.full_height);
        assert!(!roi_needs_crop(roi_frame.frame, roi));
    }

    fn detector(scan: DetectScan) -> PreparedDetector {
        let mut marker = vec![0u8; 16];
        marker[5] = 200;
        marker[10] = 90;
        PreparedDetector {
            preset: "synthetic".to_string(),
            mode: InputMode::Mouse,
            scan_height_percent: 100.0,
            thresholds: ThresholdSettings {
                min: 0,
                max: 255,
                similarity_tooltip: 0.05,
            },
            tooltip_width: 4,
            templates: vec![(
                "marker.png".to_string(),
                GrayImage {
                    width: 4,
                    height: 4,
                    pixels: marker,
                },
            )],
            scan,
        }
    }

    #[test]
    fn scan_thread_budget_changes_nothing_but_the_reported_threads() {
        let mut capture = frame(40, 30);
        let (pixels, _) = capture.pixels_bgra.as_chunks_mut::<4>();
        for (index, pixel) in pixels.iter_mut().enumerate() {
            let value = (index * 13 % 251) as u8;
            *pixel = [value, value, value, 255];
        }
        // Stamp the marker's gray pattern at (21, 17).
        let marker = detector(DetectScan::with_threads(1)).templates[0].1.clone();
        for row in 0..4usize {
            for col in 0..4usize {
                let value = marker.pixels[row * 4 + col];
                let index = ((17 + row) * 40 + 21 + col) * 4;
                capture.pixels_bgra[index..index + 4].copy_from_slice(&[value, value, value, 255]);
            }
        }

        let sequential = detector(DetectScan::with_threads(1));
        let expected =
            detect_roi_frame_prepared(RoiFrame::full_window(&capture), &sequential, 0, None, None)
                .unwrap_or_else(|error| panic!("detect failed: {error}"));
        assert_eq!(expected.timings.scan_threads, 1);
        let selected = expected
            .selected
            .clone()
            .unwrap_or_else(|| panic!("the stamped marker must be found"));
        assert_eq!((selected.rect.x, selected.rect.y), (21, 17));

        for threads in [2, 3, 8] {
            let parallel = detector(DetectScan::with_threads(threads));
            // Twice: the second run reuses the detector's buffers.
            for _ in 0..2 {
                let result = detect_roi_frame_prepared(
                    RoiFrame::full_window(&capture),
                    &parallel,
                    0,
                    None,
                    None,
                )
                .unwrap_or_else(|error| panic!("detect failed: {error}"));
                assert_eq!(result.candidates, expected.candidates, "{threads} threads");
                assert_eq!(result.selected, expected.selected, "{threads} threads");
                assert_eq!(result.crop, expected.crop, "{threads} threads");
                assert_eq!(result.timings.scan_threads, threads);
            }
        }
    }

    #[test]
    fn a_cloned_detector_keeps_its_thread_budget() {
        let original = detector(DetectScan::with_threads(3));
        let clone = original.clone();
        assert_eq!(clone.scan.threads(), 3);
        assert_eq!(clone, original);
        assert_eq!(DetectScan::with_threads(0).threads(), 1);
    }
}
