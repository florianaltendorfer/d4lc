use super::frame::{LiveTimings, ProcessedFrame};
use super::prepare::{CaptureTargetSource, LiveSetupTimings, PreparedLiveRuntime};
use super::session::{live_event, LiveLoopStats};
use crate::capture::{cursor_state_json, scale_json, window_candidate_json};
use crate::detect::{
    detection_timings_json, input_mode_label, match_candidate_json, planned_tooltip_roi_confidence,
    planned_tooltip_roi_source, thresholds_json, tooltip_detection_json,
};
use crate::to_json_text;
use d4lc_capture::{CaptureResult, CaptureSource};
use d4lc_detect::prepared::DetectFrameResult;
use d4lc_detect::{rect_pixel_count, RoiPlan, RoiRole};
use d4lc_overlay_x11::{OverlayHandle, OverlayStatus};
use d4lc_runtime::{
    CropStabilityStatus, OcrWorker, OcrWorkerStats, ProcessedFrameResult, TooltipTextSnapshot,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Minimal per-frame JSON line for the pipeline-based live loop.
pub(super) fn pipeline_frame_json(
    result: &ProcessedFrameResult,
    frame_index: u64,
    frame_event: &str,
) -> String {
    json!({
        "schema_version": 1,
        "command": "live-loop",
        "event": "frame",
        "frame_event": frame_event,
        "frame_index": frame_index,
        "sequence": result.overlay_frame.sequence,
        "found": result.overlay_frame.tooltip_found,
        "hotpath_ms": result.overlay_frame.hotpath_ms,
        "frame_status": result.frame_status.parse_status_string(),
        "cursor_window_relative": result.overlay_frame.cursor_window_relative,
    })
    .to_string()
}

pub(super) fn manual_validation_session_summary_json(
    frames_requested: u64,
    tooltip_frames: u64,
    stats: &LiveLoopStats,
    setup_timings: LiveSetupTimings,
    overlay: Option<&OverlayHandle>,
    last_processed: Option<&ProcessedFrame>,
    final_tooltip_crop_path: Option<&PathBuf>,
) -> String {
    let capture_target_source = last_processed
        .map(|processed| processed.timings.capture_target_source)
        .unwrap_or(CaptureTargetSource::DiscoveredOnce);
    let final_detection_found =
        last_processed.is_some_and(|processed| processed.detection.selected.is_some());
    let last_event = last_processed
        .map(|_| live_event(false, final_detection_found))
        .unwrap_or("no_frames");
    let final_frame_status = match last_processed {
        Some(_) if final_detection_found => "tooltip_detected",
        Some(_) => "tooltip_absent",
        None => "not_run",
    };
    let (capture, matched, hotpath) = (
        stats.capture_percentiles(),
        stats.match_percentiles(),
        stats.hotpath_percentiles(),
    );
    to_json_text(&json!({
        "schema_version": 1,
        "command": "manual-diablo-validation-bundle",
        "event": "session_summary",
        "frames_requested": frames_requested,
        "frames_processed": stats.frames_processed,
        "tooltip_frames": tooltip_frames,
        "last_event": last_event,
        "capture_target_source": capture_target_source.to_string(),
        "setup_timings": setup_timings_json(setup_timings),
        "timings": {
            "capture_p50_ms": capture.p50_ms,
            "capture_p95_ms": capture.p95_ms,
            "match_p50_ms": matched.p50_ms,
            "match_p95_ms": matched.p95_ms,
            "hotpath_p50_ms": hotpath.p50_ms,
            "hotpath_p95_ms": hotpath.p95_ms,
        },
        "capture_methods_observed": stats.capture_method_labels(),
        "final_sequence": last_processed.map(|processed| processed.sequence),
        "final_frame_status": final_frame_status,
        "final_detection_found": final_detection_found,
        "final_capture_method": last_processed
            .map(|processed| processed.timings.capture_method.to_string()),
        "final_capture_scope": last_processed
            .map(|processed| processed.timings.capture_scope.to_string()),
        "final_tooltip_crop": path_value(final_tooltip_crop_path.map(PathBuf::as_path)),
        "overlay": overlay_json(overlay.map(OverlayHandle::status)),
    }))
}

fn setup_timings_json(timings: LiveSetupTimings) -> Value {
    json!({
        "setup_ms": timings.setup_ms(),
        "config_ms": timings.config_ms,
        "window_lookup_ms": timings.window_lookup_ms,
        "template_prepare_ms": timings.template_prepare_ms,
    })
}

pub(super) fn live_loop_session_start_json(
    frames_requested: u64,
    prepared: &PreparedLiveRuntime,
    overlay: Option<&OverlayHandle>,
    ocr_worker: Option<&OcrWorker>,
) -> String {
    json!({
        "schema_version": 1,
        "command": "live-loop",
        "event": "session_start",
        "frames_requested": frames_requested,
        "window_lookup_ms": prepared.setup_timings.window_lookup_ms,
        "window_lookup_in_hotpath": false,
        "capture_target_source": prepared.capture_target.source.to_string(),
        "setup_timings": setup_timings_json(prepared.setup_timings),
        "overlay": overlay_json(overlay.map(OverlayHandle::status)),
        "ocr_worker_enabled": ocr_worker.is_some(),
        "config": prepared.config,
    })
    .to_string()
}

/// First line of `live`: where the settings of the run came from.
pub(super) fn live_session_start_json(prepared: &PreparedLiveRuntime) -> String {
    json!({
        "schema_version": 1,
        "command": "live",
        "event": "session_start",
        "capture_target_source": prepared.capture_target.source.to_string(),
        "setup_timings": setup_timings_json(prepared.setup_timings),
        "config": prepared.config,
    })
    .to_string()
}

pub(super) fn live_loop_summary_json(
    frames_requested: u64,
    prepared: &PreparedLiveRuntime,
    stats: &LiveLoopStats,
    overlay: Option<&OverlayHandle>,
    ocr_worker_enabled: bool,
    ocr_stats: Option<&OcrWorkerStats>,
    last_ocr_result: Option<&TooltipTextSnapshot>,
) -> String {
    let frames = stats.frames_processed;
    let setup = prepared.setup_timings;
    let ocr_average = |total: fn(&OcrWorkerStats) -> u64| {
        ocr_stats
            .filter(|ocr| ocr.completed > 0)
            .map(|ocr| total(ocr) as f64 / ocr.completed as f64)
    };
    json!({
        "schema_version": 1,
        "command": "live-loop",
        "event": "session_summary",
        "frames_requested": frames_requested,
        "frames_processed": frames,
        "dropped_frames": stats.dropped_frames,
        "window_lookup_ms": prepared.setup_timings.window_lookup_ms,
        "window_lookup_in_hotpath": false,
        "capture_target_source": prepared.capture_target.source.to_string(),
        "overlay": overlay_json(overlay.map(OverlayHandle::status)),
        "timings": {
            "setup_ms": setup.setup_ms(),
            "config_ms": setup.config_ms,
            "window_lookup_ms": setup.window_lookup_ms,
            "template_prepare_ms": setup.template_prepare_ms,
            "hotpath_min_ms": stats.hotpath_min_ms.unwrap_or_default(),
            "hotpath_avg_ms": average_ms(stats.hotpath_sum_ms, frames),
            "hotpath_max_ms": stats.hotpath_max_ms,
            "capture_avg_ms": average_ms(stats.capture_sum_ms, frames),
            "detection_avg_ms": average_ms(stats.detection_sum_ms, frames),
            "match_avg_ms": average_ms(stats.match_sum_ms, frames),
            "output_write_avg_ms": average_ms(stats.output_write_sum_ms, frames),
        },
        "ocr_worker_enabled": ocr_worker_enabled,
        "ocr_jobs_submitted": ocr_stats.map_or(0, |ocr| ocr.submitted),
        "ocr_jobs_completed": ocr_stats.map_or(0, |ocr| ocr.completed),
        "ocr_jobs_dropped": ocr_stats.map_or(0, |ocr| ocr.dropped),
        "ocr_jobs_failed": ocr_stats.map_or(0, |ocr| ocr.failed),
        "ocr_worker_pending": ocr_stats.is_some_and(|ocr| ocr.pending),
        "ocr_worker_in_progress": ocr_stats.is_some_and(|ocr| ocr.in_progress),
        "ocr_avg_ms": ocr_average(|ocr| ocr.total_ocr_ms),
        "ocr_capture_avg_ms": ocr_average(|ocr| ocr.total_capture_ms),
        "ocr_conversion_avg_ms": ocr_average(|ocr| ocr.total_conversion_ms),
        "ocr_splitter_detection_avg_ms":
            ocr_average(|ocr| ocr.total_splitter_detection_ms),
        "last_ocr_error": ocr_stats.and_then(|ocr| ocr.last_error.as_deref()),
        "last_ocr_preview": last_ocr_result.map(|result| result.text_preview.as_str()),
        "last_ocr_line_count": last_ocr_result.map(|result| result.normalized_lines.len()),
        "last_ocr_tooltip_signature": last_ocr_result.map(|result| result.tooltip_signature),
        "last_ocr_parity_path": last_ocr_result.map(|result| result.parity_path.as_str()),
        "config": prepared.config,
    })
    .to_string()
}

fn average_ms(total_ms: u128, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total_ms as f64 / count as f64
    }
}

fn overlay_json(status: Option<&OverlayStatus>) -> Value {
    let Some(status) = status else {
        return json!({
            "enabled": false,
            "backend": "disabled",
            "transparent": false,
            "always_on_top": false,
            "click_through": false,
            "limitation": null,
        });
    };
    json!({
        "enabled": status.enabled,
        "backend": status.backend.as_str(),
        "window_geometry": status.window_geometry,
        "interaction_zone": status.interaction_zone,
        "menu_locked": status.menu_locked,
        "menu_visible": status.menu_visible,
        "transparent": status.transparent,
        "always_on_top": status.always_on_top,
        "click_through": status.click_through,
        "limitation": status.limitation,
    })
}

fn crop_stability_source(processed: &ProcessedFrame) -> &'static str {
    if processed.crop_stability.is_none() {
        "none"
    } else if processed.detection.crop_output_path.is_some()
        && processed.roi_plan.planned_tooltip_rect.is_some()
    {
        RoiRole::TooltipContent.as_str()
    } else {
        processed.roi_plan.role.as_str()
    }
}

pub(super) fn processed_frame_event_json(processed: &ProcessedFrame, event: &str) -> String {
    let planned = processed.roi_plan.planned_tooltip_rect;
    json!({
        "schema_version": 1,
        "event": event,
        "sequence": processed.sequence,
        "dropped_frames": processed.dropped_frames,
        "found": processed.detection.selected.is_some(),
        "mode": input_mode_label(processed.detection.mode),
        "roi": processed.detection.roi,
        "roi_role": processed.roi_plan.role.to_string(),
        "planned_tooltip_roi": planned,
        "planned_tooltip_roi_source": planned_tooltip_roi_source(planned),
        "planned_tooltip_roi_confidence": planned_tooltip_roi_confidence(planned),
        "tooltip_detection": tooltip_detection_json(&processed.detection.tooltip_detection),
        "content_roi_capture_requested": processed.content_roi_capture_requested,
        "selected_match": processed.detection.selected.as_ref().map(match_candidate_json),
        "crop_stability": processed.crop_stability.map(crop_stability_json),
        "crop_stability_source": crop_stability_source(processed),
        "timings": live_timings_json(&processed.timings),
        "outputs": frame_outputs_json(processed),
    })
    .to_string()
}

pub(super) fn processed_frame_json(
    processed: &ProcessedFrame,
    event: Option<&str>,
    config: &Value,
) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "config": config,
        "event": event,
        "sequence": processed.sequence,
        "dropped_frames": processed.dropped_frames,
        "content_roi_capture_requested": processed.content_roi_capture_requested,
        "capture": capture_result_json(&processed.capture, &processed.roi_plan),
        "cursor": processed.cursor.as_ref().map(cursor_state_json),
        "detection": detection_json(&processed.detection),
        "crop_stability": processed.crop_stability.map(crop_stability_json),
        "crop_stability_source": crop_stability_source(processed),
        "timings": live_timings_json(&processed.timings),
        "outputs": frame_outputs_json(processed),
    }))
}

/// The debug files one frame wrote; `null` for each one it did not.
fn frame_outputs_json(processed: &ProcessedFrame) -> Value {
    json!({
        "frame": path_value(processed.frame_output_path.as_deref()),
        "roi": path_value(processed.roi_output_path.as_deref()),
        "trace": path_value(processed.detection.trace_output_path.as_deref()),
        "planned_tooltip_roi_crop": path_value(processed.planned_tooltip_roi_output_path.as_deref()),
        "final_tooltip_crop": path_value(processed.detection.crop_output_path.as_deref()),
        "thresholded_tooltip_crop":
            path_value(processed.detection.thresholded_crop_output_path.as_deref()),
    })
}

fn capture_result_json(result: &CaptureResult, roi_plan: &RoiPlan) -> Value {
    let (source, window) = match &result.source {
        CaptureSource::Window(candidate) => ("window", Some(window_candidate_json(candidate))),
        CaptureSource::ManualRegion(_) => ("manual_region", None),
        CaptureSource::PortalStream(_) => ("portal_stream", None),
    };
    let planned = roi_plan.planned_tooltip_rect;
    json!({
        "backend": result.backend.to_string(),
        "fallback_reason": result.fallback_reason,
        "source": source,
        "geometry": result.geometry,
        "scale": scale_json(result.scale),
        "capture_scope": result.capture_scope.to_string(),
        "capture_method": result.capture_method.to_string(),
        "overlay_window": result.overlay_window.to_string(),
        "capture_roi_role": roi_plan.role.to_string(),
        "roi_fallback_reason": roi_plan.fallback_reason,
        "roi_origin": result.roi_origin.to_string(),
        "requested_capture_rect": result.requested_capture_rect,
        "actual_capture_rect": result.actual_capture_rect,
        "detection_roi": roi_plan.detection_rect,
        "planned_tooltip_roi": planned,
        "planned_tooltip_roi_source": planned_tooltip_roi_source(planned),
        "planned_tooltip_roi_confidence": planned_tooltip_roi_confidence(planned),
        "requested_capture_width": result.requested_capture_rect.width,
        "requested_capture_height": result.requested_capture_rect.height,
        "captured_pixels": u64::from(result.frame.width) * u64::from(result.frame.height),
        "full_frame_pixels": u64::from(result.geometry.width) * u64::from(result.geometry.height),
        "marker_roi_pixels": roi_plan.detection_rect.map(rect_pixel_count).unwrap_or_default(),
        "tooltip_roi_pixels": planned.map(rect_pixel_count).unwrap_or_default(),
        "frame": {
            "width": result.frame.width,
            "height": result.frame.height,
            "stride": result.frame.stride,
        },
        "window": window,
    })
}

fn detection_json(result: &DetectFrameResult) -> Value {
    let planned = result.planned_tooltip_roi;
    json!({
        "preset": result.preset,
        "mode": input_mode_label(result.mode),
        "roi": result.roi,
        "roi_role": result.roi_role.to_string(),
        "detection_roi": result.roi,
        "planned_tooltip_roi": planned,
        "planned_tooltip_roi_source": planned_tooltip_roi_source(planned),
        "planned_tooltip_roi_confidence": planned_tooltip_roi_confidence(planned),
        "tooltip_detection": tooltip_detection_json(&result.tooltip_detection),
        "content_roi_capture_requested": result.crop_output_path.is_some(),
        "thresholds": thresholds_json(result.thresholds),
        "timings": detection_timings_json(result.timings),
        "candidate_count": result.candidates.len(),
        "selected_match": result.selected.as_ref().map(match_candidate_json),
    })
}

fn live_timings_json(timings: &LiveTimings) -> Value {
    json!({
        "setup_ms": timings.setup_ms,
        "config_ms": timings.config_ms,
        "window_lookup_ms": timings.window_lookup_ms,
        "window_lookup_in_hotpath": timings.window_lookup_in_hotpath,
        "capture_target_source": timings.capture_target_source.to_string(),
        "hotpath_total_ms": timings.hotpath_total_ms,
        "frame_total_ms": timings.frame_total_ms,
        "total_ms": timings.total_ms,
        "roi_plan_ms": timings.roi_plan_ms,
        "capture_ms": timings.capture_ms,
        "roi_capture_ms": timings.roi_capture_ms,
        "frame_convert_ms": timings.frame_convert_ms,
        "detection_ms": timings.detection_ms,
        "template_prepare_ms": timings.template_prepare_ms,
        "match_ms": timings.match_ms,
        "marker_roi_match_ms": timings.marker_roi_match_ms,
        "coordinate_translate_ms": timings.coordinate_translate_ms,
        "crop_ms": timings.crop_ms,
        "stability_ms": timings.stability_ms,
        "debug_write_ms": timings.debug_write_ms,
        "output_write_ms": timings.output_write_ms,
        "frame_debug_write_ms": timings.frame_debug_write_ms,
        "roi_debug_write_ms": timings.roi_debug_write_ms,
        "trace_write_ms": timings.trace_write_ms,
        "crop_write_ms": timings.crop_write_ms,
        "requested_capture_width": timings.requested_capture_width,
        "requested_capture_height": timings.requested_capture_height,
        "captured_pixels": timings.captured_pixels,
        "full_frame_pixels": timings.full_frame_pixels,
        "marker_roi_width": timings.marker_roi_width,
        "marker_roi_height": timings.marker_roi_height,
        "marker_roi_pixels": timings.marker_roi_pixels,
        "tooltip_roi_pixels": timings.tooltip_roi_pixels,
        "capture_scope": timings.capture_scope.to_string(),
        "overlay_window": timings.overlay_window.to_string(),
        "capture_roi_role": timings.capture_roi_role.to_string(),
        "capture_backend": timings.capture_backend.to_string(),
        "capture_method": timings.capture_method.to_string(),
        "fallback_reason": timings.fallback_reason,
    })
}

fn path_value(path: Option<&Path>) -> Option<String> {
    path.map(|path| path.display().to_string())
}

fn crop_stability_json(status: CropStabilityStatus) -> Value {
    json!({
        "hash": format!("0x{:016x}", status.hash),
        "changed": status.changed,
        "stable_count": status.stable_count,
        "should_schedule_ocr": status.should_schedule_ocr,
    })
}

#[cfg(test)]
mod tests;
