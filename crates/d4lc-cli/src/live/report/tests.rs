use super::*;
use crate::live::test_support::test_prepared_runtime;
use d4lc_capture::{CaptureBackend, CaptureScopeResult, OverlayWindowState};
use d4lc_core::{CaptureFrame, Rect};

#[test]
fn live_timings_json_reports_stage_fields() {
    let json = live_timings_json(&LiveTimings {
        setup_ms: 10,
        capture_ms: 1,
        config_ms: 2,
        window_lookup_ms: 3,
        frame_debug_write_ms: 4,
        cursor_ms: 5,
        detection_ms: 6,
        roi_debug_write_ms: 7,
        stability_ms: 8,
        hotpath_total_ms: 9,
        frame_total_ms: 19,
        total_ms: 9,
        ..LiveTimings::default()
    });
    assert_eq!(json["setup_ms"], 10);
    assert_eq!(json["window_lookup_ms"], 3);
    assert_eq!(json["window_lookup_in_hotpath"], false);
    assert_eq!(json["capture_target_source"], "discovered_once");
    assert_eq!(json["hotpath_total_ms"], 9);
    assert_eq!(json["frame_total_ms"], 19);
    assert_eq!(json["total_ms"], 9);
    assert_eq!(json["capture_ms"], 1);
    assert_eq!(json["detection_ms"], 6);
    assert_eq!(json["capture_backend"], "auto");
    assert_eq!(json["capture_method"], "x11-command");
}

#[test]
fn live_timings_json_keeps_window_lookup_out_of_hotpath_total() {
    let json = live_timings_json(&LiveTimings {
        setup_ms: 575,
        window_lookup_ms: 570,
        hotpath_total_ms: 12,
        frame_total_ms: 587,
        total_ms: 12,
        ..LiveTimings::default()
    });

    assert_eq!(json["window_lookup_ms"], 570);
    assert_eq!(json["hotpath_total_ms"], 12);
    assert_eq!(json["total_ms"], 12);
    assert_eq!(json["frame_total_ms"], 587);
    assert_eq!(json["window_lookup_in_hotpath"], false);
}

#[test]
fn no_output_live_timings_report_zero_write_costs() {
    let json = live_timings_json(&LiveTimings::default());

    assert_eq!(json["debug_write_ms"], 0);
    assert_eq!(json["output_write_ms"], 0);
    assert_eq!(json["frame_debug_write_ms"], 0);
    assert_eq!(json["roi_debug_write_ms"], 0);
    assert_eq!(json["trace_write_ms"], 0);
    assert_eq!(json["crop_write_ms"], 0);
}

#[test]
fn live_loop_session_json_reports_single_setup_lookup() {
    let prepared = test_prepared_runtime(
        CaptureTargetSource::DiscoveredOnce,
        LiveSetupTimings {
            config_ms: 2,
            window_lookup_ms: 612,
            template_prepare_ms: 5,
        },
    );

    let json = live_loop_session_start_json(100, &prepared, None, None);

    assert!(json.contains("\"command\":\"live-loop\""));
    assert!(json.contains("\"event\":\"session_start\""));
    assert!(json.contains("\"frames_requested\":100"));
    assert!(json.contains("\"window_lookup_ms\":612"));
    assert!(json.contains("\"window_lookup_in_hotpath\":false"));
    assert!(json.contains("\"capture_target_source\":\"discovered_once\""));
    assert!(json.contains("\"setup_ms\":619"));
    assert!(json.contains("\"overlay\":{\"enabled\":false"));
}

#[test]
fn live_loop_summary_json_reports_cached_hotpath_aggregates() {
    let prepared = test_prepared_runtime(
        CaptureTargetSource::DiscoveredOnce,
        LiveSetupTimings {
            config_ms: 2,
            window_lookup_ms: 612,
            template_prepare_ms: 5,
        },
    );
    let stats = LiveLoopStats {
        frames_processed: 100,
        dropped_frames: 0,
        hotpath_min_ms: Some(7),
        hotpath_max_ms: 14,
        hotpath_sum_ms: 1000,
        capture_sum_ms: 400,
        detection_sum_ms: 600,
        match_sum_ms: 500,
        output_write_sum_ms: 0,
        capture_samples_ms: Vec::new(),
        match_samples_ms: Vec::new(),
        hotpath_samples_ms: Vec::new(),
        capture_methods_observed: Vec::new(),
    };

    let json = live_loop_summary_json(100, &prepared, &stats, None, false, None, None);

    assert!(json.contains("\"event\":\"session_summary\""));
    assert!(json.contains("\"frames_requested\":100"));
    assert!(json.contains("\"frames_processed\":100"));
    assert!(json.contains("\"window_lookup_ms\":612"));
    assert!(json.contains("\"window_lookup_in_hotpath\":false"));
    assert!(json.contains("\"hotpath_min_ms\":7"));
    assert!(json.contains("\"hotpath_avg_ms\":10.0"));
    assert!(json.contains("\"hotpath_max_ms\":14"));
    assert!(json.contains("\"capture_avg_ms\":4.0"));
    assert!(json.contains("\"detection_avg_ms\":6.0"));
    assert!(json.contains("\"match_avg_ms\":5.0"));
    assert!(json.contains("\"output_write_avg_ms\":0.0"));
}

#[test]
fn live_loop_frame_timings_can_report_cached_target_without_lookup() {
    let json = live_timings_json(&LiveTimings {
        window_lookup_ms: 0,
        window_lookup_in_hotpath: false,
        capture_target_source: CaptureTargetSource::Cached,
        hotpath_total_ms: 11,
        total_ms: 11,
        ..LiveTimings::default()
    });

    assert_eq!(json["window_lookup_ms"], 0);
    assert_eq!(json["window_lookup_in_hotpath"], false);
    assert_eq!(json["capture_target_source"], "cached");
    assert_eq!(json["hotpath_total_ms"], 11);
    assert_eq!(json["total_ms"], 11);
}

#[test]
fn overlay_json_reports_headless_noop_limitation() {
    let status = OverlayStatus {
        backend: d4lc_overlay_x11::OverlayBackend::HeadlessNoop,
        enabled: true,
        window_geometry: Rect {
            x: 10,
            y: 20,
            width: 300,
            height: 200,
        },
        interaction_zone: Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        },
        menu_locked: true,
        menu_visible: false,
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
        limitation: Some("not a real window yet".to_string()),
    };

    let json = overlay_json(Some(&status));

    assert_eq!(json["enabled"], true);
    assert_eq!(json["backend"], "headless_noop");
    assert!(json["interaction_zone"].is_object());
    assert_eq!(json["menu_locked"], true);
    assert_eq!(json["menu_visible"], false);
    assert_eq!(json["transparent"], false);
    assert_eq!(json["always_on_top"], false);
    assert_eq!(json["click_through"], false);
    assert!(json["limitation"]
        .as_str()
        .is_some_and(|limitation| limitation.contains("not a real window yet")));
}

#[test]
fn path_value_is_none_for_disabled_debug_outputs() {
    assert_eq!(path_value(None), None);
}

#[test]
fn capture_result_json_keeps_marker_hotpath_pixels_with_planned_content_roi() {
    let frame = CaptureFrame {
        width: 300,
        height: 420,
        stride: 1200,
        pixels_bgra: vec![0; 300 * 420 * 4],
    };
    let capture = CaptureResult {
        backend: CaptureBackend::X11Native,
        capture_method: d4lc_capture::CaptureMethod::X11NativeXShm,
        fallback_reason: None,
        source: CaptureSource::ManualRegion(Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }),
        geometry: Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        scale: d4lc_capture::ScaleFactor::one(),
        requested_capture_rect: Rect {
            x: 700,
            y: 620,
            width: 300,
            height: 420,
        },
        actual_capture_rect: Rect {
            x: 700,
            y: 620,
            width: 300,
            height: 420,
        },
        roi_origin: d4lc_capture::RoiOrigin::WindowRelative,
        capture_scope: CaptureScopeResult::Roi,
        overlay_window: OverlayWindowState::Ignored,
        frame,
    };
    let roi_plan = RoiPlan {
        capture_rect: Some(capture.actual_capture_rect),
        detection_rect: Some(capture.actual_capture_rect),
        planned_tooltip_rect: Some(Rect {
            x: 761,
            y: 0,
            width: 520,
            height: 807,
        }),
        role: RoiRole::TooltipContent,
        fallback_reason: None,
    };

    let json = capture_result_json(&capture, &roi_plan);

    assert_eq!(json["requested_capture_width"], 300);
    assert_eq!(json["requested_capture_height"], 420);
    assert_eq!(json["captured_pixels"], 126000);
    assert_eq!(json["marker_roi_pixels"], 126000);
    assert_eq!(json["tooltip_roi_pixels"], 419640);
    assert_eq!(
        json["planned_tooltip_roi_source"],
        "marker_anchored_estimate"
    );
    assert_eq!(json["planned_tooltip_roi_confidence"], "estimated");
    assert_eq!(json["overlay_window"], "ignored");
}

// --- OCR worker architecture and JSON shape ---

#[test]
fn live_loop_summary_json_includes_ocr_worker_counters() {
    let prepared = test_prepared_runtime(CaptureTargetSource::Cached, LiveSetupTimings::default());
    let stats = LiveLoopStats::default();
    let ocr_stats = OcrWorkerStats {
        submitted: 10,
        completed: 8,
        dropped: 2,
        failed: 1,
        pending: false,
        in_progress: true,
        total_ocr_ms: 800,
        total_capture_ms: 160,
        total_conversion_ms: 24,
        total_splitter_detection_ms: 40,
        max_queue_depth: 2,
        total_busy_ms: 1100,
        max_busy_ms: 240,
        last_error: Some("Tesseract init failed".to_string()),
    };

    let json = live_loop_summary_json(50, &prepared, &stats, None, true, Some(&ocr_stats), None);

    assert!(json.contains("\"ocr_worker_enabled\":true"));
    assert!(json.contains("\"ocr_jobs_submitted\":10"));
    assert!(json.contains("\"ocr_jobs_completed\":8"));
    assert!(json.contains("\"ocr_jobs_dropped\":2"));
    assert!(json.contains("\"ocr_jobs_failed\":1"));
    assert!(json.contains("\"ocr_worker_pending\":false"));
    assert!(json.contains("\"ocr_worker_in_progress\":true"));
    assert!(json.contains("\"ocr_avg_ms\":100.0"));
    assert!(json.contains("\"ocr_capture_avg_ms\":20.0"));
    assert!(json.contains("\"ocr_conversion_avg_ms\":3.0"));
    assert!(json.contains("\"ocr_splitter_detection_avg_ms\":5.0"));
    assert!(json.contains("\"last_ocr_error\":\"Tesseract init failed\""));
    assert!(json.contains("\"last_ocr_preview\":null"));
}

#[test]
fn live_loop_summary_json_without_ocr_worker_reports_zeros() {
    let prepared = test_prepared_runtime(CaptureTargetSource::Cached, LiveSetupTimings::default());
    let stats = LiveLoopStats::default();

    let json = live_loop_summary_json(50, &prepared, &stats, None, false, None, None);

    assert!(json.contains("\"ocr_worker_enabled\":false"));
    assert!(json.contains("\"ocr_jobs_submitted\":0"));
    assert!(json.contains("\"ocr_jobs_completed\":0"));
    assert!(json.contains("\"ocr_jobs_dropped\":0"));
    assert!(json.contains("\"ocr_jobs_failed\":0"));
    assert!(json.contains("\"ocr_worker_pending\":false"));
    assert!(json.contains("\"ocr_worker_in_progress\":false"));
    assert!(json.contains("\"ocr_avg_ms\":null"));
    assert!(json.contains("\"ocr_capture_avg_ms\":null"));
    assert!(json.contains("\"ocr_conversion_avg_ms\":null"));
    assert!(json.contains("\"ocr_splitter_detection_avg_ms\":null"));
    assert!(json.contains("\"last_ocr_error\":null"));
}

#[test]
fn summary_only_flag_still_emits_session_summary() {
    // Verify that the summary-only mode includes OCR worker counters in the
    // final session_summary even when per-frame events are suppressed.
    let prepared = test_prepared_runtime(CaptureTargetSource::Cached, LiveSetupTimings::default());
    let stats = LiveLoopStats::default();
    let json = live_loop_summary_json(10, &prepared, &stats, None, true, None, None);

    assert!(json.contains("\"event\":\"session_summary\""));
    assert!(json.contains("\"ocr_worker_enabled\":true"));
    assert!(json.contains("\"ocr_jobs_submitted\":0"));
}

#[test]
fn hotpath_total_ms_does_not_include_ocr_time() {
    // The hotpath_total_ms field must be set before OCR submission.
    // We verify this by checking that the live timings JSON never
    // includes a field named "ocr_ms" inside the "timings" object.
    let timings = LiveTimings {
        hotpath_total_ms: 5,
        ..LiveTimings::default()
    };
    let json = live_timings_json(&timings);

    assert_eq!(json["hotpath_total_ms"], 5);
    // OCR timing is never part of the per-frame hotpath timings block.
    assert!(json.get("ocr_ms").is_none());
}
