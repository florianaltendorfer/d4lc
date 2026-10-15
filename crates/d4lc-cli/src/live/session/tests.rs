use super::*;
use crate::live::frame::LiveTimings;
use d4lc_capture::{CaptureBackend, CaptureScopeResult, CaptureSource, OverlayWindowState};
use d4lc_core::{CaptureFrame, InputMode, Rect};
use d4lc_detect::prepared::DetectFrameResult;
use d4lc_detect::{RoiPlan, RoiRole};

#[test]
fn live_event_reports_found_and_cleared_transitions() {
    assert_eq!(live_event(false, true), "tooltip_found");
    assert_eq!(live_event(true, false), "tooltip_cleared");
    assert_eq!(live_event(true, true), "tooltip_present");
    assert_eq!(live_event(false, false), "tooltip_absent");
}

#[test]
fn percentile_ms_uses_nearest_rank() {
    let samples = [1, 3, 5, 7, 9];

    assert_eq!(percentile_ms(&samples, 50), 5);
    assert_eq!(percentile_ms(&samples, 95), 9);
}

#[test]
fn live_loop_stats_tracks_capture_methods_and_percentiles() {
    let mut stats = LiveLoopStats::default();

    for (capture_ms, match_ms, hotpath_ms, method) in [
        (2, 1, 5, CaptureMethod::X11NativeXShm),
        (4, 2, 9, CaptureMethod::X11NativeXShm),
        (6, 3, 12, CaptureMethod::X11Command),
    ] {
        let capture = CaptureResult {
            backend: CaptureBackend::X11Native,
            capture_method: method,
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
                x: 0,
                y: 0,
                width: 300,
                height: 420,
            },
            actual_capture_rect: Rect {
                x: 0,
                y: 0,
                width: 300,
                height: 420,
            },
            roi_origin: d4lc_capture::RoiOrigin::WindowRelative,
            capture_scope: CaptureScopeResult::Roi,
            overlay_window: OverlayWindowState::Ignored,
            frame: CaptureFrame {
                width: 300,
                height: 420,
                stride: 1200,
                pixels_bgra: vec![0; 300 * 420 * 4],
            },
        };
        let processed = ProcessedFrame {
            sequence: 1,
            dropped_frames: 0,
            capture,
            cursor: None,
            roi_plan: RoiPlan {
                capture_rect: None,
                detection_rect: None,
                planned_tooltip_rect: None,
                role: RoiRole::FullFrame,
                fallback_reason: None,
            },
            detection: DetectFrameResult {
                preset: "SteamDeck".to_string(),
                mode: InputMode::Controller,
                roi: Rect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                roi_role: RoiRole::FullFrame,
                planned_tooltip_roi: None,
                tooltip_detection: d4lc_detect::TooltipDetectionTrace {
                    search_roi: Rect {
                        x: 0,
                        y: 0,
                        width: 10,
                        height: 10,
                    },
                    detected_tooltip_rect: None,
                    final_tooltip_crop_rect: None,
                    crop_source: "none",
                    upstream_rule: d4lc_detect::UPSTREAM_TOOLTIP_CROP_RULE,
                    final_tooltip_size: None,
                },
                thresholds: d4lc_detect::ThresholdSettings {
                    min: 0,
                    max: 255,
                    similarity_tooltip: 0.05,
                },
                timings: d4lc_detect::DetectionTimings {
                    match_ms,
                    ..Default::default()
                },
                candidates: Vec::new(),
                selected: None,
                crop: None,
                trace_output_path: None,
                crop_output_path: None,
                thresholded_crop_output_path: None,
            },
            crop_stability: None,
            content_roi_capture_requested: false,
            frame_output_path: None,
            roi_output_path: None,
            planned_tooltip_roi_output_path: None,
            timings: LiveTimings {
                capture_method: method,
                capture_ms,
                match_ms,
                hotpath_total_ms: hotpath_ms,
                ..LiveTimings::default()
            },
        };
        stats.observe(&processed);
    }

    assert_eq!(stats.capture_percentiles().p50_ms, 4);
    assert_eq!(stats.capture_percentiles().p95_ms, 6);
    assert_eq!(stats.hotpath_percentiles().p95_ms, 12);
    assert_eq!(stats.capture_method_label(), "mixed");
    assert_eq!(
        stats.capture_method_labels(),
        vec!["x11-native-xshm".to_string(), "x11-command".to_string()]
    );
}
