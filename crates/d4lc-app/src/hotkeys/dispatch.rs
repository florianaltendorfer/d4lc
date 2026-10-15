//! Hotkey action handlers and the dispatch of fired hotkeys to them.

use super::backend::HotkeyEvent;
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

/// Switch the active affix preset (Ctrl+F5 by default).
///
/// Upstream: `SwitchPresetKeyBindingExecute` → `SwitchPresetKeyBindingMessage`
/// (`MainWindowViewModel.cs:282-286`).
///
/// Persistence mirrors the upstream selection setter
/// (`AffixViewModel.cs:372-373`): every switch writes the new preset
/// name into `Config/Settings.json`, so the switched-to
/// preset is restored after a restart. Failures are logged to stderr
/// like the sibling `handle_switch_overlay` hotkey persist path.
pub fn handle_switch_preset(app: &AppWindow, state: &mut AppState, config_root: &std::path::Path) {
    state.affix_preset_editor.selected_preset_name = next_switched_preset_name(state);
    state.bump_rules_input_revision();
    // The switch changes the selected preset: refresh the screen preset
    // models (selection + entry rows/cards) through the same shared seam
    // every other preset-selection path uses.
    crate::actions::affix::refresh_affix_screen_preset_models(app, state);
    state.persist_selected_affix_preset_to_config(config_root);
}

/// Switch the overlay mode (Ctrl+F6 by default).
///
/// Upstream: `SwitchOverlayKeyBindingExecute` → `SwitchOverlayKeyBindingMessage`
/// (`MainWindowViewModel.cs:288-292`).
pub fn handle_switch_overlay(app: &AppWindow, state: &mut AppState, config_root: &std::path::Path) {
    // Upstream flips only `IsParagonModeActive` (`SettingsViewModel.cs:326-332`).
    let new_value = !state.shell.settings.paragon_mode_active;
    if let Err(e) = crate::actions::settings::set_paragon_mode_active(new_value, state, config_root)
    {
        eprintln!("[d4lc] hotkey: SwitchOverlay persist failed: {e}");
        return;
    }

    app.set_paragon_mode(state.shell.settings.paragon_mode_active);
    app.set_settings_paragon_mode_active(state.shell.settings.paragon_mode_active);
}

fn next_switched_preset_name(state: &AppState) -> Option<String> {
    let presets = &state.affix_preset_editor.presets;
    if presets.is_empty() {
        return None;
    }

    let next_index = state
        .affix_preset_editor
        .selected_preset_name
        .as_deref()
        .and_then(|current| presets.iter().position(|preset| preset.name == current))
        .map(|current_index| (current_index + 1) % presets.len())
        .unwrap_or(0);

    Some(presets[next_index].name.clone())
}

/// Toggle debug lock screencapture (Ctrl+F11 by default).
///
/// Upstream: `ToggleDebugLockScreencaptureKeyBindingExecute` →
/// `ToggleDebugLockScreencaptureKeyBindingMessage`
/// (`MainWindowViewModel.cs:313-317`). The message has two consumers:
///
/// - `ScreenCaptureHandler` flips `IsScreencaptureLocked`
///   (`ScreenCaptureHandler.cs:86-89`); while the bit is set the capture
///   task stops pulling a new bitmap and re-sends the one it had
///   (`ScreenCaptureHandler.cs:246-264`), which is the hotkey tooltip's
///   contract ("Locks the latest screencapture as the default input
///   image").
/// - `OverlayHandler` shows the caption notification
///   (`OverlayHandler.cs:976-983`); the port surfaces the same event as
///   the visible log line below instead of an overlay toast.
///
/// The lock bit lives on `AppState::runtime_status` as session state,
/// exactly like the upstream handler property (never persisted), and it is
/// real: the live loop reads it every cycle
/// (`runtime_bridge::DebugCaptureLock`) and stops following the game while
/// it is set, re-drawing the frame it had when the lock was engaged.
pub fn handle_toggle_debug_lock_capture(app: &AppWindow, state: &mut AppState) {
    let new_value = !state.runtime_status.debug_lock_screencapture;
    state.runtime_status.debug_lock_screencapture = new_value;
    let record = state.record_debug_action(
        crate::app_state::DebugAction::ToggleDebugLockScreencapture,
        crate::app_state::DebugActionStatus::Applied,
        format!(
            "debug_lock_screencapture toggled to {new_value}: the live loop freezes the drawn frame while it is set"
        ),
    );
    state.append_log_message(
        format!(
            "Hotkey ToggleDebugLockScreencapture applied (#{}): {}",
            record.monotonic, record.reason
        ),
        crate::clock::hh_mm_now(),
    );
    let _ = app;
}

/// Toggle the tuning-prism hints (Ctrl+F7 by default, disabled by
/// default).
///
/// Upstream: `ToggleTuningPrismHintsKeyBindingExecute` →
/// `ToggleTuningPrismHintsKeyBindingMessage`
/// (`MainWindowViewModel.cs:306-310`); the message toggles the
/// `IsTuningPrismHintsEnabled` hint-overlay feature
/// (`SettingsViewModel.cs:366-373`).
///
/// The toggle now mutates real app-shell state —
/// `is_tuning_prism_hints_enabled` is flipped and persisted through
/// the same headless path the settings dialogs use
/// (`confirm_tuning_prism_hints_config`), and the app-owned overlay
/// descriptor projection is rebuilt so the next frame picks up the
/// new gating. The DebugAction audit record is kept in the
/// established style: `Applied` with the new value on success,
/// `Blocked` with the reason when the persist fails (state
/// stays unchanged in that case).
pub fn handle_toggle_tuning_prism_hints(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &std::path::Path,
) {
    let new_value = !state.shell.settings.is_tuning_prism_hints_enabled;
    match crate::actions::settings::confirm_tuning_prism_hints_config(new_value, state, config_root)
    {
        Ok(()) => {
            let record = state.record_debug_action(
                crate::app_state::DebugAction::ToggleTuningPrismHints,
                crate::app_state::DebugActionStatus::Applied,
                format!("is_tuning_prism_hints_enabled toggled to {new_value}"),
            );
            state.append_log_message(
                format!(
                    "Hotkey ToggleTuningPrismHints applied (#{}): {}",
                    record.monotonic, record.reason
                ),
                crate::clock::hh_mm_now(),
            );
        }
        Err(error) => {
            let record = state.record_debug_action(
                crate::app_state::DebugAction::ToggleTuningPrismHints,
                crate::app_state::DebugActionStatus::Blocked,
                format!("persist failed: {error}"),
            );
            state.append_log_message(
                format!(
                    "Hotkey ToggleTuningPrismHints blocked (#{}): {}",
                    record.monotonic, record.reason
                ),
                crate::clock::hh_mm_now(),
            );
        }
    }
    let _ = app;
}

/// A live-loop lifecycle request produced by a hotkey dispatch.
///
/// The `ToggleOverlay` arm (Ctrl+F12) flips the persisted overlay flag
/// exactly like the AffixScreen power button; upstream both send the same
/// message (`ToggleOverlayMessage` / `ToggleOverlayFromGUIMessage` →
/// `ScreenProcessHandler.IsEnabled`, `ScreenProcessHandler.cs:116-169`), so
/// both must reach the same live-loop lifecycle seam. The dispatcher cannot
/// apply it itself: the drain timer holds the `AppState` lock, and stopping
/// the loop joins its thread, which takes that lock once per frame. The
/// caller reconciles the loop with this value after releasing the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayLoopRequest {
    /// The overlay-enabled value the dispatch persisted.
    pub enabled: bool,
}

/// Dispatch a single `HotkeyEvent` to the same Rust action
/// handlers the UI / menu paths use.
///
/// Mapping:
///
/// | Action name                  | Dispatch                                                  |
/// |------------------------------|-----------------------------------------------------------|
/// | `SwitchPreset`               | `handle_switch_preset` (cycle selected preset + project state) |
/// | `SwitchOverlay`              | `handle_switch_overlay` (toggle paragon mode + persist)  |
/// | `TakeScreenshot`             | `actions::debug::handle_take_screenshot` (real handler)   |
/// | `ToggleController`           | `actions::settings::confirm_controller_config` (toggle + persist) |
/// | `ToggleOverlay`              | `actions::settings::set_overlay_enabled` (toggle + persist)       |
/// | `ToggleDebugLockScreencapture` | `handle_toggle_debug_lock_capture` (flips the live capture lock) |
/// | `ToggleTuningPrismHints`     | `handle_toggle_tuning_prism_hints` (toggle + persist) |
/// | anything else                | ignored, logged at `eprintln!`                            |
///
/// Every row above reaches a real handler; `ToggleDebugLockScreencapture`
/// flips the session lock bit the live loop honors (see the handler docs),
/// so nothing on this table is a logging-only stub.
///
/// Returns the live-loop reconcile request when the dispatch flipped the
/// persisted overlay flag (see [`OverlayLoopRequest`]); `None` for every
/// other event and for a failed overlay persist, so a failed save never
/// reports a state change.
pub fn dispatch_hotkey_event(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &std::path::Path,
    event: &HotkeyEvent,
) -> Option<OverlayLoopRequest> {
    // Record the most recent action so tests / logs can
    // observe the dispatch decision without spinning up a
    // Slint runtime.
    state.last_hotkey_event = Some(event.name.clone());

    match event.name.as_str() {
        "SwitchPreset" => {
            handle_switch_preset(app, state, config_root);
            None
        }
        "SwitchOverlay" => {
            handle_switch_overlay(app, state, config_root);
            None
        }
        "TakeScreenshot" => {
            crate::actions::debug::handle_take_screenshot(app, state);
            None
        }
        "ToggleController" => {
            // Toggle the controller-mode flag and persist
            // through the same headless helper the
            // `ControllerConfig` confirm body uses. This
            // guarantees a hotkey press and a settings dialog
            // confirm reach the same on-disk state.
            let new_value = !state.shell.settings.controller_mode;
            if let Err(e) =
                crate::actions::settings::confirm_controller_config(new_value, state, config_root)
            {
                eprintln!("[d4lc] hotkey: ToggleController persist failed: {e}");
            } else {
                app.set_controller_mode(new_value);
            }
            None
        }
        "ToggleOverlay" => {
            // Toggle the overlay-enabled flag and persist it alone
            // (`set_overlay_enabled`): the other overlay rows are neither
            // rewritten nor re-validated, so a persisted value outside the
            // dialog ranges cannot block the toggle.
            //
            // Projection: on success the new value is
            // written to the `affix-overlay-enabled` root property so
            // the AffixScreen power indicator reflects hotkey/runtime
            // toggles immediately (upstream IsAffixOverlayEnabled ←
            // ToggleOverlayKeyBinding). The SettingsScreen toggle is
            // auto-bound to `state.shell.settings.overlay_enabled`,
            // so the next time the user navigates to the Settings
            // tab they see the new value; the live loop reads
            // through the same `d4lc_config` projection and
            // re-derives its descriptors on the next cycle (the
            // confirm path already rebuilt them).
            //
            // Lifecycle: the confirmed value is returned as an
            // [`OverlayLoopRequest`] so the caller starts/stops the live
            // loop through the same seam the GUI power toggle uses —
            // upstream both messages set the one `IsEnabled`
            // (`ScreenProcessHandler.cs:116-169`).
            let new_value = !state.shell.settings.overlay_enabled;
            if let Err(e) =
                crate::actions::settings::set_overlay_enabled(new_value, state, config_root)
            {
                eprintln!("[d4lc] hotkey: ToggleOverlay persist failed: {e}");
                None
            } else {
                crate::actions::settings::project_overlay_gate(app, &state.shell.settings);
                Some(OverlayLoopRequest { enabled: new_value })
            }
        }
        "ToggleDebugLockScreencapture" => {
            handle_toggle_debug_lock_capture(app, state);
            None
        }
        "ToggleTuningPrismHints" => {
            handle_toggle_tuning_prism_hints(app, state, config_root);
            None
        }
        other => {
            eprintln!("[d4lc] hotkey: unknown action name {other:?}; ignoring event");
            None
        }
    }
}

/// Drain pending hotkey events and apply them to AppState +
/// Slint UI. Mirrors `actions::runtime::drain_runtime_events`:
/// non-blocking, `try_recv` only, intended to be called from
/// a periodic Slint `Timer`.
///
/// Returns the last live-loop reconcile request the drained events produced
/// (only `ToggleOverlay` produces one, and the newest flip wins): the caller
/// must apply it through the live-loop seam after releasing the `AppState`
/// lock (see [`OverlayLoopRequest`]).
pub fn drain_hotkey_events(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &std::path::Path,
    event_rx: &std::sync::mpsc::Receiver<HotkeyEvent>,
) -> Option<OverlayLoopRequest> {
    let mut request = None;
    while let Ok(event) = event_rx.try_recv() {
        if let Some(next) = dispatch_hotkey_event(app, state, config_root, &event) {
            request = Some(next);
        }
    }
    request
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::hotkeys::test_support::with_temp_config_root;
    use slint::SharedString;

    #[test]
    fn handle_toggle_debug_lock_capture_flips_state_and_appends_log() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        assert!(!state.runtime_status.debug_lock_screencapture);

        handle_toggle_debug_lock_capture(&app, &mut state);

        assert!(state.runtime_status.debug_lock_screencapture);
        let record = state
            .runtime_status
            .last_debug_action
            .as_ref()
            .expect("last_debug_action must be populated");
        assert_eq!(
            record.action,
            crate::app_state::DebugAction::ToggleDebugLockScreencapture
        );
        assert_eq!(record.status, crate::app_state::DebugActionStatus::Applied);
        // Visible Logging screen is appended through the real
        // `LoggingViewModel`.
        assert_eq!(state.logging.messages.len(), 1);
        assert!(state.logging.messages[0]
            .message
            .contains("ToggleDebugLockScreencapture"));

        // Toggling again returns to the original value and stamps a
        // second, higher monotonic record.
        handle_toggle_debug_lock_capture(&app, &mut state);
        assert!(!state.runtime_status.debug_lock_screencapture);
        assert_eq!(state.logging.messages.len(), 2);
        assert_eq!(
            state
                .runtime_status
                .last_debug_action
                .as_ref()
                .unwrap()
                .monotonic,
            2
        );
    }

    #[test]
    fn dispatch_toggle_debug_lock_capture_routes_through_real_handler() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let config_root = tempfile::tempdir().unwrap();
        let event = HotkeyEvent::new("ToggleDebugLockScreencapture");
        dispatch_hotkey_event(&app, &mut state, config_root.path(), &event);
        assert!(state.runtime_status.debug_lock_screencapture);
        assert_eq!(
            state.last_hotkey_event.as_deref(),
            Some("ToggleDebugLockScreencapture")
        );
    }

    #[test]
    fn dispatch_toggle_tuning_prism_hints_toggles_and_persists() {
        // The dispatch flips the persisted
        // `is_tuning_prism_hints_enabled` (default off), stamps an
        // `Applied` debug action, appends a visible log entry, and a
        // second dispatch restores the original value.
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            assert!(!state.shell.settings.is_tuning_prism_hints_enabled);

            let event = HotkeyEvent::new("ToggleTuningPrismHints");
            dispatch_hotkey_event(&app, &mut state, config_root, &event);
            assert_eq!(
                state.last_hotkey_event.as_deref(),
                Some("ToggleTuningPrismHints")
            );
            assert!(state.shell.settings.is_tuning_prism_hints_enabled);

            let record = state
                .runtime_status
                .last_debug_action
                .as_ref()
                .expect("last_debug_action must be populated by the toggle");
            assert_eq!(
                record.action,
                crate::app_state::DebugAction::ToggleTuningPrismHints
            );
            assert_eq!(record.status, crate::app_state::DebugActionStatus::Applied);
            assert!(record.reason.contains("true"));
            assert_eq!(state.logging.messages.len(), 1);
            assert!(state.logging.messages[0]
                .message
                .contains("ToggleTuningPrismHints"));

            // The toggle is persisted through the settings path.
            let config = d4lc_config::load_app_settings(config_root)
                .expect("settings file should exist after the toggle");
            assert!(config.is_tuning_prism_hints_enabled);

            // Toggling again returns to the default-off value.
            dispatch_hotkey_event(&app, &mut state, config_root, &event);
            assert!(!state.shell.settings.is_tuning_prism_hints_enabled);
            assert_eq!(
                state
                    .runtime_status
                    .last_debug_action
                    .as_ref()
                    .unwrap()
                    .monotonic,
                2
            );
            let config = d4lc_config::load_app_settings(config_root)
                .expect("settings file should exist after the second toggle");
            assert!(!config.is_tuning_prism_hints_enabled);
        });
    }

    // -- dispatch_hotkey_event behaviour ---------------------------

    fn sample_preset(name: &str) -> d4lc_gui::AffixPresetEditorPreset {
        d4lc_gui::AffixPresetEditorPreset {
            name: name.to_string(),
            entries: Vec::new(),
        }
    }

    fn seed_presets(state: &mut AppState, names: &[&str], selected: Option<&str>) {
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(
            names.iter().map(|name| sample_preset(name)).collect(),
        );
        state.affix_preset_editor.selected_preset_name = selected.map(str::to_string);
    }

    fn assert_selected_preset_projection(
        app: &crate::slint_generated::AppWindow,
        state: &AppState,
        expected_selected: Option<&str>,
    ) {
        assert_eq!(
            state.affix_preset_editor.selected_preset_name.as_deref(),
            expected_selected
        );
        assert_eq!(
            state.shell.affix_presets.selected_preset.as_deref(),
            expected_selected
        );
        assert_eq!(
            state.shell.affix_presets.preset_count,
            state.affix_preset_editor.presets.len()
        );
        assert_eq!(
            app.get_selected_preset().to_string(),
            expected_selected.unwrap_or("")
        );
    }

    #[test]
    fn handle_switch_preset_empty_list_clears_selection_and_projections() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            state.affix_preset_editor.selected_preset_name = Some("Ghost".to_string());
            state.shell.affix_presets.selected_preset = Some("Ghost".to_string());
            state.shell.affix_presets.preset_count = 99;
            app.set_selected_preset(SharedString::from("Ghost"));

            handle_switch_preset(&app, &mut state, config_root);

            assert_selected_preset_projection(&app, &state, None);
        });
    }

    #[test]
    fn handle_switch_preset_single_preset_keeps_selection() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            seed_presets(&mut state, &["Solo"], Some("Solo"));

            handle_switch_preset(&app, &mut state, config_root);

            assert_selected_preset_projection(&app, &state, Some("Solo"));
            // The switched-to selection persists.
            let settings = d4lc_config::load_app_settings(config_root).unwrap_or_default();
            assert_eq!(settings.selected_affix_preset, "Solo");
        });
    }

    #[test]
    fn handle_switch_preset_advances_to_next_preset() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            seed_presets(&mut state, &["Alpha", "Beta", "Gamma"], Some("Beta"));

            handle_switch_preset(&app, &mut state, config_root);

            assert_selected_preset_projection(&app, &state, Some("Gamma"));
        });
    }

    #[test]
    fn handle_switch_preset_wraps_from_last_to_first() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            seed_presets(&mut state, &["Alpha", "Beta", "Gamma"], Some("Gamma"));

            handle_switch_preset(&app, &mut state, config_root);

            assert_selected_preset_projection(&app, &state, Some("Alpha"));
        });
    }

    #[test]
    fn handle_switch_preset_missing_current_selection_falls_back_to_first() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            seed_presets(&mut state, &["Alpha", "Beta", "Gamma"], Some("Ghost"));

            handle_switch_preset(&app, &mut state, config_root);

            assert_selected_preset_projection(&app, &state, Some("Alpha"));
        });
    }

    #[test]
    fn handle_switch_overlay_toggles_props_and_persists_round_trip() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            assert!(!state.shell.settings.paragon_mode_active);
            // The toggle flips only the mode: a configured layout survives.
            state.shell.settings.collapsed_paragonboard_enabled = true;
            state.shell.settings.paragon_border_size = 4;
            state.shell.settings.paragon_node_size = 55;
            state.shell.settings.paragon_node_size_collapsed = 30;
            state.shell.settings.paragon_left_offset_collapsed = -120;
            state.shell.settings.paragon_top_offset_collapsed = 75;

            handle_switch_overlay(&app, &mut state, config_root);

            assert!(state.shell.settings.paragon_mode_active);
            assert!(app.get_paragon_mode());
            assert!(app.get_settings_paragon_mode_active());
            let persisted =
                d4lc_config::load_app_settings(config_root).expect("settings must persist");
            assert!(persisted.paragon_mode_active);

            handle_switch_overlay(&app, &mut state, config_root);

            assert!(!state.shell.settings.paragon_mode_active);
            assert!(!app.get_paragon_mode());
            assert!(!app.get_settings_paragon_mode_active());
            let persisted =
                d4lc_config::load_app_settings(config_root).expect("settings must persist");
            assert!(!persisted.paragon_mode_active);
            assert!(persisted.rule_settings.is_collapsed_paragonboard_enabled);
            assert_eq!(persisted.paragon_border_size, 4);
            assert_eq!(persisted.paragon_node_size, 55);
            assert_eq!(persisted.paragon_node_size_collapsed, 30);
            assert_eq!(persisted.paragon_left_offset_collapsed, -120);
            assert_eq!(persisted.paragon_top_offset_collapsed, 75);
        });
    }

    #[test]
    fn dispatch_switch_preset_records_last_event_and_applies_selection() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let config_root = tempfile::tempdir().unwrap();
        seed_presets(&mut state, &["Alpha", "Beta", "Gamma"], Some("Beta"));
        let event = HotkeyEvent::new("SwitchPreset");
        dispatch_hotkey_event(&app, &mut state, config_root.path(), &event);
        assert_eq!(state.last_hotkey_event.as_deref(), Some("SwitchPreset"));
        assert_selected_preset_projection(&app, &state, Some("Gamma"));
    }

    #[test]
    fn dispatch_toggle_controller_flips_and_persists() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            assert_eq!(state.shell.settings.controller_mode, false);
            let event = HotkeyEvent::new("ToggleController");
            dispatch_hotkey_event(&app, &mut state, config_root, &event);
            assert_eq!(state.shell.settings.controller_mode, true);
            // Round-trip via the same `d4lc_config` path the
            // confirm body uses — this is the contract the
            // settings dialog confirm also satisfies.
            let persisted =
                d4lc_config::load_app_settings(config_root).expect("settings must persist");
            assert_eq!(persisted.controller_mode, true);
        });
    }

    #[test]
    fn dispatch_toggle_overlay_flips_and_persists() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            assert_eq!(state.shell.settings.overlay_enabled, false);
            // Only the on/off bit flips: the other rows keep their values.
            state.shell.settings.item_power_limit_enabled = true;
            state.shell.settings.item_power_limit = 750;
            state.shell.settings.show_overlay_icon = true;
            state.shell.settings.overlay_icon_pos_x = 120;
            state.shell.settings.overlay_marker_mode =
                d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
            let event = HotkeyEvent::new("ToggleOverlay");
            let request = dispatch_hotkey_event(&app, &mut state, config_root, &event);
            assert_eq!(state.shell.settings.overlay_enabled, true);
            assert_eq!(state.shell.settings.item_power_limit, 750);
            assert!(state.shell.settings.show_overlay_icon);
            assert_eq!(state.shell.settings.overlay_icon_pos_x, 120);
            assert_eq!(
                state.shell.settings.overlay_marker_mode,
                d4lc_config::OverlayMarkerModeSetting::HideUnwanted
            );
            // The hotkey toggle routes back to the
            // AffixScreen power indicator through the root property.
            assert_eq!(
                app.get_affix_overlay_enabled(),
                true,
                "hotkey overlay toggle must project into the screen indicator"
            );
            // Upstream `SystemPresetChangeAllowed = !IsEnabled`
            // (SettingsViewModel.cs:379): the preset gate closes while the
            // overlay is on.
            assert!(!app.get_system_preset_change_allowed());
            let persisted =
                d4lc_config::load_app_settings(config_root).expect("settings must persist");
            assert_eq!(persisted.overlay_enabled, true);
            // The dispatch hands the confirmed value to the caller, which
            // reconciles the live loop through the same seam the GUI power
            // toggle uses.
            assert_eq!(request, Some(OverlayLoopRequest { enabled: true }));
        });
    }

    #[test]
    fn dispatch_toggle_overlay_off_requests_the_loop_stop() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            state.shell.settings.overlay_enabled = true;
            let event = HotkeyEvent::new("ToggleOverlay");
            let request = dispatch_hotkey_event(&app, &mut state, config_root, &event);
            assert_eq!(state.shell.settings.overlay_enabled, false);
            assert_eq!(request, Some(OverlayLoopRequest { enabled: false }));
        });
    }

    #[test]
    fn drain_hotkey_events_returns_the_last_overlay_request() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            let (tx, rx) = std::sync::mpsc::channel();
            // Two flips queue in one tick: the drain must report the newest
            // value, never an intermediate one.
            tx.send(HotkeyEvent::new("ToggleOverlay")).expect("send on");
            tx.send(HotkeyEvent::new("ToggleOverlay"))
                .expect("send off");
            let request = drain_hotkey_events(&app, &mut state, config_root, &rx);
            assert_eq!(state.shell.settings.overlay_enabled, false);
            assert_eq!(request, Some(OverlayLoopRequest { enabled: false }));

            // Non-overlay events produce no request.
            tx.send(HotkeyEvent::new("SwitchPreset")).expect("send");
            assert_eq!(
                drain_hotkey_events(&app, &mut state, config_root, &rx),
                None
            );
        });
    }

    #[test]
    fn dispatch_unknown_action_does_not_panic_and_records_name() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let config_root = tempfile::tempdir().unwrap();
        let event = HotkeyEvent::new("NotARealAction");
        dispatch_hotkey_event(&app, &mut state, config_root.path(), &event);
        // The action name is still recorded so observability
        // / log readers can see that an unknown event was
        // received and silently dropped.
        assert_eq!(state.last_hotkey_event.as_deref(), Some("NotARealAction"));
    }

    #[test]
    fn drain_hotkey_events_is_non_blocking() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let config_root = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<HotkeyEvent>();
        tx.send(HotkeyEvent::new("SwitchOverlay")).unwrap();
        drop(tx);
        drain_hotkey_events(&app, &mut state, config_root.path(), &rx);
        assert_eq!(state.last_hotkey_event.as_deref(), Some("SwitchOverlay"));
    }
}
