use super::*;
use d4lc_capture::CaptureSource;

#[test]
fn detection_roi_frame_uses_native_roi_origin() {
    let frame = CaptureFrame {
        width: 100,
        height: 50,
        stride: 400,
        pixels_bgra: vec![0; 100 * 50 * 4],
    };
    let capture = CaptureResult {
        backend: CaptureBackend::X11Native,
        capture_method: d4lc_capture::CaptureMethod::X11NativeXGetImage,
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
            x: 300,
            y: 200,
            width: 100,
            height: 50,
        },
        actual_capture_rect: Rect {
            x: 300,
            y: 200,
            width: 100,
            height: 50,
        },
        roi_origin: d4lc_capture::RoiOrigin::WindowRelative,
        capture_scope: CaptureScopeResult::Roi,
        overlay_window: OverlayWindowState::Ignored,
        frame,
    };

    let roi = detection_roi_frame(
        &capture,
        &RoiPlan {
            capture_rect: Some(capture.actual_capture_rect),
            detection_rect: Some(capture.actual_capture_rect),
            planned_tooltip_rect: None,
            role: RoiRole::TooltipContent,
            fallback_reason: None,
        },
    );

    assert_eq!(roi.origin, Point { x: 300, y: 200 });
    assert_eq!(roi.full_width, 1920);
    assert_eq!(roi.full_height, 1080);
}
