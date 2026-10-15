//! Settings ViewModel — pure state + command methods returning typed effects.
//!
//! ## Architecture
//!
//! `SettingsViewModel` wraps [`SettingsPanelState`] and provides command methods
//! that return `Vec<SettingsEffect>`. The `d4lc-app` crate calls these methods
//! and interprets the returned effects (persist config, open dialogs, etc.).
//!
//! ## Upstream evidence
//!
//! - `SettingsViewModel.cs:94-99` — 6 command definitions
//! - `SettingsViewModel.cs:107-189` — toggle properties with immediate `SaveSettings()`
//! - `SettingsViewModel.cs:447-515` — dialog command methods (open dialog, wait, save)
//!
//! ## Boundary rules
//!
//! - No Slint handles, no file I/O, no platform state, no runtime handles.
//! - All side effects are returned as typed [`SettingsEffect`] values.
//! - The app layer (`d4lc-app`) interprets effects and executes platform code.

mod panel;

use crate::SettingsPanelState;

/// The game-data language ids the language combos offer, in upstream model
/// order (`AffixViewModel.InitAffixLanguages`,
/// `D4Companion/ViewModels/AffixViewModel.cs:1640-1663`). The persisted
/// `SelectedAffixLanguage` token is one of these ids; country-flag images
/// stay optional and are not rendered by the port's text-only combos.
pub const AFFIX_LANGUAGE_IDS: [&str; 14] = [
    "deDE", "enUS", "esES", "esMX", "frFR", "itIT", "jaJP", "koKR", "plPL", "ptBR", "ruRU", "trTR",
    "zhCN", "zhTW",
];

/// Resolve the combo selection for a stored language token.
///
/// Mirrors the upstream seed guard (`AffixViewModel.cs:1656-1662`): the
/// persisted id is selected only when it matches a known model entry. An
/// unknown stored token selects nothing (`None`) instead of silently
/// claiming a selection the runtime seam will not honor.
pub fn affix_language_selection(stored: &str) -> Option<&'static str> {
    AFFIX_LANGUAGE_IDS.iter().copied().find(|id| *id == stored)
}

/// One UI language the selector offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLanguage {
    /// Culture id persisted as `SelectedAppLanguage`.
    pub id: &'static str,
    /// Selector caption. Upstream hard-codes these English names rather than
    /// localizing them.
    pub name: &'static str,
}

/// The UI languages the selector offers, in upstream order
/// (`SettingsViewModel.InitApplanguages`,
/// `D4Companion/ViewModels/SettingsViewModel.cs:399-415`). Upstream lists the
/// other cultures of its `AppLanguage` table commented out; only the ones
/// with a resource file are offered, and `zh-CN` is among them (`:414`).
pub const APP_LANGUAGES: [AppLanguage; 5] = [
    AppLanguage {
        id: "de-DE",
        name: "German",
    },
    AppLanguage {
        id: "en-US",
        name: "English",
    },
    AppLanguage {
        id: "es-ES",
        name: "Spanish (EU)",
    },
    AppLanguage {
        id: "fr-FR",
        name: "French",
    },
    AppLanguage {
        id: "zh-CN",
        name: "Chinese (Simplified)",
    },
];

/// Resolve the UI language for a stored culture id.
///
/// Mirrors the upstream seed guard (`SettingsViewModel.cs:417-421`): the
/// stored id selects an entry only when it matches one; an unknown id selects
/// nothing and the caller keeps the neutral (English) strings.
pub fn app_language_by_id(stored: &str) -> Option<&'static AppLanguage> {
    APP_LANGUAGES.iter().find(|language| language.id == stored)
}

/// Resolve the UI language for a selector caption.
pub fn app_language_by_name(name: &str) -> Option<&'static AppLanguage> {
    APP_LANGUAGES.iter().find(|language| language.name == name)
}

/// Effect emitted by a [`SettingsViewModel`] command.
///
/// `d4lc-app` must interpret these and execute the corresponding
/// platform-specific behavior.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsEffect {
    /// Persist the current settings to config storage.
    PersistConfig,
    /// Open a config dialog. The app layer shows/hosts the dialog.
    OpenDialog(DialogKind),
}

/// Identifies which config dialog to open.
///
/// Mirrors the 5 config dialog commands in `SettingsViewModel.cs:94-99`:
/// - `SetColorsCommand` → [`DialogKind::Colors`]
/// - `SetControllerConfigCommand` → [`DialogKind::ControllerConfig`]
/// - `SetHotkeysCommand` → [`DialogKind::Hotkeys`]
/// - `SetOverlayConfigCommand` → [`DialogKind::OverlayConfig`]
/// - `SetParagonConfigCommand` → [`DialogKind::ParagonConfig`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// Open colors config dialog (`SetColorsCommand`)
    Colors,
    /// Open controller config dialog (`SetControllerConfigCommand`)
    ControllerConfig,
    /// Open hotkeys config dialog (`SetHotkeysCommand`)
    Hotkeys,
    /// Open overlay config dialog (`SetOverlayConfigCommand`)
    OverlayConfig,
    /// Open paragon config dialog (`SetParagonConfigCommand`)
    ParagonConfig,
}

/// Input values for [`SettingsViewModel::apply_toggles`].
///
/// Carries all 6 upstream toggle values plus the port-specific
/// diagnostics-export toggle, read from the Slint AppWindow properties.
/// Mirrors properties in `SettingsViewModel.cs:107-189`:
/// - `IsDebugModeEnabled`
/// - `IsCheckForUpdatesEnabled`
/// - `IsLaunchMinimizedEnabled`
/// - `IsMinimizeToTrayEnabled`
/// - `IsControllerModeEnabled`
/// - `IsParagonModeActive`
///
/// The port adds:
/// - `diagnostics_export_enabled` (local crash-log export, default off;
///   no upload path exists)
/// - `update_check_enabled` (check-only release banner, default on;
///   never downloads or installs anything)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToggleInput {
    pub debug_mode: bool,
    pub check_for_updates: bool,
    pub launch_minimized: bool,
    pub minimize_to_tray: bool,
    pub controller_mode: bool,
    pub paragon_mode_active: bool,
    pub diagnostics_export_enabled: bool,
    pub update_check_enabled: bool,
}

/// ViewModel for the Settings screen.
///
/// Pure state + command methods. Every command returns `Vec<SettingsEffect>`
/// that the app layer interprets. No Slint handles, no file I/O, no platform
/// state, no runtime handles.
///
/// ## Pattern
///
/// ```text
/// Slint callback → d4lc-app reads props → ViewModel command → effects
///                                                              ↓
///                                                      d4lc-app interprets
/// ```
///
/// ## Upstream reference
///
/// Patterns from `SettingsViewModel.cs:25-99`:
/// - Properties read/write through `SettingsManager.Settings`
/// - Commands are `RelayCommand` properties (→ methods returning effects)
/// - Dialog commands open a dialog, wait for close, then save
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsViewModel {
    /// The underlying settings panel state.
    pub state: SettingsPanelState,
    /// Active dialog state. None = no dialog open, Some = dialog of given kind open.
    pub dialog_state: Option<DialogKind>,
}

impl SettingsViewModel {
    /// Wrap an existing [`SettingsPanelState`].
    pub fn new(state: SettingsPanelState) -> Self {
        Self {
            state,
            dialog_state: None,
        }
    }

    /// Confirm the active dialog: save changes and close.
    ///
    /// Clears the dialog state and signals that config should be persisted.
    pub fn confirm_dialog(&mut self) -> Vec<SettingsEffect> {
        self.dialog_state = None;
        vec![SettingsEffect::PersistConfig]
    }

    /// Cancel the active dialog: discard draft changes and close.
    ///
    /// Clears the dialog state without persisting.
    pub fn cancel_dialog(&mut self) -> Vec<SettingsEffect> {
        self.dialog_state = None;
        vec![] // Don't persist on cancel
    }

    /// Apply 6 toggle values from the UI and return effects.
    ///
    /// Updates the internal [`SettingsPanelState`] and returns
    /// `[SettingsEffect::PersistConfig]` when any value actually changed.
    /// Returns an empty vec when no values differ (avoids unnecessary saves).
    pub fn apply_toggles(&mut self, input: ToggleInput) -> Vec<SettingsEffect> {
        let mut effects = Vec::new();

        if self.state.debug_mode != input.debug_mode
            || self.state.check_for_updates != input.check_for_updates
            || self.state.launch_minimized != input.launch_minimized
            || self.state.minimize_to_tray != input.minimize_to_tray
            || self.state.controller_mode != input.controller_mode
            || self.state.paragon_mode_active != input.paragon_mode_active
            || self.state.diagnostics_export_enabled != input.diagnostics_export_enabled
            || self.state.update_check_enabled != input.update_check_enabled
        {
            self.state.debug_mode = input.debug_mode;
            self.state.check_for_updates = input.check_for_updates;
            self.state.launch_minimized = input.launch_minimized;
            self.state.minimize_to_tray = input.minimize_to_tray;
            self.state.controller_mode = input.controller_mode;
            self.state.paragon_mode_active = input.paragon_mode_active;
            self.state.diagnostics_export_enabled = input.diagnostics_export_enabled;
            self.state.update_check_enabled = input.update_check_enabled;
            effects.push(SettingsEffect::PersistConfig);
        }

        effects
    }

    /// Command: open the default colors config dialog.
    ///
    /// Mirrors `SetColorsCommand` / `SetColorsExecute` (`SettingsViewModel.cs:461-473`).
    /// The upstream implementation opens a `ColorsConfigView` dialog, awaits close,
    /// then calls `SaveSettings()`.
    ///
    /// Returns `[SettingsEffect::OpenDialog(DialogKind::Colors)]`.
    pub fn open_colors_config(&mut self) -> Vec<SettingsEffect> {
        self.dialog_state = Some(DialogKind::Colors);
        vec![SettingsEffect::OpenDialog(DialogKind::Colors)]
    }

    /// Command: open the controller config dialog.
    ///
    /// Mirrors `SetControllerConfigCommand` / `SetControllerConfigExecute`
    /// (`SettingsViewModel.cs:447-459`).
    ///
    /// Returns `[SettingsEffect::OpenDialog(DialogKind::ControllerConfig)]`.
    pub fn open_controller_config(&mut self) -> Vec<SettingsEffect> {
        self.dialog_state = Some(DialogKind::ControllerConfig);
        vec![SettingsEffect::OpenDialog(DialogKind::ControllerConfig)]
    }

    /// Command: open the hotkeys config dialog.
    ///
    /// Mirrors `SetHotkeysCommand` / `SetHotkeysExecute` (`SettingsViewModel.cs:475-487`).
    ///
    /// Returns `[SettingsEffect::OpenDialog(DialogKind::Hotkeys)]`.
    pub fn open_hotkeys_config(&mut self) -> Vec<SettingsEffect> {
        self.dialog_state = Some(DialogKind::Hotkeys);
        vec![SettingsEffect::OpenDialog(DialogKind::Hotkeys)]
    }

    /// Command: open the overlay config dialog.
    ///
    /// Mirrors `SetOverlayConfigCommand` / `SetOverlayConfigExecute`
    /// (`SettingsViewModel.cs:489-501`).
    ///
    /// Returns `[SettingsEffect::OpenDialog(DialogKind::OverlayConfig)]`.
    pub fn open_overlay_config(&mut self) -> Vec<SettingsEffect> {
        self.dialog_state = Some(DialogKind::OverlayConfig);
        vec![SettingsEffect::OpenDialog(DialogKind::OverlayConfig)]
    }

    /// Command: open the paragon config dialog.
    ///
    /// Mirrors `SetParagonConfigCommand` / `SetParagonConfigExecute`
    /// (`SettingsViewModel.cs:503-515`).
    ///
    /// Returns `[SettingsEffect::OpenDialog(DialogKind::ParagonConfig)]`.
    pub fn open_paragon_config(&mut self) -> Vec<SettingsEffect> {
        self.dialog_state = Some(DialogKind::ParagonConfig);
        vec![SettingsEffect::OpenDialog(DialogKind::ParagonConfig)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SettingsPanelState;

    // ---------------------------------------------------------------------------
    // new
    // ---------------------------------------------------------------------------

    #[test]
    fn new_wraps_panel_state() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let vm = SettingsViewModel::new(panel.clone());
        assert_eq!(vm.state, panel);
    }

    // ---------------------------------------------------------------------------
    // apply_toggles – no change
    // ---------------------------------------------------------------------------

    #[test]
    fn apply_toggles_no_change_returns_empty_effects() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let input = ToggleInput {
            debug_mode: panel.debug_mode,
            check_for_updates: panel.check_for_updates,
            launch_minimized: panel.launch_minimized,
            minimize_to_tray: panel.minimize_to_tray,
            controller_mode: panel.controller_mode,
            paragon_mode_active: panel.paragon_mode_active,
            diagnostics_export_enabled: panel.diagnostics_export_enabled,
            update_check_enabled: panel.update_check_enabled,
        };

        let mut vm = SettingsViewModel::new(panel);
        let effects = vm.apply_toggles(input);

        assert!(effects.is_empty());
    }

    // ---------------------------------------------------------------------------
    // apply_toggles – single change
    // ---------------------------------------------------------------------------

    #[test]
    fn apply_toggles_change_returns_persist() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let input = ToggleInput {
            debug_mode: !panel.debug_mode,
            check_for_updates: panel.check_for_updates,
            launch_minimized: panel.launch_minimized,
            minimize_to_tray: panel.minimize_to_tray,
            controller_mode: panel.controller_mode,
            paragon_mode_active: panel.paragon_mode_active,
            diagnostics_export_enabled: panel.diagnostics_export_enabled,
            update_check_enabled: panel.update_check_enabled,
        };

        let mut vm = SettingsViewModel::new(panel);
        let effects = vm.apply_toggles(input);

        assert_eq!(effects, vec![SettingsEffect::PersistConfig]);
    }

    // ---------------------------------------------------------------------------
    // apply_toggles – all fields updated
    // ---------------------------------------------------------------------------

    #[test]
    fn apply_toggles_updates_all_fields() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let input = ToggleInput {
            debug_mode: !panel.debug_mode,
            check_for_updates: !panel.check_for_updates,
            launch_minimized: !panel.launch_minimized,
            minimize_to_tray: !panel.minimize_to_tray,
            controller_mode: !panel.controller_mode,
            paragon_mode_active: !panel.paragon_mode_active,
            diagnostics_export_enabled: !panel.diagnostics_export_enabled,
            update_check_enabled: !panel.update_check_enabled,
        };

        let mut vm = SettingsViewModel::new(panel);
        let effects = vm.apply_toggles(input);

        assert_eq!(effects, vec![SettingsEffect::PersistConfig]);

        assert_eq!(vm.state.debug_mode, input.debug_mode);
        assert_eq!(vm.state.check_for_updates, input.check_for_updates);
        assert_eq!(vm.state.launch_minimized, input.launch_minimized);
        assert_eq!(vm.state.minimize_to_tray, input.minimize_to_tray);
        assert_eq!(vm.state.controller_mode, input.controller_mode);
        assert_eq!(vm.state.paragon_mode_active, input.paragon_mode_active);
        assert_eq!(
            vm.state.diagnostics_export_enabled,
            input.diagnostics_export_enabled
        );
        assert_eq!(vm.state.update_check_enabled, input.update_check_enabled);
    }

    // ---------------------------------------------------------------------------
    // open_*_config
    // ---------------------------------------------------------------------------

    #[test]
    fn open_colors_config_returns_open_dialog_effect() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let mut vm = SettingsViewModel::new(panel);
        assert_eq!(
            vm.open_colors_config(),
            vec![SettingsEffect::OpenDialog(DialogKind::Colors)]
        );
        assert_eq!(vm.dialog_state, Some(DialogKind::Colors));
    }

    #[test]
    fn open_controller_config_returns_open_dialog_effect() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let mut vm = SettingsViewModel::new(panel);
        assert_eq!(
            vm.open_controller_config(),
            vec![SettingsEffect::OpenDialog(DialogKind::ControllerConfig)]
        );
        assert_eq!(vm.dialog_state, Some(DialogKind::ControllerConfig));
    }

    #[test]
    fn open_hotkeys_config_returns_open_dialog_effect() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let mut vm = SettingsViewModel::new(panel);
        assert_eq!(
            vm.open_hotkeys_config(),
            vec![SettingsEffect::OpenDialog(DialogKind::Hotkeys)]
        );
        assert_eq!(vm.dialog_state, Some(DialogKind::Hotkeys));
    }

    #[test]
    fn open_overlay_config_returns_open_dialog_effect() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let mut vm = SettingsViewModel::new(panel);
        assert_eq!(
            vm.open_overlay_config(),
            vec![SettingsEffect::OpenDialog(DialogKind::OverlayConfig)]
        );
        assert_eq!(vm.dialog_state, Some(DialogKind::OverlayConfig));
    }

    #[test]
    fn open_paragon_config_returns_open_dialog_effect() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let mut vm = SettingsViewModel::new(panel);
        assert_eq!(
            vm.open_paragon_config(),
            vec![SettingsEffect::OpenDialog(DialogKind::ParagonConfig)]
        );
        assert_eq!(vm.dialog_state, Some(DialogKind::ParagonConfig));
    }

    // -----------------------------------------------------------------------
    // confirm_dialog / cancel_dialog
    // -----------------------------------------------------------------------

    #[test]
    fn confirm_dialog_clears_state_and_returns_persist() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let mut vm = SettingsViewModel::new(panel);
        vm.dialog_state = Some(DialogKind::Colors);
        let effects = vm.confirm_dialog();
        assert_eq!(vm.dialog_state, None);
        assert_eq!(effects, vec![SettingsEffect::PersistConfig]);
    }

    #[test]
    fn cancel_dialog_clears_state_and_returns_empty() {
        let panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        let mut vm = SettingsViewModel::new(panel);
        vm.dialog_state = Some(DialogKind::Colors);
        let effects = vm.cancel_dialog();
        assert_eq!(vm.dialog_state, None);
        assert!(effects.is_empty());
    }

    // -----------------------------------------------------------------------
    // AFFIX_LANGUAGE_IDS / affix_language_selection
    // -----------------------------------------------------------------------

    /// The combo model must be exactly the 14 upstream ids in
    /// `AffixViewModel.InitAffixLanguages` order — the persisted
    /// `SelectedAffixLanguage` token is one of these ids, so a drifted
    /// model would make a stored selection unseedable.
    #[test]
    fn affix_language_model_lists_the_14_upstream_ids() {
        assert_eq!(
            AFFIX_LANGUAGE_IDS,
            [
                "deDE", "enUS", "esES", "esMX", "frFR", "itIT", "jaJP", "koKR", "plPL", "ptBR",
                "ruRU", "trTR", "zhCN", "zhTW",
            ]
        );
    }

    /// The UI languages are upstream's selector list (`SettingsViewModel.cs:399-415`).
    #[test]
    fn app_languages_match_the_upstream_selector() {
        let ids: Vec<&str> = APP_LANGUAGES.iter().map(|language| language.id).collect();
        assert_eq!(ids, ["de-DE", "en-US", "es-ES", "fr-FR", "zh-CN"]);
        assert_eq!(app_language_by_id("fr-FR").map(|l| l.name), Some("French"));
        assert_eq!(
            app_language_by_name("Spanish (EU)").map(|l| l.id),
            Some("es-ES")
        );
        assert_eq!(app_language_by_id("es-MX"), None);
        assert_eq!(app_language_by_id(""), None);
        assert_eq!(app_language_by_name("de-DE"), None);
    }

    #[test]
    fn app_language_is_independent_of_the_game_data_language() {
        let mut panel =
            SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new());
        assert_eq!(
            (panel.app_language.as_str(), panel.language.as_str()),
            ("en-US", "enUS")
        );
        assert!(panel.set_app_language("de-DE"));
        assert!(!panel.set_app_language("de-DE"));
        assert_eq!(
            (panel.app_language.as_str(), panel.language.as_str()),
            ("de-DE", "enUS")
        );
        let mut config = d4lc_config::AppSettings::default();
        panel.apply_to_config(&mut config);
        assert_eq!(config.selected_app_language, "de-DE");
        assert_eq!(config.selected_affix_language, "enUS");
    }

    /// Upstream seed guard (`AffixViewModel.cs:1656-1662`): a stored id
    /// matching a model entry selects it; an unknown stored token falls
    /// back to "no selection" and must never invent a default.
    #[test]
    fn affix_language_selection_matches_known_ids_and_falls_back() {
        assert_eq!(affix_language_selection("enUS"), Some("enUS"));
        assert_eq!(affix_language_selection("zhTW"), Some("zhTW"));
        assert_eq!(affix_language_selection("klingon"), None);
        assert_eq!(affix_language_selection(""), None);
        // Short codes from the pre-parity combo defaults are not ids.
        assert_eq!(affix_language_selection("EN"), None);
    }
}
