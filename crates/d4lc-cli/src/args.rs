use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{parser::ValueSource, CommandFactory, FromArgMatches, Parser, Subcommand};
use d4lc_config::parse_manual_region;

use super::{capture, detect, live, manual_validation, ocr, perf_baseline, perf_ocr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub(super) enum Command {
    /// Report workspace members and paths.
    WorkspaceReport,
    /// Report the pinned upstream baseline (developer checkout only).
    UpstreamReport,
    /// Generate the upstream file manifest (developer checkout only).
    UpstreamManifest,
    /// Generate upstream license provenance (developer checkout only).
    UpstreamProvenance,
    /// Compare the baseline with --remote-head.
    UpstreamChanged,
    /// Verify imported assets against their provenance manifest (developer checkout only).
    UpstreamAssetParity,
    /// Report data and preset inventory.
    AssetReport,
    /// List available system presets.
    AssetPresets,
    /// List supported game languages.
    AssetLanguages,
    /// Count templates in system presets.
    AssetTemplateCounts,
    /// Report available and selected controller images.
    AssetControllerImages,
    /// Report persisted settings.
    SettingsReport,
    /// List matching X11 windows.
    X11Windows,
    /// Capture a window to PNG.
    CaptureWindowRaw,
    /// Report the cursor position relative to a window.
    CursorPosition,
    /// Capture and process one live frame.
    LiveOnce,
    /// Run the live capture loop, optionally with overlay and OCR.
    LiveLoop,
    /// Run continuous live detection.
    Live,
    /// Benchmark live capture and detection.
    PerfDetect,
    /// Benchmark OCR scheduling and write a report.
    PerfOcrReport,
    /// Benchmark detection on --input, or on the fixtures of a developer checkout.
    PerfDetectOffline,
    /// Benchmark fuzzy parsing.
    PerfFuzzy,
    /// Capture a debug bundle.
    CaptureDebugBundle,
    /// Prepare a timestamped bundle under ./debug-output/manual-diablo-iv (or --output-dir).
    ManualDiabloValidationBundle,
    /// Detect a tooltip in --input without a running game.
    DetectTooltip,
    /// Read a tooltip with in-process OCR from --input.
    ReadTooltip,
    /// List interrupted configuration changes, or resolve one with --complete or --rollback [--record <id>].
    ConfigRecover,
}

/// Shared options stay global for compatibility with existing scripts, which
/// may place them before or after the command. Command handlers retain their
/// domain-specific defaults and requirements.
#[derive(Debug, Clone, PartialEq, Parser)]
#[command(
    name = "d4lc",
    version,
    about = "Diablo IV Companion diagnostics and development tools",
    args_override_self = true,
    after_help = "Settings come from Config/Settings.json and Config/Controllers.json under the config root (--config-root, else $D4LC_CONFIG_ROOT, else ~/.config/d4lc, as in the desktop app) unless flags override them.\nGame data comes from the asset root (--root, else $D4LC_ASSET_ROOT, else the current directory); the packaged launchers set $D4LC_ASSET_ROOT.\nRelative input and output paths, and the default debug-output/ directories, resolve against the current directory.\nLive commands write no debug files unless an output flag is given.\nUse --delay-seconds <s> on live commands when switching focus from a terminal to Diablo IV.\nBenchmark reports default to debug-output/perf/.\nupstream-report, upstream-manifest, upstream-provenance, upstream-asset-parity and perf-detect-offline without --input need a developer checkout."
)]
pub(super) struct ParsedArgs {
    #[command(subcommand)]
    pub(super) command: Option<Command>,
    /// Read-only asset root [default: $D4LC_ASSET_ROOT, else the current directory].
    #[arg(
        long,
        global = true,
        default_value = ".",
        hide_default_value = true,
        value_name = "PATH"
    )]
    pub(super) root: PathBuf,
    /// User configuration root [default: $D4LC_CONFIG_ROOT, else ~/.config/d4lc].
    #[arg(long, global = true, value_name = "PATH")]
    pub(super) config_root: Option<PathBuf>,
    /// Report output path.
    #[arg(long = "write", global = true, value_name = "PATH")]
    pub(super) write_path: Option<PathBuf>,
    #[arg(long, global = true, value_name = "COMMIT")]
    pub(super) remote_head: Option<String>,
    #[arg(long, global = true, value_name = "NAME")]
    pub(super) preset: Option<String>,
    #[arg(long, global = true, value_name = "CODE")]
    pub(super) language: Option<String>,
    #[arg(skip)]
    pub(super) controller_config: Vec<String>,
    #[arg(long = "input", global = true, value_name = "PNG")]
    pub(super) input_path: Option<PathBuf>,
    #[arg(long, global = true, value_name = "MODE")]
    pub(super) mode: Option<String>,
    #[arg(long, global = true, allow_negative_numbers = true)]
    pub(super) mouse_x: Option<i32>,
    #[arg(long, global = true, allow_negative_numbers = true)]
    pub(super) mouse_y: Option<i32>,
    #[arg(long, global = true)]
    pub(super) threshold_min: Option<u8>,
    #[arg(long, global = true)]
    pub(super) threshold_max: Option<u8>,
    #[arg(long, global = true, value_parser = parse_similarity_flag)]
    pub(super) threshold_similarity_tooltip: Option<f64>,
    #[arg(long = "trace-output", global = true, value_name = "JSON")]
    pub(super) trace_output_path: Option<PathBuf>,
    #[arg(long = "crop-output", global = true, value_name = "PNG")]
    pub(super) crop_output_path: Option<PathBuf>,
    #[arg(long = "match", global = true, value_name = "TITLE")]
    pub(super) match_title: Option<String>,
    #[arg(long = "class", global = true, value_name = "CLASS")]
    pub(super) class_name: Option<String>,
    #[arg(long, global = true)]
    pub(super) pid: Option<u32>,
    #[arg(long, global = true)]
    pub(super) exact_title: bool,
    #[arg(long = "output", global = true, value_name = "PNG")]
    pub(super) output_path: Option<PathBuf>,
    #[arg(long = "capture-output", global = true, value_name = "PNG")]
    pub(super) capture_output_path: Option<PathBuf>,
    #[arg(long, global = true, value_name = "DIR")]
    pub(super) output_dir: Option<PathBuf>,
    #[arg(long, visible_alias = "region", global = true, value_name = "X,Y,W,H", allow_hyphen_values = true, value_parser = parse_manual_region)]
    pub(super) manual_region: Option<d4lc_core::Rect>,
    #[arg(long, global = true, value_name = "BACKEND")]
    pub(super) backend: Option<String>,
    #[arg(long, global = true, value_parser = parse_capture_scope)]
    pub(super) capture_scope: Option<String>,
    /// Wait before capturing, in seconds (fractional values allowed).
    #[arg(long = "delay-seconds", visible_alias = "delay", global = true, value_name = "s", value_parser = parse_delay_millis)]
    pub(super) delay_millis: Option<u64>,
    #[arg(long, global = true, value_parser = parse_positive_integer)]
    pub(super) frames: Option<u64>,
    #[arg(long = "interval-ms", global = true)]
    pub(super) interval_millis: Option<u64>,
    #[arg(long, global = true)]
    pub(super) overlay: bool,
    #[arg(long, global = true)]
    pub(super) summary_only: bool,
    /// Prepare the manual validation layout without capturing.
    #[arg(long, global = true)]
    pub(super) dry_run: bool,
    #[arg(long, visible_alias = "ocr", global = true, value_name = "BACKEND")]
    pub(super) ocr_backend: Option<String>,
    #[arg(long, global = true, value_name = "MODE")]
    pub(super) ocr_psm: Option<String>,
    #[arg(long = "ocr-text-output", global = true, value_name = "TXT")]
    pub(super) ocr_text_output_path: Option<PathBuf>,
    #[arg(long = "read-trace-output", global = true, value_name = "JSON")]
    pub(super) read_trace_output_path: Option<PathBuf>,
    #[arg(
        long = "tessdata",
        visible_alias = "tessdata-path",
        global = true,
        value_name = "DIR"
    )]
    pub(super) tessdata_path: Option<PathBuf>,
    /// Run OCR off the frame hot path.
    #[arg(long, global = true)]
    pub(super) ocr_worker: bool,
    #[arg(long, global = true)]
    pub(super) ocr_interval_ms: Option<u64>,
    #[arg(long, global = true, value_parser = parse_positive_integer)]
    pub(super) ocr_text_preview_chars: Option<u64>,
    #[arg(long = "write-json", global = true, value_name = "JSON")]
    pub(super) write_json_path: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(super) synthetic_crops: bool,
    /// config-recover: finish the interrupted change.
    #[arg(long, global = true, conflicts_with = "rollback")]
    pub(super) complete: bool,
    /// config-recover: restore the files to their state before the change.
    #[arg(long, global = true)]
    pub(super) rollback: bool,
    /// config-recover: the interrupted change to list or resolve (txn-<id>).
    #[arg(long, global = true, value_name = "ID")]
    pub(super) record: Option<String>,
}

impl ParsedArgs {
    pub(super) fn cli() -> clap::Command {
        // Global Append arguments replace the parent's list when used after
        // a subcommand. Keep controller lists local and join them in parse.
        let controller = clap::Arg::new("controller_config")
            .long("controller")
            .value_name("FILE")
            .help("Controller template filename; repeat for multiple images")
            .action(clap::ArgAction::Append);
        Self::command()
            .arg(controller.clone())
            .mut_subcommands(|command| command.arg(controller.clone()))
    }

    pub(super) fn parse<I, T>(args: I, default_root: &Path) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let argv = std::iter::once(OsString::from("d4lc")).chain(args.into_iter().map(Into::into));
        let matches = Self::cli().try_get_matches_from(argv)?;
        let mut parsed = Self::from_arg_matches(&matches)?;
        for level in std::iter::once(&matches).chain(matches.subcommand().map(|(_, args)| args)) {
            if let Some(controllers) = level.get_many::<String>("controller_config") {
                parsed.controller_config.extend(controllers.cloned());
            }
        }
        if matches.value_source("root") == Some(ValueSource::DefaultValue) {
            parsed.root = default_root.to_path_buf();
        }
        Ok(parsed)
    }

    pub(super) fn capture_args(&self) -> capture::CaptureArgs {
        capture::CaptureArgs {
            match_title: self.match_title.clone(),
            class_name: self.class_name.clone(),
            pid: self.pid,
            exact_title: self.exact_title,
            output_path: self.output_path.clone(),
            capture_output_path: self.capture_output_path.clone(),
            output_dir: self.output_dir.clone(),
            manual_region: self.manual_region,
            backend: self.backend.clone(),
            preset: self.preset.clone(),
            delay_millis: self.delay_millis,
        }
    }

    pub(super) fn live_args(&self) -> live::LiveArgs {
        live::LiveArgs {
            match_title: self.match_title.clone(),
            class_name: self.class_name.clone(),
            pid: self.pid,
            exact_title: self.exact_title,
            manual_region: self.manual_region,
            preset: self.preset.clone(),
            language: self.language.clone(),
            controller_config: self.controller_config.clone(),
            mode: self.mode.clone(),
            threshold_min: self.threshold_min,
            threshold_max: self.threshold_max,
            threshold_similarity_tooltip: self.threshold_similarity_tooltip,
            output_dir: self.output_dir.clone(),
            capture_output_path: self.capture_output_path.clone(),
            delay_millis: self.delay_millis,
            frames: self.frames,
            interval_millis: self.interval_millis,
            backend: self.backend.clone(),
            capture_scope: self.capture_scope.clone(),
            overlay: self.overlay,
            summary_only: self.summary_only,
            ocr_worker: self.ocr_worker,
            ocr_interval_ms: self.ocr_interval_ms,
            ocr_text_preview_chars: self.ocr_text_preview_chars,
        }
    }

    pub(super) fn manual_validation_args(&self) -> manual_validation::ManualValidationArgs {
        manual_validation::ManualValidationArgs {
            live: self.live_args(),
            dry_run: self.dry_run,
        }
    }

    pub(super) fn ocr_args(&self) -> ocr::OcrArgs {
        ocr::OcrArgs {
            input_path: self.input_path.clone(),
            preset: self.preset.clone(),
            language: self.language.clone(),
            backend: self.ocr_backend.clone(),
            psm: self.ocr_psm.clone(),
            text_output_path: self.ocr_text_output_path.clone(),
            trace_output_path: self.read_trace_output_path.clone(),
            tessdata_path: self.tessdata_path.clone(),
        }
    }

    pub(super) fn perf_ocr_args(&self) -> perf_ocr::PerfOcrArgs {
        perf_ocr::PerfOcrArgs {
            write_path: self.write_path.clone(),
            write_json_path: self.write_json_path.clone(),
            preset: self.preset.clone(),
            language: self.language.clone(),
            backend: self.ocr_backend.clone(),
            tessdata_path: self.tessdata_path.clone(),
            synthetic_crops: self.synthetic_crops,
        }
    }

    pub(super) fn perf_detect_offline_args(&self) -> perf_baseline::PerfDetectOfflineArgs {
        perf_baseline::PerfDetectOfflineArgs {
            input_path: self.input_path.clone(),
            runs: self.frames,
            write_path: self.write_path.clone(),
        }
    }

    pub(super) fn perf_fuzzy_args(&self) -> perf_baseline::PerfFuzzyArgs {
        perf_baseline::PerfFuzzyArgs {
            language: self.language.clone(),
            write_path: self.write_path.clone(),
        }
    }

    pub(super) fn recovery_action(&self) -> Option<d4lc_config::Recovery> {
        if self.complete {
            Some(d4lc_config::Recovery::Complete)
        } else if self.rollback {
            Some(d4lc_config::Recovery::Rollback)
        } else {
            None
        }
    }

    pub(super) fn detect_args(&self) -> detect::DetectArgs {
        detect::DetectArgs {
            input_path: self.input_path.clone(),
            preset: self.preset.clone(),
            language: self.language.clone(),
            controller_config: self.controller_config.clone(),
            mode: self.mode.clone(),
            mouse_x: self.mouse_x,
            mouse_y: self.mouse_y,
            threshold_min: self.threshold_min,
            threshold_max: self.threshold_max,
            threshold_similarity_tooltip: self.threshold_similarity_tooltip,
            trace_output_path: self.trace_output_path.clone(),
            crop_output_path: self.crop_output_path.clone(),
        }
    }
}

pub(super) fn help_text() -> String {
    ParsedArgs::cli().render_long_help().to_string()
}

fn parse_delay_millis(value: &str) -> Result<u64, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|error| format!("invalid delay value '{value}': {error}"))?;
    if seconds.is_sign_negative() || !seconds.is_finite() {
        return Err(format!(
            "delay must be a finite non-negative value; got '{value}'"
        ));
    }
    Ok((seconds * 1000.0).round() as u64)
}

fn parse_similarity_flag(value: &str) -> Result<f64, String> {
    let similarity = value.parse::<f64>().map_err(|error| {
        format!("invalid --threshold-similarity-tooltip value '{value}': {error}")
    })?;
    if !similarity.is_finite() || similarity.is_sign_negative() {
        return Err(format!(
            "--threshold-similarity-tooltip must be a finite non-negative number; got '{value}'"
        ));
    }
    Ok(similarity)
}

fn parse_capture_scope(value: &str) -> Result<String, String> {
    d4lc_capture::CaptureScope::parse(value)?;
    Ok(value.to_owned())
}

fn parse_positive_integer(value: &str) -> Result<u64, String> {
    let number = value.parse::<u64>().map_err(|error| error.to_string())?;
    if number == 0 {
        return Err("must be greater than zero".to_string());
    }
    Ok(number)
}
