//! The Debug screen's action handlers and their `last_debug_action` /
//! Logging-screen outcome records.

use std::path::Path;

use d4lc_gui::debug::{DebugEffect, DebugViewModel};

use super::output::{project_debug_panels, project_perf_rows_to_ui};
use crate::app_state::{AppState, DebugAction, DebugActionStatus};
use crate::runtime_bridge::DebugCommandController;
use crate::slint_generated::AppWindow;

/// Reason text appended to the Logging screen when a Debug-screen
/// action is blocked because the `real-runtime` feature (which
/// exposes the `d4lc_capture` + `image` deps) is not enabled in
/// the current build.
#[cfg(not(feature = "real-runtime"))]
const BLOCKED_REASON_REAL_RUNTIME_DISABLED: &str = "real-runtime feature is disabled in this build; TakeScreenshot requires d4lc_capture + image (see Cargo.toml real-runtime feature)";

/// Reason text appended to the Logging screen when
/// `ExportDebugImages` / `ReloadPresetImages` are requested but
/// the `DebugCommandController` was not wired (or no live loop
/// is running).
const BLOCKED_REASON_NO_CONTROLLER: &str =
    "DebugCommandController is not installed; cannot drive live-runtime debug commands";

/// Reason text appended to the Logging screen when a crash-log
/// export is requested while the opt-in diagnostics toggle is off.
/// Diagnostics export is a local copy for a GitHub issue the user
/// opens manually — there is no upload path anywhere in the app.
const BLOCKED_REASON_DIAGNOSTICS_EXPORT_DISABLED: &str = "diagnostics export is disabled (opt-in in Settings); the app never uploads crash logs — enable diagnostics export, then export a log to attach it to a GitHub issue you open manually";

/// Reason text appended to the Logging screen when a crash-log
/// export is requested but no crash-log directory could be resolved
/// (no HOME / XDG data dir available).
const BLOCKED_REASON_NO_CRASH_DIR: &str =
    "no crash-log directory could be resolved (HOME / XDG data dir unavailable)";

/// Handle take-screenshot callback.
///
/// When the `real-runtime` feature is enabled, the handler asks
/// the `screenshot` helper to perform a real capture +
/// PNG-encode + disk-write of the selected window. The result
/// is stamped as `Applied` (with the output path) or `Failed`
/// (with the error) on `last_debug_action` and appended to the
/// Logging screen.
///
/// Without `real-runtime` the action is `Blocked`; no placeholder
/// screenshot is produced.
pub fn handle_take_screenshot(app: &AppWindow, state: &mut AppState) {
    let effects = apply_take_screenshot_state(state);
    dispatch_debug_effects(&effects, app, state);
    // A successful capture writes a new PNG into the debug-output
    // directory, so re-project the artifact/stage panels from disk: the
    // link appears without a restart (upstream re-binds its live bitmap
    // through `ScreenProcessItemTooltipReadyMessage`).
    project_debug_panels(
        app,
        state,
        &crate::actions::debug::screenshot::default_screenshot_dir(),
    );
}

/// State-only portion of `handle_take_screenshot`. Public so tests
/// can exercise the state-mutation path on headless runners that
/// cannot construct a Slint `AppWindow`.
pub fn apply_take_screenshot_state(state: &mut AppState) -> Vec<DebugEffect> {
    let mut vm = DebugViewModel::new(state.debug().clone());
    let effects = vm.take_screenshot();
    *state.debug_mut() = vm.state;
    record_take_screenshot_outcome(state);
    effects
}

/// Record the outcome of a TakeScreenshot invocation.
///
/// When the `real-runtime` feature is enabled, the real capture
/// path is invoked through `screenshot::attempt_real_screenshot`.
/// In the default (non-feature) build, the handler stays on the
/// Blocked path.
#[cfg(feature = "real-runtime")]
fn record_take_screenshot_outcome(state: &mut AppState) {
    use crate::actions::debug::screenshot::{
        attempt_real_screenshot, default_screenshot_dir, default_window_query, ScreenshotOutcome,
    };

    let dir = default_screenshot_dir();
    let outcome = attempt_real_screenshot(&default_window_query(), &dir);

    match outcome {
        ScreenshotOutcome::Applied {
            path,
            width,
            height,
            bytes_written,
        } => {
            state.debug_mut().debug_artifact_count =
                state.debug_mut().debug_artifact_count.saturating_add(1);
            let record = state.record_debug_action(
                DebugAction::TakeScreenshot,
                DebugActionStatus::Applied,
                format!(
                    "wrote {width}x{height} PNG ({bytes_written} bytes) to {}",
                    path.display()
                ),
            );
            push_applied_log(state, &record);
        }
        ScreenshotOutcome::Failed { reason } => {
            let record = state.record_debug_action(
                DebugAction::TakeScreenshot,
                DebugActionStatus::Failed,
                reason.clone(),
            );
            push_failed_log(state, &record);
        }
    }
}

/// Non-`real-runtime` build: the screenshot is `Blocked`.
#[cfg(not(feature = "real-runtime"))]
fn record_take_screenshot_outcome(state: &mut AppState) {
    let record = state.record_debug_action(
        DebugAction::TakeScreenshot,
        DebugActionStatus::Blocked,
        BLOCKED_REASON_REAL_RUNTIME_DISABLED,
    );
    push_blocked_log(state, &record);
}

/// Handle export-debug-images callback. See [`handle_take_screenshot`]
/// for the-blocked policy.
///
/// `controller` is the app-owned `DebugCommandController` that
/// owns the live-loop handle. When the controller is `None` the
/// handler records an `Blocked` and the user can still
/// see the request on the Logging screen.
///
/// Lock choreography: the same phases as
/// [`handle_reload_preset_images`] (see
/// [`apply_export_debug_images_state`]); the caller must not hold the
/// `AppState` lock.
pub fn handle_export_debug_images(
    app: &AppWindow,
    app_state: &std::sync::Arc<std::sync::Mutex<AppState>>,
    controller: Option<&DebugCommandController>,
) {
    let effects = apply_export_debug_images_state(app_state, controller);
    let mut state = lock_app_state(app_state);
    dispatch_debug_effects(&effects, app, &mut state);
    // A confirmed export writes a new bundle under the debug-output
    // directory; re-project so the artifact links, stage panels and the
    // per-stage timing table show the freshly written files.
    project_debug_panels(
        app,
        &mut state,
        &crate::actions::debug::screenshot::default_screenshot_dir(),
    );
}

/// State-only portion of `handle_export_debug_images`.
///
/// When the `real-runtime` feature is enabled and a live loop
/// is running, the controller drives a real
/// `prepare_app_runtime` + `process_one_frame` pass and writes
/// the bounded bundle. The `last_debug_action` record follows
/// the controller's confirmed outcome (`Applied` with the
/// bundle path, `Failed` with the error, or `Blocked`
/// when no live loop is running).
///
/// Lock choreography: snapshot the ViewModel and read the live capture
/// engine under the `AppState` lock, run the controller call (a full
/// one-shot runtime preparation, capture and OCR pass) with the lock
/// released, then re-lock to record the outcome. The caller must not
/// hold the `AppState` lock: the live loop takes it once per frame, and
/// the controller follows the lock order on [`DebugCommandController`].
pub fn apply_export_debug_images_state(
    app_state: &std::sync::Arc<std::sync::Mutex<AppState>>,
    controller: Option<&DebugCommandController>,
) -> Vec<DebugEffect> {
    // Phase 1 — ViewModel snapshot and capture-engine read under the lock.
    let (effects, live_capture_is_wayland_stream) = {
        let mut state = lock_app_state(app_state);
        let mut vm = DebugViewModel::new(state.debug().clone());
        let effects = vm.export_debug_images();
        *state.debug_mut() = vm.state;
        let wayland = state
            .capture_engine
            .as_ref()
            .is_some_and(|engine| engine.wayland_stream_geometry.is_some());
        (effects, wayland)
    };
    // Phase 2 — the export itself, with no AppState lock held.
    let outcome = match controller {
        Some(c) => c.export_debug_images(live_capture_is_wayland_stream),
        None => crate::runtime_bridge::DebugCommandOutcome::Blocked {
            reason: BLOCKED_REASON_NO_CONTROLLER.to_string(),
        },
    };
    // Phase 3 — record under a fresh lock.
    let mut state = lock_app_state(app_state);
    record_command_outcome(&mut state, DebugAction::ExportDebugImages, &outcome);
    effects
}

/// Handle reload-preset-images callback. See [`handle_take_screenshot`]
/// for the-blocked policy.
///
/// Lock choreography: the handler is phased — snapshot the ViewModel
/// under the lock, run the controller call (which stops the previous
/// loop without a join and prepares the runtime again) with the lock
/// released, then re-lock to record the outcome and dispatch effects.
/// The live-loop thread takes the `AppState` lock once per frame, so
/// the slow preparation must not run under it. See the stop contract on
/// [`crate::runtime_bridge::LiveLoopHandle`] and the regression test
/// `reload_preset_images_join_completes_without_holding_the_app_state_lock`.
pub fn handle_reload_preset_images(
    app: &AppWindow,
    app_state: &std::sync::Arc<std::sync::Mutex<AppState>>,
    controller: Option<&DebugCommandController>,
) {
    // Phase 1 — ViewModel snapshot under the AppState lock.
    let effects = {
        let mut state = lock_app_state(app_state);
        begin_reload_preset_images(&mut state)
    };
    // Phase 2 — join point: no AppState lock is held here (see the
    // choreography doc above).
    let outcome = reload_preset_images_outcome(controller);
    // Phase 3 — record + dispatch under a fresh lock.
    let mut state = lock_app_state(app_state);
    record_command_outcome(&mut state, DebugAction::ReloadPresetImages, &outcome);
    dispatch_debug_effects(&effects, app, &mut state);
}

/// State-only ViewModel snapshot of a reload-preset-images request
/// (phase 1 of [`handle_reload_preset_images`]). Must stay free of any
/// blocking work: the controller call that restarts the live loop happens
/// later, with the `AppState` lock released.
pub fn begin_reload_preset_images(state: &mut AppState) -> Vec<DebugEffect> {
    let mut vm = DebugViewModel::new(state.debug().clone());
    let effects = vm.reload_preset_images();
    *state.debug_mut() = vm.state;
    effects
}

/// Resolve the controller call for a reload-preset-images request.
///
/// When the `real-runtime` feature is enabled and a live loop
/// is running, the controller stops the live loop, calls
/// `prepare_app_runtime` to validate the rebuild, and starts a
/// new live loop. Without a controller (headless wiring) the
/// request is `Blocked`. The `last_debug_action` record
/// follows the controller's confirmed outcome (`Applied` only on
/// a confirmed rebuild + restart).
fn reload_preset_images_outcome(
    controller: Option<&DebugCommandController>,
) -> crate::runtime_bridge::DebugCommandOutcome {
    match controller {
        Some(c) => c.reload_preset_images(),
        None => crate::runtime_bridge::DebugCommandOutcome::Blocked {
            reason: BLOCKED_REASON_NO_CONTROLLER.to_string(),
        },
    }
}

/// Lock the shared `AppState`, recovering from poisoning like every
/// other `d4lc-app` lock site (poisoning only marks a panicking
/// writer; the state stays usable).
fn lock_app_state(
    app_state: &std::sync::Arc<std::sync::Mutex<AppState>>,
) -> std::sync::MutexGuard<'_, AppState> {
    match app_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Stamp the `last_debug_action` record + Logging screen line
/// for a `DebugCommandOutcome`. Keeps the policy in one place
/// so callers cannot drift between Applied / Failed / Blocked
/// handling.
fn record_command_outcome(
    state: &mut AppState,
    action: DebugAction,
    outcome: &crate::runtime_bridge::DebugCommandOutcome,
) {
    let mut reason = outcome.reason().to_string();
    if let Some(path) = outcome.artifact_path() {
        reason.push_str(&format!(" (bundle={})", path.display()));
    }
    let record = state.record_debug_action(action, outcome.status(), reason);
    match outcome.status() {
        DebugActionStatus::Applied => push_applied_log(state, &record),
        DebugActionStatus::Failed => push_failed_log(state, &record),
        DebugActionStatus::Blocked => push_blocked_log(state, &record),
    }
}

/// Handle the crash-log export callback.
///
/// Diagnostics are local-only: when the opt-in settings toggle
/// (`diagnostics-export`, default off) is enabled, this copies one
/// crash log from the local crash directory into
/// `<config_root>/crash-exports/` so the user can attach the file to
/// a GitHub issue they open manually. No network code exists on this
/// path and the application never uploads anything. When the toggle
/// is off the request is recorded as `Blocked`.
pub fn handle_export_crash_log(
    app: &AppWindow,
    state: &mut AppState,
    crash_dir: Option<&std::path::Path>,
    config_root: &Path,
    file_name: &str,
) {
    let effects = apply_export_crash_log_state(state, crash_dir, config_root, file_name);
    dispatch_debug_effects(&effects, app, state);
}

/// State-only portion of `handle_export_crash_log`. Public so tests
/// can exercise the gated local-copy path on headless runners.
pub fn apply_export_crash_log_state(
    state: &mut AppState,
    crash_dir: Option<&std::path::Path>,
    config_root: &Path,
    file_name: &str,
) -> Vec<DebugEffect> {
    let mut vm = DebugViewModel::new(state.debug().clone());
    let effects = vm.export_crash_log(file_name.to_string());
    *state.debug_mut() = vm.state;

    if !state.shell.settings.diagnostics_export_enabled {
        let record = state.record_debug_action(
            DebugAction::ExportCrashLog,
            DebugActionStatus::Blocked,
            BLOCKED_REASON_DIAGNOSTICS_EXPORT_DISABLED,
        );
        push_blocked_log(state, &record);
        return effects;
    }

    let Some(dir) = crash_dir else {
        let record = state.record_debug_action(
            DebugAction::ExportCrashLog,
            DebugActionStatus::Blocked,
            BLOCKED_REASON_NO_CRASH_DIR,
        );
        push_blocked_log(state, &record);
        return effects;
    };

    let outcome = export_crash_log_locally(dir, config_root, file_name);
    match outcome {
        Ok(export_path) => {
            let record = state.record_debug_action(
                DebugAction::ExportCrashLog,
                DebugActionStatus::Applied,
                format!(
                    "copied crash log {file_name} to {} — attach it manually to a GitHub issue; the app never uploads",
                    export_path.display()
                ),
            );
            push_applied_log(state, &record);
        }
        Err(error) => {
            let record = state.record_debug_action(
                DebugAction::ExportCrashLog,
                DebugActionStatus::Failed,
                format!("crash-log export failed for '{file_name}': {error}"),
            );
            push_failed_log(state, &record);
        }
    }
    effects
}

/// Local copy of one validated crash log into
/// `<config_root>/crash-exports/`. The caller must have read the
/// content through [`crate::crash_report::read_crash_log`], which
/// enforces the strict crash-log file-name guard before this join.
fn export_crash_log_locally(
    crash_dir: &std::path::Path,
    config_root: &Path,
    file_name: &str,
) -> Result<std::path::PathBuf, String> {
    let content = crate::crash_report::read_crash_log(crash_dir, file_name)
        .map_err(|error| format!("could not read crash log: {error}"))?;
    let export_dir = config_root.join("crash-exports");
    std::fs::create_dir_all(&export_dir)
        .map_err(|error| format!("could not create {}: {error}", export_dir.display()))?;
    let export_path = export_dir.join(file_name);
    std::fs::write(&export_path, content)
        .map_err(|error| format!("could not write {}: {error}", export_path.display()))?;
    Ok(export_path)
}

/// Handle reset-perf callback.
///
/// Real, observable state mutation: the `DebugViewModel` clears its
/// `timing_metrics` and resets `debug_artifact_count`, and the
/// `DebugActionRecord` is stamped as `Applied` so tests can assert
/// the difference between the two reset paths.
pub fn handle_reset_perf(app: &AppWindow, state: &mut AppState) {
    let effects = apply_reset_perf_state(state);
    dispatch_debug_effects(&effects, app, state);
    // The port's timing surface is the per-stage table fed from the last
    // debug capture; clearing the metrics must clear the table too so the
    // reset is visible (upstream clears its 11 chart series here,
    // DebugViewModel.cs:713-722).
    project_perf_rows_to_ui(app, state, PERF_ROWS_CLEARED_NOTE);
}

/// Note shown in the timing table after `Reset Perf` cleared it.
const PERF_ROWS_CLEARED_NOTE: &str =
    "Cleared by Reset Perf; run Export debug images to capture new per-stage timings.";

/// State-only portion of `handle_reset_perf`.
pub fn apply_reset_perf_state(state: &mut AppState) -> Vec<DebugEffect> {
    let mut vm = DebugViewModel::new(state.debug().clone());
    let effects = vm.reset_performance_results();
    *state.debug_mut() = vm.state;
    for row in &mut state.debug_mut().perf_rows {
        row.value_ms = None;
    }
    let record = state.record_debug_action(
        DebugAction::ResetPerformanceResults,
        DebugActionStatus::Applied,
        "reset_performance_results cleared debug panel timing_metrics and debug_artifact_count",
    );
    push_applied_log(state, &record);
    effects
}

/// Handle debug-info-toggled callback.
///
/// Upstream writes `Settings.IsDebugInfoEnabled` and saves on every flip
/// (`DebugViewModel.cs:168-178`); the pipeline gates its debug drawing on
/// the bit. The port keeps the bit on the debug panel state, persists it
/// through the `d4lc_config` load/save pair, and records the flip on the
/// Logging screen. `config_root` is the resolved user settings root.
pub fn handle_debug_info_toggled(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    enabled: bool,
) {
    let effects = apply_debug_info_toggled_state(state, enabled);
    persist_debug_info_bit(state, config_root, enabled);
    app.set_debug_info_enabled(enabled);
    dispatch_debug_effects(&effects, app, state);
}

/// Seed the DebugInfo toggle from the persisted bit (startup projection).
///
/// Returns the persisted value so the caller can write the root property;
/// the value is the same one `persist_debug_info_bit` writes, so the
/// toggle survives a restart.
pub fn seed_debug_info_from_settings(state: &mut AppState, config_root: &Path) -> bool {
    let settings = d4lc_config::load_app_settings(config_root).unwrap_or_default();
    state.debug_mut().debug_info_enabled = settings.is_debug_info_enabled;
    settings.is_debug_info_enabled
}

/// State-only portion of `handle_debug_info_toggled`.
pub fn apply_debug_info_toggled_state(state: &mut AppState, enabled: bool) -> Vec<DebugEffect> {
    let mut vm = DebugViewModel::new(state.debug().clone());
    let effects = vm.debug_info_toggled(enabled);
    *state.debug_mut() = vm.state;
    // The live loop reads the bit from the rules snapshot: bump the input
    // revision so the publisher republishes it and the
    // OCR worker's debug-bundle gate follows the toggle without a restart.
    state.bump_rules_input_revision();
    let record = state.record_debug_action(
        DebugAction::ToggleDebugInfo,
        DebugActionStatus::Applied,
        format!("debug_info_toggled(enabled={enabled}) persisted as Settings.IsDebugInfoEnabled"),
    );
    push_applied_log(state, &record);
    effects
}

/// Write `Settings.IsDebugInfoEnabled` through the shared
/// `d4lc_config` load/save pair.
///
/// The bit lives on the debug panel state, not on `SettingsPanelState`,
/// so the aggregated `SettingsEffect::PersistConfig` write
/// (`settings.apply_to_config`) cannot carry it; this targeted write
/// reuses the same two `d4lc_config` entry points and surfaces a refused
/// or failed save on the Logging screen exactly like
/// `persist_settings_with_warnings` does.
fn persist_debug_info_bit(state: &mut AppState, config_root: &Path, enabled: bool) {
    let (mut config, warnings) = match d4lc_config::load_app_settings_with_warnings(config_root) {
        Ok(loaded) => loaded,
        Err(error) => {
            state.append_log_message(
                format!("settings load failed: {error}"),
                crate::clock::hh_mm_now(),
            );
            return;
        }
    };
    config.is_debug_info_enabled = enabled;
    if let Err(error) = d4lc_config::save_app_settings(config_root, &config) {
        for warning in &warnings {
            state.append_log_message(warning.to_string(), crate::clock::hh_mm_now());
        }
        state.append_log_message(
            format!("settings save failed: {error}"),
            crate::clock::hh_mm_now(),
        );
    }
}

/// Handle topmost-toggled callback.
///
/// Slint's `set_window_topmost` is the real side effect; the persisted
/// `Settings.IsTopMost` bit is what makes the toggle survive a restart
/// (`DebugViewModel.cs:180-192` sends `TopMostStateChangedMessage` after
/// saving). The bit lives on `SettingsPanelState.overlay_top_most`, the
/// same field the overlay config dialog and the Settings tab write, so
/// this handler persists through the shared settings save path and the
/// startup projection in `main` reads the same field.
pub fn handle_topmost_toggled(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    topmost: bool,
) {
    let effects = apply_topmost_toggled_state(state, topmost);
    persist_topmost(state, config_root, topmost);
    dispatch_debug_effects(&effects, app, state);
}

/// Store + persist the topmost bit. `SettingsPanelState.overlay_top_most`
/// is the field `SettingsPanelState::from_config` seeds from
/// `AppSettings::is_top_most`, which is what `main` applies to the window
/// at startup — so this write is the whole "survives restart" chain.
fn persist_topmost(state: &mut AppState, config_root: &Path, topmost: bool) {
    state.shell.settings.overlay_top_most = topmost;
    state.bump_rules_input_revision();
    crate::actions::settings::persist_settings_with_warnings(state, config_root);
}

/// State-only portion of `handle_topmost_toggled`.
pub fn apply_topmost_toggled_state(state: &mut AppState, topmost: bool) -> Vec<DebugEffect> {
    let mut vm = DebugViewModel::new(state.debug().clone());
    let effects = vm.topmost_toggled(topmost);
    *state.debug_mut() = vm.state;
    let record = state.record_debug_action(
        DebugAction::ToggleTopMost,
        DebugActionStatus::Applied,
        format!("topmost_toggled(topmost={topmost}) set AppWindow::set_window_topmost"),
    );
    push_applied_log(state, &record);
    effects
}

/// Append a one-line "blocked" log entry to the
/// `LoggingViewModel` so the user sees the action + reason on the
/// Logging screen.
fn push_blocked_log(state: &mut AppState, record: &crate::app_state::DebugActionRecord) {
    state.append_log_message(
        format!(
            "Debug action {} BLOCKED (#{}): {}",
            record.action.as_str(),
            record.monotonic,
            record.reason
        ),
        crate::clock::hh_mm_now(),
    );
}

/// Append a one-line "applied" log entry to the
/// `LoggingViewModel` so the user sees the action + reason on the
/// Logging screen.
fn push_applied_log(state: &mut AppState, record: &crate::app_state::DebugActionRecord) {
    state.append_log_message(
        format!(
            "Debug action {} applied (#{}): {}",
            record.action.as_str(),
            record.monotonic,
            record.reason
        ),
        crate::clock::hh_mm_now(),
    );
}

/// Append a one-line "failed" log entry to the
/// `LoggingViewModel`. Distinct from `push_blocked_log` so the
/// user can see that the action was attempted (not just blocked
/// from being compiled in): the capture / encode / write step
/// ran and returned an error.
fn push_failed_log(state: &mut AppState, record: &crate::app_state::DebugActionRecord) {
    state.append_log_message(
        format!(
            "Debug action {} FAILED (#{}): {}",
            record.action.as_str(),
            record.monotonic,
            record.reason
        ),
        crate::clock::hh_mm_now(),
    );
}

/// Dispatch `DebugEffect` values.
fn dispatch_debug_effects(effects: &[DebugEffect], app: &AppWindow, state: &mut AppState) {
    for effect in effects {
        match effect {
            DebugEffect::RequestScreenshot => {
                eprintln!(
                    "[d4lc] debug: RequestScreenshot effect observed (status={:?}); see last_debug_action and Logging screen",
                    state.runtime_status.last_debug_action.as_ref().map(|r| r.status)
                );
            }
            DebugEffect::ExportDebugImages => {
                eprintln!(
                    "[d4lc] debug: ExportDebugImages effect observed (status={:?}); see last_debug_action and Logging screen",
                    state.runtime_status.last_debug_action.as_ref().map(|r| r.status)
                );
            }
            DebugEffect::ReloadPresetImages => {
                eprintln!(
                    "[d4lc] debug: ReloadPresetImages effect observed (status={:?}); see last_debug_action and Logging screen",
                    state.runtime_status.last_debug_action.as_ref().map(|r| r.status)
                );
            }
            DebugEffect::ResetPerformanceResults => {
                eprintln!(
                    "[d4lc] debug: performance results reset (applied; see last_debug_action)"
                );
            }
            DebugEffect::PersistConfig => {
                eprintln!(
                    "[d4lc] debug: PersistConfig effect observed; d4lc-gui does not persist this bit, see last_debug_action"
                );
            }
            DebugEffect::ExportCrashLog(file_name) => {
                eprintln!(
                    "[d4lc] debug: ExportCrashLog('{file_name}') effect observed (status={:?}); see last_debug_action and Logging screen",
                    state.runtime_status.last_debug_action.as_ref().map(|r| r.status)
                );
            }
            DebugEffect::SetWindowTopmost(topmost) => {
                app.set_window_topmost(*topmost);
            }
        }
    }
}

/// Handle the "open artifacts folder" affordance.
///
/// Upstream opens the Screenshots directory with `Process.Start`
/// (`DebugViewModel.cs:693-696`); the port opens the debug-output
/// directory with the same `xdg-open` helper the GitHub/Ko-fi/Wiki links
/// use, and records the outcome (a missing directory is a
/// `Blocked` record, never a silent no-op).
pub fn handle_open_artifacts_folder(state: &mut AppState, debug_dir: &Path) {
    if !debug_dir.is_dir() {
        let record = state.record_debug_action(
            DebugAction::OpenArtifactsFolder,
            DebugActionStatus::Blocked,
            format!(
                "no debug-output directory yet at {}; run Take screenshot or Export debug images first",
                debug_dir.display()
            ),
        );
        push_blocked_log(state, &record);
        return;
    }
    match crate::actions::open_url(&debug_dir.display().to_string()) {
        Ok(()) => {
            let record = state.record_debug_action(
                DebugAction::OpenArtifactsFolder,
                DebugActionStatus::Applied,
                format!(
                    "opened {} with the desktop file handler",
                    debug_dir.display()
                ),
            );
            push_applied_log(state, &record);
        }
        Err(error) => {
            let record = state.record_debug_action(
                DebugAction::OpenArtifactsFolder,
                DebugActionStatus::Failed,
                error,
            );
            push_failed_log(state, &record);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::actions::debug::test_support::{debug_test_root, fresh_state};
    use crate::app_state::{AppState, DebugActionStatus};

    #[test]
    fn apply_reset_perf_state_clears_state_and_records_applied() {
        let mut state = fresh_state();
        // Seed a real debug panel state.
        state
            .debug_mut()
            .timing_metrics
            .push(d4lc_gui::DebugTimingMetric {
                name: "capture".to_string(),
                value_ms: 12.5,
            });
        state.debug_mut().debug_artifact_count = 3;
        state.debug_mut().perf_rows.push(d4lc_gui::DebugPerfRow {
            name: "Total".to_string(),
            value_ms: Some(9.5),
        });

        let _effects = apply_reset_perf_state(&mut state);

        assert!(state.debug().timing_metrics.is_empty());
        assert_eq!(state.debug().debug_artifact_count, 0);
        // The per-stage table is the port's timing surface, so the reset
        // clears its measured values too.
        assert_eq!(state.debug().perf_rows[0].value_ms, None);
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ResetPerformanceResults);
        assert_eq!(record.status, DebugActionStatus::Applied);
        assert_eq!(record.monotonic, 1);
        // Applied action is appended to the Logging screen.
        assert_eq!(state.logging.messages.len(), 1);
        assert!(state.logging.messages[0]
            .message
            .contains("reset_performance_results"));
    }

    #[test]
    fn apply_take_screenshot_state_records_blocked_and_appends_log() {
        let mut state = fresh_state();
        let _effects = apply_take_screenshot_state(&mut state);
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::TakeScreenshot);
        // The default build (no real-runtime feature) must remain
        // on the Blocked path. Feature-enabled builds route
        // through the real capture / encode / write path, which
        // is exercised separately below.
        #[cfg(not(feature = "real-runtime"))]
        {
            assert_eq!(record.status, DebugActionStatus::Blocked);
            assert!(record.reason.contains("real-runtime feature is disabled"));
        }
        // Blocked / Failed action is appended to the Logging screen.
        assert_eq!(state.logging.messages.len(), 1);
        assert!(
            state.logging.messages[0].message.contains("BLOCKED")
                || state.logging.messages[0].message.contains("FAILED"),
            "expected BLOCKED/FAILED log line, got {:?}",
            state.logging.messages[0].message
        );
    }

    /// `real-runtime` build: a TakeScreenshot invocation that
    /// cannot reach a Diablo window (no `DISPLAY` / no
    /// `Diablo IV` window) must be stamped as `Failed`, not
    /// silently turned into `Applied`. The Logging screen
    /// receives a FAILED line.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn apply_take_screenshot_state_records_failed_on_real_runtime_without_diablo_window() {
        // Force a missing-display path so the capture crate
        // fails fast. Both values are deliberately unreachable.
        let prev_display = std::env::var("DISPLAY").ok();
        let prev_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", ":99");
        std::env::remove_var("WAYLAND_DISPLAY");

        let mut state = fresh_state();
        let _effects = apply_take_screenshot_state(&mut state);

        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::TakeScreenshot);

        // Restore env so we do not leak state into sibling tests.
        match prev_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match prev_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }

        // Two outcomes are acceptable on a real-runtime build:
        //   - Failed  (no Diablo window / no X11 display)
        //   - Applied (real Diablo window reachable in this test
        //     runner — extremely unlikely on a headless CI host
        //     but technically possible on a developer box)
        // Anything else (notably Blocked) is a regression: the
        // real-runtime build must attempt the capture.
        assert_ne!(
            record.status,
            DebugActionStatus::Blocked,
            "real-runtime build must not Block a TakeScreenshot invocation"
        );
        assert!(
            state.logging.messages[0].message.contains("FAILED")
                || state.logging.messages[0].message.contains("applied"),
            "expected failed or applied log line, got {:?}",
            state.logging.messages[0].message
        );
    }

    #[test]
    fn apply_export_debug_images_state_records_blocked_without_controller() {
        let app_state = std::sync::Arc::new(std::sync::Mutex::new(fresh_state()));
        let _effects = apply_export_debug_images_state(&app_state, None);
        let state = app_state.lock().unwrap();
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ExportDebugImages);
        assert_eq!(record.status, DebugActionStatus::Blocked);
        assert!(record.reason.contains("DebugCommandController"));
        assert_eq!(state.logging.messages.len(), 1);
    }

    #[test]
    fn reload_preset_images_outcome_is_blocked_without_controller() {
        let mut state = fresh_state();
        let _effects = begin_reload_preset_images(&mut state);
        let outcome = reload_preset_images_outcome(None);
        record_command_outcome(&mut state, DebugAction::ReloadPresetImages, &outcome);
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ReloadPresetImages);
        assert_eq!(record.status, DebugActionStatus::Blocked);
        assert!(record.reason.contains("DebugCommandController"));
        assert_eq!(state.logging.messages.len(), 1);
    }

    #[test]
    fn apply_export_debug_images_state_with_controller_no_live_handle_returns_blocked_or_failed() {
        // With a controller installed but no live loop
        // running, the export must be refused, never
        // Applied. Either Blocked or Failed is accepted
        // for the "no live runtime" precondition; the
        // controller returns Failed ("no live loop is
        // currently running") so the Logging screen surfaces
        // the reason.
        use crate::runtime_bridge::DebugCommandController;
        let app_state = std::sync::Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = crate::runtime_bridge::RuntimeEventSink::new();
        let controller = DebugCommandController::new(
            app_state,
            event_tx,
            std::path::PathBuf::from("/tmp/d4lc-app-test-no-live"),
        );

        let ui_state = std::sync::Arc::new(std::sync::Mutex::new(fresh_state()));
        let _effects = apply_export_debug_images_state(&ui_state, Some(&controller));
        let state = ui_state.lock().unwrap();
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ExportDebugImages);
        assert_ne!(
            record.status,
            DebugActionStatus::Applied,
            "export must never be Applied when no live loop is running"
        );
        assert!(
            record.reason.contains("no live runtime")
                || record.reason.contains("real-runtime feature is disabled"),
            "expected a reason, got {:?}",
            record.reason
        );
    }

    #[test]
    fn reload_preset_images_with_controller_no_live_handle_returns_blocked() {
        use crate::runtime_bridge::DebugCommandController;
        let app_state = std::sync::Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = crate::runtime_bridge::RuntimeEventSink::new();
        let controller = DebugCommandController::new(
            app_state,
            event_tx,
            std::path::PathBuf::from("/tmp/d4lc-app-test-no-live"),
        );

        let mut state = fresh_state();
        let _effects = begin_reload_preset_images(&mut state);
        let outcome = reload_preset_images_outcome(Some(&controller));
        record_command_outcome(&mut state, DebugAction::ReloadPresetImages, &outcome);
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ReloadPresetImages);
        assert_eq!(record.status, DebugActionStatus::Blocked);
        assert!(record.reason.contains("no live runtime"));
    }

    /// Regression guard for the reload join invariant:
    /// the controller call in the reload choreography joins the
    /// live-loop thread, and the live loop takes the `AppState` lock
    /// once per frame, so the caller must NOT hold the `AppState` lock
    /// across that call. This pins the phased choreography of
    /// [`handle_reload_preset_images`] end to end: a worker runs the
    /// exact begin → controller-call → record sequence against a real
    /// live loop and probes, immediately before the join, that the
    /// `AppState` is acquirable again. Under the pre-fix shape (one
    /// lock scope held from the ViewModel snapshot through the
    /// controller call) the probe deterministically fails — the worker
    /// still holds its own guard — and the join then deadlocks against
    /// the lock-blocked loop, hitting the completion timeout.
    #[test]
    fn reload_preset_images_join_completes_without_holding_the_app_state_lock() {
        use std::sync::mpsc;
        use std::time::Duration;

        let root = crate::test_root::TestRoot::new("app-test-reload-join");
        let app_state = std::sync::Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = crate::runtime_bridge::RuntimeEventSink::new();
        let controller = crate::runtime_bridge::DebugCommandController::new(
            std::sync::Arc::clone(&app_state),
            event_tx.clone(),
            root.to_path_buf(),
        );

        // Live loop that takes the AppState lock once per frame.
        #[cfg(feature = "real-runtime")]
        let handle = crate::runtime_bridge::start_live_loop(
            std::sync::Arc::clone(&app_state),
            event_tx,
            root.to_path_buf(),
            d4lc_runtime::RuntimeReloadSignal::new(),
        );
        #[cfg(not(feature = "real-runtime"))]
        let handle = crate::runtime_bridge::start_live_loop(
            std::sync::Arc::clone(&app_state),
            event_tx,
            Duration::from_millis(1),
        );
        controller.install_live_handle(handle);

        // Run the exact phased choreography of
        // `handle_reload_preset_images` on a worker thread while the
        // loop keeps contending the lock.
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let worker_state = std::sync::Arc::clone(&app_state);
        let worker_controller = controller.clone();
        let worker = std::thread::spawn(move || {
            // Phase 1 — under the lock, released at scope end.
            let effects = {
                let mut state = worker_state.lock().unwrap_or_else(|p| p.into_inner());
                begin_reload_preset_images(&mut state)
            };
            // Join-invariant probe: with this worker's own guard
            // released, the AppState must be acquirable again before
            // the join. The live loop holds it only for microseconds
            // per 1 ms frame, so a short retry loop must succeed. If a
            // regression fuses the lock scope, this worker still holds
            // its own guard and every attempt fails.
            let mut acquirable = false;
            for _ in 0..1_000 {
                if worker_state.try_lock().is_ok() {
                    acquirable = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                acquirable,
                "AppState lock still held by the reload caller across the \
                 live-loop join (join invariant violated)"
            );
            // Phase 2 — join with no AppState lock held.
            let outcome = reload_preset_images_outcome(Some(&worker_controller));
            // Phase 3 — re-lock and record.
            let mut state = worker_state.lock().unwrap_or_else(|p| p.into_inner());
            record_command_outcome(&mut state, DebugAction::ReloadPresetImages, &outcome);
            drop(state);
            let _ = effects;
            done_tx
                .send(())
                .unwrap_or_else(|error| panic!("test receiver alive: {error}"));
        });

        done_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| {
                panic!(
                    "reload join deadlocked against the AppState lock: the live loop must \
                 stay joinable while the caller holds no AppState lock"
                )
            });
        worker.join().expect("reload worker thread must not panic");

        // Cleanup: the default-build reload stopped the loop itself;
        // a real-runtime reload that failed preparation leaves none.
        // Either way nothing keeps running past the test.
        if let Some(handle) = controller.take_live_handle() {
            handle.stop();
        }
    }

    /// Regression guard for the export deadlock (audit N-01): with the
    /// live loop running, the export used to run under the UI callback's
    /// `AppState` guard while the controller locked the same `AppState`
    /// again (`std::sync::Mutex` is not reentrant), hanging the UI thread
    /// for good. The export path must return a controlled outcome within
    /// the timeout, for a Wayland and for an X11 capture engine, while the
    /// loop is running. Against the pre-fix shape (lock, then
    /// `apply_export_debug_images_state(&mut state, ..)`) the same setup
    /// times out.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn export_debug_images_with_running_live_loop_returns_without_deadlock() {
        use std::sync::mpsc;
        use std::time::Duration;

        let root = debug_test_root("export-running-loop");
        for wayland in [true, false] {
            let app_state = std::sync::Arc::new(std::sync::Mutex::new(AppState::default_startup()));
            let geometry = wayland.then_some(d4lc_core::Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            });
            app_state
                .lock()
                .unwrap()
                .set_capture_engine("test engine".to_string(), geometry);
            let (event_tx, _event_rx) = crate::runtime_bridge::RuntimeEventSink::new();
            let controller = crate::runtime_bridge::DebugCommandController::new(
                std::sync::Arc::clone(&app_state),
                event_tx,
                root.to_path_buf(),
            );
            controller.install_live_handle(crate::runtime_bridge::LiveLoopHandle::idle_for_test());
            assert!(controller.is_live_loop_running());

            let (done_tx, done_rx) = mpsc::channel();
            let worker_state = std::sync::Arc::clone(&app_state);
            let worker_controller = controller.clone();
            std::thread::spawn(move || {
                let _effects =
                    apply_export_debug_images_state(&worker_state, Some(&worker_controller));
                let _ = done_tx.send(());
            });
            done_rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| {
                    panic!("export deadlocked with a running live loop (wayland={wayland})")
                });

            let state = app_state.lock().unwrap();
            let record = state
                .runtime_status
                .last_debug_action
                .as_ref()
                .expect("last_debug_action must be populated");
            assert_eq!(record.action, DebugAction::ExportDebugImages);
            assert_eq!(
                record.status,
                DebugActionStatus::Failed,
                "{}",
                record.reason
            );
            if wayland {
                assert_eq!(
                    record.reason,
                    crate::actions::debug::export::WAYLAND_EXPORT_GAP
                );
            } else {
                assert!(
                    !record.reason.contains("no live runtime"),
                    "the export must have been attempted: {}",
                    record.reason
                );
            }
            drop(state);
            if let Some(handle) = controller.take_live_handle() {
                handle.stop();
            }
        }
    }

    #[test]
    fn apply_debug_info_toggled_state_records_applied() {
        let mut state = fresh_state();
        let _effects = apply_debug_info_toggled_state(&mut state, true);
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ToggleDebugInfo);
        assert_eq!(record.status, DebugActionStatus::Applied);
    }

    #[test]
    fn apply_topmost_toggled_state_records_applied() {
        let mut state = fresh_state();
        let _effects = apply_topmost_toggled_state(&mut state, true);
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ToggleTopMost);
        assert_eq!(record.status, DebugActionStatus::Applied);
    }

    /// Test root for crash-log export tests; lives under the
    /// workspace target dir so the real home is never touched.
    fn export_test_root(label: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-crash-dirs")
            .join(format!("{label}-{}", std::process::id()))
    }

    /// Named decision point: the DebugInfo bit survives a restart — the
    /// toggle writes `IsDebugInfoEnabled` and the startup seed reads it
    /// back into the panel state.
    #[test]
    fn debug_info_toggle_persists_and_seeds_back_on_startup() {
        let root = debug_test_root("debug-info");
        let mut state = fresh_state();
        assert!(!seed_debug_info_from_settings(&mut state, &root));

        persist_debug_info_bit(&mut state, &root, true);
        let reloaded = d4lc_config::load_app_settings(&root).unwrap();
        assert!(reloaded.is_debug_info_enabled);

        let mut restarted = fresh_state();
        assert!(seed_debug_info_from_settings(&mut restarted, &root));
        assert!(restarted.debug().debug_info_enabled);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Named decision point: the Debug-screen TopMost toggle persists
    /// `IsTopMost`, which is the field `main` applies to the window at
    /// startup (`SettingsPanelState::from_config` → `overlay_top_most`).
    #[test]
    fn topmost_toggle_persists_is_top_most() {
        let root = debug_test_root("topmost");
        let mut state = fresh_state();
        assert!(!state.shell.settings.overlay_top_most);

        persist_topmost(&mut state, &root, true);
        assert!(state.shell.settings.overlay_top_most);
        let reloaded = d4lc_config::load_app_settings(&root).unwrap();
        assert!(reloaded.is_top_most);

        // The startup projection seeds the same field back.
        let panel = d4lc_gui::SettingsPanelState::from_config(&reloaded, Vec::new());
        assert!(panel.overlay_top_most);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Named decision point: crash-log export is strictly opt-in —
    /// with the default settings the local copy must never run.
    /// Oracle: the recorded action status + absence of the export
    /// file.
    #[test]
    fn apply_export_crash_log_state_blocked_while_diagnostics_disabled() {
        let root = export_test_root("export-disabled");
        let crash_dir = root.join("crashes");
        std::fs::create_dir_all(&crash_dir).unwrap();
        let file_name = "crash-1700000000-000000000.log";
        std::fs::write(crash_dir.join(file_name), "payload").unwrap();

        let mut state = fresh_state();
        assert!(
            !state.shell.settings.diagnostics_export_enabled,
            "default settings must keep diagnostics export disabled"
        );
        let _effects = apply_export_crash_log_state(
            &mut state,
            Some(&crash_dir),
            &root.join("config"),
            file_name,
        );

        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ExportCrashLog);
        assert_eq!(record.status, DebugActionStatus::Blocked);
        assert!(record.reason.contains("diagnostics export is disabled"));
        assert!(record.reason.contains("never uploads"));
        assert!(!root.join("config/crash-exports").join(file_name).is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Named decision point: with the toggle enabled, export performs
    /// a real local copy into `<config_root>/crash-exports/` and the
    /// record points the user at the manual GitHub-issue attachment
    /// step. Oracle: readback of the exported file.
    #[test]
    fn apply_export_crash_log_state_copies_log_when_enabled() {
        let root = export_test_root("export-enabled");
        let crash_dir = root.join("crashes");
        std::fs::create_dir_all(&crash_dir).unwrap();
        let file_name = "crash-1700000001-000000000.log";
        std::fs::write(
            crash_dir.join(file_name),
            "d4lc crash log\napp_version=1.0.0\n",
        )
        .unwrap();

        let mut state = fresh_state();
        state.shell.settings.diagnostics_export_enabled = true;
        let _effects = apply_export_crash_log_state(
            &mut state,
            Some(&crash_dir),
            &root.join("config"),
            file_name,
        );

        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(record.action, DebugAction::ExportCrashLog);
        assert_eq!(record.status, DebugActionStatus::Applied);
        assert!(record
            .reason
            .contains("attach it manually to a GitHub issue"));
        let exported = root.join("config/crash-exports").join(file_name);
        assert!(exported.is_file());
        assert_eq!(
            std::fs::read_to_string(&exported).unwrap(),
            "d4lc crash log\napp_version=1.0.0\n"
        );
        // The action is visible on the Logging screen.
        assert!(state.logging.messages[0]
            .message
            .contains("export_crash_log"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn debug_action_seq_increments_per_call() {
        let mut state = fresh_state();
        let _ = apply_reset_perf_state(&mut state);
        let _ = apply_take_screenshot_state(&mut state);
        let app_state = std::sync::Arc::new(std::sync::Mutex::new(state));
        let records: Vec<_> = (0..3)
            .map(|_| {
                let _ = apply_export_debug_images_state(&app_state, None);
                app_state
                    .lock()
                    .unwrap()
                    .runtime_status
                    .last_debug_action
                    .as_ref()
                    .unwrap()
                    .monotonic
            })
            .collect();
        // 1 (reset), 2 (screenshot), 3 (first export), 4, 5.
        assert_eq!(records, vec![3, 4, 5]);
    }
}
