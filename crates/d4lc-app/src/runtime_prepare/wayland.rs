//! Wayland runtime preparation: xdg-desktop-portal ScreenCast session +
//! PipeWire frame source, restore-token rotation, stream capture references.

use std::path::Path;
use std::time::Duration;

use d4lc_capture::wayland::RestoreTokenUpdate;
use d4lc_capture::CaptureReference;
use d4lc_core::Rect;
use d4lc_runtime::{PreparedRuntime, RuntimeSession};

use super::{
    build_detector_and_ocr_worker, AppRuntimePrepareError, PrepareProgress, ReusableCapture,
    RuntimeConfig,
};

/// The portal refusal of [`prepare_app_runtime_without_portal`](super::prepare_app_runtime_without_portal).
pub(super) fn refuse_forbidden_portal(
    portal_forbidden_for: Option<&str>,
) -> Result<(), AppRuntimePrepareError> {
    match portal_forbidden_for {
        Some(caller) => Err(AppRuntimePrepareError::PortalSessionNotAllowed(
            caller.to_string(),
        )),
        None => Ok(()),
    }
}

/// Wayland runtime preparation: portal session + PipeWire frame source.
///
/// Order matters: templates and the OCR worker are prepared first so a
/// broken preset fails before the compositor consent dialog appears.
/// Window discovery is deliberately skipped — title/pid lookup has no
/// Wayland equivalent; the user selects the shared source in the
/// compositor dialog (or a restore token restores it silently).
///
/// The prepared capture reference is the
/// [`CaptureSource::PortalStream`](d4lc_capture::CaptureSource::PortalStream)
/// rect the connected session delivers: the overlay start and the paragon
/// geometry refresh follow it instead of an X11 window query (which is
/// blind on a Wayland session). It is not captured through — frames come
/// from the stream, and OCR jobs crop the planned tooltip ROI from the
/// latest stream frame via the same shared source.
///
/// On success the single-use restore token is rotated: the portal hands
/// out a fresh token per successful session, which is persisted so the
/// next start can restore without a dialog. A persistence failure is
/// non-fatal (the next start simply shows the dialog again) and is
/// reported on stderr — there is no warning channel in preparation.
///
/// A settings re-preparation hands the running session in `reuse`: the
/// runtime is rebuilt around it (new templates, OCR worker and settings)
/// without a portal request, a dialog, or a token rotation.
pub(super) fn prepare_wayland_runtime(
    config_root: &Path,
    config: &RuntimeConfig,
    reuse: ReusableCapture,
    progress: &dyn Fn(PrepareProgress),
) -> Result<(PreparedRuntime, RuntimeSession), AppRuntimePrepareError> {
    let (detector, ocr_worker) = build_detector_and_ocr_worker(config_root, config)?;

    let (frame_source, capture_ref) = match reuse.wayland {
        Some(running) => {
            progress(PrepareProgress::ReusingScreenCastSession);
            running
        }
        None => connect_wayland_capture(config_root, config, progress)?,
    };

    // The loop paces at the negotiated stream rate (at most
    // 60 Hz); a variable rate without a maximum keeps the announced 60 Hz
    // fallback.
    let pacing = frame_source
        .negotiated_rate()
        .and_then(|rate| rate.max_hz())
        .map(|hz| d4lc_runtime::FramePacing::from_hz(hz, d4lc_runtime::PacingSource::StreamRate))
        .unwrap_or_else(d4lc_runtime::FramePacing::fallback);
    let runtime = PreparedRuntime::new(config.capture_backend, capture_ref, detector)
        .with_shared_wayland_capture_source(frame_source)
        .with_frame_pacing(pacing);
    let session = RuntimeSession::new(Some(ocr_worker), config.ocr_interval);

    Ok((runtime, session))
}

/// Establish a new portal session, persist the restore-token rotation and
/// wait for the stream's first frame; returns the source and the capture
/// reference of its stream rect.
fn connect_wayland_capture(
    config_root: &Path,
    config: &RuntimeConfig,
    progress: &dyn Fn(PrepareProgress),
) -> Result<
    (
        std::sync::Arc<d4lc_capture::WaylandFrameSource>,
        CaptureReference,
    ),
    AppRuntimePrepareError,
> {
    use d4lc_capture::wayland::{failed_restore_token_rotation, RestoreTokenUpdate};
    use d4lc_capture::{WaylandFrameSource, WaylandFrameSourceConfig};

    progress(PrepareProgress::AwaitingScreenCastSelection {
        restore_token_offered: config.wayland_restore_token.is_some(),
    });
    let connected = WaylandFrameSource::connect(WaylandFrameSourceConfig {
        stored_restore_token: config.wayland_restore_token.clone(),
        ..WaylandFrameSourceConfig::default()
    });
    let (frame_source, token_update) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            // Restore-token hygiene for the failed attempt: a failure at
            // or after Start consumed the stored single-use token, so it
            // is discarded and the next start performs a fresh dialog
            // instead of hammering a dead token. A failure before Start
            // never reached the portal with the token and keeps the
            // stored value.
            let rotation =
                failed_restore_token_rotation(config.wayland_restore_token.clone(), &error);
            persist_restore_token_rotation(config_root, &rotation);
            let mut reason = format!("{error} (missing piece: {})", error.missing_piece());
            if matches!(rotation, RestoreTokenUpdate::Clear) {
                reason.push_str(
                    "; the stored restore token was discarded — the next start shows \
                     the compositor selection dialog again",
                );
            }
            return Err(AppRuntimePrepareError::WaylandCaptureFailed(reason));
        }
    };

    persist_restore_token_rotation(config_root, &token_update);

    // The stream rect is only known once the stream delivers its first
    // converted buffer (readiness is reported at connect, before the first
    // frame). Waiting here once keeps the reference consistent with what
    // the per-frame capture results report as geometry; a stream that
    // cannot deliver a frame is a structured preparation failure instead
    // of a loop that errors on every frame.
    progress(PrepareProgress::AwaitingFirstStreamFrame);
    let first_frame = frame_source
        .wait_for_first_frame(WAYLAND_FIRST_FRAME_TIMEOUT)
        .map_err(|error| {
            AppRuntimePrepareError::WaylandCaptureFailed(format!(
                "the ScreenCast stream delivered no frame: {error}"
            ))
        })?;
    let capture_ref = portal_stream_reference(first_frame.width, first_frame.height);

    Ok((std::sync::Arc::new(frame_source), capture_ref))
}

/// The capture reference of a ScreenCast stream delivering frames of
/// `width` × `height`.
///
/// The compositor-selected stream is the capture target. The rect is in
/// stream space (origin 0,0), the same geometry the stream's frame
/// results report — a resolution change is a legitimate coordinator
/// window revision. The portal-reported monitor position (multi-monitor
/// origin) is not part of the delivered frame rect; the overlay start
/// reads it from the frame source to pick the output the stream shows.
pub(super) fn portal_stream_reference(width: u32, height: u32) -> CaptureReference {
    let stream_rect = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    CaptureReference {
        source: d4lc_capture::CaptureSource::PortalStream(stream_rect),
        geometry: stream_rect,
    }
}

/// The capture reference a settings reload carries over for a running
/// stream: the stream rect of the newest delivered frame, so the reloaded
/// runtime reports the same geometry the stream's frame results do. With
/// no delivered size (not reachable after preparation waited for the
/// first frame) the running reference is kept.
pub(super) fn reused_stream_reference(
    running: &CaptureReference,
    latest_frame_size: Option<(u32, u32)>,
) -> CaptureReference {
    match latest_frame_size {
        Some((width, height)) => portal_stream_reference(width, height),
        None => running.clone(),
    }
}

/// How long Wayland preparation waits for the portal stream's first
/// converted frame. Matches the frame source's own readiness bound: the
/// compositor delivers the initial buffer at stream start, so the wait is
/// bounded and a dead stream fails preparation.
const WAYLAND_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// Persist one restore-token rotation. A persistence failure is
/// non-fatal (the next start simply shows the dialog again) and is
/// reported on stderr — there is no warning channel in preparation.
fn persist_restore_token_rotation(config_root: &Path, rotation: &RestoreTokenUpdate) {
    let persisted = match rotation {
        RestoreTokenUpdate::Store(token) => {
            d4lc_config::save_wayland_restore_token(config_root, Some(token))
        }
        RestoreTokenUpdate::Clear => d4lc_config::save_wayland_restore_token(config_root, None),
        RestoreTokenUpdate::Keep => Ok(()),
    };
    if let Err(error) = persisted {
        eprintln!("wayland restore-token persistence failed: {error}");
    }
}
