use super::prepare::{
    reported_capture_target_source, CaptureTargetSource, LiveSetupTimings, PreparedLiveRuntime,
};
use super::{elapsed_ms, LiveArgs};
use d4lc_capture::{
    capture_reference_with_backend_and_request, crop_frame, read_cursor_state, CaptureBackend,
    CaptureMethod, CaptureRequest, CaptureResult, CaptureScopeResult, CursorState,
    OverlayWindowState,
};
use d4lc_core::{CaptureFrame, InputMode, Point, Rect};
use d4lc_debug::{write_capture_png, write_gray_png};
use d4lc_detect::prepared::{
    derive_thresholded_crop_path, detect_roi_frame_prepared, DetectFrameResult, RoiFrame,
};
use d4lc_detect::{rect_pixel_count, RoiPlan, RoiRole};
use d4lc_runtime::{hash_capture_frame, CropStabilityGate, CropStabilityStatus};
use d4lc_vision::{threshold_binary_inv, to_grayscale};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[allow(clippy::too_many_arguments)]
pub(super) fn process_latest_frame(
    output_base: &Path,
    args: &LiveArgs,
    sequence: u64,
    capture: CaptureResult,
    dropped_frames: u64,
    capture_ms: u128,
    roi_plan_ms: u128,
    window_lookup_ms: u128,
    hotpath_start: Instant,
    roi_plan: RoiPlan,
    pre_capture_cursor: Option<CursorState>,
    stability_gate: &mut CropStabilityGate,
    prepared: &PreparedLiveRuntime,
    setup_timings: LiveSetupTimings,
) -> Result<ProcessedFrame, String> {
    let detector = &prepared.detector;
    let mut roi_plan = roi_plan;
    let mut timings = initial_live_timings(
        prepared,
        setup_timings,
        &capture,
        &roi_plan,
        capture_ms,
        roi_plan_ms,
        window_lookup_ms,
    );

    let cursor_start = Instant::now();
    let (cursor, mouse_x) = resolve_frame_cursor(detector.mode, pre_capture_cursor, &capture)?;
    timings.cursor_ms = elapsed_ms(cursor_start);

    let trace_output_path = args
        .output_dir()
        .map(|output_dir| output_dir.join(format!("detect-trace-{sequence:06}.json")));
    let content_roi_capture_requested = args.output_dir().is_some();

    let detection_start = Instant::now();
    let mut detection = detect_roi_frame_prepared(
        detection_roi_frame(&capture, &roi_plan),
        detector,
        mouse_x,
        trace_output_path,
        None,
    )?;
    timings.detection_ms = elapsed_ms(detection_start);
    timings.marker_roi_match_ms = detection.timings.match_ms;
    copy_detection_timings(&mut timings, &detection);
    roi_plan.planned_tooltip_rect = detection.planned_tooltip_roi;
    timings.tooltip_roi_pixels = roi_plan
        .planned_tooltip_rect
        .map(rect_pixel_count)
        .unwrap_or_default();

    let debug_paths = write_frame_debug_artifacts(
        output_base,
        args,
        prepared,
        sequence,
        &capture,
        &mut detection,
        &mut timings,
    )?;

    let stability_start = Instant::now();
    let crop_stability = detection.crop.as_ref().map(|crop| {
        let hash = hash_capture_frame(crop);
        stability_gate.observe_hash(hash)
    });
    timings.stability_ms = elapsed_ms(stability_start);
    finish_live_timings(&mut timings, &capture, &roi_plan, &detection, hotpath_start);

    Ok(ProcessedFrame {
        sequence,
        dropped_frames,
        capture,
        cursor,
        roi_plan,
        detection,
        crop_stability,
        content_roi_capture_requested,
        frame_output_path: debug_paths.frame_output_path,
        roi_output_path: debug_paths.roi_output_path,
        planned_tooltip_roi_output_path: debug_paths.planned_tooltip_roi_output_path,
        timings,
    })
}

/// Capture- and setup-derived timings known before detection runs.
fn initial_live_timings(
    prepared: &PreparedLiveRuntime,
    setup_timings: LiveSetupTimings,
    capture: &CaptureResult,
    roi_plan: &RoiPlan,
    capture_ms: u128,
    roi_plan_ms: u128,
    window_lookup_ms: u128,
) -> LiveTimings {
    LiveTimings {
        setup_ms: setup_timings.setup_ms(),
        capture_ms,
        roi_plan_ms,
        window_lookup_ms: setup_timings
            .window_lookup_ms
            .saturating_add(window_lookup_ms),
        window_lookup_in_hotpath: window_lookup_ms > 0,
        config_ms: setup_timings.config_ms,
        template_prepare_ms: setup_timings.template_prepare_ms,
        capture_target_source: reported_capture_target_source(prepared, setup_timings),
        capture_backend: capture.backend,
        capture_method: capture.capture_method,
        requested_capture_width: capture.requested_capture_rect.width,
        requested_capture_height: capture.requested_capture_rect.height,
        captured_pixels: u64::from(capture.frame.width) * u64::from(capture.frame.height),
        full_frame_pixels: u64::from(capture.geometry.width) * u64::from(capture.geometry.height),
        capture_scope: capture.capture_scope,
        overlay_window: capture.overlay_window,
        capture_roi_role: roi_plan.role,
        marker_roi_width: roi_plan
            .detection_rect
            .map(|rect| rect.width)
            .unwrap_or_default(),
        marker_roi_height: roi_plan
            .detection_rect
            .map(|rect| rect.height)
            .unwrap_or_default(),
        marker_roi_pixels: roi_plan
            .detection_rect
            .map(rect_pixel_count)
            .unwrap_or_default(),
        tooltip_roi_pixels: roi_plan
            .planned_tooltip_rect
            .map(rect_pixel_count)
            .unwrap_or_default(),
        roi_capture_ms: if matches!(capture.capture_scope, CaptureScopeResult::Roi) {
            capture_ms
        } else {
            0
        },
        ..LiveTimings::default()
    }
}

/// Mouse mode needs the cursor (pre-capture reading, else read now) and its
/// window-relative x; controller mode uses neither.
fn resolve_frame_cursor(
    mode: InputMode,
    pre_capture_cursor: Option<CursorState>,
    capture: &CaptureResult,
) -> Result<(Option<CursorState>, i32), String> {
    match mode {
        InputMode::Mouse => {
            let cursor = match pre_capture_cursor {
                Some(cursor) => cursor,
                None => read_cursor_state(Some(capture.geometry))?,
            };
            let mouse_x = cursor
                .window_relative
                .ok_or_else(|| {
                    "mouse mode requires a cursor position relative to the captured frame"
                        .to_string()
                })?
                .x;
            Ok((Some(cursor), mouse_x))
        }
        InputMode::Controller => Ok((None, 0)),
    }
}

struct FrameDebugPaths {
    frame_output_path: Option<PathBuf>,
    roi_output_path: Option<PathBuf>,
    planned_tooltip_roi_output_path: Option<PathBuf>,
}

/// Writes the per-frame debug images (full frame, planned tooltip ROI, final
/// tooltip crop plus its thresholded copy, detection ROI) below
/// `output_base` and records the final crop on `detection`.
fn write_frame_debug_artifacts(
    output_base: &Path,
    args: &LiveArgs,
    prepared: &PreparedLiveRuntime,
    sequence: u64,
    capture: &CaptureResult,
    detection: &mut DetectFrameResult,
    timings: &mut LiveTimings,
) -> Result<FrameDebugPaths, String> {
    let detector = &prepared.detector;
    let planned_tooltip_roi_output_path = args
        .output_dir()
        .map(|output_dir| output_dir.join(format!("planned-tooltip-roi-{sequence:06}.png")));
    let final_tooltip_crop_output_path = args
        .output_dir()
        .map(|output_dir| output_dir.join(format!("tooltip-crop-{sequence:06}.png")));

    let frame_output_path = if args.debug_enabled() {
        Some(
            args.capture_output_path
                .clone()
                .or_else(|| {
                    args.output_dir()
                        .map(|output_dir| output_dir.join(format!("frame-{sequence:06}.png")))
                })
                .ok_or_else(|| {
                    "debug output requested but no output path is available".to_string()
                })?,
        )
    } else {
        None
    };

    if let Some(path) = &frame_output_path {
        let frame_write_start = Instant::now();
        write_capture_png(&output_base.join(path), &capture.frame)?;
        timings.frame_debug_write_ms = elapsed_ms(frame_write_start);
    }

    if let (Some(path), Some(tooltip_roi)) = (
        &planned_tooltip_roi_output_path,
        detection.planned_tooltip_roi,
    ) {
        let planned_crop_start = Instant::now();
        let crop = tooltip_crop_for_debug(prepared, capture, tooltip_roi)?;
        write_capture_png(&output_base.join(path), &crop)?;
        timings.crop_write_ms = timings
            .crop_write_ms
            .saturating_add(elapsed_ms(planned_crop_start));
    }

    if let (Some(path), Some(final_crop_rect)) = (
        &final_tooltip_crop_output_path,
        detection.tooltip_detection.final_tooltip_crop_rect,
    ) {
        let crop_start = Instant::now();
        let crop = tooltip_crop_for_debug(prepared, capture, final_crop_rect)?;
        write_capture_png(&output_base.join(path), &crop)?;
        let crop_gray = to_grayscale(&crop);
        let crop_thresholded =
            threshold_binary_inv(&crop_gray, detector.thresholds.min, detector.thresholds.max);
        let thresholded_path = derive_thresholded_crop_path(path);
        write_gray_png(&output_base.join(&thresholded_path), &crop_thresholded)?;
        detection.crop = Some(crop);
        detection.crop_output_path = Some(path.clone());
        detection.thresholded_crop_output_path = Some(thresholded_path);
        timings.crop_write_ms = timings.crop_write_ms.saturating_add(elapsed_ms(crop_start));
        timings.crop_ms = timings.crop_write_ms;
    }

    let roi_output_path = args
        .output_dir()
        .map(|output_dir| output_dir.join(format!("roi-{sequence:06}.png")));
    if let Some(path) = &roi_output_path {
        let roi_write_start = Instant::now();
        let roi_capture = crop_capture_by_full_window_rect(capture, detection.roi)?;
        write_capture_png(&output_base.join(path), &roi_capture)?;
        timings.roi_debug_write_ms = elapsed_ms(roi_write_start);
    }

    Ok(FrameDebugPaths {
        frame_output_path,
        roi_output_path,
        planned_tooltip_roi_output_path,
    })
}

/// Debug-write totals, fallback reason and the hot-path end time.
fn finish_live_timings(
    timings: &mut LiveTimings,
    capture: &CaptureResult,
    roi_plan: &RoiPlan,
    detection: &DetectFrameResult,
    hotpath_start: Instant,
) {
    timings.trace_write_ms = if detection.trace_output_path.is_some() {
        detection.timings.trace_write_ms
    } else {
        0
    };
    timings.debug_write_ms = timings
        .frame_debug_write_ms
        .saturating_add(timings.roi_debug_write_ms)
        .saturating_add(timings.trace_write_ms)
        .saturating_add(timings.crop_write_ms);
    if timings.fallback_reason.is_none() {
        timings.fallback_reason = combine_runtime_fallback_reasons(
            capture.fallback_reason.clone(),
            roi_plan.fallback_reason.clone(),
        );
    }
    let hotpath_total_ms = elapsed_ms(hotpath_start);
    timings.hotpath_total_ms = hotpath_total_ms;
    timings.total_ms = hotpath_total_ms;
    timings.frame_total_ms = timings.setup_ms.saturating_add(hotpath_total_ms);
    timings.output_write_ms = timings.debug_write_ms;
}

fn copy_detection_timings(timings: &mut LiveTimings, detection: &DetectFrameResult) {
    timings.match_ms = detection.timings.match_ms;
    timings.coordinate_translate_ms = detection.timings.coordinate_translate_ms;
    timings.trace_write_ms = if detection.trace_output_path.is_some() {
        detection.timings.trace_write_ms
    } else {
        0
    };
}

fn tooltip_crop_for_debug(
    prepared: &PreparedLiveRuntime,
    capture: &CaptureResult,
    tooltip_roi: Rect,
) -> Result<CaptureFrame, String> {
    match crop_capture_by_full_window_rect(capture, tooltip_roi) {
        Ok(crop) => Ok(crop),
        Err(_) => {
            let tooltip_capture = capture_reference_with_backend_and_request(
                &prepared.capture_target.reference,
                prepared.capture_backend,
                CaptureRequest::WindowRelativeRoi(tooltip_roi),
            )?;
            if matches!(tooltip_capture.capture_scope, CaptureScopeResult::Roi)
                && tooltip_capture.actual_capture_rect == tooltip_roi
            {
                Ok(tooltip_capture.frame)
            } else {
                crop_capture_by_full_window_rect(&tooltip_capture, tooltip_roi)
            }
        }
    }
}

fn combine_runtime_fallback_reasons(
    primary: Option<String>,
    secondary: Option<String>,
) -> Option<String> {
    match (primary, secondary) {
        (Some(primary), Some(secondary)) => Some(format!("{primary}; {secondary}")),
        (Some(primary), None) => Some(primary),
        (None, Some(secondary)) => Some(secondary),
        (None, None) => None,
    }
}

fn detection_roi_frame<'a>(capture: &'a CaptureResult, roi_plan: &RoiPlan) -> RoiFrame<'a> {
    match (roi_plan.detection_rect, capture.capture_scope) {
        (Some(_), CaptureScopeResult::Roi) => RoiFrame::window_relative_roi(
            &capture.frame,
            Point {
                x: capture.actual_capture_rect.x,
                y: capture.actual_capture_rect.y,
            },
            capture.geometry.width,
            capture.geometry.height,
            capture.actual_capture_rect,
            roi_plan.role,
        ),
        (Some(roi), _) => RoiFrame::window_relative_roi(
            &capture.frame,
            Point { x: 0, y: 0 },
            capture.geometry.width,
            capture.geometry.height,
            roi,
            roi_plan.role,
        ),
        (None, _) => RoiFrame::full_window(&capture.frame),
    }
}

fn crop_capture_by_full_window_rect(
    capture: &CaptureResult,
    rect: Rect,
) -> Result<CaptureFrame, String> {
    let origin = match capture.capture_scope {
        CaptureScopeResult::Roi => Point {
            x: capture.actual_capture_rect.x,
            y: capture.actual_capture_rect.y,
        },
        CaptureScopeResult::FullFrame | CaptureScopeResult::FullFrameFallback => {
            Point { x: 0, y: 0 }
        }
    };
    crop_frame(
        &capture.frame,
        Rect {
            x: rect.x - origin.x,
            y: rect.y - origin.y,
            width: rect.width,
            height: rect.height,
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiveTimings {
    pub(super) capture_backend: CaptureBackend,
    pub(super) capture_method: CaptureMethod,
    pub(super) fallback_reason: Option<String>,
    pub(super) setup_ms: u128,
    pub(super) hotpath_total_ms: u128,
    pub(super) frame_total_ms: u128,
    pub(super) capture_ms: u128,
    pub(super) roi_plan_ms: u128,
    pub(super) roi_capture_ms: u128,
    pub(super) config_ms: u128,
    pub(super) window_lookup_ms: u128,
    pub(super) window_lookup_in_hotpath: bool,
    pub(super) frame_convert_ms: u128,
    pub(super) requested_capture_width: u32,
    pub(super) requested_capture_height: u32,
    pub(super) captured_pixels: u64,
    pub(super) full_frame_pixels: u64,
    pub(super) marker_roi_pixels: u64,
    pub(super) tooltip_roi_pixels: u64,
    pub(super) marker_roi_width: u32,
    pub(super) marker_roi_height: u32,
    pub(super) capture_scope: CaptureScopeResult,
    pub(super) overlay_window: OverlayWindowState,
    pub(super) capture_roi_role: RoiRole,
    pub(super) template_prepare_ms: u128,
    pub(super) match_ms: u128,
    pub(super) marker_roi_match_ms: u128,
    pub(super) coordinate_translate_ms: u128,
    pub(super) crop_ms: u128,
    pub(super) debug_write_ms: u128,
    pub(super) output_write_ms: u128,
    pub(super) trace_write_ms: u128,
    pub(super) crop_write_ms: u128,
    pub(super) frame_debug_write_ms: u128,
    pub(super) cursor_ms: u128,
    pub(super) detection_ms: u128,
    pub(super) roi_debug_write_ms: u128,
    pub(super) stability_ms: u128,
    pub(super) total_ms: u128,
    pub(super) capture_target_source: CaptureTargetSource,
}

impl Default for LiveTimings {
    fn default() -> Self {
        Self {
            capture_backend: CaptureBackend::Auto,
            capture_method: CaptureMethod::X11Command,
            fallback_reason: None,
            setup_ms: 0,
            hotpath_total_ms: 0,
            frame_total_ms: 0,
            capture_ms: 0,
            roi_plan_ms: 0,
            roi_capture_ms: 0,
            config_ms: 0,
            window_lookup_ms: 0,
            window_lookup_in_hotpath: false,
            frame_convert_ms: 0,
            requested_capture_width: 0,
            requested_capture_height: 0,
            captured_pixels: 0,
            full_frame_pixels: 0,
            marker_roi_pixels: 0,
            tooltip_roi_pixels: 0,
            marker_roi_width: 0,
            marker_roi_height: 0,
            capture_scope: CaptureScopeResult::FullFrame,
            overlay_window: OverlayWindowState::Ignored,
            capture_roi_role: RoiRole::FullFrame,
            template_prepare_ms: 0,
            match_ms: 0,
            marker_roi_match_ms: 0,
            coordinate_translate_ms: 0,
            crop_ms: 0,
            debug_write_ms: 0,
            output_write_ms: 0,
            trace_write_ms: 0,
            crop_write_ms: 0,
            frame_debug_write_ms: 0,
            cursor_ms: 0,
            detection_ms: 0,
            roi_debug_write_ms: 0,
            stability_ms: 0,
            total_ms: 0,
            capture_target_source: CaptureTargetSource::DiscoveredOnce,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProcessedFrame {
    pub(super) sequence: u64,
    pub(super) dropped_frames: u64,
    pub(super) capture: CaptureResult,
    pub(super) cursor: Option<CursorState>,
    pub(super) roi_plan: RoiPlan,
    pub(super) detection: DetectFrameResult,
    pub(super) crop_stability: Option<CropStabilityStatus>,
    pub(super) content_roi_capture_requested: bool,
    pub(super) frame_output_path: Option<PathBuf>,
    pub(super) roi_output_path: Option<PathBuf>,
    pub(super) planned_tooltip_roi_output_path: Option<PathBuf>,
    pub(super) timings: LiveTimings,
}

#[cfg(test)]
mod tests;
