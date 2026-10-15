use super::*;
use image::ImageReader;
use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn parses_workspace_members_from_multiline_array() {
    let cargo_toml = r#"
[workspace]
members = [
  "crates/d4lc-core",
  "crates/d4lc-cli",
]
"#;

    assert_eq!(
        parse_workspace_members(cargo_toml),
        vec!["crates/d4lc-core", "crates/d4lc-cli"]
    );
}

#[test]
fn parses_asset_flags() {
    let parsed = ParsedArgs::parse(
        [
            "asset-controller-images".to_string(),
            "--root".to_string(),
            "/tmp/repo".to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--language".to_string(),
            "frFR".to_string(),
            "--controller".to_string(),
            "tooltip_kb_all_frfr.png".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::AssetControllerImages));
    assert_eq!(parsed.root, PathBuf::from("/tmp/repo"));
    assert_eq!(parsed.preset, Some("1080p_SMF".to_string()));
    assert_eq!(parsed.language, Some("frFR".to_string()));
    assert_eq!(
        parsed.controller_config,
        vec!["tooltip_kb_all_frfr.png".to_string()]
    );
}

#[test]
fn parses_live_once_capture_flags() {
    let parsed = ParsedArgs::parse(
        [
            "live-once".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--exact-title".to_string(),
            "--backend".to_string(),
            "x11-native".to_string(),
            "--manual-region".to_string(),
            "10,20,300,400".to_string(),
            "--capture-output".to_string(),
            "debug-output/capture/live-once.png".to_string(),
            "--delay-seconds".to_string(),
            "3".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::LiveOnce));
    assert_eq!(parsed.match_title.as_deref(), Some("Diablo IV"));
    assert!(parsed.exact_title);
    assert_eq!(parsed.backend.as_deref(), Some("x11-native"));
    assert_eq!(parsed.delay_millis, Some(3000));
    assert_eq!(
        parsed.capture_output_path,
        Some(PathBuf::from("debug-output/capture/live-once.png"))
    );
    assert_eq!(
        parsed.manual_region,
        Some(d4lc_core::Rect {
            x: 10,
            y: 20,
            width: 300,
            height: 400,
        })
    );
}

#[test]
fn parses_live_loop_delay_alias_for_focus_switching() {
    let parsed = ParsedArgs::parse(
        [
            "live-loop".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--exact-title".to_string(),
            "--delay".to_string(),
            "2.5".to_string(),
            "--frames".to_string(),
            "100".to_string(),
            "--interval-ms".to_string(),
            "0".to_string(),
            "--overlay".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::LiveLoop));
    assert_eq!(parsed.match_title.as_deref(), Some("Diablo IV"));
    assert!(parsed.exact_title);
    assert_eq!(parsed.delay_millis, Some(2500));
    assert_eq!(parsed.frames, Some(100));
    assert_eq!(parsed.interval_millis, Some(0));
    assert!(parsed.overlay);
}

#[test]
fn parses_live_loop_summary_only_flag() {
    let parsed = ParsedArgs::parse(
        [
            "live-loop".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--exact-title".to_string(),
            "--frames".to_string(),
            "100".to_string(),
            "--summary-only".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::LiveLoop));
    assert_eq!(parsed.match_title.as_deref(), Some("Diablo IV"));
    assert!(parsed.exact_title);
    assert_eq!(parsed.frames, Some(100));
    assert!(parsed.summary_only);
    assert!(parsed.live_args().summary_only);
}

#[test]
fn parses_perf_detect_command_and_write_path() {
    let parsed = ParsedArgs::parse(
        [
            "perf-detect".to_string(),
            "--write".to_string(),
            "debug-output/perf/detect.json".to_string(),
            "--capture-scope".to_string(),
            "roi".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::PerfDetect));
    assert_eq!(
        parsed.write_path,
        Some(PathBuf::from("debug-output/perf/detect.json"))
    );
    assert_eq!(parsed.live_args().capture_scope.as_deref(), Some("roi"));
}

#[test]
fn parses_perf_ocr_report_command_and_ocr_flags() {
    let parsed = ParsedArgs::parse(
        [
            "perf-ocr-report".to_string(),
            "--write".to_string(),
            "debug-output/perf/ocr.md".to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--language".to_string(),
            "enUS".to_string(),
            "--ocr-backend".to_string(),
            "leptess".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::PerfOcrReport));
    assert_eq!(
        parsed.write_path,
        Some(PathBuf::from("debug-output/perf/ocr.md"))
    );
    assert_eq!(parsed.perf_ocr_args().preset.as_deref(), Some("1080p_SMF"));
    assert_eq!(parsed.perf_ocr_args().language.as_deref(), Some("enUS"));
    assert_eq!(parsed.perf_ocr_args().backend.as_deref(), Some("leptess"));
}

#[test]
fn help_text_documents_live_delay_flag() {
    let help = help_text();

    assert!(help.contains("--delay-seconds <s>"));
    assert!(help.contains("--summary-only"));
    assert!(help.contains("perf-detect"));
    assert!(help.contains("perf-ocr-report"));
    assert!(help.contains("--delay-seconds <s> on live commands"));
    assert!(help.contains("switching focus from a terminal to Diablo IV"));
}

#[test]
fn parses_capture_scope_flags() {
    for value in ["auto", "full", "roi"] {
        let parsed = ParsedArgs::parse(
            [
                "live-once".to_string(),
                "--capture-scope".to_string(),
                value.to_string(),
            ],
            Path::new("."),
        )
        .unwrap_or_else(|error| panic!("parse failed for {value}: {error}"));
        assert_eq!(parsed.capture_scope.as_deref(), Some(value));
    }

    let error = match ParsedArgs::parse(
        [
            "live-once".to_string(),
            "--capture-scope".to_string(),
            "bad".to_string(),
        ],
        Path::new("."),
    ) {
        Ok(_) => panic!("invalid capture scope parsed successfully"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("unsupported --capture-scope"));
}

#[test]
fn parses_root_write_and_remote_head_flags() {
    let parsed = ParsedArgs::parse(
        [
            "upstream-changed".to_string(),
            "--root".to_string(),
            "/tmp/repo".to_string(),
            "--write".to_string(),
            "out.json".to_string(),
            "--remote-head".to_string(),
            "abc123".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::UpstreamChanged));
    assert_eq!(parsed.root, PathBuf::from("/tmp/repo"));
    assert_eq!(parsed.write_path, Some(PathBuf::from("out.json")));
    assert_eq!(parsed.remote_head, Some("abc123".to_string()));
}

#[test]
fn parses_detect_tooltip_flags() {
    let parsed = ParsedArgs::parse(
        [
            "detect-tooltip".to_string(),
            "--root".to_string(),
            "/tmp/repo".to_string(),
            "--input".to_string(),
            "tests/fixtures/detect/input.png".to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--controller".to_string(),
            "tooltip_gc_x.png".to_string(),
            "--trace-output".to_string(),
            "debug-output/detect/trace.json".to_string(),
            "--crop-output".to_string(),
            "debug-output/detect/crop.png".to_string(),
            "--threshold-min".to_string(),
            "65".to_string(),
            "--threshold-max".to_string(),
            "240".to_string(),
            "--threshold-similarity-tooltip".to_string(),
            "0.02".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::DetectTooltip));
    assert_eq!(
        parsed.input_path,
        Some(PathBuf::from("tests/fixtures/detect/input.png"))
    );
    assert_eq!(parsed.preset.as_deref(), Some("1080p_SMF"));
    assert_eq!(parsed.mode.as_deref(), Some("controller"));
    assert_eq!(
        parsed.trace_output_path,
        Some(PathBuf::from("debug-output/detect/trace.json"))
    );
    assert_eq!(
        parsed.crop_output_path,
        Some(PathBuf::from("debug-output/detect/crop.png"))
    );
    assert_eq!(
        parsed.controller_config,
        vec!["tooltip_gc_x.png".to_string()]
    );
    assert_eq!(parsed.threshold_min, Some(65));
    assert_eq!(parsed.threshold_max, Some(240));
    assert_eq!(parsed.threshold_similarity_tooltip, Some(0.02));
}

#[test]
fn parses_live_flags() {
    let parsed = ParsedArgs::parse(
        [
            "live".to_string(),
            "--root".to_string(),
            "/tmp/repo".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--exact-title".to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--controller".to_string(),
            "tooltip_gc_x.png".to_string(),
            "--frames".to_string(),
            "3".to_string(),
            "--interval-ms".to_string(),
            "250".to_string(),
            "--capture-scope".to_string(),
            "roi".to_string(),
            "--output-dir".to_string(),
            "debug-output/live".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::Live));
    assert_eq!(parsed.root, PathBuf::from("/tmp/repo"));
    assert_eq!(parsed.match_title.as_deref(), Some("Diablo IV"));
    assert!(parsed.exact_title);
    assert_eq!(parsed.preset.as_deref(), Some("1080p_SMF"));
    assert_eq!(parsed.mode.as_deref(), Some("controller"));
    assert_eq!(
        parsed.controller_config,
        vec!["tooltip_gc_x.png".to_string()]
    );
    assert_eq!(parsed.frames, Some(3));
    assert_eq!(parsed.interval_millis, Some(250));
    assert_eq!(parsed.capture_scope.as_deref(), Some("roi"));
    assert_eq!(parsed.output_dir, Some(PathBuf::from("debug-output/live")));
}

#[test]
fn parses_live_loop_flags_and_overlay() {
    let parsed = ParsedArgs::parse(
        [
            "live-loop".to_string(),
            "--root".to_string(),
            "/tmp/repo".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--exact-title".to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--frames".to_string(),
            "100".to_string(),
            "--interval-ms".to_string(),
            "16".to_string(),
            "--overlay".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::LiveLoop));
    assert_eq!(parsed.root, PathBuf::from("/tmp/repo"));
    assert_eq!(parsed.match_title.as_deref(), Some("Diablo IV"));
    assert!(parsed.exact_title);
    assert_eq!(parsed.preset.as_deref(), Some("1080p_SMF"));
    assert_eq!(parsed.mode.as_deref(), Some("controller"));
    assert_eq!(parsed.frames, Some(100));
    assert_eq!(parsed.interval_millis, Some(16));
    assert!(parsed.overlay);
    assert!(parsed.live_args().overlay);
}

#[test]
fn detect_tooltip_command_matches_real_controller_template_without_diablo() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| panic!("failed to resolve workspace root"));
    let output_root =
        create_temp_repo("detect_tooltip_command_matches_real_controller_template_without_diablo");

    let result = run(
        [
            "detect-tooltip".to_string(),
            "--root".to_string(),
            root.display().to_string(),
            "--input".to_string(),
            "assets/original/systempresets/1080p_SMF/Tooltips/tooltip_gc_x.png".to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--controller".to_string(),
            "tooltip_gc_x.png".to_string(),
            "--trace-output".to_string(),
            output_root.join("trace.json").display().to_string(),
            "--crop-output".to_string(),
            output_root.join("crop.png").display().to_string(),
            "--config-root".to_string(),
            output_root.display().to_string(),
        ],
        &invocation(root),
    )
    .unwrap_or_else(|error| panic!("detect-tooltip failed: {error}"));

    assert!(result.contains("\"selected_match\""));
    assert!(result.contains("\"candidate_count\""));
    assert!(result.contains("\"trace_output\""));
    assert!(result.contains("\"crop_output\""));
    assert!(output_root.join("trace.json").is_file());
    assert!(!output_root.join("crop.png").is_file());

    remove_dir(&output_root);
}

#[test]
fn detect_tooltip_reports_preset_threshold_defaults_and_thresholded_crop() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| panic!("failed to resolve workspace root"));
    let output_root =
        create_temp_repo("detect_tooltip_reports_preset_threshold_defaults_and_thresholded_crop");

    let result = run(
        [
            "detect-tooltip".to_string(),
            "--root".to_string(),
            root.display().to_string(),
            "--input".to_string(),
            "assets/original/systempresets/1080p_SMF/Tooltips/tooltip_gc_x.png".to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--controller".to_string(),
            "tooltip_gc_x.png".to_string(),
            "--trace-output".to_string(),
            output_root.join("trace.json").display().to_string(),
            "--crop-output".to_string(),
            output_root.join("crop.png").display().to_string(),
            "--config-root".to_string(),
            output_root.display().to_string(),
        ],
        &invocation(root),
    )
    .unwrap_or_else(|error| panic!("detect-tooltip failed: {error}"));

    assert!(result.contains("\"threshold_min\": 70"));
    assert!(result.contains("\"threshold_max\": 255"));
    assert!(result.contains("\"thresholded_crop_output\""));
    assert!(!output_root.join("crop-thresholded.png").is_file());

    remove_dir(&output_root);
}

#[test]
fn detect_tooltip_similarity_override_changes_result() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| panic!("failed to resolve workspace root"));
    let output_root = create_temp_repo("detect_tooltip_similarity_override_changes_result");
    let input_path = output_root.join("tooltip_gc_x_modified.png");
    write_modified_template_png(
        &root.join("assets/original/systempresets/1080p_SMF/Tooltips/tooltip_gc_x.png"),
        &input_path,
    );

    let default_result = run(
        [
            "detect-tooltip".to_string(),
            "--root".to_string(),
            root.display().to_string(),
            "--input".to_string(),
            input_path.display().to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--controller".to_string(),
            "tooltip_gc_x.png".to_string(),
            "--config-root".to_string(),
            output_root.display().to_string(),
        ],
        &invocation(&output_root),
    )
    .unwrap_or_else(|error| panic!("default detect-tooltip failed: {error}"));

    let strict_result = run(
        [
            "detect-tooltip".to_string(),
            "--root".to_string(),
            root.display().to_string(),
            "--input".to_string(),
            input_path.display().to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--controller".to_string(),
            "tooltip_gc_x.png".to_string(),
            "--threshold-similarity-tooltip".to_string(),
            "0.0".to_string(),
            "--config-root".to_string(),
            output_root.display().to_string(),
        ],
        &invocation(&output_root),
    )
    .unwrap_or_else(|error| panic!("strict detect-tooltip failed: {error}"));

    assert!(!default_result.contains("\"selected_match\": null"));
    assert!(strict_result.contains("\"selected_match\": null"));

    remove_dir(&output_root);
}

#[test]
fn workspace_report_lists_member_presence() {
    let root = create_temp_repo("workspace_report_lists_member_presence");
    create_file(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\n  \"crates/d4lc-core\",\n]\n",
    );
    create_dir(&root.join("crates/d4lc-core"));

    let report = workspace_report(&root).unwrap_or_else(|error| error);

    assert!(report.contains("WORKSPACE_REPORT"));
    assert!(report.contains("workspace_members=1"));
    assert!(report.contains("member crates/d4lc-core present"));

    remove_dir(&root);
}

#[test]
fn asset_languages_outputs_json() {
    let output = asset_languages();
    assert!(output.starts_with("{\n"));
    assert!(output.contains("\"enUS\""));
    assert!(output.ends_with("}\n"));
}

// --- OCR worker flag parsing ---

#[test]
fn parses_ocr_worker_flag_for_live_loop() {
    let parsed = ParsedArgs::parse(
        [
            "live-loop".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--exact-title".to_string(),
            "--frames".to_string(),
            "50".to_string(),
            "--ocr-worker".to_string(),
            "--ocr-interval-ms".to_string(),
            "500".to_string(),
            "--ocr-text-preview-chars".to_string(),
            "120".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::LiveLoop));
    assert!(parsed.ocr_worker);
    assert_eq!(parsed.ocr_interval_ms, Some(500));
    assert_eq!(parsed.ocr_text_preview_chars, Some(120));
    assert!(parsed.live_args().ocr_worker);
    assert_eq!(parsed.live_args().ocr_interval_ms, Some(500));
    assert_eq!(parsed.live_args().ocr_text_preview_chars, Some(120));
}

#[test]
fn live_loop_without_ocr_worker_flag_does_not_enable_ocr() {
    let parsed = ParsedArgs::parse(
        [
            "live-loop".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--frames".to_string(),
            "10".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert!(!parsed.ocr_worker);
    assert!(!parsed.live_args().ocr_worker);
    assert_eq!(parsed.ocr_interval_ms, None);
    assert_eq!(parsed.ocr_text_preview_chars, None);
}

#[test]
fn ocr_interval_ms_zero_is_accepted() {
    let parsed = ParsedArgs::parse(
        [
            "live-loop".to_string(),
            "--ocr-worker".to_string(),
            "--ocr-interval-ms".to_string(),
            "0".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.ocr_interval_ms, Some(0));
}

#[test]
fn ocr_text_preview_chars_zero_is_rejected() {
    let result = ParsedArgs::parse(
        [
            "live-loop".to_string(),
            "--ocr-text-preview-chars".to_string(),
            "0".to_string(),
        ],
        Path::new("."),
    );
    let error = match result {
        Err(e) => e,
        Ok(_) => panic!("expected parse to fail"),
    };

    assert!(
        error.to_string().contains("greater than zero"),
        "unexpected error: {error}"
    );
}

#[test]
fn help_text_documents_ocr_worker_flags() {
    let help = help_text();

    assert!(help.contains("--ocr-worker"));
    assert!(help.contains("--ocr-interval-ms"));
    assert!(help.contains("--ocr-text-preview-chars"));
}

#[test]
fn parses_manual_diablo_validation_bundle_flags() {
    let parsed = ParsedArgs::parse(
        [
            "manual-diablo-validation-bundle".to_string(),
            "--match".to_string(),
            "Diablo IV".to_string(),
            "--frames".to_string(),
            "20".to_string(),
            "--output-dir".to_string(),
            "debug-output/manual-diablo-iv/run-42".to_string(),
            "--dry-run".to_string(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("parse failed: {error}"));

    assert_eq!(parsed.command, Some(Command::ManualDiabloValidationBundle));
    assert_eq!(parsed.match_title.as_deref(), Some("Diablo IV"));
    assert_eq!(parsed.frames, Some(20));
    assert_eq!(
        parsed.output_dir,
        Some(PathBuf::from("debug-output/manual-diablo-iv/run-42"))
    );
    assert!(parsed.dry_run);
    assert!(parsed.manual_validation_args().dry_run);
}

#[test]
fn help_text_documents_manual_diablo_validation_bundle() {
    let help = help_text();

    assert!(help.contains("manual-diablo-validation-bundle"));
    assert!(help.contains("debug-output/manual-diablo-iv"));
    assert!(help.contains("--dry-run"));
}

#[test]
fn generated_help_covers_every_command_without_running_it() {
    let cli = ParsedArgs::cli();
    cli.clone().debug_assert();
    for command in cli.get_subcommands() {
        let name = command.get_name();
        let help = run(
            [name, "--help"],
            &invocation(Path::new("/nonexistent/d4lc-root")),
        )
        .unwrap_or_else(|error| panic!("{name} --help: {error}"));
        assert!(help.contains(&format!("d4lc {name}")), "{help}");
        assert!(help.contains("--root"), "{help}");
        assert!(run(["help", name], &invocation(Path::new("."))).is_ok());
    }
    for args in [vec![], vec!["help"], vec!["-h"], vec!["--help"]] {
        assert!(run(args, &invocation(Path::new("."))).is_ok());
    }
    assert!(run(["--version"], &invocation(Path::new(".")))
        .unwrap_or_else(|error| panic!("version: {error}"))
        .contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn preserves_aliases_coordinates_repeated_values_and_root_defaults() {
    let parsed = ParsedArgs::parse(
        [
            "--root",
            "before",
            "live-loop",
            "--root",
            "after",
            "--region",
            "-4,-5,6,7",
            "--delay",
            "0.125",
            "--ocr",
            "leptess",
            "--tessdata-path",
            "packs",
            "--controller",
            "first.png",
            "--controller",
            "second.png",
            "--overlay",
            "--overlay",
            "--mouse-x",
            "-20",
            "--mouse-y=-30",
        ],
        Path::new("fallback"),
    )
    .unwrap_or_else(|error| panic!("legacy flags: {error}"));
    assert_eq!(parsed.root, Path::new("after"));
    assert_eq!(parsed.controller_config, ["first.png", "second.png"]);
    let across_command = ParsedArgs::parse(
        [
            "--controller",
            "first.png",
            "live-loop",
            "--controller",
            "second.png",
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("global controller flags: {error}"));
    assert_eq!(
        across_command.controller_config,
        ["first.png", "second.png"]
    );
    assert_eq!(
        parsed.manual_region,
        Some(d4lc_core::Rect {
            x: -4,
            y: -5,
            width: 6,
            height: 7
        })
    );
    assert_eq!(parsed.delay_millis, Some(125));
    assert_eq!(parsed.ocr_backend.as_deref(), Some("leptess"));
    assert_eq!(parsed.tessdata_path.as_deref(), Some(Path::new("packs")));
    assert_eq!((parsed.mouse_x, parsed.mouse_y), (Some(-20), Some(-30)));
    assert!(parsed.overlay);
    let defaulted = ParsedArgs::parse(["asset-report"], Path::new("fallback"))
        .unwrap_or_else(|error| panic!("default root: {error}"));
    assert_eq!(defaulted.root, Path::new("fallback"));
    let explicit = ParsedArgs::parse(["asset-report", "--root", "."], Path::new("fallback"))
        .unwrap_or_else(|error| panic!("explicit root: {error}"));
    assert_eq!(explicit.root, Path::new("."));
}

#[test]
fn rejects_invalid_input_before_any_command_runs() {
    for args in [
        vec!["unknown-command"],
        vec!["asset-report", "--typo"],
        vec!["asset-report", "extra"],
        vec!["asset-report", "--root"],
        vec!["live-loop", "--frames", "0"],
        vec!["live-loop", "--frames", "-1"],
        vec!["live-loop", "--interval-ms", "nope"],
        vec!["live-loop", "--pid", "4294967296"],
        vec!["detect-tooltip", "--threshold-min", "256"],
        vec!["detect-tooltip", "--threshold-similarity-tooltip", "NaN"],
        vec!["detect-tooltip", "--threshold-similarity-tooltip", "inf"],
        vec!["live-once", "--delay", "NaN"],
        vec!["live-once", "--delay", "-0"],
        vec!["live-once", "--region", "1,2,3"],
    ] {
        assert!(
            ParsedArgs::parse(args.clone(), Path::new(".")).is_err(),
            "accepted {args:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn path_options_accept_non_utf8_names() {
    use std::os::unix::ffi::OsStringExt;
    let path = OsString::from_vec(b"capture-\xff.png".to_vec());
    let parsed = ParsedArgs::parse(
        [
            OsString::from("read-tooltip"),
            OsString::from("--input"),
            path.clone(),
        ],
        Path::new("."),
    )
    .unwrap_or_else(|error| panic!("non-UTF-8 path: {error}"));
    assert_eq!(parsed.input_path, Some(PathBuf::from(path)));
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| panic!("failed to resolve workspace root"))
}

#[test]
fn relative_and_default_outputs_land_in_the_current_directory_not_the_asset_root() {
    let root = workspace_root();
    let work = create_temp_repo("outputs_land_in_the_current_directory");
    let input = root.join("assets/original/systempresets/1080p_SMF/Tooltips/tooltip_gc_x.png");
    let detect = |extra: &[&str]| {
        let mut args = vec![
            "detect-tooltip".to_string(),
            "--root".to_string(),
            root.display().to_string(),
            "--config-root".to_string(),
            work.join("config").display().to_string(),
            "--input".to_string(),
            input.display().to_string(),
            "--preset".to_string(),
            "1080p_SMF".to_string(),
            "--mode".to_string(),
            "controller".to_string(),
            "--controller".to_string(),
            "tooltip_gc_x.png".to_string(),
        ];
        args.extend(extra.iter().map(|arg| (*arg).to_string()));
        run(args, &invocation(&work)).unwrap_or_else(|error| panic!("detect-tooltip: {error}"))
    };

    detect(&["--trace-output", "relative-trace.json"]);
    detect(&[]);

    assert!(work.join("relative-trace.json").is_file());
    assert!(!root.join("relative-trace.json").exists());
    assert!(work.join("debug-output/detect/detect-trace.json").is_file());
    remove_dir(&work);
}

#[test]
fn the_asset_root_is_the_flag_then_the_environment_then_the_current_directory() {
    let root = workspace_root();
    let empty = create_temp_repo("asset_root_precedence");
    let packaged = Invocation {
        cwd: empty.clone(),
        asset_root: Some(root.clone()),
        ..Invocation::default()
    };

    let presets = run(["asset-presets"], &packaged)
        .unwrap_or_else(|error| panic!("asset root from the environment: {error}"));
    assert!(presets.contains("1080p_SMF"), "{presets}");
    let flagged = run(
        ["asset-presets", "--root", &empty.display().to_string()],
        &packaged,
    )
    .err()
    .unwrap_or_else(|| panic!("--root must win over D4LC_ASSET_ROOT"));
    assert!(flagged.contains(&empty.display().to_string()), "{flagged}");
    let from_cwd = run(["asset-presets"], &invocation(&empty))
        .err()
        .unwrap_or_else(|| panic!("without --root and D4LC_ASSET_ROOT the cwd is the root"));
    assert!(
        from_cwd.contains(&empty.display().to_string()),
        "{from_cwd}"
    );
    remove_dir(&empty);
}

#[test]
fn settings_come_from_the_config_root_with_the_desktop_app_precedence() {
    let base = create_temp_repo("settings_come_from_the_config_root");
    let settings = |dir: &str, preset: &str, language: &str| {
        let root = base.join(dir);
        create_file(
            &root.join(d4lc_config::SETTINGS_PATH),
            &format!(
                "{{ \"SelectedSystemPreset\": \"{preset}\", \"SelectedAffixLanguage\": \"{language}\" }}"
            ),
        );
        root
    };
    let assets = settings("assets", "2160p_HSF", "esES");
    let env_root = settings("env", "2160p_SSF", "deDE");
    let flag_root = settings("flag", "1080p_SSF", "frFR");
    let xdg_root = settings("xdg", "1600p_SMF", "itIT");
    let report = |extra: &[&str], invocation: &Invocation| -> Value {
        let mut args = vec![
            "settings-report".to_string(),
            "--root".to_string(),
            assets.display().to_string(),
        ];
        args.extend(extra.iter().map(|arg| (*arg).to_string()));
        let output = run(args, invocation).unwrap_or_else(|error| panic!("{error}"));
        serde_json::from_str(&output).unwrap_or_else(|error| panic!("{error}: {output}"))
    };
    let with_env = Invocation {
        cwd: base.clone(),
        config_root: Some(env_root.clone()),
        xdg_config_root: Some(xdg_root.clone()),
        ..Invocation::default()
    };

    let from_env = report(&[], &with_env);
    assert_eq!(from_env["selected_system_preset"], "2160p_SSF");
    assert_eq!(from_env["selected_affix_language"], "deDE");
    assert_eq!(
        from_env["config"]["config_root"],
        env_root.display().to_string()
    );
    assert_eq!(from_env["config"]["config_root_source"], "D4LC_CONFIG_ROOT");
    assert_eq!(from_env["config"]["settings"]["status"], "loaded");

    let from_flag = report(
        &["--config-root", &flag_root.display().to_string()],
        &with_env,
    );
    assert_eq!(from_flag["selected_system_preset"], "1080p_SSF");
    assert_eq!(from_flag["config"]["config_root_source"], "--config-root");

    let from_xdg = report(
        &[],
        &Invocation {
            config_root: None,
            ..with_env.clone()
        },
    );
    assert_eq!(from_xdg["selected_affix_language"], "itIT");
    assert_eq!(from_xdg["config"]["config_root_source"], "xdg");

    let no_root = run(
        ["settings-report"],
        &Invocation {
            config_root: None,
            xdg_config_root: None,
            ..with_env.clone()
        },
    )
    .err()
    .unwrap_or_else(|| panic!("no config root must fail instead of using '.'"));
    assert!(no_root.contains("--config-root"), "{no_root}");
    remove_dir(&base);
}

#[test]
fn settings_report_tells_missing_from_loaded_and_stops_on_a_broken_file() {
    let base = create_temp_repo("settings_report_status");
    let config = base.join("config");
    let args = [
        "settings-report".to_string(),
        "--config-root".to_string(),
        config.display().to_string(),
    ];

    let missing: Value = serde_json::from_str(
        &run(args.clone(), &invocation(&base)).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(missing["config"]["settings"]["status"], "missing");
    assert_eq!(missing["config"]["settings"]["effect"], "defaults");
    assert_eq!(missing["config"]["controllers"]["status"], "missing");

    create_file(&config.join(d4lc_config::SETTINGS_PATH), "{ broken");
    let error = run(args, &invocation(&base))
        .err()
        .unwrap_or_else(|| panic!("a broken settings file must not be reported"));
    assert!(error.contains("is invalid"), "{error}");
    assert!(
        error.contains(
            &config
                .join(d4lc_config::SETTINGS_PATH)
                .display()
                .to_string()
        ),
        "{error}"
    );
    remove_dir(&base);
}

/// A run from `cwd` with no D4LC_* variables and no XDG config directory.
fn invocation(cwd: &Path) -> Invocation {
    Invocation {
        cwd: cwd.to_path_buf(),
        ..Invocation::default()
    }
}

fn create_temp_repo(test_name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|_| "time_error".to_string());
    let root = env::temp_dir().join(format!("d4lc-{test_name}-{suffix}"));
    create_dir(&root);
    root
}

fn create_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        create_dir(parent);
    }
    fs::write(path, contents).unwrap_or_else(|error| {
        panic!("failed to write {}: {error}", path.display());
    });
}

fn create_dir(path: &Path) {
    fs::create_dir_all(path).unwrap_or_else(|error| {
        panic!("failed to create {}: {error}", path.display());
    });
}

fn remove_dir(path: &Path) {
    fs::remove_dir_all(path).unwrap_or_else(|error| {
        panic!("failed to remove {}: {error}", path.display());
    });
}

fn write_modified_template_png(input: &Path, output: &Path) {
    if let Some(parent) = output.parent() {
        create_dir(parent);
    }
    let mut image = ImageReader::open(input)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", input.display()))
        .decode()
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", input.display()))
        .to_rgba8();
    let pixel = image.get_pixel_mut(0, 0);
    pixel.0[0] = pixel.0[0].wrapping_add(1);
    image
        .save(output)
        .unwrap_or_else(|error| panic!("failed to save {}: {error}", output.display()));
}
