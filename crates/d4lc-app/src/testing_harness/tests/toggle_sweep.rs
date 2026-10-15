use super::*;

// ── toggle-sweep: systematic toggle sweep ─────────────────────────────

/// Evidence directory for the toggle sweep.
const TOGGLE_SWEEP_EVIDENCE_DIR: &str = "pilots/toggle-sweep";

/// One row of the source-derived toggle enumeration: every
/// `ToggleSwitch` instance and every inlined `accessible-role:
/// switch` element in the UI, with its label, defining source,
/// reachability surface, covering scenario, and the
/// persistence surface the flip writes.
struct ToggleRow {
    label: &'static str,
    source: &'static str,
    surface: &'static str,
    scenario: &'static str,
    persistence: &'static str,
}

/// The enumeration table. The in-test source re-scan asserts this
/// table matches the .slint sources exactly: an unlisted toggle (or
/// a stale row) fails the sweep.
///
/// The scan reads the literal `accessible-label` that follows each
/// switch-role element, so a row whose label is computed (the
/// `ui/dialogs/hotkeys_config_dialog.slint` per-binding enable
/// switches carry static labels and ARE listed below; any future
/// data-driven label would fall outside the scan) cannot be
/// enumerated. Every listed row is reachable and exercised by the
/// named scenario.
const TOGGLE_SWEEP_TABLE: [ToggleRow; 48] = [
    ToggleRow {
        label: "Debug Mode",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row, hidden while the stored dev_mode is false (default; the \
         app seeds the screen's dev-mode property from AppSettings)",
        scenario: "documented-unreachable",
        persistence: "settings-changed → d4lc-config debug_mode (proven via the sibling rows)",
    },
    ToggleRow {
        label: "Check for Updates",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row",
        scenario: "settings-toggles",
        persistence: "d4lc-config check_for_updates",
    },
    ToggleRow {
        label: "Launch Minimized",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row",
        scenario: "settings-toggles",
        persistence: "d4lc-config launch_minimized",
    },
    ToggleRow {
        label: "Minimize to Tray",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row",
        scenario: "settings-toggles",
        persistence: "d4lc-config minimize_to_tray",
    },
    ToggleRow {
        label: "Diagnostics Export",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row",
        scenario: "settings-toggles",
        persistence: "d4lc-config is_diagnostics_export_enabled",
    },
    ToggleRow {
        label: "Check for New Releases",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row",
        scenario: "settings-toggles",
        persistence: "d4lc-config update_check_enabled",
    },
    ToggleRow {
        label: "Controller Mode",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row",
        scenario: "settings-toggles",
        persistence: "d4lc-config controller_mode",
    },
    ToggleRow {
        label: "Paragon Mode",
        source: "ui/screens/settings_screen.slint",
        surface: "Settings row",
        scenario: "settings-toggles",
        persistence: "d4lc-config paragon_mode_active",
    },
    ToggleRow {
        label: "On",
        source: "ui/screens/debug_screen.slint",
        surface: "Debug row (self-relabels On/Off with state)",
        scenario: "debug-toggles",
        persistence: "session-only screen property + dynamic label (config write unwired)",
    },
    ToggleRow {
        label: "Top Most",
        source: "ui/screens/debug_screen.slint",
        surface: "Debug row",
        scenario: "debug-toggles",
        persistence: "window-topmost side effect, persisted as overlay_top_most (debug/commands.rs handle_topmost_toggled)",
    },
    ToggleRow {
        label: "Enable trade overlay",
        source: "ui/dialogs/trade_config_dialog.slint",
        surface: "TradeConfig dialog draft, Apply persists",
        scenario: "trade-config",
        persistence: "d4lc-config rule_settings.trade_overlay_enabled",
    },
    ToggleRow {
        label: "Show current item",
        source: "ui/dialogs/trade_config_dialog.slint",
        surface: "TradeConfig dialog draft, Apply persists",
        scenario: "trade-config",
        persistence: "d4lc-config show_current_item",
    },
    ToggleRow {
        label: "Greater affix",
        source: "ui/dialogs/set_affix_dialog.slint",
        surface: "SetAffix dialog draft, Apply persists",
        scenario: "set-affix",
        persistence: "session preset-entry flag (no config write exists for entries)",
    },
    ToggleRow {
        label: "Implicit affix",
        source: "ui/dialogs/set_affix_dialog.slint",
        surface: "SetAffix dialog draft, Apply persists",
        scenario: "set-affix",
        persistence: "session preset-entry flag (no config write exists for entries)",
    },
    ToggleRow {
        label: "Tempered affix",
        source: "ui/dialogs/set_affix_dialog.slint",
        surface: "SetAffix dialog draft, Apply persists",
        scenario: "set-affix",
        persistence: "session preset-entry flag (no config write exists for entries)",
    },
    ToggleRow {
        label: "Enable overlay",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog draft, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config overlay_enabled",
    },
    ToggleRow {
        label: "Always on top",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog draft, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config is_top_most",
    },
    ToggleRow {
        label: "Lock overlay menu",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog draft, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config overlay_menu_locked",
    },
    ToggleRow {
        label: "Enable controller mode",
        source: "ui/dialogs/controller_config_dialog.slint",
        surface: "ControllerConfig dialog draft, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config controller_mode",
    },
    ToggleRow {
        label: "Enable paragon mode",
        source: "ui/dialogs/paragon_config_dialog.slint",
        surface: "ParagonConfig dialog draft, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config paragon_mode_active",
    },
    ToggleRow {
        label: "Show Mobalytics browser",
        source: "ui/dialogs/paragon_config_dialog.slint",
        surface: "ParagonConfig dialog draft, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config is_show_browser_mobalytics_enabled",
    },
    ToggleRow {
        label: "Enabled",
        source: "ui/dialogs/hotkey_config_dialog.slint",
        surface: "HotkeyConfig dialog draft, Apply persists",
        scenario: "hotkey-toggle",
        persistence: "d4lc-config hotkey_config.bindings[SwitchPreset].enabled",
    },
    // Additions.
    ToggleRow {
        label: "Use collapsed board",
        source: "ui/dialogs/paragon_config_dialog.slint",
        surface: "ParagonConfig dialog draft, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config rule_settings.is_collapsed_paragonboard_enabled",
    },
    ToggleRow {
        label: "Show overlay icon",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog (General), Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config show_overlay_icon",
    },
    ToggleRow {
        label: "Item power limit",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog (Features), Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config rule_settings.item_power.enabled",
    },
    ToggleRow {
        label: "Overlay trade overlay",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog (Features) cross-entry, Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config rule_settings.trade_overlay_enabled",
    },
    ToggleRow {
        label: "Socket detection",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog (Performance), Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config is_socket_detection_enabled",
    },
    ToggleRow {
        label: "Tempered affix detection",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog (Performance), Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config is_tempered_affix_detection_enabled",
    },
    ToggleRow {
        label: "Aspect detection",
        source: "ui/dialogs/overlay_config_dialog.slint",
        surface: "OverlayConfig dialog (Performance), Apply persists",
        scenario: "settings-dialogs",
        persistence: "d4lc-config rule_settings.aspect_detection_enabled",
    },
    ToggleRow {
        label: "Switch preset enabled",
        source: "ui/dialogs/hotkeys_config_dialog.slint",
        surface: "Hotkeys manager row draft, Apply persists + re-registers",
        scenario: "settings-dialogs",
        persistence: "d4lc-config hotkey_config.bindings[SwitchPreset].enabled",
    },
    ToggleRow {
        label: "Switch overlay enabled",
        source: "ui/dialogs/hotkeys_config_dialog.slint",
        surface: "Hotkeys manager row draft, Apply persists + re-registers",
        scenario: "settings-dialogs",
        persistence: "d4lc-config hotkey_config.bindings[SwitchOverlay].enabled",
    },
    ToggleRow {
        label: "Take screenshot enabled",
        source: "ui/dialogs/hotkeys_config_dialog.slint",
        surface: "Hotkeys manager row draft, Apply persists + re-registers",
        scenario: "settings-dialogs",
        persistence: "d4lc-config hotkey_config.bindings[TakeScreenshot].enabled",
    },
    ToggleRow {
        label: "Toggle controller mode enabled",
        source: "ui/dialogs/hotkeys_config_dialog.slint",
        surface: "Hotkeys manager row draft, Apply persists + re-registers",
        scenario: "settings-dialogs",
        persistence: "d4lc-config hotkey_config.bindings[ToggleController].enabled",
    },
    ToggleRow {
        label: "Toggle overlay enabled",
        source: "ui/dialogs/hotkeys_config_dialog.slint",
        surface: "Hotkeys manager row draft, Apply persists + re-registers",
        scenario: "settings-dialogs",
        persistence: "d4lc-config hotkey_config.bindings[ToggleOverlay].enabled",
    },
    ToggleRow {
        label: "Debug lock capture enabled",
        source: "ui/dialogs/hotkeys_config_dialog.slint",
        surface: "Hotkeys manager row draft, Apply persists + re-registers",
        scenario: "settings-dialogs",
        persistence: "d4lc-config hotkey_config.bindings[ToggleDebugLockScreencapture].enabled",
    },
    ToggleRow {
        label: "Tuning prism hints enabled",
        source: "ui/dialogs/hotkeys_config_dialog.slint",
        surface: "Hotkeys manager row draft, Apply persists + re-registers",
        scenario: "settings-dialogs",
        persistence: "d4lc-config hotkey_config.bindings[ToggleTuningPrismHints].enabled",
    },
    // The affix-family config dialogs' rule-setting
    // switches. Upstream saves from every toggle setter
    // (AffixConfigViewModel.cs:55-101, SigilConfigViewModel.cs:60-98),
    // so each of these writes through on the click itself; the
    // affix-family-config-toggles / sigil-config-toggles scenarios
    // assert click → persisted file → reload → re-seed → restore.
    ToggleRow {
        label: "Tempered affix detection",
        source: "ui/dialogs/affix_config_dialog.slint",
        surface: "AffixConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config is_tempered_affix_detection_enabled",
    },
    ToggleRow {
        label: "Tuning prism hints",
        source: "ui/dialogs/affix_config_dialog.slint",
        surface: "AffixConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config is_tuning_prism_hints_enabled",
    },
    ToggleRow {
        label: "Minimal affix value filter",
        source: "ui/dialogs/affix_config_dialog.slint",
        surface: "AffixConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.minimal_affix_value_enabled",
    },
    ToggleRow {
        label: "Multi-build mode",
        source: "ui/dialogs/affix_config_dialog.slint",
        surface: "AffixConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.multi_build_enabled",
    },
    ToggleRow {
        label: "Aspect detection",
        source: "ui/dialogs/aspect_config_dialog.slint",
        surface: "AspectConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.aspect_detection_enabled",
    },
    ToggleRow {
        label: "Unique detection",
        source: "ui/dialogs/unique_config_dialog.slint",
        surface: "UniqueConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.unique_detection_enabled",
    },
    ToggleRow {
        label: "Rune detection",
        source: "ui/dialogs/rune_config_dialog.slint",
        surface: "RuneConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.rune_detection_enabled",
    },
    ToggleRow {
        label: "Multi-build mode",
        source: "ui/dialogs/aspect_config_dialog.slint",
        surface: "AspectConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.multi_build_enabled",
    },
    ToggleRow {
        label: "Multi-build mode",
        source: "ui/dialogs/unique_config_dialog.slint",
        surface: "UniqueConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.multi_build_enabled",
    },
    ToggleRow {
        label: "Multi-build mode",
        source: "ui/dialogs/rune_config_dialog.slint",
        surface: "RuneConfig dialog switch, write-through on click",
        scenario: "affix-family-config-toggles",
        persistence: "d4lc-config rule_settings.multi_build_enabled",
    },
    ToggleRow {
        label: "Dungeon tiers",
        source: "ui/dialogs/sigil_config_dialog.slint",
        surface: "SigilConfig dialog switch, write-through on click",
        scenario: "sigil-config-toggles",
        persistence: "d4lc-config rule_settings.dungeon_tiers_enabled",
    },
    ToggleRow {
        label: "Multi-build mode",
        source: "ui/dialogs/sigil_config_dialog.slint",
        surface: "SigilConfig dialog switch, write-through on click",
        scenario: "sigil-config-toggles",
        persistence: "d4lc-config rule_settings.multi_build_enabled",
    },
];

fn write_toggle_sweep_screenshot(name: &str, png: &[u8]) {
    let dir = workspace_root()
        .join("debug-output")
        .join(TOGGLE_SWEEP_EVIDENCE_DIR);
    std::fs::create_dir_all(&dir).expect("create the evidence directory");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write toggle-sweep screenshot");
    eprintln!("[toggle-sweep] screenshot written: {}", path.display());
}

fn toggle_sweep_screenshot(client: &McpClient, window: McpHandle, name: &str) {
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic("toggle sweep screenshot", &error));
    assert_png_and_dimensions(&png);
    write_toggle_sweep_screenshot(name, &png);
}

/// Strip `//` line comments so example toggles inside comments never
/// count as source occurrences.
fn strip_slint_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text following every occurrence of `needle`, bounded to
/// `span` characters (enough to reach the label that belongs to the
/// occurrence, not the next element).
fn windows_after<'a>(source: &'a str, needle: &str, span: usize) -> Vec<&'a str> {
    let mut windows = Vec::new();
    let mut rest = source;
    while let Some(position) = rest.find(needle) {
        let start = position + needle.len();
        let end = (start + span).min(rest.len());
        windows.push(&rest[start..end]);
        rest = &rest[start..];
    }
    windows
}

/// The first double-quoted string in `window`, if any.
fn first_quoted(window: &str) -> Option<String> {
    let start = window.find('"')? + 1;
    let rest = &window[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The live re-scan (acceptance 1): enumerate every `ToggleSwitch`
/// instance and every inlined switch-role element straight from the
/// .slint sources, by their accessible labels. An unlisted toggle
/// makes the scan disagree with `TOGGLE_SWEEP_TABLE` and fails.
fn scan_toggle_sources() -> Vec<(String, String)> {
    let mut scanned: Vec<(String, String)> = Vec::new();
    for (path, contents) in UI_SOURCES {
        let source = strip_slint_line_comments(contents);
        for window in windows_after(&source, "ToggleSwitch {", 200) {
            if let Some(label) = first_quoted(window) {
                scanned.push(((*path).to_string(), label));
            }
        }
        for window in windows_after(&source, "accessible-role: switch", 200) {
            if let Some(label) = first_quoted(window) {
                scanned.push(((*path).to_string(), label));
            }
        }
    }
    scanned
}

/// The table must equal the scan exactly, both directions.
fn assert_source_scan_matches_table() {
    let mut scanned = scan_toggle_sources();
    let mut expected: Vec<(String, String)> = TOGGLE_SWEEP_TABLE
        .iter()
        .map(|row| (row.source.to_string(), row.label.to_string()))
        .collect();
    scanned.sort();
    expected.sort();
    assert_eq!(
        scanned, expected,
        "toggle-sweep: the toggle sweep table must match the .slint sources exactly — \
         a scanned-but-unlisted toggle or a stale table row fails here; \
         scanned = {scanned:?}"
    );
}

/// Print the coverage table (toggle → scenario → persistence).
fn print_coverage_table() {
    eprintln!("[toggle-sweep] toggle coverage table (toggle @ source -> scenario -> persistence):");
    for row in &TOGGLE_SWEEP_TABLE {
        eprintln!(
            "[toggle-sweep]   {:?} @ {} -> {} [{}] -> {}",
            row.label, row.source, row.scenario, row.surface, row.persistence
        );
    }
    eprintln!(
        "[toggle-sweep]   \"Start\"/\"Stop\" (button-role runtime pair) @ \
         ui/screens/debug_screen.slint -> runtime-start-stop -> session runtime status \
         (stub-loop flip asserted headless; real-runtime build stays display-gated)"
    );
}

/// Read one bool out of the persisted settings file (the app's own
/// `d4lc_config::load_app_settings` surface).
fn config_bool(read: fn(&d4lc_config::AppSettings) -> bool) -> bool {
    let config_root = crate::actions::settings::resolve_config_root();
    let config = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| harness_panic("config readback", &error));
    read(&config)
}

/// Reload the wired app state from the persisted config — the same
/// projection a fresh `main()` start performs (config → AppState →
/// root properties, plus the hotkey-draft hydration from
/// `wired_test_app`). The single-process harness cannot spawn a
/// second window, so this in-process reload is the harness pattern
/// for "restart the app": it exercises the real
/// `d4lc_config::load_app_settings` path and the real startup
/// property projection.
fn reload_from_config() {
    let config_root = crate::actions::settings::resolve_config_root();
    let config = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| harness_panic("reload app settings", &error));
    on_ui(move |app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state.shell.settings = d4lc_gui::SettingsPanelState::from_config(&config, Vec::new());
        state.hydrate_hotkey_drafts_from_settings(&config);
        // The startup projection (main.rs) over the reloaded state.
        app.set_debug_mode(state.shell.settings.debug_mode);
        app.set_dev_mode(state.shell.settings.dev_mode);
        app.set_check_updates(state.shell.settings.check_for_updates);
        app.set_launch_minimized(state.shell.settings.launch_minimized);
        app.set_minimize_to_tray(state.shell.settings.minimize_to_tray);
        app.set_controller_mode(state.shell.settings.controller_mode);
        app.set_paragon_mode(state.shell.settings.paragon_mode_active);
        app.set_diagnostics_export(state.shell.settings.diagnostics_export_enabled);
        app.set_update_check_enabled(state.shell.settings.update_check_enabled);
    });
}

/// Poll a UI read until it holds, with a generous real-time bound;
/// the mock-clock pump keeps advancing the drain timer while the
/// test thread sleeps.
fn wait_for(mut check: impl FnMut() -> bool, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "toggle-sweep: timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Count elements that carry `label` AND the switch accessible
/// role. Plain label counts double-count the propagated Text child
/// (Slint propagates a label to its text children), so the
/// DebugInfo assertions count switches only.
fn switch_count(label: &str) -> usize {
    let label = label.to_string();
    read_ui(move |app| {
        ElementHandle::find_by_accessible_label(app, &label)
            .filter(|element| {
                matches!(
                    element.accessible_role(),
                    Some(i_slint_backend_testing::AccessibleRole::Switch)
                )
            })
            .count()
    })
}

/// The seeded AttackSpeed entry's (greater, implicit, tempered)
/// flags — the session persistence surface of the SetAffix switches.
fn attack_speed_flags() -> (bool, bool, bool) {
    read_ui(|_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        let entry = state
            .affix_preset_editor
            .presets
            .iter()
            .find(|preset| preset.name == "Alpha")
            .and_then(|preset| {
                preset
                    .entries
                    .iter()
                    .find(|entry| entry.id == "AttackSpeed")
            })
            .unwrap_or_else(|| panic!("toggle-sweep: seeded AttackSpeed entry missing"));
        (entry.is_greater, entry.is_implicit, entry.is_tempered)
    })
}

/// Read one of the three SetAffix draft flags off the root form.
fn set_affix_form_flag(app: &AppWindow, index: usize) -> bool {
    match index {
        0 => app.get_set_affix_is_greater(),
        1 => app.get_set_affix_is_implicit(),
        _ => app.get_set_affix_is_tempered(),
    }
}

/// The persisted enabled flag of the SwitchPreset hotkey binding.
fn switch_preset_enabled(config: &d4lc_config::AppSettings) -> bool {
    config
        .hotkey_config
        .bindings
        .iter()
        .find(|binding| binding.name == "SwitchPreset")
        .map(|binding| binding.enabled)
        .unwrap_or(false)
}

/// Entry point of the data-driven sweep over every toggle in the UI.
pub(super) fn toggle_sweep(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[toggle-sweep] starting systematic toggle sweep");
    assert_source_scan_matches_table();
    print_coverage_table();
    settings_toggles_scenario(client, window);
    debug_toggles_scenario(client, window);
    runtime_start_stop_scenario(client, window);
    settings_dialog_toggles_scenario(client, window);
    trade_config_toggles_scenario(client, window);
    set_affix_toggles_scenario(client, window);
    affix_family_config_toggles_scenario(client, window, root);
    sigil_config_toggles_scenario(client, window);
    hotkey_toggle_scenario(client, window);
    eprintln!("[toggle-sweep] toggle sweep complete");
}

/// (a) Settings-screen toggles: every reachable row flips via a
/// real click, persists synchronously through `settings-changed` →
/// `save_app_settings`, survives the startup-projection reload, and
/// restores. Rapid double-toggle: both real clicks land, so the
/// deterministic final state equals the start (no race, no lost
/// click, no debounce).
fn settings_toggles_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (a) settings-screen toggles");
    click_on_ui("Settings");
    assert_eq!(read_ui(|app| app.get_active_tab()), 4);

    type RowRead = fn(&AppWindow) -> bool;
    type FieldRead = fn(&d4lc_config::AppSettings) -> bool;
    let rows: [(&str, RowRead, FieldRead); 7] = [
        (
            "Check for Updates",
            |app| app.get_check_updates(),
            |config| config.check_for_updates,
        ),
        (
            "Launch Minimized",
            |app| app.get_launch_minimized(),
            |config| config.launch_minimized,
        ),
        (
            "Minimize to Tray",
            |app| app.get_minimize_to_tray(),
            |config| config.minimize_to_tray,
        ),
        (
            "Diagnostics Export",
            |app| app.get_diagnostics_export(),
            |config| config.is_diagnostics_export_enabled,
        ),
        (
            "Check for New Releases",
            |app| app.get_update_check_enabled(),
            |config| config.update_check_enabled,
        ),
        (
            "Controller Mode",
            |app| app.get_controller_mode(),
            |config| config.controller_mode,
        ),
        (
            "Paragon Mode",
            |app| app.get_paragon_mode(),
            |config| config.paragon_mode_active,
        ),
    ];
    for (label, read, field) in rows {
        let before = read_ui(read);
        let config_before = config_bool(field);

        click_control_by_label(label);
        assert_ne!(
            read_ui(read),
            before,
            "toggle-sweep: the {label} toggle must flip via the real click"
        );
        assert_eq!(
            config_bool(field),
            !before,
            "toggle-sweep: the {label} flip must persist through settings-changed"
        );

        reload_from_config();
        assert_eq!(
            read_ui(read),
            !before,
            "toggle-sweep: the {label} flip must survive the startup-projection reload"
        );

        click_control_by_label(label);
        assert_eq!(
            read_ui(read),
            before,
            "toggle-sweep: the {label} toggle must restore"
        );
        assert_eq!(
            config_bool(field),
            config_before,
            "toggle-sweep: the {label} restore must persist"
        );
    }
    toggle_sweep_screenshot(client, window, "settings-toggles.png");

    // Rapid double-toggle on one representative row.
    let before = read_ui(|app| app.get_check_updates());
    click_control_by_label("Check for Updates");
    click_control_by_label("Check for Updates");
    assert_eq!(
        read_ui(|app| app.get_check_updates()),
        before,
        "toggle-sweep: two rapid clicks must toggle exactly twice (back to the start)"
    );
    assert_eq!(
        config_bool(|config| config.check_for_updates),
        before,
        "toggle-sweep: both rapid flips must have been persisted (net zero)"
    );

    // Documented unreachable row: the dev-mode-gated Debug Mode
    // switch (kept in the enumeration table; the sweep cannot reach
    // it by a real click).
    eprintln!(
        "[toggle-sweep] expected-mismatch: Debug Mode settings toggle — the app seeds the \
         screen's dev-mode property from the stored AppSettings.dev_mode (upstream \
         SettingsView.xaml:88,97), and with the default dev_mode=false the switch stays \
         hidden, so it cannot be reached by a real click; its settings-changed persistence \
         contract is proven by the seven sibling rows"
    );

    click_on_ui("Debug");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        3,
        "cleanup: leave Settings"
    );
}

/// (b) Debug-screen toggles: DebugInfo (self-relabeling "Off"/"On")
/// and Top Most. Persistence: DebugInfo writes no d4lc-config today (the
/// DebugViewModel returns PersistConfig and the dispatch logs it as
/// unwired); Top Most applies the `window-topmost` root property and
/// persists `overlay_top_most`. Both flip via real clicks and restore.
fn debug_toggles_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (b) debug-screen toggles");
    assert_eq!(read_ui(|app| app.get_active_tab()), 3);
    assert!(
        !read_ui(|app| app.get_window_topmost()),
        "toggle-sweep: precondition — the window must not be topmost"
    );

    // DebugInfo: the label IS the state ("Off" ↔ "On").
    assert_eq!(
        switch_count("Off"),
        1,
        "toggle-sweep: the disabled DebugInfo switch must be labeled \"Off\""
    );
    assert_eq!(
        switch_count("On"),
        0,
        "toggle-sweep: no \"On\" switch may exist while DebugInfo is off"
    );
    click_control_by_label("Off");
    assert_eq!(
        switch_count("On"),
        1,
        "toggle-sweep: the flip must relabel the switch to \"On\""
    );
    assert_eq!(
        switch_count("Off"),
        0,
        "toggle-sweep: the \"Off\" switch must be gone after the flip"
    );

    // Top Most: the real side effect lands on the window.
    click_control_by_label("Top Most");
    assert!(
        read_ui(|app| app.get_window_topmost()),
        "toggle-sweep: the Top Most flip must apply the SetWindowTopmost effect"
    );
    toggle_sweep_screenshot(client, window, "debug-toggles-on.png");

    // Restore both.
    click_control_by_label("On");
    assert_eq!(
        switch_count("Off"),
        1,
        "toggle-sweep: DebugInfo must relabel back to \"Off\""
    );
    click_control_by_label("Top Most");
    assert!(
        !read_ui(|app| app.get_window_topmost()),
        "toggle-sweep: Top Most must restore the window state"
    );

    eprintln!(
        "[toggle-sweep] expected-mismatch: DebugInfo persistence — no d4lc-config write \
         exists for that bit (DebugEffect::PersistConfig is observed but deliberately \
         unwired), so the asserted surface is the session state (label swap); Top Most \
         persists overlay_top_most through handle_topmost_toggled"
    );
}

/// (c) Runtime Start/Stop (the pair the coverage matrix routes as an
/// expected mismatch). On the default (non-real-runtime)
/// test build the live loop is the stub thread — state reads only,
/// no capture, no display, no network — and the harness CAN observe
/// the flip: Start emits `RuntimeEvent::Started`, the mock-time
/// 50 ms drain timer applies it, and `runtime-running` flips; Stop
/// mirrors it. The real-runtime build stays display-gated and keeps
/// a documented row.
fn runtime_start_stop_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (c) runtime start/stop");
    assert_eq!(read_ui(|app| app.get_active_tab()), 3);
    assert!(
        !read_ui(|app| app.get_runtime_running()),
        "toggle-sweep: precondition — the runtime must be stopped"
    );

    click_on_ui("Start");
    wait_for(
        || read_ui(|app| app.get_runtime_running()),
        "the Start click to flip runtime-running (drain timer)",
    );
    assert_eq!(
        read_ui(|app| app.get_runtime_status_text().to_string()),
        "Running",
        "toggle-sweep: the Started event must project the status text"
    );

    // Double-start guard: the second Start click must be ignored.
    click_on_ui("Start");
    assert!(
        read_ui(|app| app.get_runtime_running()),
        "toggle-sweep: a duplicate Start must neither stop nor corrupt the loop"
    );
    toggle_sweep_screenshot(client, window, "runtime-running.png");

    click_on_ui("Stop");
    wait_for(
        || !read_ui(|app| app.get_runtime_running()),
        "the Stop click to flip runtime-running back (drain timer)",
    );
    assert!(
        read_ui(|app| app.get_runtime_status_text().to_string()).starts_with("Stopped"),
        "toggle-sweep: the Stopped event must project the status text"
    );

    eprintln!(
        "[toggle-sweep] expected-mismatch (narrowed): the real-runtime build still cannot be \
         started headless (live capture requires a display); the state flip is asserted \
         on the default stub-loop build, replacing the blanket matrix exclusion"
    );
}

/// One dialog-switch cycle: the Cancel-discard proof (draft flips,
/// Cancel discards, config untouched, reopen re-seeds the old
/// value), then flip → Apply → config persisted → startup-projection
/// reload → the persisted value re-seeds the reopened dialog →
/// flip back → Apply → restored.
fn dialog_switch_cycle(
    opener: impl Fn(),
    dialog_id: &'static str,
    switch: &'static str,
    read: fn(&AppWindow) -> bool,
    field: fn(&d4lc_config::AppSettings) -> bool,
    reload_before_readback: bool,
) {
    dialog_switch_cycle_section(
        opener,
        dialog_id,
        None,
        switch,
        read,
        field,
        reload_before_readback,
    );
}

/// Section-aware variant of [`dialog_switch_cycle`] for
/// controls that live behind a section chip (the overlay dialog
/// mirrors the upstream TabControl with General / Features /
/// Performance sections): the chip is clicked right after every open
/// so the addressed switch is rendered and hit-testable.
fn dialog_switch_cycle_section(
    opener: impl Fn(),
    dialog_id: &'static str,
    section: Option<&'static str>,
    switch: &'static str,
    read: fn(&AppWindow) -> bool,
    field: fn(&d4lc_config::AppSettings) -> bool,
    reload_before_readback: bool,
) {
    let config_before = config_bool(field);

    // Cancel edge: no half-applied state.
    opener();
    if let Some(section) = section {
        click_control_by_label(section);
    }
    let (visible, seen) = dialog_state();
    assert!(
        visible && seen == dialog_id,
        "toggle-sweep: the {dialog_id:?} opener must open the dialog"
    );
    // The open handler seeds the draft from the app state; the
    // seeded value (not the stale closed-dialog form value) is the
    // cycle's baseline.
    let before = read_ui(read);
    click_control_by_label(switch);
    assert_ne!(
        read_ui(read),
        before,
        "toggle-sweep: the {switch} draft must flip in {dialog_id:?}"
    );
    click_on_ui("Cancel");
    assert_dialog_closed(
        dialog_id,
        &format!("toggle-sweep: Cancel must close {dialog_id:?}"),
    );
    assert_eq!(
        config_bool(field),
        config_before,
        "toggle-sweep: Cancel after toggling {switch} must not persist"
    );
    opener();
    assert!(
        dialog_state().0 && dialog_state().1 == dialog_id,
        "toggle-sweep: the {dialog_id:?} opener must reopen the dialog for the discard readback"
    );
    if let Some(section) = section {
        click_control_by_label(section);
    }
    assert_eq!(
        read_ui(read),
        before,
        "toggle-sweep: the cancelled {switch} draft must not survive the reopen"
    );

    // Apply path: persist → reload → persisted → restore.
    click_control_by_label(switch);
    click_on_ui("Apply");
    assert_dialog_closed(
        dialog_id,
        &format!("toggle-sweep: Apply must close {dialog_id:?}"),
    );
    assert_eq!(
        config_bool(field),
        !before,
        "toggle-sweep: Apply must persist the {switch} flip"
    );
    if reload_before_readback {
        reload_from_config();
    }
    opener();
    if let Some(section) = section {
        click_control_by_label(section);
    }
    assert_eq!(
        read_ui(read),
        !before,
        "toggle-sweep: the persisted {switch} flip must survive the reload + re-seed"
    );
    click_control_by_label(switch);
    click_on_ui("Apply");
    assert_dialog_closed(
        dialog_id,
        &format!("toggle-sweep: Apply must close {dialog_id:?} on restore"),
    );
    assert_eq!(
        config_bool(field),
        config_before,
        "toggle-sweep: the {switch} restore must persist"
    );
}

/// (d) Settings-family config-dialog switches (controller, overlay
/// x3, paragon x2): drafts flip by real click, Apply persists
/// through d4lc-config, the reload surfaces the persisted value,
/// Cancel discards.
fn settings_dialog_toggles_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (d) settings-family dialog switches");
    click_on_ui("Settings");
    assert_eq!(read_ui(|app| app.get_active_tab()), 4);

    dialog_switch_cycle(
        || click_on_ui("Open controller config"),
        "controller_config",
        "Enable controller mode",
        |app| app.get_settings_controller_mode(),
        |config| config.controller_mode,
        true,
    );

    click_on_ui("Open overlay config");
    assert!(dialog_state().0 && dialog_state().1 == "overlay_config");
    toggle_sweep_screenshot(client, window, "dialog-switches-overlay.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "cleanup: overlay config closed");

    dialog_switch_cycle(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        "Enable overlay",
        |app| app.get_settings_overlay_enabled(),
        |config| config.overlay_enabled,
        true,
    );
    dialog_switch_cycle(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        "Always on top",
        |app| app.get_settings_overlay_top_most(),
        |config| config.is_top_most,
        true,
    );
    dialog_switch_cycle(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        "Lock overlay menu",
        |app| app.get_settings_overlay_menu_locked(),
        |config| config.overlay_menu_locked,
        true,
    );
    dialog_switch_cycle(
        || click_on_ui("Open paragon config"),
        "paragon_config",
        "Enable paragon mode",
        |app| app.get_settings_paragon_mode_active(),
        |config| config.paragon_mode_active,
        true,
    );
    dialog_switch_cycle(
        || click_on_ui("Open paragon config"),
        "paragon_config",
        "Show Mobalytics browser",
        |app| app.get_settings_paragon_show_browser_mobalytics_enabled(),
        |config| config.is_show_browser_mobalytics_enabled,
        true,
    );

    // Rows: the collapsed-board switch, the upstream
    // overlay rows (which live behind the Features / Performance
    // sections) and one hotkeys-manager row. Each flips by a real
    // click, persists on Apply, and restores.
    dialog_switch_cycle(
        || click_on_ui("Open paragon config"),
        "paragon_config",
        "Use collapsed board",
        |app| app.get_settings_paragon_collapsed_board(),
        |config| config.rule_settings.is_collapsed_paragonboard_enabled,
        true,
    );
    dialog_switch_cycle(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        "Show overlay icon",
        |app| app.get_settings_overlay_show_icon(),
        |config| config.show_overlay_icon,
        true,
    );
    dialog_switch_cycle_section(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        Some("Overlay features section"),
        "Item power limit",
        |app| app.get_settings_overlay_item_power_enabled(),
        |config| config.rule_settings.item_power.enabled,
        true,
    );
    dialog_switch_cycle_section(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        Some("Overlay features section"),
        "Overlay trade overlay",
        |app| app.get_settings_overlay_trade_overlay(),
        |config| config.rule_settings.trade_overlay_enabled,
        true,
    );
    dialog_switch_cycle_section(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        Some("Overlay performance section"),
        "Socket detection",
        |app| app.get_settings_overlay_socket_detection(),
        |config| config.is_socket_detection_enabled,
        true,
    );
    dialog_switch_cycle_section(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        Some("Overlay performance section"),
        "Tempered affix detection",
        |app| app.get_settings_overlay_tempered_detection(),
        |config| config.is_tempered_affix_detection_enabled,
        true,
    );
    dialog_switch_cycle_section(
        || click_on_ui("Open overlay config"),
        "overlay_config",
        Some("Overlay performance section"),
        "Aspect detection",
        |app| app.get_settings_overlay_aspect_detection(),
        |config| config.rule_settings.aspect_detection_enabled,
        true,
    );
    hotkey_manager_row_cycle(
        "Switch overlay enabled",
        "SwitchOverlay",
        "Ctrl+F6",
        |config| {
            config
                .hotkey_config
                .find_by_name("SwitchOverlay")
                .map(|binding| binding.enabled)
                .unwrap_or(false)
        },
    );

    // Rapid double-toggle on one representative draft switch.
    click_on_ui("Open overlay config");
    assert!(dialog_state().0);
    let before = read_ui(|app| app.get_settings_overlay_top_most());
    click_control_by_label("Always on top");
    click_control_by_label("Always on top");
    assert_eq!(
        read_ui(|app| app.get_settings_overlay_top_most()),
        before,
        "toggle-sweep: two rapid clicks must toggle the draft exactly twice"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0);
}

/// One hotkeys-manager row cycle: open the manager, flip a row's
/// enable switch by real click, Apply (which persists the draft and
/// re-registers the grabs), assert the persisted binding, then flip
/// back and restore.
fn hotkey_manager_row_cycle(
    switch: &'static str,
    binding_name: &'static str,
    binding_text: &'static str,
    config_read: fn(&d4lc_config::AppSettings) -> bool,
) {
    click_on_ui("Settings");
    assert_eq!(read_ui(|app| app.get_active_tab()), 4);
    let before = config_bool(config_read);

    click_on_ui("Open hotkeys config");
    assert!(
        dialog_state().0 && dialog_state().1 == "hotkeys_config",
        "toggle-sweep: the Hotkeys row must open the manager dialog"
    );
    assert_eq!(
        read_ui(|app| app.get_settings_hotkey_rows().row_count()),
        7,
        "toggle-sweep: the manager must list the seven upstream bindings"
    );
    click_control_by_label(switch);
    let draft_enabled = read_ui(move |app| {
        app.get_settings_hotkey_rows()
            .iter()
            .find(|row| row.name.as_str() == binding_name)
            .map(|row| row.enabled)
    });
    assert_eq!(
        draft_enabled,
        Some(!before),
        "toggle-sweep: the {switch} row switch must flip the draft"
    );
    click_on_ui("Apply");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Apply must close the dialog"
    );
    assert_no_live_hotkey_backend("after the hotkeys-manager Apply (re-registration)");
    assert_eq!(
        config_bool(config_read),
        !before,
        "toggle-sweep: the {switch} row draft must persist through Apply"
    );
    let persisted_text = read_ui(move |app| {
        app.get_settings_hotkey_rows()
            .iter()
            .find(|row| row.name.as_str() == binding_name)
            .map(|row| row.binding.to_string())
    });
    assert_eq!(
        persisted_text.as_deref(),
        Some(binding_text),
        "toggle-sweep: the row must keep the persisted binding string"
    );

    // Restore.
    click_on_ui("Open hotkeys config");
    assert!(dialog_state().0 && dialog_state().1 == "hotkeys_config");
    click_control_by_label(switch);
    click_on_ui("Apply");
    assert!(!dialog_state().0);
    assert_eq!(
        config_bool(config_read),
        before,
        "toggle-sweep: the {switch} row must restore its persisted value"
    );
}

/// (e) TradeConfig switches: both drafts flip by real click, Apply
/// persists through d4lc-config (overlay into rule_settings,
/// show-current-item into its own field — the dialog open handler
/// re-reads that field straight from disk), Cancel discards.
fn trade_config_toggles_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (e) trade-config switches");
    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);

    let overlay_before = config_bool(|config| config.rule_settings.trade_overlay_enabled);
    let item_before = config_bool(|config| config.show_current_item);

    dialog_switch_cycle(
        || click_on_ui("Trade config"),
        "trade_config",
        "Enable trade overlay",
        |app| app.get_trade_config_overlay_enabled(),
        |config| config.rule_settings.trade_overlay_enabled,
        true,
    );
    dialog_switch_cycle(
        || click_on_ui("Trade config"),
        "trade_config",
        "Show current item",
        |app| app.get_trade_config_show_current_item(),
        |config| config.show_current_item,
        true,
    );

    // Both drafts flipped at once: screenshot, then Cancel discards.
    click_on_ui("Trade config");
    assert!(dialog_state().0 && dialog_state().1 == "trade_config");
    click_control_by_label("Enable trade overlay");
    click_control_by_label("Show current item");
    assert_ne!(
        read_ui(|app| app.get_trade_config_overlay_enabled()),
        overlay_before,
        "toggle-sweep: the overlay draft must flip"
    );
    assert_ne!(
        read_ui(|app| app.get_trade_config_show_current_item()),
        item_before,
        "toggle-sweep: the show-current-item draft must flip"
    );
    toggle_sweep_screenshot(client, window, "trade-config-toggles-drafted.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close trade_config");
    assert_eq!(
        config_bool(|config| config.rule_settings.trade_overlay_enabled),
        overlay_before,
        "toggle-sweep: the cancelled drafts must not persist"
    );
    assert_eq!(
        config_bool(|config| config.show_current_item),
        item_before,
        "toggle-sweep: the cancelled drafts must not persist"
    );
}

/// (f) SetAffix draft switches: each flag flips by real click, Apply
/// persists it into the selected preset's entry (the session editor
/// — no config write exists for entries), the reopen re-seeds the
/// dialog from the entry, and flipping back restores. Cancel
/// discards without touching the entry.
fn set_affix_toggles_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (f) set-affix draft switches");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
    assert_eq!(
        attack_speed_flags(),
        (false, false, true),
        "toggle-sweep: the seeded AttackSpeed flags"
    );

    const SWITCHES: [&str; 3] = ["Greater affix", "Implicit affix", "Tempered affix"];
    for (index, switch) in SWITCHES.iter().copied().enumerate() {
        let flags_before = attack_speed_flags();
        click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
        assert!(
            dialog_state().0 && dialog_state().1 == "set_affix",
            "toggle-sweep: the entry card must open set_affix for {switch:?}"
        );
        // The open seeds the draft from the entry; the seeded value
        // (not the stale closed-dialog form value) is the baseline,
        // and it must equal the persisted entry flag.
        let form_before = read_ui(move |app| set_affix_form_flag(app, index));
        let entry_flag = match index {
            0 => flags_before.0,
            1 => flags_before.1,
            _ => flags_before.2,
        };
        assert_eq!(
            form_before, entry_flag,
            "toggle-sweep: {switch:?} must seed from the persisted entry flag"
        );

        click_control_by_label(switch);
        assert_ne!(
            read_ui(move |app| set_affix_form_flag(app, index)),
            form_before,
            "toggle-sweep: the {switch:?} draft must flip"
        );
        click_on_ui("Apply");
        assert!(
            !dialog_state().0,
            "toggle-sweep: Apply must close set_affix for {switch:?}"
        );

        let mut flags_after = flags_before;
        match index {
            0 => flags_after.0 = !flags_after.0,
            1 => flags_after.1 = !flags_after.1,
            _ => flags_after.2 = !flags_after.2,
        }
        assert_eq!(
            attack_speed_flags(),
            flags_after,
            "toggle-sweep: Apply must persist the {switch:?} flag into the preset entry"
        );

        click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
        assert!(dialog_state().0 && dialog_state().1 == "set_affix");
        assert_eq!(
            read_ui(move |app| set_affix_form_flag(app, index)),
            !form_before,
            "toggle-sweep: the confirmed {switch:?} flag must re-seed the dialog"
        );
        click_control_by_label(switch);
        click_on_ui("Apply");
        assert!(!dialog_state().0, "cleanup: {switch:?} restored");
        assert_eq!(
            attack_speed_flags(),
            flags_before,
            "toggle-sweep: the {switch:?} restore must persist into the entry"
        );
    }

    // Rapid double-toggle + a drafted screenshot, then Cancel.
    click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
    assert!(dialog_state().0);
    let before = read_ui(|app| app.get_set_affix_is_greater());
    click_control_by_label("Greater affix");
    click_control_by_label("Greater affix");
    assert_eq!(
        read_ui(|app| app.get_set_affix_is_greater()),
        before,
        "toggle-sweep: two rapid clicks must toggle the draft exactly twice"
    );
    click_control_by_label("Greater affix");
    toggle_sweep_screenshot(client, window, "set-affix-toggles-drafted.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close set_affix");
    assert_eq!(
        attack_speed_flags(),
        (false, false, true),
        "toggle-sweep: the cancelled draft must not touch the entry"
    );

    eprintln!(
        "[toggle-sweep] expected-mismatch: set-affix flags persist into the session preset \
         editor only — the port has no config write for preset entries (the import/export \
         path owns preset files), so the readback surface is the in-memory entry"
    );
}

/// (g) HotkeyConfig draft switch: the SwitchPreset binding's
/// enabled flag flips by real click, Apply persists it through
/// `save_app_settings` into `hotkey_config.bindings`, the reload
/// re-hydrates the drafts and the reopen re-seeds the form, Cancel
/// discards.
fn hotkey_toggle_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (g) hotkey draft toggle");
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    dialog_switch_cycle(
        || {
            click_on_ui("Settings");
            click_on_ui("Open hotkeys config");
            click_control_by_label("Edit switch preset binding");
        },
        "hotkey_config",
        "Enabled",
        |app| app.get_hotkey_config_enabled(),
        switch_preset_enabled,
        true,
    );
    assert_no_live_hotkey_backend("after the HotkeyConfig Apply (re-registration)");

    // Rapid double-toggle, drafted screenshot, Cancel discards.
    click_on_ui("Settings");
    click_on_ui("Open hotkeys config");
    click_control_by_label("Edit switch preset binding");
    assert!(dialog_state().0 && dialog_state().1 == "hotkey_config");
    let before = read_ui(|app| app.get_hotkey_config_enabled());
    click_control_by_label("Enabled");
    click_control_by_label("Enabled");
    assert_eq!(
        read_ui(|app| app.get_hotkey_config_enabled()),
        before,
        "toggle-sweep: two rapid clicks must toggle the draft exactly twice"
    );
    click_control_by_label("Enabled");
    toggle_sweep_screenshot(client, window, "hotkey-toggle-drafted.png");
    click_on_ui("Cancel");
    assert_dialog_closed("hotkey_config", "Cancel must close hotkey_config");
    assert_eq!(
        config_bool(switch_preset_enabled),
        before,
        "toggle-sweep: the cancelled draft must not persist"
    );
}

/// One write-through config-dialog switch cycle (config-dialogs host
/// wiring): the affix-family config dialogs persist from the toggle
/// setter itself (upstream `AffixConfigViewModel.cs:55-101`,
/// `SigilConfigViewModel.cs:60-98`), so the asserted sequence is
/// open → the open handler seeds the switch from the persisted value →
/// click → the config file holds the new value (write-through, no
/// Apply step) → Cancel keeps the persisted edit (there is no draft to
/// roll back) → startup-projection reload → the reopened dialog
/// re-seeds the persisted value → flip back → restored.
fn write_through_switch_cycle(
    opener_label: &'static str,
    opener: impl Fn(),
    dialog_id: &'static str,
    switch: &'static str,
    read: fn(&AppWindow) -> bool,
    field: fn(&d4lc_config::AppSettings) -> bool,
) {
    let config_before = config_bool(field);

    opener();
    let (visible, seen) = dialog_state();
    assert!(
        visible && seen == dialog_id,
        "toggle-sweep: {opener_label:?} must open {dialog_id:?} (saw visible={visible}, id={seen:?})"
    );
    assert_eq!(
        read_ui(read),
        config_before,
        "toggle-sweep: the open handler must seed {switch:?} from the persisted value"
    );

    click_control_by_label(switch);
    assert_ne!(
        read_ui(read),
        config_before,
        "toggle-sweep: the {switch:?} click must flip the root value"
    );
    assert_eq!(
        config_bool(field),
        !config_before,
        "toggle-sweep: the {switch:?} flip must persist on the click itself (write-through)"
    );

    // Write-through: Cancel closes without rolling the edit back
    // (upstream saves from the setter; there is no draft).
    click_on_ui("Cancel");
    assert_dialog_closed(
        dialog_id,
        &format!("toggle-sweep: Cancel must close {dialog_id:?}"),
    );
    assert_eq!(
        config_bool(field),
        !config_before,
        "toggle-sweep: the write-through {switch:?} edit must survive Cancel"
    );

    reload_from_config();
    opener();
    assert!(
        dialog_state().0 && dialog_state().1 == dialog_id,
        "toggle-sweep: {opener_label:?} must reopen {dialog_id:?} for the re-seed readback"
    );
    assert_eq!(
        read_ui(read),
        !config_before,
        "toggle-sweep: the persisted {switch:?} flip must re-seed the reopened dialog"
    );

    click_control_by_label(switch);
    click_on_ui("Close");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Close must confirm-close {dialog_id:?} on the restore"
    );
    assert_eq!(
        config_bool(field),
        config_before,
        "toggle-sweep: the {switch:?} restore must persist"
    );
}

/// The stored dungeon tier of one sigil id (the upstream default `F`
/// when the map holds no entry).
fn sigil_tier(sigil_id: &str) -> String {
    let sigil_id = sigil_id.to_string();
    read_ui(move |_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state
            .dungeon_tiers
            .get(&sigil_id)
            .cloned()
            .unwrap_or_else(|| d4lc_gui::affix::SIGIL_DUNGEON_TIER_DEFAULT.to_string())
    })
}

/// Re-seed the standard preset state and persist it through the real
/// seam, so the write-through legs leave the presets file as the
/// following scenario families expect it.
pub(super) fn restore_standard_presets() {
    seed_affix_presets();
    let config_root = crate::actions::settings::resolve_config_root();
    on_ui(move |_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state
            .persist_affix_presets_to_config(&config_root)
            .expect("restore standard presets");
    });
}

/// (h) Affix-family config dialog switches + the SetAffix host-seeded
/// model: the four Affix config switches and
/// the shared multi-build switch of the aspect / unique / rune dialogs
/// each run the write-through cycle; the SetAffix minimal-value field
/// reports through the real key path, the per-slot add appends and
/// re-seeds the counters, and the confirmed minimal-values reset
/// empties the threshold file and the next open re-seeds an empty
/// field.
fn affix_family_config_toggles_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[toggle-sweep] (h) affix-family config dialog switches");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
    click_control_by_label("Affixes");

    write_through_switch_cycle(
        "Affix config",
        || click_on_ui("Affix config"),
        "affix_config",
        "Tempered affix detection",
        |app| app.get_settings_rule_tempered_affix_detection_enabled(),
        |config| config.is_tempered_affix_detection_enabled,
    );
    write_through_switch_cycle(
        "Affix config",
        || click_on_ui("Affix config"),
        "affix_config",
        "Tuning prism hints",
        |app| app.get_settings_rule_tuning_prism_hints_enabled(),
        |config| config.is_tuning_prism_hints_enabled,
    );
    write_through_switch_cycle(
        "Affix config",
        || click_on_ui("Affix config"),
        "affix_config",
        "Minimal affix value filter",
        |app| app.get_settings_rule_minimal_affix_value_filter_enabled(),
        |config| config.rule_settings.minimal_affix_value_enabled,
    );
    write_through_switch_cycle(
        "Affix config",
        || click_on_ui("Affix config"),
        "affix_config",
        "Multi-build mode",
        |app| app.get_settings_rule_multi_build_enabled(),
        |config| config.rule_settings.multi_build_enabled,
    );

    // The detection switch of the aspect / unique / rune dialogs, and the
    // same shared multi-build switch in all three (one persisted field,
    // three more real UI invokers).
    type SwitchRead = fn(&AppWindow) -> bool;
    type ConfigRead = fn(&d4lc_config::AppSettings) -> bool;
    let detection_rows: [(&str, &str, &str, &str, SwitchRead, ConfigRead); 3] = [
        (
            "Aspects",
            "Aspect config",
            "aspect_config",
            "Aspect detection",
            |app| app.get_settings_rule_aspect_detection_enabled(),
            |config| config.rule_settings.aspect_detection_enabled,
        ),
        (
            "Uniques",
            "Unique config",
            "unique_config",
            "Unique detection",
            |app| app.get_settings_rule_unique_detection_enabled(),
            |config| config.rule_settings.unique_detection_enabled,
        ),
        (
            "Runes",
            "Rune config",
            "rune_config",
            "Rune detection",
            |app| app.get_settings_rule_rune_detection_enabled(),
            |config| config.rule_settings.rune_detection_enabled,
        ),
    ];
    for (category, cell, dialog_id, detection_label, read_detection, read_config) in detection_rows
    {
        click_control_by_label(category);
        write_through_switch_cycle(
            cell,
            || click_on_ui(cell),
            dialog_id,
            detection_label,
            read_detection,
            read_config,
        );
        write_through_switch_cycle(
            cell,
            || click_on_ui(cell),
            dialog_id,
            "Multi-build mode",
            |app| app.get_settings_rule_multi_build_enabled(),
            |config| config.rule_settings.multi_build_enabled,
        );
    }

    // ── SetAffix host-seeded model (minimal value + slot counters) ──
    click_control_by_label("Affixes");
    click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
    assert!(
        dialog_state().0 && dialog_state().1 == "set_affix",
        "toggle-sweep: the entry card must open set_affix"
    );
    // The open seeds the counters from the preset's entries; the
    // seeded standard AttackSpeed entry carries the free-form item
    // type "Amulet", which is not an upstream slot token, so every
    // slot starts at zero.
    assert_eq!(
        read_ui(|app| app.get_set_affix_slot_counters().row_data(5)),
        Some(0),
        "toggle-sweep: the seeded slot counters must start at zero"
    );

    // Type a threshold through the real key path: every complete entry
    // persists write-through (upstream saves from the value setter).
    let value_input = find_text_input(client, root, "e.g. 12.5");
    client
        .set_element_value(value_input, "")
        .expect("toggle-sweep: clear the minimal-value input");
    client
        .click_element(value_input, None)
        .expect("toggle-sweep: click into the minimal-value input");
    for character in "12.5".chars() {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| {
                harness_panic("toggle-sweep: type into the minimal-value input", &error)
            });
    }
    assert_eq!(
        read_ui(|app| app.get_set_affix_minimal_value().to_string()),
        "12.5",
        "toggle-sweep: the typed text must land in the root form property"
    );
    let config_root = crate::actions::settings::resolve_config_root();
    let stored = d4lc_config::load_affix_minimal_values(&config_root)
        .unwrap_or_else(|error| harness_panic("toggle-sweep: minimal-values readback", &error));
    assert_eq!(
        stored.get("AttackSpeed").copied(),
        Some(12.5),
        "toggle-sweep: the typed minimal value must persist write-through"
    );

    // Per-slot add: the click appends a typed entry for the edited
    // affix (the counter is derived from the preset's entries), the
    // wired re-seed refreshes the counters model, and the append
    // persists through the same preset-file seam as every mutation.
    click_control_by_label("Amulet");
    assert_eq!(
        read_ui(|app| app.get_set_affix_slot_counters().row_data(5)),
        Some(1),
        "toggle-sweep: the slot add must append the entry and re-seed the counter"
    );
    let presets_text = std::fs::read_to_string(config_root.join("Config/AffixPresets-v2.json"))
        .unwrap_or_default();
    assert!(
        presets_text.contains("amulet"),
        "toggle-sweep: the slot add must persist the typed entry into the presets file"
    );
    click_on_ui("Cancel");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Cancel must close set_affix"
    );

    // The reset asks through the shared window-modal prompt
    // the upstream copy
    // (`AffixConfigViewModel.cs:116`) asks first, No keeps the
    // thresholds and returns to the config dialog, and Yes empties the
    // threshold file through the same wired callback as before.
    click_control_by_label("Affixes");
    click_on_ui("Affix config");
    assert!(dialog_state().0 && dialog_state().1 == "affix_config");
    click_on_ui("Reset minimal affix values");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "toggle-sweep: the reset button must ask through the confirm prompt (observed: \
         visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_dialog_title().to_string()),
        "Reset",
        "toggle-sweep: the reset prompt must carry the upstream title"
    );
    assert_eq!(
        read_ui(|app| app.get_dialog_message().to_string()),
        "Are you sure you want to reset the minimal affix values?",
        "toggle-sweep: the reset prompt must carry the upstream copy"
    );
    click_on_ui("No");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "affix_config",
        "toggle-sweep: No must return to the affix config dialog the prompt was shown over \
         (observed: visible={visible}, id={dialog_id:?})"
    );
    let stored = d4lc_config::load_affix_minimal_values(&config_root)
        .unwrap_or_else(|error| harness_panic("toggle-sweep: minimal-values readback", &error));
    assert_eq!(
        stored.get("AttackSpeed").copied(),
        Some(12.5),
        "toggle-sweep: No must keep the thresholds, saw {stored:?}"
    );

    click_on_ui("Reset minimal affix values");
    click_on_ui("Yes");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "affix_config",
        "toggle-sweep: Yes must return to the affix config dialog as well (observed: \
         visible={visible}, id={dialog_id:?})"
    );
    let stored = d4lc_config::load_affix_minimal_values(&config_root)
        .unwrap_or_else(|error| harness_panic("toggle-sweep: minimal-values readback", &error));
    assert!(
        stored.is_empty(),
        "toggle-sweep: the confirmed reset must empty the threshold file, saw {stored:?}"
    );
    click_on_ui("Close");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Close must close affix_config"
    );

    // The affix config dialog's "Manage colors" button is the entry
    // point to the default-colors dialog (upstream `SetColorsCommand`,
    // AffixConfigView.xaml:78-85): a real click switches the shared
    // dialog host to `colors_config`, whose Cancel closes it.
    click_on_ui("Affix config");
    assert!(dialog_state().0 && dialog_state().1 == "affix_config");
    click_on_ui("Manage colors");
    let (colors_visible, colors_id) = dialog_state();
    assert!(
        colors_visible && colors_id == "colors_config",
        "toggle-sweep: the affix config Manage colors entry point must open the colors dialog \
         (saw visible={colors_visible}, id={colors_id:?})"
    );
    click_on_ui("Cancel");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Cancel must close the colors dialog"
    );

    // The next open re-seeds the (now empty) minimal-value field.
    click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
    assert!(dialog_state().0 && dialog_state().1 == "set_affix");
    assert_eq!(
        read_ui(|app| app.get_set_affix_minimal_value().to_string()),
        "",
        "toggle-sweep: the reset minimal value must re-seed as an empty field"
    );
    click_on_ui("Cancel");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Cancel must close set_affix"
    );

    restore_standard_presets();
}

/// (i) Sigil config dialog switches + per-entry tier cycle: both switches run the write-through
/// cycle; the preset's sigil entry seeds one tier row with the upstream
/// `F` default, the cycle button advances it to `S`, persists
/// `Config/DungeonTiers.json`, and the wired re-seed updates the row
/// label while the dialog stays open.
fn sigil_config_toggles_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[toggle-sweep] (i) sigil config dialog switches + tier cycle");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
    click_control_by_label("Sigils");

    write_through_switch_cycle(
        "Sigil config",
        || click_on_ui("Sigil config"),
        "sigil_config",
        "Dungeon tiers",
        |app| app.get_settings_rule_dungeon_tiers_enabled(),
        |config| config.rule_settings.dungeon_tiers_enabled,
    );
    write_through_switch_cycle(
        "Sigil config",
        || click_on_ui("Sigil config"),
        "sigil_config",
        "Multi-build mode",
        |app| app.get_settings_rule_multi_build_enabled(),
        |config| config.rule_settings.multi_build_enabled,
    );

    let config_root = crate::actions::settings::resolve_config_root();
    click_on_ui("Sigil config");
    assert!(
        dialog_state().0 && dialog_state().1 == "sigil_config",
        "toggle-sweep: the sigil config cell must reopen the dialog"
    );
    assert_eq!(
        label_count("Cycle tier for Torture"),
        1,
        "toggle-sweep: the preset's sigil entry must seed exactly one tier row"
    );
    assert_eq!(
        read_ui(|app| app
            .get_sigil_config_entry_tiers()
            .row_data(0)
            .map(|tier| tier.to_string())),
        Some("F".to_string()),
        "toggle-sweep: a sigil without a stored tier must seed the upstream F default"
    );
    toggle_sweep_screenshot(client, window, "sigil-config-tier-rows.png");

    click_on_ui("Cycle tier for Torture");
    assert_eq!(
        sigil_tier("Torture"),
        "S",
        "toggle-sweep: the cycle must advance the tier through the upstream S..F list"
    );
    let tiers = d4lc_config::load_dungeon_tiers(&config_root)
        .unwrap_or_else(|error| harness_panic("toggle-sweep: dungeon-tiers readback", &error));
    assert_eq!(
        tiers.get("Torture").map(String::as_str),
        Some("S"),
        "toggle-sweep: the cycle must persist Config/DungeonTiers.json"
    );
    assert_eq!(
        read_ui(|app| app
            .get_sigil_config_entry_tiers()
            .row_data(0)
            .map(|tier| tier.to_string())),
        Some("S".to_string()),
        "toggle-sweep: the wired re-seed must update the tier row while the dialog is open"
    );

    // Restore the tier state (no stored tier = the upstream F default)
    // and close the dialog.
    on_ui(move |_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state.bump_rules_input_revision();
        state.dungeon_tiers.remove("Torture");
        if let Err(error) = d4lc_config::save_dungeon_tiers(&config_root, &state.dungeon_tiers) {
            harness_panic("toggle-sweep: restore dungeon tiers", &error);
        }
    });
    click_on_ui("Cancel");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Cancel must close sigil_config"
    );

    // The sigil config also carries the display-mode selector: the
    // write-through selection persists and re-seeds like the switches.
    let mode_before = config_bool(|config| {
        config.rule_settings.sigil_display_mode
            == d4lc_config::SigilDisplayModeSetting::Blacklisting
    });
    click_on_ui("Sigil config");
    assert!(dialog_state().0 && dialog_state().1 == "sigil_config");
    assert_eq!(
        read_ui(|app| app.get_settings_rule_sigil_display_mode().to_string()),
        if mode_before {
            "Blacklisting"
        } else {
            "Whitelisting"
        },
        "toggle-sweep: the display-mode selector must seed from the persisted mode"
    );
    click_control_by_label("Sigil display mode");
    assert_eq!(
        config_bool(|config| {
            config.rule_settings.sigil_display_mode
                == d4lc_config::SigilDisplayModeSetting::Blacklisting
        }),
        !mode_before,
        "toggle-sweep: the display-mode selection must persist write-through"
    );
    click_control_by_label("Sigil display mode");
    assert_eq!(
        config_bool(|config| {
            config.rule_settings.sigil_display_mode
                == d4lc_config::SigilDisplayModeSetting::Blacklisting
        }),
        mode_before,
        "toggle-sweep: the display-mode restore must persist"
    );
    click_on_ui("Close");
    assert!(
        !dialog_state().0,
        "toggle-sweep: Close must close sigil_config"
    );

    // Leave the affix screen on its default Affixes category: the
    // card grid of a category is only instantiated while it is the
    // active one, and the affix-surface family that follows the sweep
    // clicks the Affix entry cards directly.
    click_control_by_label("Affixes");
}
