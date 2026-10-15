use super::*;

// -----------------------------------------------------------------------
// AppState tests
// -----------------------------------------------------------------------

#[test]
fn default_startup_has_no_overlay_state() {
    let state = AppState::default_startup();
    assert!(state.overlay_descriptor_state.is_none());
    assert_eq!(state.shell.window_title, "d4lc");
    assert!(state.affix_preset_editor.presets.is_empty());
}

#[test]
fn default_startup_has_no_last_hotkey_event() {
    // The runtime hotkey event
    // dispatcher sets `last_hotkey_event` on every event;
    // it must be `None` until then.
    let state = AppState::default_startup();
    assert!(state.last_hotkey_event.is_none());
}

#[test]
fn app_state_wraps_gui_shell_and_exposes_all_panels() {
    let shell = GuiShellState::default_startup();
    let state = AppState::from_shell(shell);

    assert_eq!(state.current_item().parse_status, "not_item");
    assert_eq!(state.trade_imports().trade_items.len(), 0);
    assert_eq!(state.debug().debug_artifact_count, 0);
    assert_eq!(state.settings().selected_system_preset, "1440p_SMF");
}

#[test]
fn app_state_allows_mutable_panel_access() {
    let mut state = AppState::default_startup();
    state.current_item_mut().parse_status = "ok".to_string();
    assert_eq!(state.current_item().parse_status, "ok");
}

#[test]
fn app_state_holds_affix_preset_editor() {
    let mut state = AppState::default_startup();
    assert!(state.affix_preset_editor.create_preset("Speed").is_ok());
    assert_eq!(state.affix_preset_editor.presets.len(), 1);
    assert_eq!(
        state.affix_preset_editor.selected_preset_name.as_deref(),
        Some("Speed")
    );
}

#[test]
fn app_state_is_clone_and_debug() {
    let state = AppState::default_startup();
    let cloned = state.clone();
    assert!(!format!("{:?}", cloned).is_empty());
}

// -----------------------------------------------------------------------
// Hotkey draft round-trip
// -----------------------------------------------------------------------

#[test]
fn app_state_default_startup_has_empty_hotkey_drafts() {
    // The drafts list is populated by
    // `hydrate_hotkey_drafts_from_settings`; the default
    // startup state is empty so the dialog can seed a
    // fresh default on first open.
    let state = AppState::default_startup();
    assert!(state.hotkey_drafts.is_empty());
}

#[test]
fn hydrate_hotkey_drafts_from_settings_projects_seven_entries() {
    use d4lc_config::AppSettings;
    let mut state = AppState::default_startup();
    let settings = AppSettings::default();
    state.hydrate_hotkey_drafts_from_settings(&settings);
    assert_eq!(state.hotkey_drafts.len(), 7);
    // Names mirror the stable upstream identifiers.
    let names: Vec<&str> = state
        .hotkey_drafts
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "SwitchPreset",
            "SwitchOverlay",
            "TakeScreenshot",
            "ToggleController",
            "ToggleOverlay",
            "ToggleDebugLockScreencapture",
            "ToggleTuningPrismHints",
        ]
    );
    // Default bindings + all disabled.
    assert_eq!(state.hotkey_drafts[0].binding, "Ctrl+F5");
    assert!(state.hotkey_drafts.iter().all(|d| !d.enabled));
}

#[test]
fn apply_hotkey_drafts_to_settings_round_trips_through_state() {
    use d4lc_config::{AppSettings, HotkeyBinding};
    let mut state = AppState::default_startup();
    let mut settings = AppSettings::default();
    // User edits: enable SwitchPreset, change its binding.
    let draft = HotkeyDraft {
        name: "SwitchPreset".to_string(),
        label: "Switch preset".to_string(),
        binding: "Ctrl+Shift+F5".to_string(),
        enabled: true,
    };
    state.hotkey_drafts.push(draft.clone());
    state.apply_hotkey_drafts_to_settings(&mut settings);
    let updated: &HotkeyBinding = settings
        .hotkey_config
        .find_by_name("SwitchPreset")
        .expect("SwitchPreset entry must exist after round-trip");
    assert_eq!(updated.binding, "Ctrl+Shift+F5");
    assert!(updated.enabled);
    // Other entries keep their upstream defaults (still 7 total).
    assert_eq!(settings.hotkey_config.bindings.len(), 7);
}

#[test]
fn hydrate_then_apply_round_trips_through_settings() {
    use d4lc_config::AppSettings;
    let mut state = AppState::default_startup();
    let mut settings = AppSettings::default();
    // Simulate a user toggling one binding and one
    // custom binding edit.
    settings
        .hotkey_config
        .find_by_name_mut("TakeScreenshot")
        .unwrap()
        .binding = "Ctrl+Shift+F10".to_string();
    state.hydrate_hotkey_drafts_from_settings(&settings);
    assert_eq!(state.hotkey_drafts.len(), 7);
    // Apply back to a fresh AppSettings.
    let mut reloaded = AppSettings::default();
    state.apply_hotkey_drafts_to_settings(&mut reloaded);
    assert_eq!(
        reloaded
            .hotkey_config
            .find_by_name("TakeScreenshot")
            .unwrap()
            .binding,
        "Ctrl+Shift+F10"
    );
}

#[test]
fn startup_hydration_populates_seven_drafts_before_dialog_open() {
    // Mirrors the main.rs startup path: `AppState::default_startup`
    // produces an empty drafts list, and the hydration call we
    // added in main.rs (`state.hydrate_hotkey_drafts_from_settings(&config_settings)`)
    // is the single point that populates the in-memory list before
    // the first `handle_open_hotkey_config` call.
    use d4lc_config::AppSettings;
    let mut state = AppState::default_startup();
    assert!(
        state.hotkey_drafts.is_empty(),
        "precondition: default_startup must leave drafts empty so hydration is the single source"
    );
    // Load settings (this is what `d4lc_config::load_app_settings(&config_root)`
    // returns in main.rs).
    let settings = AppSettings::default();
    // Mirror the call site in main.rs.
    state.hydrate_hotkey_drafts_from_settings(&settings);
    // HotkeyConfig dialog now opens against the hydrated list,
    // not the empty default.
    assert_eq!(state.hotkey_drafts.len(), 7);
    assert_eq!(state.hotkey_drafts[0].name, "SwitchPreset");
}
