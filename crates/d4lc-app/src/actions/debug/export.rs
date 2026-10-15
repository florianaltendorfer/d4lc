//! `ExportDebugImages`: a bounded one-shot export pass beside the running
//! live loop that writes only the artifacts the runtime pipeline actually
//! produced, into
//!
//! ```text
//! debug-output/export-<unix>/
//! ```
//!
//! The export needs the capture, detect and runtime crates the
//! `real-runtime` feature enables; without it the action is `Blocked`.
//!
//! - The bundle directory is created only after the runtime preparation
//!   succeeded; a failed preparation is `Failed` and leaves no partial
//!   bundle.
//! - The preparation never opens a ScreenCast portal session: the live
//!   loop owns the process's exclusive session, so a Wayland engine is
//!   refused by name before any portal call.
//! - On failure only `bundle-summary.json` is written, carrying the error.
//! - Images are written only when their step produced a non-empty image;
//!   no placeholder images are written.

#[cfg(feature = "real-runtime")]
use std::path::{Path, PathBuf};
#[cfg(feature = "real-runtime")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "real-runtime")]
use d4lc_core::CaptureFrame;
#[cfg(feature = "real-runtime")]
use serde_json::{json, Value};

#[cfg(feature = "real-runtime")]
use crate::runtime_bridge::DebugCommandOutcome;

/// Why the debug export cannot run on the Wayland capture engine: it
/// needs the running live loop, which owns the process's exclusive
/// ScreenCast session, and it captures its own frame through X11. Stopping
/// the loop does not help (the export then has no live runtime), so the
/// text names the gap instead of a remedy.
#[cfg(feature = "real-runtime")]
pub const WAYLAND_EXPORT_GAP: &str = "debug image export is not available on the Wayland \
     capture engine (Wayland gap): the export captures its own frame through X11 and cannot \
     open a second ScreenCast session beside the live loop, which owns it; missing piece: an \
     export from the live loop's own stream frames";

/// One-shot export pass anchored to the live loop's
/// `config_root`. Returns the confirmed `DebugCommandOutcome`.
#[cfg(feature = "real-runtime")]
pub fn attempt_real_export(config_root: &Path) -> DebugCommandOutcome {
    // 1. Prepare the one-shot `PreparedRuntime` + `RuntimeSession`
    //    first: it is the anchor of the export, and a failed prep
    //    leaves no bundle behind. It runs beside the live loop, so
    //    it must never open a ScreenCast portal session — that
    //    session is exclusive per process and owned by the loop
    //    (also while the loop is still preparing it). A Wayland
    //    engine resolution is refused by name before any portal call.
    let (runtime, mut session) = match crate::runtime_prepare::prepare_app_runtime_without_portal(
        config_root,
        WAYLAND_EXPORT_GAP,
    ) {
        Ok((runtime, session, _engine_resolution)) => (runtime, session),
        Err(crate::runtime_prepare::AppRuntimePrepareError::PortalSessionNotAllowed(gap)) => {
            return DebugCommandOutcome::Failed { reason: gap };
        }
        Err(e) => {
            return DebugCommandOutcome::Failed {
                reason: format!("runtime preparation failed: {e}"),
            };
        }
    };

    // 2. Rebuild the detector: the live loop's `PreparedDetector` is
    //    private to its runtime, so the export derives its own.
    let detector = match build_export_detector(config_root) {
        Ok(d) => d,
        Err(reason) => {
            return DebugCommandOutcome::Failed { reason };
        }
    };

    // 3. Capture a fresh frame for the bundle's `raw-capture.png` through
    //    the default window query the live loop also uses. A failure is
    //    reported; no placeholder is written.
    let window_query = crate::actions::debug::screenshot::default_window_query();
    let raw_capture = match d4lc_capture::capture_selected_window_native(&window_query) {
        Ok(c) => c,
        Err(e) => {
            return DebugCommandOutcome::Failed {
                reason: format!("capture failed: {e}"),
            };
        }
    };

    // 4. Detect the tooltip in the captured frame.
    let roi_frame = d4lc_detect::RoiFrame::full_window(&raw_capture.frame);
    let detect_result =
        match d4lc_detect::detect_roi_frame_prepared(roi_frame, &detector, 0, None, None) {
            Ok(r) => r,
            Err(e) => {
                return DebugCommandOutcome::Failed {
                    reason: format!("detection failed: {e}"),
                };
            }
        };

    // Run one frame through the separate runtime for its overlay and OCR
    // view, so the live loop's frame is not interrupted.
    let process_result = match d4lc_runtime::process_one_frame(&mut session, &runtime) {
        Ok(r) => r,
        Err(e) => {
            return DebugCommandOutcome::Failed {
                reason: format!("process_one_frame failed: {e}"),
            };
        }
    };

    // 5. Write the bundle.
    let bundle_dir = export_bundle_dir();
    match write_bundle(
        &bundle_dir,
        &raw_capture.frame,
        &detect_result,
        &process_result,
    ) {
        Ok(written) => DebugCommandOutcome::Applied {
            reason: format!(
                "exported live debug bundle to {} ({} files)",
                bundle_dir.display(),
                written
            ),
            artifact_path: Some(bundle_dir),
        },
        Err(e) => {
            // Best effort: leave a summary that records the failure.
            let _ = write_failure_summary(&bundle_dir, &e);
            DebugCommandOutcome::Failed {
                reason: format!("bundle write failed: {e}"),
            }
        }
    }
}

#[cfg(feature = "real-runtime")]
fn build_export_detector(config_root: &Path) -> Result<d4lc_detect::PreparedDetector, String> {
    let config = crate::runtime_prepare::resolve_runtime_config(config_root, None, None, None)
        .map_err(|e| format!("runtime prepare: {e}"))?;
    let _presets = d4lc_assets::lock_preset_trees_shared();
    crate::runtime_prepare::build_prepared_detector(config_root, &config)
        .map_err(|e| format!("detector prep: {e}"))
}

#[cfg(feature = "real-runtime")]
fn export_bundle_dir() -> PathBuf {
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::actions::debug::screenshot::default_screenshot_dir().join(format!("export-{unix_secs}"))
}

/// Write the bundle files and return how many were written. Images are
/// written only when their step produced one.
#[cfg(feature = "real-runtime")]
fn write_bundle(
    bundle_dir: &Path,
    raw_frame: &CaptureFrame,
    detect_result: &d4lc_detect::DetectFrameResult,
    process_result: &d4lc_runtime::ProcessedFrameResult,
) -> Result<usize, String> {
    let mut written = 0usize;

    d4lc_debug::write_capture_png(&bundle_dir.join("raw-capture.png"), raw_frame)?;
    written += 1;

    match d4lc_capture::crop_frame(raw_frame, detect_result.roi) {
        Ok(roi_frame) => {
            d4lc_debug::write_capture_png(&bundle_dir.join("roi.png"), &roi_frame)?;
            written += 1;
        }
        Err(e) => {
            eprintln!("[d4lc] export: skip roi.png (crop_frame failed: {e}); bundle still usable");
        }
    }

    if let Some(crop) = &detect_result.crop {
        d4lc_debug::write_capture_png(&bundle_dir.join("tooltip-crop.png"), crop)?;
        written += 1;
    }

    let mut documents = vec![
        ("detect-trace.json", detect_trace_json(detect_result)),
        (
            "overlay-status.json",
            overlay_status_json(&process_result.overlay_frame),
        ),
        ("timings.json", timings_json(detect_result, process_result)),
    ];
    if let Some(item) = &process_result.current_item {
        documents.push(("parsed-item.json", parsed_item_json(item)));
    }
    documents.push((
        "rule-result.json",
        rule_result_json(&process_result.overlay_frame.rule_descriptors),
    ));
    for (name, document) in &documents {
        write_json(&bundle_dir.join(name), document)?;
        written += 1;
    }

    // The summary comes last and counts itself.
    written += 1;
    write_json(
        &bundle_dir.join("bundle-summary.json"),
        &bundle_summary_json(bundle_dir, detect_result, process_result, written - 1),
    )?;

    Ok(written)
}

#[cfg(feature = "real-runtime")]
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    d4lc_debug::write_debug_text(path, &text)
}

#[cfg(feature = "real-runtime")]
fn write_failure_summary(bundle_dir: &Path, reason: &str) -> Result<(), String> {
    write_json(
        &bundle_dir.join("bundle-summary.json"),
        &json!({
            "status": "failed",
            "reason": reason,
            "bundle_dir": bundle_dir.display().to_string(),
        }),
    )
}

#[cfg(feature = "real-runtime")]
fn detection_timings_json(t: &d4lc_detect::DetectionTimings) -> Value {
    json!({
        "asset_load_ms": t.asset_load_ms,
        "roi_crop_ms": t.roi_crop_ms,
        "grayscale_ms": t.grayscale_ms,
        "template_select_ms": t.template_select_ms,
        "template_load_ms": t.template_load_ms,
        "template_prepare_ms": t.template_prepare_ms,
        "integral_ms": t.integral_ms,
        "match_ms": t.match_ms,
        "scan_threads": t.scan_threads,
        "coordinate_translate_ms": t.coordinate_translate_ms,
        "trace_write_ms": t.trace_write_ms,
        "crop_write_ms": t.crop_write_ms,
        "total_ms": t.total_ms,
    })
}

#[cfg(feature = "real-runtime")]
fn detect_trace_json(detect_result: &d4lc_detect::DetectFrameResult) -> Value {
    let candidate = |candidate: &d4lc_core::MatchCandidate| {
        json!({
            "template_name": candidate.template_name,
            "score": candidate.score,
            "rect": candidate.rect,
        })
    };
    json!({
        "preset": detect_result.preset,
        "mode": format!("{:?}", detect_result.mode),
        "roi": detect_result.roi,
        "tooltip_found": detect_result.selected.is_some(),
        "candidates": detect_result.candidates.iter().map(candidate).collect::<Vec<_>>(),
        "selected": detect_result.selected.as_ref().map(candidate),
        "timings": detection_timings_json(&detect_result.timings),
    })
}

#[cfg(feature = "real-runtime")]
fn overlay_status_json(overlay_frame: &d4lc_overlay_x11::OverlayFrame) -> Value {
    json!({
        "sequence": overlay_frame.sequence,
        "tooltip_found": overlay_frame.tooltip_found,
        "hotpath_ms": overlay_frame.hotpath_ms,
        "descriptor_count": overlay_frame.rule_descriptors.len(),
    })
}

#[cfg(feature = "real-runtime")]
fn timings_json(
    detect_result: &d4lc_detect::DetectFrameResult,
    process_result: &d4lc_runtime::ProcessedFrameResult,
) -> Value {
    json!({
        "detect": detection_timings_json(&detect_result.timings),
        "process_one_frame": {
            "hotpath_ms": process_result.overlay_frame.hotpath_ms,
            "sequence": process_result.overlay_frame.sequence,
        },
        "frame_status": process_result.frame_status.parse_status_string(),
    })
}

#[cfg(feature = "real-runtime")]
fn parsed_item_json(item: &d4lc_runtime::CurrentItemSnapshot) -> Value {
    json!({
        "item_name": item.item_name,
        "parse_status": item.parse_status,
    })
}

#[cfg(feature = "real-runtime")]
fn rule_result_json(descriptors: &[d4lc_rules::RuleOverlayDescriptor]) -> Value {
    json!({
        "descriptor_count": descriptors.len(),
        "descriptors": descriptors,
    })
}

#[cfg(feature = "real-runtime")]
fn bundle_summary_json(
    bundle_dir: &Path,
    detect_result: &d4lc_detect::DetectFrameResult,
    process_result: &d4lc_runtime::ProcessedFrameResult,
    files_written: usize,
) -> Value {
    let status = if detect_result.selected.is_some() {
        "captured"
    } else {
        "captured_without_tooltip"
    };
    json!({
        "status": status,
        "bundle_dir": bundle_dir.display().to_string(),
        "files_written": files_written,
        "tooltip_found": detect_result.selected.is_some(),
        "frame_status": process_result.frame_status.parse_status_string(),
    })
}

#[cfg(test)]
#[cfg(feature = "real-runtime")]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(feature = "wayland-portal")]
    #[test]
    fn export_refuses_a_wayland_engine_without_opening_a_portal_session() {
        // Beside a live loop the portal session is taken; the export's
        // preparation must fail by name at once instead of waiting on the
        // session slot (or taking it from a loop that is still preparing).
        let dir = std::env::temp_dir().join(format!(
            "d4lc-export-no-portal-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("Config")).unwrap();
        fs::write(
            dir.join(d4lc_config::SETTINGS_PATH),
            format!(
                "{{\n  \"schema_version\": {},\n  \"DisplayBackend\": \"wayland\"\n}}\n",
                d4lc_config::SETTINGS_SCHEMA_VERSION
            ),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let outcome = attempt_real_export(&dir);
        let _ = fs::remove_dir_all(&dir);
        match outcome {
            DebugCommandOutcome::Failed { reason } => {
                assert_eq!(
                    reason, WAYLAND_EXPORT_GAP,
                    "the refusal names the Wayland gap, not a generic preparation failure"
                );
                assert!(reason.contains("Wayland gap"), "{reason}");
                assert!(!reason.contains("stop the live loop"), "{reason}");
            }
            other => panic!("expected a named failure, got {other:?}"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "the refusal must not wait on the portal-session slot"
        );
    }

    #[test]
    fn write_failure_summary_produces_a_failure_file() {
        let dir = std::env::temp_dir().join(format!("d4lc-export-fail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_failure_summary(&dir, "boom \"quoted\"").unwrap();
        let content = fs::read_to_string(dir.join("bundle-summary.json")).unwrap();
        let summary: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(summary["status"], "failed");
        assert_eq!(summary["reason"], "boom \"quoted\"");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_summary_json_records_tooltip_state() {
        let dir = PathBuf::from("/tmp/d4lc-export-test");
        let detect = d4lc_detect::DetectFrameResult {
            preset: "Speed".to_string(),
            mode: d4lc_core::InputMode::Controller,
            thresholds: d4lc_detect::ThresholdSettings {
                min: 0,
                max: 255,
                similarity_tooltip: 0.8,
            },
            roi: d4lc_core::Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            roi_role: d4lc_detect::RoiRole::FullFrame,
            planned_tooltip_roi: None,
            tooltip_detection: d4lc_detect::TooltipDetectionTrace {
                search_roi: d4lc_core::Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                detected_tooltip_rect: None,
                final_tooltip_crop_rect: None,
                crop_source: "test",
                upstream_rule: "test",
                final_tooltip_size: None,
            },
            candidates: Vec::new(),
            selected: None,
            crop: None,
            trace_output_path: None,
            crop_output_path: None,
            thresholded_crop_output_path: None,
            timings: d4lc_detect::DetectionTimings::default(),
        };
        let process = d4lc_runtime::ProcessedFrameResult {
            stage_us: Default::default(),
            overlay_frame: d4lc_overlay_x11::OverlayFrame {
                sequence: 0,
                tooltip_found: false,
                hotpath_ms: 0,
                cursor_window_relative: None,
                rule_descriptors: std::sync::Arc::from([]),
                text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
            },
            current_item: None,
            frame_status: d4lc_runtime::FrameStatus::NoTooltip,
            tooltip_window_relative: None,
            analysis: std::sync::Arc::new(d4lc_runtime::AnalysisSnapshot::empty(1)),
        };
        let json = bundle_summary_json(&dir, &detect, &process, 5);
        assert_eq!(json["status"], "captured_without_tooltip");
        assert_eq!(json["files_written"], 5);
        assert_eq!(json["tooltip_found"], false);
    }
}
