use super::prepare::{
    CaptureTargetSource, LiveSetupTimings, PreparedLiveRuntime, ResolvedLiveCaptureTarget,
};
use d4lc_capture::{
    CaptureBackend, CaptureReference, CaptureScope, CaptureSource, WindowCandidate,
};
use d4lc_core::{InputMode, Rect};
use d4lc_detect::prepared::PreparedDetector;

pub(super) fn test_prepared_runtime(
    source: CaptureTargetSource,
    setup_timings: LiveSetupTimings,
) -> PreparedLiveRuntime {
    let reference_rect = Rect {
        x: 1920,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let reference_source = match source {
        CaptureTargetSource::ExplicitManualRegion => CaptureSource::ManualRegion(reference_rect),
        CaptureTargetSource::DiscoveredOnce | CaptureTargetSource::Cached => {
            CaptureSource::Window(WindowCandidate {
                info: d4lc_core::WindowInfo {
                    id: 42,
                    title: "Diablo IV".to_string(),
                    class_name: Some("steam_app".to_string()),
                    rect: reference_rect,
                },
                pid: Some(1234),
                is_active: true,
                matches_query: true,
            })
        }
        // The CLI cannot produce a portal stream target; the fixture
        // maps it to the stream-shaped manual region so the match
        // stays total.
        CaptureTargetSource::PortalStream => CaptureSource::PortalStream(reference_rect),
    };

    PreparedLiveRuntime {
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
                source: reference_source,
                geometry: reference_rect,
            },
            source,
        },
        overlay_descriptor_state: None,
        overlay_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        setup_timings,
        config: serde_json::Value::Null,
    }
}
