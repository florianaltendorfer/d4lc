//! Settings panel projections, the display-backend and language selectors,
//! and settings persistence.

use std::path::{Path, PathBuf};

use d4lc_config::{load_app_settings, load_app_settings_with_warnings, save_app_settings};
use d4lc_gui::settings::{
    affix_language_selection, SettingsEffect, SettingsViewModel, ToggleInput, AFFIX_LANGUAGE_IDS,
};
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::app_state::AppState;
use crate::display_backend::{resolve_display_backend, DisplayBackendStatus};
use crate::slint_generated::AppWindow;

/// Resolve the config storage root directory.
///
/// Priority:
/// 1. `D4LC_CONFIG_ROOT` environment variable (for tests / development)
/// 2. `dirs::config_dir().join("d4lc")` (XDG default: ~/.config/d4lc/)
pub fn resolve_config_root() -> PathBuf {
    d4lc_config::config_root_from_env().unwrap_or_else(default_config_root)
}

/// The XDG location of the user's configuration (shared with the CLI).
fn user_config_root() -> PathBuf {
    d4lc_config::xdg_config_root().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(not(test))]
fn default_config_root() -> PathBuf {
    user_config_root()
}

/// Unit tests never fall back to the user's real configuration: a handler
/// that resolves the root itself while no test override is set reads and
/// writes one scratch directory of this test process instead.
#[cfg(test)]
fn default_config_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("d4lc-unit-test-config-{}", std::process::id()));
        std::fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("create {}: {error}", root.display()));
        root
    })
    .clone()
}

/// Handle the aggregated `settings-changed()` callback.
///
/// Reads all toggle properties from the AppWindow (the 6 upstream
/// toggles plus the port-specific diagnostics-export and release
/// check toggles), calls ViewModel command, and dispatches
/// resulting effects.
pub fn handle_settings_changed(app: &AppWindow, state: &mut AppState, config_root: &Path) {
    let input = ToggleInput {
        debug_mode: app.get_debug_mode(),
        check_for_updates: app.get_check_updates(),
        launch_minimized: app.get_launch_minimized(),
        minimize_to_tray: app.get_minimize_to_tray(),
        controller_mode: app.get_controller_mode(),
        paragon_mode_active: app.get_paragon_mode(),
        diagnostics_export_enabled: app.get_diagnostics_export(),
        update_check_enabled: app.get_update_check_enabled(),
    };

    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let effects = vm.apply_toggles(input);
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();

    dispatch_settings_effects(&effects, state, config_root);
    state.rebuild_overlay_descriptor_state();
}

/// Map a persisted `DisplayBackend` token (`auto|x11|wayland`) to its
/// selector label. An unknown stored value is shown verbatim so the UI
/// never silently claims a selection the runtime seam will refuse.
pub fn display_backend_label(token: &str) -> String {
    match token {
        "auto" => "Auto".to_string(),
        "x11" => "X11".to_string(),
        "wayland" => "Wayland".to_string(),
        other => other.to_string(),
    }
}

/// Seed the SettingsScreen display-backend selector from the persisted
/// `DisplayBackend` config value. This is the startup projection: both
/// the real launch path (`wire_settings_actions`) and the headless
/// restart simulation in the testing harness call it, so the selector
/// always re-seeds from `Config/Settings.json`.
pub fn apply_settings_display_backend_projection(app: &AppWindow, config_root: &Path) {
    let settings = load_app_settings(config_root).unwrap_or_default();
    app.set_settings_display_backend(display_backend_label(&settings.display_backend).into());
}

/// Handle the SettingsScreen display-backend selection.
///
/// Validates the chosen label through the display-backend selection seam
/// ([`resolve_display_backend`]), persists the canonical token to
/// `Config/Settings.json`, and re-projects the normalized label.
///
/// Persistence is fire-and-forget: a
/// structured `AppSettingsWarning` (a settings file written by a newer
/// schema version) is the reason `save_app_settings` refuses the write,
/// so both the warning and the concrete refusal are surfaced on the
/// Logging screen instead of being swallowed. Selecting Wayland
/// additionally logs the structured unavailability the runtime seam
/// reports — Wayland is never silently exchanged for the X11 paths.
///
/// # Errors
///
/// Returns the structured seam error for a label outside the selector
/// options, or the load error when the config file cannot be parsed.
pub fn handle_display_backend_changed(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    label: &str,
) -> Result<(), String> {
    let (backend, status) = resolve_display_backend(&label.trim().to_ascii_lowercase())?;

    let (mut config, warnings) = match load_app_settings_with_warnings(config_root) {
        Ok(loaded) => loaded,
        Err(error) => {
            state.append_log_message(
                format!("settings load failed: {error}"),
                crate::clock::hh_mm_now(),
            );
            return Err(format!("settings load failed: {error}"));
        }
    };
    config.display_backend = backend.as_str().to_string();
    let saved = save_app_settings(config_root, &config);
    if let Err(error) = &saved {
        for warning in &warnings {
            state.append_log_message(warning.to_string(), crate::clock::hh_mm_now());
        }
        state.append_log_message(
            format!("settings save failed: {error}"),
            crate::clock::hh_mm_now(),
        );
    }

    // The Wayland line only claims "set to Wayland" when the selection
    // actually persisted; a refused save already surfaced its own reason
    // above. Both states stay about what the selection means, and
    // `Auto` describes the detection semantics (the engine is resolved
    // once per runtime start — this settings path never probes).
    if saved.is_ok() {
        match &status {
            DisplayBackendStatus::WaylandUnavailable { reason } => {
                state.append_log_message(
                    format!(
                        "Display backend set to Wayland: {reason}; applies at next runtime start"
                    ),
                    crate::clock::hh_mm_now(),
                );
            }
            DisplayBackendStatus::Wayland { .. } => {
                state.append_log_message(
                    "Display backend set to Wayland: capture runs through the ScreenCast \
                     portal (compositor dialog or restore token) at next runtime start",
                    crate::clock::hh_mm_now(),
                );
            }
            DisplayBackendStatus::Auto => {
                state.append_log_message(
                    "Display backend set to Auto: the capture engine is detected at the next \
                     runtime start (an X11 Diablo window selects X11 capture; without one, a \
                     Wayland session uses the ScreenCast portal)",
                    crate::clock::hh_mm_now(),
                );
            }
            // The parse-level seam resolves only Auto / X11 / Wayland /
            // WaylandUnavailable; the detection-only variants cannot occur
            // here.
            DisplayBackendStatus::X11 { .. } | DisplayBackendStatus::NoEngine { .. } => {}
        }
    }

    app.set_settings_display_backend(display_backend_label(backend.as_str()).into());
    Ok(())
}

/// Project the persisted languages onto the language combos.
///
/// This is the startup seed for the game-data language combo (the
/// AffixScreen control row) and the UI-language combo (the SettingsScreen
/// sidebar row, `apply_app_language_projection`), which also applies the
/// persisted UI language. Both the real launch path
/// (`wire_settings_actions`) and the headless harness composition call it,
/// so the combos always re-seed from `Config/Settings.json`.
///
/// Seed semantics of the game-data combo mirror upstream
/// `AffixViewModel.InitAffixLanguages` (`AffixViewModel.cs:1656-1662`): the
/// stored `SelectedAffixLanguage` token is selected only when it matches one
/// of the 14 upstream ids; an unknown stored token selects nothing, so the
/// combo never claims a selection the runtime preparation will not honor. The
/// stored token itself is left untouched either way.
pub fn apply_settings_language_projection(app: &AppWindow, config_root: &Path) {
    let options: Vec<SharedString> = AFFIX_LANGUAGE_IDS
        .iter()
        .map(|id| SharedString::from(*id))
        .collect();
    if let Some(model) = replacement_options(&app.get_language_options(), options) {
        app.set_language_options(model);
    }

    let settings = load_app_settings(config_root).unwrap_or_default();
    let selection = affix_language_selection(&settings.selected_affix_language);
    app.set_selected_language(SharedString::from(selection.unwrap_or("")));
    super::apply_app_language_projection(app, config_root);
}

/// The model to install on a combo's options property, or `None` when it
/// already lists `options`. Replacing a ComboBox model makes it re-resolve its
/// selection on the next event-loop turn, which would overwrite the selection
/// projected right after the options.
pub(super) fn replacement_options(
    current: &ModelRc<SharedString>,
    options: Vec<SharedString>,
) -> Option<ModelRc<SharedString>> {
    let unchanged = current.row_count() == options.len()
        && options
            .iter()
            .enumerate()
            .all(|(row, option)| current.row_data(row).as_ref() == Some(option));
    (!unchanged).then(|| ModelRc::new(VecModel::from(options)))
}

/// Handle the game-data language selection from the affix screen's combo
///
/// Forwards the selection through the existing
/// `SettingsPanelState::set_language` chain — the parity-shaped
/// `SettingsChangeEffects` switch state machine
/// (`AffixViewModel.cs:312-353`) — and persists via the normal settings
/// save path. The persisted token lands in `Config/Settings.json`, where
/// the settings hot-reload monitor observes it: `selected_affix_language`
/// is part of the runtime preparation signature, so a running live loop
/// requests its OCR/fuzzy rebuild within one drain tick and the next
/// runtime start prepares with the new language otherwise (upstream's
/// `AffixLanguageChangedMessage` → `OcrHandler.SetLanguage` + engine-pool
/// clear, `OcrHandler.cs:104-117`).
///
/// The selector can only produce the 14 upstream ids; anything else is a
/// programmatic caller and is structurally rejected like the
/// display-backend seam.
///
/// # Errors
///
/// Returns an error for an id outside the upstream model.
pub fn handle_language_selected(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    language: &str,
) -> Result<(), String> {
    let language = language.trim();
    if !AFFIX_LANGUAGE_IDS.contains(&language) {
        return Err(format!(
            "unknown data language '{language}'; the selector offers only the upstream ids"
        ));
    }

    let effects = state.shell.apply_language_change(language);
    state.bump_rules_input_revision();
    // An unchanged selection must not touch the file: the hot-reload
    // monitor compares file stamps, and a no-op rewrite would request a
    // pointless runtime rebuild.
    if effects.ocr_profile_rebuild_required {
        persist_settings_with_warnings(state, config_root);
        // Upstream re-projects the preset lists on a language switch
        // (`AffixViewModel.cs:321-350`); the overlay descriptors are the
        // port's equivalent projection from the preset entries.
        state.rebuild_overlay_descriptor_state();
    }
    app.set_selected_language(language.into());
    Ok(())
}

/// Persist the current `state.shell.settings` through the
/// `d4lc_config::load_app_settings_with_warnings` / `save_app_settings`
/// path shared by the fire-and-forget persist sites (the aggregated
/// `SettingsEffect::PersistConfig` dispatch and the Debug screen's
/// type-area-offset stepper in `wiring.rs`).
///
/// These sites have no `Result` channel to the caller, so a refused or
/// failed save must never be discarded: a structured
/// `AppSettingsWarning` from the load (a settings file written by a
/// newer schema version) is the reason `save_app_settings` refuses the
/// write, so both the warning and the concrete refusal are surfaced on
/// the Logging screen. The in-memory settings change stays applied
/// either way, mirroring the primary `persist_settings` semantics.
pub(crate) fn persist_settings_with_warnings(state: &mut AppState, config_root: &Path) {
    let (mut config, warnings) = match load_app_settings_with_warnings(config_root) {
        Ok(loaded) => loaded,
        Err(error) => {
            state.append_log_message(
                format!("settings load failed: {error}"),
                crate::clock::hh_mm_now(),
            );
            return;
        }
    };
    state.shell.settings.apply_to_config(&mut config);
    if let Err(error) = save_app_settings(config_root, &config) {
        for warning in &warnings {
            state.append_log_message(warning.to_string(), crate::clock::hh_mm_now());
        }
        state.append_log_message(
            format!("settings save failed: {error}"),
            crate::clock::hh_mm_now(),
        );
    }
}

/// Dispatch `SettingsEffect` values returned by ViewModel commands.
///
/// Each effect is a typed instruction that `d4lc-app` knows how to execute.
pub(super) fn dispatch_settings_effects(
    effects: &[SettingsEffect],
    state: &mut AppState,
    config_root: &Path,
) {
    for effect in effects {
        match effect {
            SettingsEffect::PersistConfig => {
                persist_settings_with_warnings(state, config_root);
            }
            SettingsEffect::OpenDialog(_kind) => {
                // OpenDialog is handled by the Slint forwarding setting
                // dialog-visible directly.  The effect is returned for
                // ViewModel consistency but doesn't need dispatching here.
            }
        }
    }
}

/// Persist the current `state.shell.settings` through
/// `d4lc_config::load_app_settings` / `save_app_settings`, like the
/// `PersistConfig` effect of `dispatch_settings_effects`. Returns `Err` on
/// filesystem failures.
pub(super) fn persist_settings(state: &AppState, config_root: &Path) -> Result<(), String> {
    let mut config =
        load_app_settings(config_root).map_err(|e| format!("Failed to load app settings: {e}"))?;
    state.shell.settings.apply_to_config(&mut config);
    save_app_settings(config_root, &config).map_err(|e| format!("Failed to save app settings: {e}"))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::actions::settings::test_support::{with_temp_config_root, ENV_MUTEX};
    use d4lc_gui::settings::DialogKind as SettingsVariant;

    #[test]
    fn options_are_replaced_only_when_the_rows_differ() {
        let rows = |names: &[&str]| -> Vec<SharedString> {
            names.iter().map(|name| SharedString::from(*name)).collect()
        };
        let current = ModelRc::new(VecModel::from(rows(&["German", "English"])));
        assert!(replacement_options(&current, rows(&["German", "English"])).is_none());
        assert!(replacement_options(&current, rows(&["German"])).is_some());
        assert!(replacement_options(&current, rows(&["German", "French"])).is_some());
        assert!(replacement_options(&ModelRc::default(), rows(&["German"])).is_some());
    }

    #[test]
    fn dispatch_persist_config_saves_settings() {
        with_temp_config_root(|config_root| {
            let mut app_state = crate::app_state::AppState::default_startup();

            // Verify no config file exists yet
            assert!(!config_root.join("Config").join("Settings.json").exists());

            dispatch_settings_effects(
                &[SettingsEffect::PersistConfig],
                &mut app_state,
                config_root,
            );

            // The confirm persisted the config file.
            assert!(config_root.join("Config").join("Settings.json").exists());
        });
    }

    /// The fire-and-forget persist path must surface a refused save
    /// instead of discarding it: a settings file written by a newer
    /// schema version loads read-only with a structured
    /// `AppSettingsWarning`, `save_app_settings` refuses fail-closed,
    /// and both the warning and the concrete refusal land on the
    /// Logging screen while the newer file stays untouched.
    #[test]
    fn dispatch_persist_config_surfaces_refused_save_from_newer_schema() {
        with_temp_config_root(|config_root| {
            let settings_path = config_root.join(d4lc_config::SETTINGS_PATH);
            std::fs::create_dir_all(settings_path.parent().unwrap_or(config_root))
                .unwrap_or_else(|error| panic!("create config dir: {error}"));
            let newer_file = r#"{"schema_version": 999}"#;
            std::fs::write(&settings_path, newer_file)
                .unwrap_or_else(|error| panic!("write settings: {error}"));

            let mut app_state = crate::app_state::AppState::default_startup();
            dispatch_settings_effects(
                &[SettingsEffect::PersistConfig],
                &mut app_state,
                config_root,
            );

            // Fail-closed: the newer file must not be overwritten.
            let after = std::fs::read_to_string(&settings_path)
                .unwrap_or_else(|error| panic!("read settings: {error}"));
            assert_eq!(
                after, newer_file,
                "a newer schema file must never be overwritten"
            );

            // The refusal is surfaced, not silently discarded.
            let messages: Vec<String> = app_state
                .logging
                .messages
                .iter()
                .map(|message| message.message.clone())
                .collect();
            assert!(
                messages
                    .iter()
                    .any(|m| m.contains("newer than supported version")),
                "structured schema warning must be surfaced, got: {messages:?}"
            );
            assert!(
                messages
                    .iter()
                    .any(|m| m.contains("settings save failed")
                        && m.contains("refusing to overwrite")),
                "save refusal must be surfaced, got: {messages:?}"
            );
        });
    }

    #[test]
    fn dispatch_open_dialog_is_noop() {
        with_temp_config_root(|config_root| {
            let mut app_state = crate::app_state::AppState::default_startup();

            // OpenDialog is a no-op in dispatch (Slint forwarding opens the dialog).
            dispatch_settings_effects(
                &[SettingsEffect::OpenDialog(SettingsVariant::Colors)],
                &mut app_state,
                config_root,
            );

            // No file should be saved for a dialog effect
            assert!(!config_root.join("Config").join("Settings.json").exists());
        });
    }

    #[test]
    fn resolve_config_root_defaults_to_xdg() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("D4LC_CONFIG_ROOT");
        let root = user_config_root();
        let path_str = root.to_string_lossy();
        assert!(
            path_str.contains("d4lc"),
            "expected d4lc in path, got: {path_str}"
        );
        // A test build never resolves to the user's configuration.
        assert_ne!(resolve_config_root(), root);
    }

    #[test]
    fn resolve_config_root_respects_env_override() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("D4LC_CONFIG_ROOT", "/tmp/d4lc-test-override");
        let root = resolve_config_root();
        assert_eq!(root, std::path::PathBuf::from("/tmp/d4lc-test-override"));
        std::env::remove_var("D4LC_CONFIG_ROOT");
    }
}
