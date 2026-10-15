use crate::live;
use crate::roots::{cli_overrides_json, ConfigFiles, Roots};
use crate::to_json_text;
use d4lc_capture::{CaptureResult, CaptureSource, CursorState};
use d4lc_config::SETTINGS_PATH;
use d4lc_debug::write_debug_text;
use d4lc_overlay_x11::{OverlayFrame, OverlayStatus};
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "leptess-backend")]
use crate::ocr;
#[cfg(feature = "leptess-backend")]
use d4lc_ocr::OcrProfile;

const BUNDLE_ROOT: &str = "debug-output/manual-diablo-iv";
const DEFAULT_FRAMES: u64 = 30;
const DEFAULT_INTERVAL_MS: u64 = 250;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ManualValidationArgs {
    pub live: live::LiveArgs,
    pub dry_run: bool,
}

pub fn manual_diablo_validation_bundle(
    roots: &Roots,
    args: &ManualValidationArgs,
) -> Result<String, String> {
    let files = roots.config_files()?;
    let bundle_dir = resolve_bundle_dir(&roots.work, args.live.output_dir.as_deref())?;
    fs::create_dir_all(&bundle_dir).map_err(|error| {
        format!(
            "failed to create the bundle directory {}: {error}; pass --output-dir <writable directory>",
            bundle_dir.display()
        )
    })?;

    // The snapshot comes first: it records where the settings came from even
    // when they stop the run.
    let live_args = manual_live_args(args, &bundle_dir);
    let snapshot_path = bundle_dir.join("settings-snapshot.json");
    write_debug_text(&snapshot_path, &settings_snapshot_json(&files, &live_args))?;
    files
        .clone()
        .into_settings()
        .and_then(|_| files.into_controllers())
        .map_err(|error| {
            format!(
                "{error}; manual-diablo-validation-bundle aborted (see {})",
                snapshot_path.display()
            )
        })?;

    if args.dry_run {
        return write_dry_run_bundle(args, &bundle_dir);
    }

    let capture = live::capture_manual_validation_run(roots, &live_args)?;
    write_debug_text(
        &bundle_dir.join("live-session.jsonl"),
        &capture.session_jsonl,
    )?;
    write_debug_text(
        &bundle_dir.join("live-session-summary.json"),
        &capture.session_summary_json,
    )?;
    if let Some(final_frame_json) = capture.final_frame_json.as_deref() {
        write_debug_text(&bundle_dir.join("final-frame.json"), final_frame_json)?;
    }

    let selected_window_json = selected_window_json(
        &live_args,
        capture.final_capture.as_ref(),
        capture.final_cursor.as_ref(),
    );
    write_debug_text(
        &bundle_dir.join("selected-window.json"),
        &selected_window_json,
    )?;

    let overlay_frame_json = overlay_frame_json(
        capture.final_overlay_frame.as_ref(),
        capture.final_overlay_status.as_ref(),
    );
    write_debug_text(&bundle_dir.join("overlay-frame.json"), &overlay_frame_json)?;

    let ocr_artifacts = write_ocr_artifacts(roots, &live_args, &bundle_dir, &capture)?;
    write_debug_text(
        &bundle_dir.join("rule-result.json"),
        &rule_result_unavailable_json(),
    )?;

    let status = if !capture.final_detection_found {
        "captured_without_tooltip"
    } else if ocr_artifacts.status == "ok" {
        "captured"
    } else {
        "partial"
    };
    let bundle_summary = bundle_summary_json(
        status,
        false,
        &bundle_dir,
        Some(capture.final_detection_found),
        ocr_artifacts.status,
    );
    write_debug_text(&bundle_dir.join("bundle-summary.json"), &bundle_summary)?;

    Ok(bundle_summary)
}

fn manual_live_args(args: &ManualValidationArgs, bundle_dir: &Path) -> live::LiveArgs {
    let mut live_args = args.live.clone();
    live_args.frames = Some(live_args.frames.unwrap_or(DEFAULT_FRAMES));
    live_args.interval_millis = Some(live_args.interval_millis.unwrap_or(DEFAULT_INTERVAL_MS));
    live_args.output_dir = Some(bundle_dir.to_path_buf());
    live_args.capture_output_path = None;
    live_args.overlay = true;
    live_args.summary_only = false;
    live_args.ocr_worker = false;
    live_args
}

fn write_dry_run_bundle(args: &ManualValidationArgs, bundle_dir: &Path) -> Result<String, String> {
    write_debug_text(
        &bundle_dir.join("selected-window.json"),
        &selected_window_json(&args.live, None, None),
    )?;
    write_debug_text(
        &bundle_dir.join("overlay-frame.json"),
        &overlay_frame_json(None, None),
    )?;
    write_debug_text(
        &bundle_dir.join("live-session-summary.json"),
        &placeholder_artifact_json(
            "live_session_summary",
            "dry_run does not capture a real Diablo IV X11 session",
        ),
    )?;
    write_debug_text(
        &bundle_dir.join("final-frame.json"),
        &placeholder_artifact_json(
            "final_frame",
            "dry_run does not capture screenshots or detection traces",
        ),
    )?;
    write_debug_text(
        &bundle_dir.join("ocr-output.json"),
        &placeholder_artifact_json(
            "ocr_output",
            "dry_run does not execute OCR; run against a real Diablo IV X11 tooltip to collect text and boxes",
        ),
    )?;
    write_debug_text(
        &bundle_dir.join("parsed-item.json"),
        &placeholder_artifact_json("parsed_item", "dry_run does not execute OCR/parsing"),
    )?;
    write_debug_text(
        &bundle_dir.join("rule-result.json"),
        &rule_result_unavailable_json(),
    )?;
    let summary = bundle_summary_json("dry_run_ready", true, bundle_dir, None, "dry_run");
    write_debug_text(&bundle_dir.join("bundle-summary.json"), &summary)?;
    Ok(summary)
}

/// The absolute bundle directory. An absolute `--output-dir` is the user's
/// explicit choice; a relative one resolves against the current directory
/// and must stay below `debug-output/manual-diablo-iv/`.
fn resolve_bundle_dir(work: &Path, requested: Option<&Path>) -> Result<PathBuf, String> {
    let base = Path::new(BUNDLE_ROOT);
    let relative = match requested {
        Some(path) if path.is_absolute() => return Ok(path.to_path_buf()),
        Some(path) => {
            if !path.starts_with(base)
                || path
                    .components()
                    .any(|component| component == Component::ParentDir)
            {
                return Err(format!(
                    "manual-diablo-validation-bundle: a relative --output-dir must stay under {BUNDLE_ROOT}/ (no '..'); pass an absolute path for any other location"
                ));
            }
            path.to_path_buf()
        }
        None => base.join(format!("run-{}", timestamp_suffix())),
    };
    Ok(work.join(relative))
}

fn timestamp_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "time_error".to_string())
}

struct OcrArtifacts {
    status: &'static str,
}

fn write_ocr_artifacts(
    roots: &Roots,
    live_args: &live::LiveArgs,
    bundle_dir: &Path,
    capture: &live::ManualValidationCapture,
) -> Result<OcrArtifacts, String> {
    let Some(crop_path) = capture.final_tooltip_crop_path.as_ref() else {
        write_debug_text(
            &bundle_dir.join("ocr-output.json"),
            &placeholder_artifact_json(
                "ocr_output",
                "no final tooltip crop was detected during the bounded live session",
            ),
        )?;
        write_debug_text(
            &bundle_dir.join("parsed-item.json"),
            &placeholder_artifact_json(
                "parsed_item",
                "no final tooltip crop was detected during the bounded live session",
            ),
        )?;
        return Ok(OcrArtifacts { status: "no_crop" });
    };

    write_ocr_artifacts_from_crop(roots, live_args, bundle_dir, crop_path)
}

#[cfg(feature = "leptess-backend")]
fn write_ocr_artifacts_from_crop(
    roots: &Roots,
    live_args: &live::LiveArgs,
    bundle_dir: &Path,
    crop_path: &Path,
) -> Result<OcrArtifacts, String> {
    let settings = roots.settings()?;
    let preset = live_args
        .preset
        .clone()
        .unwrap_or_else(|| settings.selected_system_preset.clone());
    let language = live_args
        .language
        .clone()
        .unwrap_or_else(|| settings.selected_affix_language.clone());
    let ocr_output_path = bundle_dir.join("ocr-text.txt");
    let ocr_trace_path = bundle_dir.join("ocr-trace.json");
    let ocr_json = ocr::read_tooltip(
        roots,
        &ocr::OcrArgs {
            input_path: Some(crop_path.to_path_buf()),
            preset: Some(preset.clone()),
            language: Some(language.clone()),
            backend: None,
            psm: None,
            text_output_path: Some(ocr_output_path),
            trace_output_path: Some(ocr_trace_path),
            tessdata_path: None,
        },
    )?;
    write_debug_text(&bundle_dir.join("ocr-output.json"), &ocr_json)?;

    let backend = ocr::parse_ocr_backend(None)?;
    let profile = OcrProfile {
        backend,
        language,
        tessdata_path: None,
    };
    let registry = ocr::create_ocr_registry(profile.resolved_backend())?;
    let frame = crate::detect::load_capture_frame(&roots.work_path(crop_path))?;
    let parity = ocr::recognize_tooltip_frame_with_read_tooltip_parity(
        roots, frame, preset, profile, &registry, 120,
    )?;
    let parsed_item_json = to_json_text(&json!({
        "status": "ok",
        "parity_path": parity.parity_path,
        "segment_count": parity.segment_count,
        "line_count": parity.line_count,
        "item": parity.item,
    }));
    write_debug_text(&bundle_dir.join("parsed-item.json"), &parsed_item_json)?;

    Ok(OcrArtifacts { status: "ok" })
}

#[cfg(not(feature = "leptess-backend"))]
fn write_ocr_artifacts_from_crop(
    _roots: &Roots,
    _live_args: &live::LiveArgs,
    bundle_dir: &Path,
    _crop_path: &Path,
) -> Result<OcrArtifacts, String> {
    write_debug_text(
        &bundle_dir.join("ocr-output.json"),
        &placeholder_artifact_json(
            "ocr_output",
            "rebuild d4lc-cli with --features leptess-backend to collect OCR text and boxes",
        ),
    )?;
    write_debug_text(
        &bundle_dir.join("parsed-item.json"),
        &placeholder_artifact_json(
            "parsed_item",
            "rebuild d4lc-cli with --features leptess-backend to collect parsed item snapshots",
        ),
    )?;
    Ok(OcrArtifacts {
        status: "ocr_backend_unavailable",
    })
}

fn bundle_summary_json(
    status: &str,
    dry_run: bool,
    bundle_dir: &Path,
    final_detection_found: Option<bool>,
    ocr_status: &str,
) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "command": "manual-diablo-validation-bundle",
        "status": status,
        "dry_run": dry_run,
        "bundle_dir": bundle_dir.display().to_string(),
        "final_detection_found": final_detection_found,
        "ocr_status": ocr_status,
    }))
}

/// The effective settings of the run and where they came from. `status` is
/// the state of `Config/Settings.json`: `loaded`, `missing` (defaults used),
/// `invalid` or `unreadable` (the command aborts).
fn settings_snapshot_json(files: &ConfigFiles, live_args: &live::LiveArgs) -> String {
    let mut snapshot = json!({
        "status": files.settings.status.as_str(),
        "settings_path": SETTINGS_PATH,
        "config": files.identity_json(live_cli_overrides_json(live_args)),
        "requested": requested_runtime_json(live_args),
    });
    let fields = match &files.settings.value {
        Some(settings) => json!({
            "selected_system_preset": settings.selected_system_preset,
            "selected_affix_language": settings.selected_affix_language,
            "capture_backend": settings.capture_backend,
            "controller_mode": settings.controller_mode,
            "overlay_enabled": settings.overlay_enabled,
            "show_overlay_icon": settings.show_overlay_icon,
            "overlay_icon_pos_x": settings.overlay_icon_pos_x,
            "overlay_icon_pos_y": settings.overlay_icon_pos_y,
        }),
        None => json!({ "error": files.settings.status.error() }),
    };
    if let (Some(snapshot), Value::Object(fields)) = (snapshot.as_object_mut(), fields) {
        snapshot.extend(fields);
    }
    to_json_text(&snapshot)
}

/// The flags that replace settings-file values in a live run.
fn live_cli_overrides_json(live_args: &live::LiveArgs) -> Value {
    cli_overrides_json(&[
        ("preset", json!(live_args.preset)),
        ("language", json!(live_args.language)),
        ("mode", json!(live_args.mode)),
        ("controller", json!(live_args.controller_config)),
        ("threshold_min", json!(live_args.threshold_min)),
        ("threshold_max", json!(live_args.threshold_max)),
        (
            "threshold_similarity_tooltip",
            json!(live_args.threshold_similarity_tooltip),
        ),
    ])
}

fn requested_runtime_json(live_args: &live::LiveArgs) -> Value {
    json!({
        "match": live_args.match_title,
        "class": live_args.class_name,
        "pid_requested": live_args.pid,
        "exact_title": live_args.exact_title,
        "manual_region": live_args.manual_region,
        "preset": live_args.preset,
        "language": live_args.language,
        "mode": live_args.mode,
        "frames": live_args.frames,
        "interval_ms": live_args.interval_millis,
        "backend": live_args.backend,
        "capture_scope": live_args.capture_scope,
        "overlay": live_args.overlay,
    })
}

fn selected_window_json(
    live_args: &live::LiveArgs,
    capture: Option<&CaptureResult>,
    cursor: Option<&CursorState>,
) -> String {
    let cursor_window_relative = cursor.and_then(|cursor| cursor.window_relative);
    let selection_query = requested_runtime_json(live_args);
    let selection = match capture.map(|capture| &capture.source) {
        // The window title and class are redacted: the bundle is meant to be
        // shared, and both can carry account or machine details.
        Some(CaptureSource::Window(candidate)) => json!({
            "status": "captured",
            "source": "window",
            "title": "<redacted>",
            "class_name": candidate.info.class_name.as_ref().map(|_| "<redacted>"),
            "pid_present": candidate.pid.is_some(),
            "active": candidate.is_active,
            "matches_query": candidate.matches_query,
            "rect": candidate.info.rect,
            "cursor_window_relative": cursor_window_relative,
            "selection_query": selection_query,
        }),
        Some(CaptureSource::ManualRegion(rect)) => json!({
            "status": "captured",
            "source": "manual_region",
            "rect": rect,
            "cursor_window_relative": cursor_window_relative,
            "selection_query": selection_query,
        }),
        // The CLI captures through the X11 paths only; a portal stream
        // result cannot be produced here, but the report stays total.
        Some(CaptureSource::PortalStream(rect)) => json!({
            "status": "captured",
            "source": "portal_stream",
            "rect": rect,
            "cursor_window_relative": cursor_window_relative,
            "selection_query": selection_query,
        }),
        None => json!({
            "status": "not_captured",
            "selection_query": selection_query,
        }),
    };
    to_json_text(&selection)
}

fn overlay_frame_json(frame: Option<&OverlayFrame>, status: Option<&OverlayStatus>) -> String {
    let overlay = match frame {
        Some(frame) => json!({
            "status": "captured",
            "sequence": frame.sequence,
            "tooltip_found": frame.tooltip_found,
            "hotpath_ms": frame.hotpath_ms,
            "cursor_window_relative": frame.cursor_window_relative,
            "descriptor_count": frame.rule_descriptors.len(),
            "descriptors": frame.rule_descriptors,
            "descriptor_source": "caller_provided_app_state",
            "descriptor_reason": "d4lc-runtime leaves rule_descriptors empty until the caller \
                provides app-owned overlay state",
            "overlay_status": overlay_status_json(status),
        }),
        None => json!({
            "status": "not_captured",
            "descriptors": [],
            "overlay_status": overlay_status_json(status),
        }),
    };
    to_json_text(&overlay)
}

fn overlay_status_json(status: Option<&OverlayStatus>) -> Value {
    match status {
        Some(status) => json!({
            "enabled": status.enabled,
            "backend": status.backend.as_str(),
            "transparent": status.transparent,
            "always_on_top": status.always_on_top,
            "click_through": status.click_through,
            "limitation": status.limitation,
        }),
        None => json!({
            "enabled": false,
            "backend": "disabled",
            "transparent": false,
            "always_on_top": false,
            "click_through": false,
            "limitation": null,
        }),
    }
}

fn rule_result_unavailable_json() -> String {
    placeholder_artifact_json(
        "rule_result",
        "the CLI has no app-backed rule evaluation state; keep the final tooltip/OCR artifacts for a manual review",
    )
}

fn placeholder_artifact_json(name: &str, reason: &str) -> String {
    to_json_text(&json!({
        "status": "unavailable",
        "artifact": name,
        "reason": reason,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::Invocation;
    use std::env;

    #[test]
    fn manual_diablo_validation_dry_run_writes_bundle_layout() {
        let root = temp_root("manual_diablo_validation_dry_run_writes_bundle_layout");
        let output = manual_diablo_validation_bundle(
            &Roots::for_tests(&root),
            &ManualValidationArgs {
                dry_run: true,
                ..ManualValidationArgs::default()
            },
        )
        .unwrap_or_else(|error| panic!("dry-run bundle failed: {error}"));
        let summary: Value = serde_json::from_str(&output)
            .unwrap_or_else(|error| panic!("summary json parse failed: {error}"));
        let bundle_dir = summary["bundle_dir"]
            .as_str()
            .unwrap_or_else(|| panic!("bundle_dir missing from summary"));
        for relative in [
            "bundle-summary.json",
            "selected-window.json",
            "settings-snapshot.json",
            "overlay-frame.json",
            "live-session-summary.json",
            "final-frame.json",
            "ocr-output.json",
            "parsed-item.json",
            "rule-result.json",
        ] {
            assert!(
                root.join(bundle_dir).join(relative).is_file(),
                "missing dry-run artifact {relative}"
            );
        }
        remove_dir(&root);
    }

    #[test]
    fn manual_diablo_validation_rejects_output_outside_the_bundle_root() {
        let result = resolve_bundle_dir(
            Path::new("/work"),
            Some(Path::new("debug-output/not-allowed/run-1")),
        );
        let error = match result {
            Ok(path) => panic!("unexpected bundle dir {path:?}"),
            Err(error) => error,
        };
        assert!(error.contains(BUNDLE_ROOT));
    }

    #[test]
    fn a_relative_output_dir_cannot_escape_the_bundle_root() {
        let escape = Path::new("debug-output/manual-diablo-iv/../../../etc");
        assert!(resolve_bundle_dir(Path::new("/work"), Some(escape)).is_err());
        assert_eq!(
            resolve_bundle_dir(
                Path::new("/work"),
                Some(Path::new("debug-output/manual-diablo-iv/run-1"))
            ),
            Ok(PathBuf::from("/work/debug-output/manual-diablo-iv/run-1"))
        );
    }

    #[test]
    fn dry_run_with_a_read_only_asset_root_writes_below_the_current_directory_only() {
        let fixture = Fixture::new("dry_run_read_only_asset_root");
        set_read_only(&fixture.assets, true);

        let summary = fixture.dry_run(None);

        set_read_only(&fixture.assets, false);
        let bundle_dir = PathBuf::from(summary["bundle_dir"].as_str().unwrap_or_default());
        assert!(
            bundle_dir.starts_with(fixture.work.join(BUNDLE_ROOT)),
            "{bundle_dir:?}"
        );
        assert!(bundle_dir.join("bundle-summary.json").is_file());
        assert_eq!(dir_entries(&fixture.assets), Vec::<String>::new());
        assert_eq!(dir_entries(&fixture.config), Vec::<String>::new());
    }

    #[test]
    fn an_absolute_output_dir_is_accepted() {
        let fixture = Fixture::new("absolute_output_dir");
        let target = fixture.base.join("elsewhere/bundle");

        let summary = fixture.dry_run(Some(&target));

        assert_eq!(summary["bundle_dir"], target.display().to_string());
        assert!(target.join("bundle-summary.json").is_file());
        assert!(!fixture.work.join("debug-output").exists());
    }

    #[test]
    fn the_snapshot_says_missing_without_a_settings_file() {
        let fixture = Fixture::new("snapshot_missing");

        let snapshot = fixture.snapshot(&fixture.dry_run(None));

        assert_eq!(snapshot["status"], "missing");
        assert_eq!(snapshot["config"]["settings"]["status"], "missing");
        assert_eq!(snapshot["config"]["settings"]["effect"], "defaults");
        assert_eq!(
            snapshot["config"]["config_root"],
            fixture.config.display().to_string()
        );
        assert_eq!(snapshot["config"]["config_root_source"], "--config-root");
        assert_eq!(
            snapshot["selected_system_preset"],
            d4lc_config::AppSettings::default().selected_system_preset
        );
    }

    #[test]
    fn the_snapshot_says_loaded_with_the_file_values_and_lists_overrides_apart() {
        let fixture = Fixture::new("snapshot_loaded");
        fixture.write_settings(
            r#"{ "SelectedSystemPreset": "2160p_SSF", "SelectedAffixLanguage": "deDE" }"#,
        );
        let args = ManualValidationArgs {
            dry_run: true,
            live: live::LiveArgs {
                language: Some("frFR".to_string()),
                ..live::LiveArgs::default()
            },
        };

        let summary = manual_diablo_validation_bundle(&fixture.roots(), &args)
            .unwrap_or_else(|error| panic!("dry run failed: {error}"));
        let snapshot = fixture.snapshot(&serde_json::from_str(&summary).unwrap_or_default());

        assert_eq!(snapshot["status"], "loaded");
        assert_eq!(snapshot["selected_system_preset"], "2160p_SSF");
        assert_eq!(snapshot["selected_affix_language"], "deDE");
        assert_eq!(
            snapshot["config"]["cli_overrides"],
            json!({ "language": "frFR" })
        );
    }

    #[test]
    fn an_invalid_settings_file_is_recorded_and_stops_the_command() {
        let fixture = Fixture::new("snapshot_invalid");
        fixture.write_settings("{ not json");

        let error = match manual_diablo_validation_bundle(
            &fixture.roots(),
            &ManualValidationArgs {
                dry_run: true,
                ..ManualValidationArgs::default()
            },
        ) {
            Ok(summary) => panic!("an invalid settings file must stop the run: {summary}"),
            Err(error) => error,
        };

        assert!(error.contains("is invalid"), "{error}");
        let snapshot_path = error
            .rsplit_once("(see ")
            .map(|(_, path)| PathBuf::from(path.trim_end_matches(')')))
            .unwrap_or_default();
        let snapshot: Value = serde_json::from_str(
            &fs::read_to_string(&snapshot_path)
                .unwrap_or_else(|error| panic!("{}: {error}", snapshot_path.display())),
        )
        .unwrap_or_default();
        assert_eq!(snapshot["status"], "invalid");
        assert_eq!(snapshot["config"]["settings"]["effect"], "aborted");
        assert!(snapshot["error"]
            .as_str()
            .is_some_and(|text| !text.is_empty()));
        assert!(!snapshot_path.with_file_name("bundle-summary.json").exists());
    }

    /// Separate asset, config and work directories below one temp base.
    struct Fixture {
        base: PathBuf,
        assets: PathBuf,
        config: PathBuf,
        work: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base = temp_root(name);
            let fixture = Self {
                assets: base.join("assets"),
                config: base.join("config"),
                work: base.join("work"),
                base,
            };
            for dir in [&fixture.assets, &fixture.config, &fixture.work] {
                fs::create_dir_all(dir)
                    .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
            }
            fixture
        }

        fn roots(&self) -> Roots {
            Roots::resolve(
                &self.assets,
                Some(&self.config),
                &Invocation {
                    cwd: self.work.clone(),
                    ..Invocation::default()
                },
            )
        }

        fn dry_run(&self, output_dir: Option<&Path>) -> Value {
            let args = ManualValidationArgs {
                dry_run: true,
                live: live::LiveArgs {
                    output_dir: output_dir.map(Path::to_path_buf),
                    ..live::LiveArgs::default()
                },
            };
            let summary = manual_diablo_validation_bundle(&self.roots(), &args)
                .unwrap_or_else(|error| panic!("dry run failed: {error}"));
            serde_json::from_str(&summary).unwrap_or_else(|error| panic!("{error}"))
        }

        fn snapshot(&self, summary: &Value) -> Value {
            let path = Path::new(summary["bundle_dir"].as_str().unwrap_or_default())
                .join("settings-snapshot.json");
            serde_json::from_str(
                &fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display())),
            )
            .unwrap_or_else(|error| panic!("{error}"))
        }

        fn write_settings(&self, text: &str) {
            let path = self.config.join(SETTINGS_PATH);
            fs::create_dir_all(path.parent().unwrap_or(&self.config))
                .unwrap_or_else(|error| panic!("{error}"));
            fs::write(&path, text).unwrap_or_else(|error| panic!("{error}"));
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            set_read_only(&self.assets, false);
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn set_read_only(dir: &Path, read_only: bool) {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if read_only { 0o555 } else { 0o755 };
        fs::set_permissions(dir, fs::Permissions::from_mode(mode))
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    }

    fn dir_entries(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn temp_root(test_name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|_| "time_error".to_string());
        let root = env::temp_dir().join(format!("d4lc-{test_name}-{suffix}"));
        fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", root.display()));
        root
    }

    fn remove_dir(path: &Path) {
        fs::remove_dir_all(path)
            .unwrap_or_else(|error| panic!("failed to remove {}: {error}", path.display()));
    }
}
