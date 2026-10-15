use super::*;
use crate::live::test_support::test_prepared_runtime;
use d4lc_core::Point;

#[test]
fn live_frame_planning_accepts_cached_capture_target() {
    let reference_rect = Rect {
        x: 1920,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let prepared = PreparedLiveRuntime {
        detector: PreparedDetector {
            preset: "1080p_SMF".to_string(),
            mode: InputMode::Controller,
            scan_height_percent: 40.0,
            thresholds: d4lc_detect::ThresholdSettings {
                min: 0,
                max: 255,
                similarity_tooltip: 0.05,
            },
            tooltip_width: 375,
            templates: Vec::new(),
            scan: d4lc_detect::prepared::DetectScan::default(),
        },
        capture_backend: CaptureBackend::X11Native,
        capture_scope: CaptureScope::Auto,
        capture_target: ResolvedLiveCaptureTarget {
            reference: CaptureReference {
                source: CaptureSource::ManualRegion(reference_rect),
                geometry: reference_rect,
            },
            source: CaptureTargetSource::Cached,
        },
        overlay_descriptor_state: None,
        overlay_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        setup_timings: LiveSetupTimings::default(),
        config: serde_json::Value::Null,
    };

    let plan = plan_live_capture(&prepared)
        .unwrap_or_else(|error| panic!("cached capture target planning failed: {error}"));

    assert_eq!(plan.window_lookup_ms, 0);
    assert_eq!(prepared.capture_target.source, CaptureTargetSource::Cached);
    assert_eq!(plan.roi_plan.role, RoiRole::FullFrame);
    assert_eq!(
        plan.roi_plan.detection_rect,
        Some(full_frame_rect(reference_rect))
    );
    match plan.request {
        CaptureRequest::WindowRelativeRoi(rect) => {
            assert_eq!(rect, full_frame_rect(reference_rect));
        }
        other => panic!("expected window-relative ROI capture request, got {other:?}"),
    }
}

fn full_frame_rect(reference_rect: Rect) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: reference_rect.width,
        height: reference_rect.height,
    }
}

#[test]
fn live_loop_frames_report_cached_capture_target_after_session_lookup() {
    let prepared = test_prepared_runtime(
        CaptureTargetSource::DiscoveredOnce,
        LiveSetupTimings {
            config_ms: 1,
            window_lookup_ms: 600,
            template_prepare_ms: 1,
        },
    );

    assert_eq!(
        reported_capture_target_source(&prepared, LiveSetupTimings::default()),
        CaptureTargetSource::Cached
    );
    assert_eq!(
        reported_capture_target_source(&prepared, prepared.setup_timings),
        CaptureTargetSource::DiscoveredOnce
    );
}

#[test]
fn controller_mode_plans_the_whole_capture_as_the_detection_roi() {
    let reference_rect = Rect {
        x: 1920,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let plan = full_frame_detection_roi_plan(reference_rect.width, reference_rect.height);

    assert_eq!(plan.role, RoiRole::FullFrame);
    assert_eq!(plan.capture_rect, Some(full_frame_rect(reference_rect)));
    assert_eq!(plan.detection_rect, Some(full_frame_rect(reference_rect)));
    assert_eq!(plan.planned_tooltip_rect, None);
    assert_eq!(plan.fallback_reason, None);
}

#[test]
fn full_frame_detection_roi_plan_includes_observed_marker_points() {
    let plan = full_frame_detection_roi_plan(1920, 1080);
    let roi = plan
        .detection_rect
        .unwrap_or_else(|| panic!("detection ROI must be planned"));

    // Controller markers observed live at x=825, keyboard markers at
    // x=1038/1094: all inside the captured-frame search region.
    assert!(point_is_inside_rect(Point { x: 825, y: 749 }, roi));
    assert!(point_is_inside_rect(Point { x: 825, y: 940 }, roi));
    assert!(point_is_inside_rect(Point { x: 1038, y: 882 }, roi));
    assert!(point_is_inside_rect(Point { x: 1094, y: 843 }, roi));
}

fn point_is_inside_rect(point: Point, rect: Rect) -> bool {
    point.x >= rect.x
        && point.y >= rect.y
        && u32::try_from(point.x - rect.x)
            .ok()
            .is_some_and(|x| x < rect.width)
        && u32::try_from(point.y - rect.y)
            .ok()
            .is_some_and(|y| y < rect.height)
}

#[test]
fn mouse_mode_detection_roi_ignores_the_cursor_position() {
    let left = full_frame_detection_roi_plan(1920, 1080);
    let right = full_frame_detection_roi_plan(1920, 1080);

    assert_eq!(left.detection_rect, right.detection_rect);
    assert_eq!(
        left.detection_rect,
        Some(Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        })
    );
    assert_eq!(left.role, RoiRole::FullFrame);
}
