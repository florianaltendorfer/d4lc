mod args;
mod capture;
mod config_recovery;
mod detect;
mod live;
mod manual_validation;
mod ocr;
#[cfg(all(test, feature = "leptess-backend"))]
mod ocr_handoff_equivalence;
mod perf_baseline;
mod perf_detect;
mod perf_ocr;
mod roots;
#[cfg(test)]
mod runtime_manifest;
#[cfg(test)]
mod tests;
mod upstream;

use args::{help_text, Command, ParsedArgs};
use d4lc_assets::{AssetRepository, LANGUAGES};
use roots::{cli_overrides_json, Invocation, Roots};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run_from_env() {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run_from_env() -> Result<String, String> {
    run(std::env::args_os().skip(1), &Invocation::from_process()?)
}

fn run<I, T>(args: I, invocation: &Invocation) -> Result<String, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let parsed = match ParsedArgs::parse(args, &invocation.default_asset_root()) {
        Ok(parsed) => parsed,
        Err(error) if !error.use_stderr() => return Ok(error.to_string()),
        Err(error) => return Err(error.to_string().trim_start_matches("error: ").to_string()),
    };
    let roots = Roots::resolve(&parsed.root, parsed.config_root.as_deref(), invocation);
    let write_path = parsed
        .write_path
        .as_deref()
        .map(|path| roots.work_path(path));

    match parsed.command {
        None => Ok(help_text()),
        Some(Command::WorkspaceReport) => workspace_report(&roots.assets),
        Some(Command::UpstreamReport) => upstream::upstream_report(&roots.assets),
        Some(Command::UpstreamManifest) => {
            upstream::upstream_manifest_command(&roots.assets, write_path.as_deref())
        }
        Some(Command::UpstreamProvenance) => {
            upstream::upstream_provenance_command(&roots.assets, write_path.as_deref())
        }
        Some(Command::UpstreamChanged) => {
            let remote_head = parsed
                .remote_head
                .as_deref()
                .ok_or_else(|| "upstream-changed requires --remote-head <commit>".to_string())?;
            upstream::upstream_changed_command(&roots.assets, remote_head)
        }
        Some(Command::UpstreamAssetParity) => {
            upstream::upstream_asset_parity_command(&roots.assets)
        }
        Some(Command::AssetReport) => asset_report(&roots.assets),
        Some(Command::AssetPresets) => asset_presets(&roots.assets),
        Some(Command::AssetLanguages) => Ok(asset_languages()),
        Some(Command::AssetTemplateCounts) => {
            asset_template_counts(&roots.assets, parsed.preset.as_deref())
        }
        Some(Command::AssetControllerImages) => asset_controller_images(
            &roots,
            parsed.preset.as_deref(),
            parsed.language.as_deref(),
            &parsed.controller_config,
        ),
        Some(Command::SettingsReport) => settings_report(&roots),
        Some(Command::X11Windows) => capture::x11_windows(&parsed.capture_args()),
        Some(Command::CaptureWindowRaw) => {
            capture::capture_window_raw(&roots, &parsed.capture_args())
        }
        Some(Command::CursorPosition) => capture::cursor_position(&parsed.capture_args()),
        Some(Command::LiveOnce) => live::live_once(&roots, &parsed.live_args()),
        Some(Command::LiveLoop) => live::live_loop(&roots, &parsed.live_args()),
        Some(Command::Live) => live::live(&roots, &parsed.live_args()),
        Some(Command::PerfDetect) => {
            perf_detect::perf_detect(&roots, &parsed.live_args(), write_path.as_deref())
        }
        Some(Command::PerfOcrReport) => perf_ocr::perf_ocr_report(&roots, &parsed.perf_ocr_args()),
        Some(Command::PerfDetectOffline) => {
            perf_baseline::perf_detect_offline(&roots, &parsed.perf_detect_offline_args())
        }
        Some(Command::PerfFuzzy) => perf_baseline::perf_fuzzy(&roots, &parsed.perf_fuzzy_args()),
        Some(Command::CaptureDebugBundle) => {
            capture::capture_debug_bundle(&roots, &parsed.capture_args())
        }
        Some(Command::ManualDiabloValidationBundle) => {
            manual_validation::manual_diablo_validation_bundle(
                &roots,
                &parsed.manual_validation_args(),
            )
        }
        Some(Command::DetectTooltip) => detect::detect_tooltip(&roots, &parsed.detect_args()),
        Some(Command::ReadTooltip) => ocr::read_tooltip(&roots, &parsed.ocr_args()),
        Some(Command::ConfigRecover) => config_recovery::config_recover_command(
            roots.config_path()?,
            parsed.recovery_action(),
            parsed.record.as_deref(),
        ),
    }
}

fn workspace_report(root: &Path) -> Result<String, String> {
    let cargo_toml = root.join("Cargo.toml");
    let cargo_text = fs::read_to_string(&cargo_toml)
        .map_err(|error| format!("failed to read {}: {error}", cargo_toml.display()))?;
    let members = parse_workspace_members(&cargo_text);

    let mut output = String::new();
    output.push_str("WORKSPACE_REPORT\n");
    append_presence(&mut output, "root", root);
    append_presence(&mut output, "cargo_toml", &cargo_toml);
    output.push_str(&format!("workspace_members={}\n", members.len()));

    for member in members {
        let path = root.join(&member);
        let status = if path.is_dir() { "present" } else { "missing" };
        output.push_str(&format!("member {member} {status}\n"));
    }

    Ok(output)
}

fn append_presence(output: &mut String, label: &str, path: &Path) {
    let status = if path.exists() { "present" } else { "missing" };
    output.push_str(&format!("{label}={} {status}\n", path.display()));
}

fn parse_workspace_members(cargo_toml: &str) -> Vec<String> {
    let mut in_members = false;
    let mut members = Vec::new();

    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members") && trimmed.contains('[') {
            in_members = true;
            continue;
        }

        if in_members && trimmed.starts_with(']') {
            break;
        }

        if in_members {
            members.extend(upstream::extract_quoted_values(trimmed));
        }
    }

    members
}

fn asset_report(root: &Path) -> Result<String, String> {
    let repo = AssetRepository::new(root);
    let data = repo.data_inventory()?;
    let presets = repo.list_system_presets()?;
    Ok(to_json_text(&json!({
        "schema_version": 1,
        "asset_manifest": repo.asset_manifest_path().display().to_string(),
        "data_file_count": data.len(),
        "preset_count": presets.len(),
        "languages": LANGUAGES,
        "data": data.iter().map(data_file_json).collect::<Vec<_>>(),
        "presets": presets.iter().map(preset_json).collect::<Vec<_>>(),
    })))
}

fn asset_presets(root: &Path) -> Result<String, String> {
    let presets = AssetRepository::new(root).list_system_presets()?;
    Ok(to_json_text(&json!({
        "presets": presets.iter().map(preset_json).collect::<Vec<_>>(),
    })))
}

fn asset_languages() -> String {
    to_json_text(&json!({ "languages": LANGUAGES }))
}

fn asset_template_counts(root: &Path, preset: Option<&str>) -> Result<String, String> {
    let repo = AssetRepository::new(root);
    let preset_names = match preset {
        Some(preset) => vec![preset.to_string()],
        None => repo
            .list_system_presets()?
            .into_iter()
            .map(|metadata| metadata.preset_name)
            .collect(),
    };
    let template_counts = preset_names
        .iter()
        .map(|preset_name| {
            let counts = repo.template_counts_for_preset(preset_name)?;
            Ok(json!({ "preset": preset_name, "counts": counts }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(to_json_text(&json!({ "template_counts": template_counts })))
}

fn settings_report(roots: &Roots) -> Result<String, String> {
    let files = roots.config_files()?;
    let config = files.identity_json(json!({}));
    let settings = files
        .clone()
        .into_settings()
        .map_err(|error| format!("settings-report aborted: {error}"))?;
    let controllers = files
        .into_controllers()
        .map_err(|error| format!("settings-report aborted: {error}"))?;
    Ok(to_json_text(&json!({
        "config": config,
        "settings_path": d4lc_config::SETTINGS_PATH.replace('\\', "/"),
        "controllers_path": d4lc_config::CONTROLLERS_PATH.replace('\\', "/"),
        "selected_system_preset": settings.selected_system_preset,
        "selected_affix_language": settings.selected_affix_language,
        "controller_mode": settings.controller_mode,
        "scan_height_percent": settings.scan_height_percent,
        "type_area_offset_left": settings.type_area_offset_left,
        "threshold_min": settings.threshold_min,
        "threshold_max": settings.threshold_max,
        "threshold_similarity_tooltip": settings.threshold_similarity_tooltip,
        "controller_config": controllers,
    })))
}

fn asset_controller_images(
    roots: &Roots,
    preset: Option<&str>,
    language: Option<&str>,
    configured_names: &[String],
) -> Result<String, String> {
    let files = roots.config_files()?;
    let config = files.identity_json(cli_overrides_json(&[
        ("preset", json!(preset)),
        ("language", json!(language)),
        ("controller", json!(configured_names)),
    ]));
    let settings = files.clone().into_settings()?;
    let preset = preset.map_or(settings.selected_system_preset, str::to_string);
    let language = language.map_or(settings.selected_affix_language, str::to_string);
    let repo = roots.preset_repo(&preset)?;
    let available = repo.controller_image_names(&preset, &language)?;
    let configured = if configured_names.is_empty() {
        files.into_controllers()?
    } else {
        configured_names.to_vec()
    };
    let active = repo.active_controller_image_names(&preset, &language, &configured)?;
    Ok(to_json_text(&json!({
        "config": config,
        "preset": preset,
        "language": language,
        "available": available,
        "configured": configured,
        "active": active,
    })))
}

fn data_file_json(item: &d4lc_assets::DataFileSummary) -> Value {
    json!({
        "category": item.category,
        "language": item.language,
        "items": item.item_count,
        "bytes": item.bytes,
        "path": item.path,
    })
}

fn preset_json(preset: &d4lc_assets::SystemPresetMetadata) -> Value {
    json!({
        "name": preset.preset_name,
        "file_name": preset.file_name,
        "status": preset.status,
        "resolution": preset.resolution,
        "config": preset.config,
        "tooltip_width": preset.tooltip_width,
        "tooltip_height_type": preset.tooltip_height_type,
        "ready_languages": preset.ready_languages,
    })
}

/// The indented text of a JSON report, with a trailing newline.
pub(crate) fn to_json_text(value: &Value) -> String {
    // A `Value` tree always serializes.
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}
