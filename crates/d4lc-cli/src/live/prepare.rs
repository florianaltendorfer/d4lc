use super::overlay::LiveOverlayDescriptorState;
use super::{elapsed_ms, LiveArgs};
use crate::detect::{prepare_detector, resolve_detection_config};
use crate::roots::Roots;
use d4lc_capture::{
    capture_reference_with_backend_and_request, read_cursor_state, resolve_capture_reference,
    CaptureBackend, CaptureReference, CaptureRequest, CaptureResult, CaptureScope,
    CaptureScopeResult, CaptureSource, CursorState, WindowQuery,
};
use d4lc_core::{InputMode, Rect};
use d4lc_detect::prepared::PreparedDetector;
use d4lc_detect::{compute_roi, RoiPlan, RoiRole};
use serde_json::Value;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PreparedLiveRuntime {
    pub(super) detector: PreparedDetector,
    pub(super) capture_backend: CaptureBackend,
    pub(super) capture_scope: CaptureScope,
    pub(super) capture_target: ResolvedLiveCaptureTarget,
    pub(super) overlay_descriptor_state: Option<LiveOverlayDescriptorState>,
    /// `OverlayFontSize`, bounded: the size of every overlay text.
    pub(super) overlay_font_px: u32,
    pub(super) setup_timings: LiveSetupTimings,
    /// Where the settings came from (`config` of the reports).
    pub(super) config: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResolvedLiveCaptureTarget {
    pub(super) reference: CaptureReference,
    pub(super) source: CaptureTargetSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum CaptureTargetSource {
    #[default]
    DiscoveredOnce,
    Cached,
    ExplicitManualRegion,
    PortalStream,
}

impl CaptureTargetSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DiscoveredOnce => "discovered_once",
            Self::Cached => "cached",
            Self::ExplicitManualRegion => "explicit_manual_region",
            Self::PortalStream => "portal_stream",
        }
    }
}

impl std::fmt::Display for CaptureTargetSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct LiveSetupTimings {
    pub(super) config_ms: u128,
    pub(super) window_lookup_ms: u128,
    pub(super) template_prepare_ms: u128,
}

impl LiveSetupTimings {
    pub(super) const fn setup_ms(self) -> u128 {
        self.config_ms + self.window_lookup_ms + self.template_prepare_ms
    }
}

pub(super) fn prepare_live_runtime(
    roots: &Roots,
    args: &LiveArgs,
) -> Result<PreparedLiveRuntime, String> {
    let config_start = Instant::now();
    let (resolved, config) = resolve_detection_config(
        roots,
        args.preset.clone(),
        args.language.clone(),
        &args.controller_config,
        args.mode.as_deref(),
        args.threshold_min,
        args.threshold_max,
        args.threshold_similarity_tooltip,
    )?;
    let overlay_font_px = d4lc_config::bounded_overlay_font_size(
        roots
            .settings()
            .map_err(|error| format!("settings: {error}"))?
            .overlay_font_size,
    );
    let config_ms = elapsed_ms(config_start);
    let capture_backend = match args.backend.as_deref() {
        Some(value) => CaptureBackend::parse(value)?,
        None => CaptureBackend::Auto,
    };
    let capture_scope = match args.capture_scope.as_deref() {
        Some(value) => CaptureScope::parse(value)?,
        None => CaptureScope::Auto,
    };
    let lookup_start = Instant::now();
    let capture_target = resolve_live_capture_target(&args.window_query(), args.manual_region)?;
    let window_lookup_ms = elapsed_ms(lookup_start);
    let prepare_start = Instant::now();
    let detector = prepare_detector(roots, resolved)?;
    let template_prepare_ms = elapsed_ms(prepare_start);

    Ok(PreparedLiveRuntime {
        detector,
        capture_backend,
        capture_scope,
        capture_target,
        overlay_descriptor_state: None,
        overlay_font_px,
        setup_timings: LiveSetupTimings {
            config_ms,
            window_lookup_ms,
            template_prepare_ms,
        },
        config,
    })
}

fn resolve_live_capture_target(
    query: &WindowQuery,
    manual_region: Option<Rect>,
) -> Result<ResolvedLiveCaptureTarget, String> {
    let reference = resolve_capture_reference(query, manual_region)?;
    let source = match &reference.source {
        CaptureSource::Window(_) => CaptureTargetSource::DiscoveredOnce,
        CaptureSource::ManualRegion(_) => CaptureTargetSource::ExplicitManualRegion,
        // The CLI resolves X11 targets only; a portal stream reference
        // cannot be produced here (the Wayland frame source is the app's
        // runtime dispatch), but the mapping stays total and.
        CaptureSource::PortalStream(_) => CaptureTargetSource::PortalStream,
    };

    Ok(ResolvedLiveCaptureTarget { reference, source })
}

pub(super) fn reported_capture_target_source(
    prepared: &PreparedLiveRuntime,
    setup_timings: LiveSetupTimings,
) -> CaptureTargetSource {
    match prepared.capture_target.source {
        CaptureTargetSource::DiscoveredOnce if setup_timings.window_lookup_ms == 0 => {
            CaptureTargetSource::Cached
        }
        source => source,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CapturedLiveFrame {
    pub(super) capture: CaptureResult,
    pub(super) capture_ms: u128,
    pub(super) roi_plan_ms: u128,
    pub(super) window_lookup_ms: u128,
    pub(super) hotpath_start: Instant,
    pub(super) roi_plan: RoiPlan,
    pub(super) cursor: Option<CursorState>,
}

pub(super) fn capture_live_frame(
    prepared: &PreparedLiveRuntime,
) -> Result<CapturedLiveFrame, String> {
    let hotpath_start = Instant::now();
    let plan_start = hotpath_start;
    let plan = plan_live_capture(prepared)?;
    let roi_plan_ms = elapsed_ms(plan_start);
    let capture_start = Instant::now();
    let capture = capture_reference_with_backend_and_request(
        &prepared.capture_target.reference,
        prepared.capture_backend,
        plan.request,
    )?;
    if matches!(prepared.capture_scope, CaptureScope::Roi)
        && !matches!(capture.capture_scope, CaptureScopeResult::Roi)
    {
        return Err(format!(
            "--capture-scope roi requires true ROI capture; backend {} reported {}{}",
            capture.backend,
            capture.capture_scope,
            capture
                .fallback_reason
                .as_deref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default()
        ));
    }
    let capture_ms = elapsed_ms(capture_start);

    Ok(CapturedLiveFrame {
        capture,
        capture_ms,
        roi_plan_ms,
        window_lookup_ms: plan.window_lookup_ms,
        hotpath_start,
        roi_plan: plan.roi_plan,
        cursor: plan.cursor,
    })
}

#[derive(Debug, Clone, PartialEq)]
struct LiveCapturePlan {
    request: CaptureRequest,
    roi_plan: RoiPlan,
    cursor: Option<CursorState>,
    window_lookup_ms: u128,
}

fn plan_live_capture(prepared: &PreparedLiveRuntime) -> Result<LiveCapturePlan, String> {
    if matches!(prepared.capture_scope, CaptureScope::Full) {
        return Ok(LiveCapturePlan {
            request: CaptureRequest::Full,
            roi_plan: RoiPlan {
                role: RoiRole::FullFrame,
                ..RoiPlan::default()
            },
            cursor: None,
            window_lookup_ms: 0,
        });
    }

    let window_lookup_ms = 0;
    let reference_rect = prepared.capture_target.reference.geometry;

    let (roi_plan, cursor) = match prepared.detector.mode {
        InputMode::Controller => (
            full_frame_detection_roi_plan(reference_rect.width, reference_rect.height),
            None,
        ),
        InputMode::Mouse => {
            let cursor = read_cursor_state(Some(reference_rect))?;
            if cursor.window_relative.is_none() {
                return Err(
                    "mouse mode requires a cursor position relative to the selected reference"
                        .to_string(),
                );
            }
            (
                full_frame_detection_roi_plan(reference_rect.width, reference_rect.height),
                Some(cursor),
            )
        }
    };

    let capture_rect = roi_plan
        .capture_rect
        .ok_or_else(|| "ROI capture plan did not produce a capture rectangle".to_string())?;

    Ok(LiveCapturePlan {
        request: CaptureRequest::WindowRelativeRoi(capture_rect),
        roi_plan,
        cursor,
        window_lookup_ms,
    })
}

/// Detection ROI plan for the captured frame.
///
/// Upstream `ScreenProcessHandler.FindTooltips` matches tooltip templates over
/// the captured screen; the port keeps that whole-capture search region in both
/// input modes so a marker outside the cursor's neighbourhood is still found
/// (see `d4lc_detect::compute_roi`).
fn full_frame_detection_roi_plan(frame_width: u32, frame_height: u32) -> RoiPlan {
    let detection_rect = compute_roi(frame_width, frame_height);
    RoiPlan {
        capture_rect: Some(detection_rect),
        detection_rect: Some(detection_rect),
        planned_tooltip_rect: None,
        role: RoiRole::FullFrame,
        fallback_reason: None,
    }
}

#[cfg(test)]
mod tests;
