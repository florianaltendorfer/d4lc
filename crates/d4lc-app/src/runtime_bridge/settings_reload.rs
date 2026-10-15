//! Settings hot-reload monitor — observes `Config/Settings.json` and the
//! runtime assets for changes and drives the runtime hot-reload signal.
//!
//! ```text
//! Config/Settings.json ─┐
//! Config/Controllers.json ├─► SettingsReloadMonitor::poll() ──► decision
//! preset templates       │     (UI thread, 50 ms drain timer)      │
//! language data ─────────┘                                         │
//!      Changed{reprepare}  → controller.request_settings_reload()  │
//!      Changed{restart}    → visible "restart runtime to apply"    │
//!      LoadFailed          → non-fatal UI status + log line        │
//!      NotRelevant         → nothing                               │
//!                                                                  ▼
//!                          live-loop thread: apply_pending_reload → swap
//! ```
//!
//! What a changed setting needs is decided by the consumer matrix
//! ([`crate::settings_consumers`]): detector and OCR-preparation settings,
//! and every change of the controller list, the preset's template files or
//! the language data, re-prepare the runtime (a new configuration revision
//! that discards OCR results of the old configuration); settings read only
//! at runtime start are reported as "restart runtime to apply"; presentation
//! and rule settings reach the runtime through the app state and its rules
//! snapshot and need no reload.
//!
//! This is the std-only implementation of the Alacritty
//! `live_config_reload` model: the monitor stamps the settings file
//! (mtime + size) once per drain-timer tick and the asset sets once per
//! [`ASSET_POLL_INTERVAL`], instead of using a file-watcher dependency, and
//! the live-loop thread performs the re-preparation so template/OCR I/O
//! never blocks the UI thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use d4lc_config::{load_app_settings_with_warnings, AppSettings, AppSettingsWarning};

use crate::app_state::AppState;
use crate::runtime_bridge::DebugCommandController;
use crate::settings_consumers::{changed_settings, setting_entries, AssetStamps, UpdateKind};

/// How often the runtime asset sets are stamped (directory walks, so not
/// every drain-timer tick).
pub const ASSET_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Outcome of one observation tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsReloadDecision {
    /// Neither the settings file nor a runtime asset changed.
    NoChange,
    /// The settings file changed, but nothing the runtime preparation or
    /// the runtime start reads.
    NotRelevant,
    /// Runtime-relevant changes. `reprepare` names the settings and assets
    /// that need a new runtime preparation (hot reload), `restart` the
    /// settings read only when the runtime starts. `warnings` carries
    /// non-fatal schema warnings for the Logging screen.
    Changed {
        reprepare: Vec<String>,
        restart: Vec<String>,
        warnings: Vec<AppSettingsWarning>,
    },
    /// The changed settings file could not be loaded. Non-fatal: the
    /// caller surfaces the error and the live loop keeps running with the
    /// previously applied settings.
    LoadFailed { message: String },
}

/// UI-side settings and asset observer.
///
/// The baseline is taken at construction, matching the settings the app
/// loaded at startup; every later change is compared against the last
/// observed settings and asset stamps.
pub struct SettingsReloadMonitor {
    settings_path: PathBuf,
    config_root: PathBuf,
    game_data_root: PathBuf,
    last_stamp: Option<(SystemTime, u64)>,
    settings: AppSettings,
    assets: AssetStamps,
    asset_interval: Duration,
    last_asset_check: Instant,
}

impl SettingsReloadMonitor {
    /// Build a monitor over `config_root` whose baseline matches the
    /// settings the app loaded at startup. The game data (vendored presets
    /// and language data) resolves like the runtime preparation's.
    pub fn new(config_root: &Path) -> Self {
        let game_data_root =
            crate::actions::affix::catalog_repo_root().unwrap_or_else(|| config_root.to_path_buf());
        Self::with_game_data_root(config_root, &game_data_root)
    }

    /// A monitor with an explicit game-data root.
    pub fn with_game_data_root(config_root: &Path, game_data_root: &Path) -> Self {
        let settings_path = config_root.join(d4lc_config::SETTINGS_PATH);
        let last_stamp = current_stamp(&settings_path);
        let settings = load_app_settings_with_warnings(config_root)
            .map(|(settings, _)| settings)
            .unwrap_or_default();
        let assets = AssetStamps::observe(config_root, game_data_root, &settings);
        Self {
            settings_path,
            config_root: config_root.to_path_buf(),
            game_data_root: game_data_root.to_path_buf(),
            last_stamp,
            settings,
            assets,
            asset_interval: ASSET_POLL_INTERVAL,
            last_asset_check: Instant::now(),
        }
    }

    /// Stamp the asset sets on every poll (tests).
    #[cfg(test)]
    pub fn with_asset_interval(mut self, interval: Duration) -> Self {
        self.asset_interval = interval;
        self
    }

    /// The settings as last observed on disk (the desired settings).
    pub fn desired(&self) -> &AppSettings {
        &self.settings
    }

    /// Observe once. Cheap when unchanged: one `fs::metadata` per tick and
    /// the asset walk once per asset interval; called from the UI-thread
    /// drain timer.
    pub fn poll(&mut self) -> SettingsReloadDecision {
        let stamp = current_stamp(&self.settings_path);
        let mut file_changed = false;
        let mut changed = Vec::new();
        let mut warnings = Vec::new();
        if stamp != self.last_stamp {
            // A disappearing file is treated as transient (editors replace
            // the target atomically and a missing file must never force
            // defaults into the running runtime). The stamp baseline is
            // cleared so a re-created file is compared against the last
            // observed settings.
            match stamp {
                None => self.last_stamp = None,
                Some(stamp) => {
                    self.last_stamp = Some(stamp);
                    let (settings, loaded_warnings) =
                        match load_app_settings_with_warnings(&self.config_root) {
                            Ok(loaded) => loaded,
                            Err(error) => {
                                return SettingsReloadDecision::LoadFailed { message: error }
                            }
                        };
                    file_changed = true;
                    warnings = loaded_warnings;
                    changed = changed_settings(&self.settings, &settings);
                    self.settings = settings;
                }
            }
        }

        let mut reprepare: Vec<String> = changed
            .iter()
            .filter(|entry| entry.kind.requires_reprepare())
            .map(|entry| entry.key.to_string())
            .collect();
        let restart: Vec<String> = changed
            .iter()
            .filter(|entry| entry.kind == UpdateKind::SessionRestart)
            .map(|entry| entry.key.to_string())
            .collect();

        let now = Instant::now();
        if file_changed || now.duration_since(self.last_asset_check) >= self.asset_interval {
            self.last_asset_check = now;
            let assets =
                AssetStamps::observe(&self.config_root, &self.game_data_root, &self.settings);
            for asset in assets.changed(&self.assets) {
                reprepare.push(asset.as_str().to_string());
            }
            self.assets = assets;
        }

        if reprepare.is_empty() && restart.is_empty() {
            return if file_changed {
                SettingsReloadDecision::NotRelevant
            } else {
                SettingsReloadDecision::NoChange
            };
        }
        SettingsReloadDecision::Changed {
            reprepare,
            restart,
            warnings,
        }
    }
}

/// Observe the settings file and runtime assets from the UI-thread drain
/// timer and act on the decision:
///
/// - `Changed` — surface any schema warnings on the Logging screen, request
///   a runtime hot reload for re-preparation changes, and record what is
///   pending or needs a runtime restart on the settings status (desired
///   versus applied) while a runtime runs.
/// - `LoadFailed` — surface a non-fatal status; the live loop keeps running
///   with the previously applied settings and `is_running` is untouched.
/// - `NoChange` / `NotRelevant` — nothing to do.
///
/// After every observed change, settings the runtime reads from the app
/// state are compared against the desired file values: a value edited
/// outside the app is shown as "stored but not live" instead of being
/// claimed applied.
pub fn observe_settings_change(
    monitor: &mut SettingsReloadMonitor,
    controller: &DebugCommandController,
    state: &mut AppState,
) {
    match monitor.poll() {
        SettingsReloadDecision::NoChange => return,
        SettingsReloadDecision::NotRelevant => {}
        SettingsReloadDecision::Changed {
            reprepare,
            restart,
            warnings,
        } => {
            for warning in &warnings {
                state.append_log_message(warning.to_string(), crate::clock::hh_mm_now());
            }
            let running = state.runtime_status.is_running;
            if !reprepare.is_empty() {
                controller.request_settings_reload();
                if running {
                    state.runtime_status.settings.request_reprepare(&reprepare);
                }
            }
            if running && !restart.is_empty() {
                state.runtime_status.settings.require_restart(&restart);
            }
            // A changed display backend applies through the existing
            // start/stop paths: the resolved engine is
            // read once per start, so the app restarts the live loop itself
            // instead of waiting for a manual restart. A restart whose
            // prerequisites fail keeps the old loop running and stays a
            // visible "restart runtime to apply" status.
            if running && restart.iter().any(|key| key == "display_backend") {
                match controller.restart_for_display_backend() {
                    Ok(()) => {
                        state.append_log_message(
                            "display backend changed: restarting the live runtime".to_string(),
                            crate::clock::hh_mm_now(),
                        );
                    }
                    Err(error) => {
                        state.append_log_message(
                            format!("display backend restart failed: {error}"),
                            crate::clock::hh_mm_now(),
                        );
                    }
                }
            }
        }
        SettingsReloadDecision::LoadFailed { message } => {
            state.runtime_status.status_message = format!("Settings reload failed: {message}");
            state.append_log_message(
                format!("settings reload failed: {message}"),
                crate::clock::hh_mm_now(),
            );
            return;
        }
    }
    let mut live = monitor.desired().clone();
    state.shell.settings.apply_to_config(&mut live);
    let not_live = not_live_settings(monitor.desired(), &live);
    state.runtime_status.settings.set_not_live(not_live);
}

/// Settings the runtime reads from the app state whose stored value differs
/// from the app state's value.
fn not_live_settings(desired: &AppSettings, live: &AppSettings) -> Vec<String> {
    setting_entries(desired)
        .into_iter()
        .zip(setting_entries(live))
        .filter(|(desired, live)| desired.kind.follows_app_state() && desired.value != live.value)
        .map(|(desired, _)| desired.key.to_string())
        .collect()
}

/// Cheap (mtime, size) stamp of the settings file; `None` when absent.
fn current_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::runtime_bridge::DebugCommandController;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A settings file that exists but cannot be read as text.
    fn write_unreadable_settings(settings_path: &Path) {
        fs::create_dir_all(settings_path.parent().unwrap_or(Path::new(".")))
            .unwrap_or_else(|error| panic!("create config dir: {error}"));
        fs::write(settings_path, [0xff, 0xfe, 0x00, 0x7b])
            .unwrap_or_else(|error| panic!("write settings: {error}"));
    }

    /// Unique temp config root per test (never a shared path).
    fn temp_config_root(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("d4lc-settings-reload-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("temp root: {error}"));
        dir
    }

    fn write_settings(root: &Path, mutate: impl FnOnce(&mut AppSettings)) {
        let mut settings = AppSettings::default();
        mutate(&mut settings);
        d4lc_config::save_app_settings(root, &settings)
            .unwrap_or_else(|error| panic!("save settings: {error}"));
    }

    /// The decision re-prepares the runtime for `key` (asset stamps of the
    /// newly selected preset may join the list).
    fn assert_reprepares(decision: SettingsReloadDecision, key: &str) {
        match decision {
            SettingsReloadDecision::Changed { reprepare, .. } => assert!(
                reprepare.iter().any(|changed| changed == key),
                "{key} must re-prepare the runtime, got {reprepare:?}"
            ),
            other => panic!("expected a re-preparation for {key}, got {other:?}"),
        }
    }

    fn fresh_controller(config_root: &Path) -> DebugCommandController {
        let app_state = Arc::new(Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = crate::runtime_bridge::RuntimeEventSink::new();
        DebugCommandController::new(app_state, event_tx, config_root.to_path_buf())
    }

    /// Start a live loop so the controller's reload-request gate
    /// (`live_handle.is_some()`) opens. Works for both the structured stub
    /// loop and the real-runtime loop: a real-runtime preparation failure
    /// only emits events into a dropped receiver.
    fn started_live_loop() -> crate::runtime_bridge::LiveLoopHandle {
        let app_state = Arc::new(Mutex::new(crate::app_state::AppState::default_startup()));
        let (event_tx, _event_rx) = crate::runtime_bridge::RuntimeEventSink::new();
        #[cfg(feature = "real-runtime")]
        {
            crate::runtime_bridge::start_live_loop(
                app_state,
                event_tx,
                std::env::temp_dir().join("d4lc-settings-reload-loop-root"),
                d4lc_runtime::RuntimeReloadSignal::new(),
            )
        }
        #[cfg(not(feature = "real-runtime"))]
        {
            crate::runtime_bridge::start_live_loop(app_state, event_tx, Duration::from_millis(100))
        }
    }

    #[test]
    fn monitor_without_settings_file_polls_no_change() {
        let root = temp_config_root("absent");
        let mut monitor = SettingsReloadMonitor::new(&root);
        assert_eq!(monitor.poll(), SettingsReloadDecision::NoChange);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn monitor_requests_reload_when_system_preset_changes() {
        let root = temp_config_root("preset-change");
        let mut monitor = SettingsReloadMonitor::new(&root);

        // No file yet: baseline is the defaults signature. An external edit
        // that changes the system preset must request a reload.
        write_settings(&root, |settings| {
            settings.selected_system_preset = "1080p_custom".to_string();
        });
        assert_reprepares(monitor.poll(), "selected_system_preset");

        // The change is consumed: a second poll without edits is NoChange.
        assert_eq!(monitor.poll(), SettingsReloadDecision::NoChange);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn monitor_ignores_changes_that_do_not_affect_runtime_preparation() {
        let root = temp_config_root("not-relevant");
        let mut monitor = SettingsReloadMonitor::new(&root);

        // debug_mode is not consumed by runtime preparation: the descriptor
        // path owns it live, so no reload may be requested.
        write_settings(&root, |settings| settings.debug_mode = true);
        assert_eq!(monitor.poll(), SettingsReloadDecision::NotRelevant);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn monitor_surfaces_unreadable_settings_and_recovers_on_fix() {
        let root = temp_config_root("load-failure");
        let mut monitor = SettingsReloadMonitor::new(&root);

        // A read failure, deterministically and for every user (permission
        // bits would not stop root): the file exists, so `is_file()` passes,
        // but its bytes are not UTF-8 and cannot be read as text.
        let settings_path = root.join(d4lc_config::SETTINGS_PATH);
        write_unreadable_settings(&settings_path);

        match monitor.poll() {
            SettingsReloadDecision::LoadFailed { message } => {
                assert!(!message.is_empty());
            }
            other => panic!("expected LoadFailed, got {other:?}"),
        }

        // Same broken state: the stamp is unchanged, so the monitor must not
        // retry the failure every tick.
        assert_eq!(monitor.poll(), SettingsReloadDecision::NoChange);

        // Fixing the file recovers into a request.
        fs::write(&settings_path, "{}").unwrap_or_else(|error| panic!("repair settings: {error}"));
        write_settings(&root, |settings| {
            settings.selected_system_preset = "1080p_custom".to_string();
        });
        assert_reprepares(monitor.poll(), "selected_system_preset");
        let _ = fs::remove_dir_all(&root);
    }

    /// A settings file overwritten with text that is not JSON fails the
    /// load; the monitor reports it instead of reloading defaults.
    #[test]
    fn garbage_settings_file_is_a_load_failure() {
        let root = temp_config_root("garbage");
        let mut monitor = SettingsReloadMonitor::new(&root);

        write_settings(&root, |settings| {
            settings.selected_system_preset = "1080p_custom".to_string();
        });
        assert_reprepares(monitor.poll(), "selected_system_preset");

        fs::write(root.join(d4lc_config::SETTINGS_PATH), "{not valid json")
            .unwrap_or_else(|error| panic!("write garbage settings: {error}"));

        // An unparsable file is a non-fatal load failure: the loop keeps
        // the applied settings instead of falling back to defaults.
        assert!(matches!(
            monitor.poll(),
            SettingsReloadDecision::LoadFailed { .. }
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn observe_settings_change_surfaces_load_failure_as_non_fatal_status() {
        let root = temp_config_root("observe-failure");
        let mut monitor = SettingsReloadMonitor::new(&root);
        let controller = fresh_controller(&root);

        write_unreadable_settings(&root.join(d4lc_config::SETTINGS_PATH));

        let mut state = crate::app_state::AppState::default_startup();
        state.runtime_status.is_running = true;
        observe_settings_change(&mut monitor, &controller, &mut state);

        assert!(
            state
                .runtime_status
                .status_message
                .starts_with("Settings reload failed:"),
            "load failure must surface as a non-fatal status, got: {}",
            state.runtime_status.status_message
        );
        assert!(
            state.runtime_status.is_running,
            "load failure must never stop the live loop"
        );
        assert_eq!(state.logging.messages.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn observe_settings_change_requests_reload_on_prep_relevant_change() {
        let root = temp_config_root("observe-request");
        let mut monitor = SettingsReloadMonitor::new(&root);
        let controller = fresh_controller(&root);
        let handle = started_live_loop();
        controller.install_live_handle(handle);

        write_settings(&root, |settings| {
            settings.controller_mode = true;
        });

        let mut state = crate::app_state::AppState::default_startup();
        observe_settings_change(&mut monitor, &controller, &mut state);

        assert!(
            controller.settings_reload_signal().is_requested(),
            "a preparation-relevant change must request a runtime reload"
        );
        // The status text is reserved for the loop's actual reload outcome
        // events, so the observation step itself must not touch it.
        assert_eq!(state.runtime_status.status_message, "Stopped");
        assert_eq!(state.logging.messages.len(), 0);

        if let Some(handle) = controller.take_live_handle() {
            handle.stop();
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn monitor_poll_does_not_block_for_the_default_frame_interval() {
        // Guard the drain-timer contract: an unchanged stamp must be a
        // metadata-only check (the load path never runs), so the UI thread
        // never pays a config parse per tick.
        let root = temp_config_root("fast-poll");
        let mut monitor = SettingsReloadMonitor::new(&root);
        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(monitor.poll(), SettingsReloadDecision::NoChange);
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "100 unchanged-stamp polls took {:?}; the drain timer must stay cheap",
            started.elapsed()
        );
        let _ = fs::remove_dir_all(&root);
    }
}
