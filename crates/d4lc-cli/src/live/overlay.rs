use super::frame::ProcessedFrame;
use super::prepare::PreparedLiveRuntime;
use super::LiveArgs;
use crate::roots::Roots;
use d4lc_capture::{CaptureSource, OverlayWindowState};
use d4lc_overlay_x11::{OverlayBackend, OverlayConfig, OverlayFrame, OverlayHandle, OverlayStatus};
use d4lc_rules::{
    build_live_overlay_descriptors, ParagonOverlayLayout, ParagonOverlayState,
    RuleEvaluationResult, RuleOverlayLayout, RuleOverlaySettings,
};
use d4lc_runtime::ProcessedFrameResult;

/// The runtime's overlay frame with the CLI's rule descriptors attached.
pub(super) fn build_live_overlay_frame_from_result(
    result: &ProcessedFrameResult,
    overlay_state: Option<&LiveOverlayDescriptorState>,
    text_font_px: u32,
) -> OverlayFrame {
    OverlayFrame {
        rule_descriptors: live_overlay_descriptors(overlay_state),
        text_font_px,
        ..result.overlay_frame.clone()
    }
}

/// The overlay start a persisted display-backend token resolves to in this
/// binary: `auto` (the default) and `x11` create the X11 overlay; `wayland`
/// names the layer-shell backend this binary does not contain.
#[derive(Debug)]
enum OverlayBackendStart {
    X11,
    /// A wayland selection in a binary that does not contain the
    /// `d4lc-overlay-wayland` layer-shell backend (the CLI links only the
    /// X11 overlay crate). Carries the structured unavailability reason —
    /// the same rule the app runtime preparation applies to a
    /// Wayland selection without the capture feature: surface the missing
    /// build piece instead of silently creating the X11 overlay while the
    /// setting claims Wayland.
    WaylandUnavailable {
        reason: String,
    },
}

/// Resolve the persisted display-backend token to the CLI overlay start.
///
/// Token parsing runs through the shared `d4lc_config::DisplayBackend`
/// parser — the same single parser the app's display-backend
/// selection seam and overlay start use — so both binaries accept exactly
/// `auto|x11|wayland` (whitespace-tolerant) and reject every other value
/// with the same structured error, never a silent fallback to X11. The
/// mapping to this binary's overlay start stays CLI-owned: the headless
/// CLI links no layer-shell backend, so a wayland selection resolves to
/// the structured unavailability naming the desktop app build.
fn resolve_overlay_backend_start(
    display_backend_setting: &str,
) -> Result<OverlayBackendStart, String> {
    match d4lc_config::DisplayBackend::parse(display_backend_setting)? {
        d4lc_config::DisplayBackend::Auto | d4lc_config::DisplayBackend::X11 => {
            Ok(OverlayBackendStart::X11)
        }
        d4lc_config::DisplayBackend::Wayland => Ok(OverlayBackendStart::WaylandUnavailable {
            reason: "the wayland layer-shell overlay backend is not part of this binary; \
                     wayland overlay support ships in the desktop app build"
                .to_string(),
        }),
    }
}

pub(super) fn create_live_overlay(
    roots: &Roots,
    args: &LiveArgs,
    prepared: &PreparedLiveRuntime,
) -> Result<Option<OverlayHandle>, String> {
    if !args.overlay {
        return Ok(None);
    }

    let settings = roots.settings().map_err(|e| format!("overlay: {e}"))?;
    let config = OverlayConfig {
        window_geometry: prepared.capture_target.reference.geometry,
        show_overlay_icon: settings.show_overlay_icon,
        overlay_icon_pos_x: settings.overlay_icon_pos_x,
        overlay_icon_pos_y: settings.overlay_icon_pos_y,
        menu_locked: true,
    };

    let backend = resolve_overlay_backend_start(&settings.display_backend)
        .map_err(|e| format!("overlay: {e}"))?;
    match backend {
        OverlayBackendStart::X11 => d4lc_overlay_x11::create_overlay(config).map(Some),
        OverlayBackendStart::WaylandUnavailable { reason } => Err(format!("overlay: {reason}")),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LiveOverlayDescriptorState {
    rule: Option<LiveRuleOverlayState>,
    paragon: Option<LiveParagonOverlayState>,
}

#[derive(Debug, Clone, PartialEq)]
struct LiveRuleOverlayState {
    result: RuleEvaluationResult,
    settings: RuleOverlaySettings,
    layout: RuleOverlayLayout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveParagonOverlayState {
    state: ParagonOverlayState,
    layout: ParagonOverlayLayout,
}

pub(super) fn overlay_window_state_for_capture(
    status: Option<&OverlayStatus>,
    source: &CaptureSource,
) -> OverlayWindowState {
    let Some(status) = status else {
        return OverlayWindowState::Ignored;
    };

    if !status.enabled || matches!(status.backend, OverlayBackend::Failed) {
        return OverlayWindowState::Hidden;
    }

    match status.backend {
        OverlayBackend::RealX11 => match source {
            CaptureSource::Window(_) => OverlayWindowState::Excluded,
            CaptureSource::ManualRegion(_) => OverlayWindowState::Visible,
            // A monitor stream captures the whole output including the
            // overlay surface: no exclusion protocol exists on Wayland, so
            // the state is visible.
            CaptureSource::PortalStream(_) => OverlayWindowState::Visible,
        },
        // The Wayland layer surface carries the same capture semantics as the
        // real X11 overlay window: excluded from window-scoped capture, visible
        // to manual regions.
        OverlayBackend::RealWayland => match source {
            CaptureSource::Window(_) => OverlayWindowState::Excluded,
            CaptureSource::ManualRegion(_) => OverlayWindowState::Visible,
            CaptureSource::PortalStream(_) => OverlayWindowState::Visible,
        },
        OverlayBackend::HeadlessNoop => OverlayWindowState::Ignored,
        OverlayBackend::Failed => OverlayWindowState::Hidden,
    }
}

pub(super) fn build_live_overlay_frame(
    processed: &ProcessedFrame,
    tooltip_found: bool,
    overlay_state: Option<&LiveOverlayDescriptorState>,
    text_font_px: u32,
) -> OverlayFrame {
    OverlayFrame {
        sequence: processed.sequence,
        tooltip_found,
        hotpath_ms: processed.timings.hotpath_total_ms,
        cursor_window_relative: processed
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.window_relative),
        rule_descriptors: live_overlay_descriptors(overlay_state),
        text_font_px,
    }
}

fn live_overlay_descriptors(
    overlay_state: Option<&LiveOverlayDescriptorState>,
) -> std::sync::Arc<[d4lc_rules::RuleOverlayDescriptor]> {
    overlay_state
        .map(|state| {
            build_live_overlay_descriptors(
                state
                    .rule
                    .as_ref()
                    .map(|rule| (&rule.result, &rule.settings, &rule.layout)),
                state
                    .paragon
                    .as_ref()
                    .map(|paragon| (&paragon.state, &paragon.layout)),
                &d4lc_overlay_x11::text::SystemTextMeasure,
            )
            .into()
        })
        .unwrap_or_else(|| std::sync::Arc::from([]))
}

#[cfg(test)]
mod tests;
