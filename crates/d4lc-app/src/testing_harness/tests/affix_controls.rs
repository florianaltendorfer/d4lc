use super::*;

// ── dead-controls family (per-family "+" + overlay) ──────

/// Build a labeled probe entry for the dead-controls seed.
fn any_type_entry(
    kind: d4lc_gui::PresetEntryKind,
    id: &str,
    label: &str,
    color: &str,
) -> d4lc_gui::PresetEntry {
    d4lc_gui::PresetEntry {
        kind,
        id: id.to_string(),
        label: Some(label.to_string()),
        item_type: None,
        color: color.to_string(),
        is_any_type: true,
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    }
}

/// Seed the dead-controls probe state: the selected "AddProbe" preset
/// carries one entry per non-affix family (so every family tab
/// renders exactly one card with predictable labels) and the empty
/// "AddEmpty" preset gives the re-projection leg a preset to
/// switch to and back.
fn seed() {
    let presets = vec![
        d4lc_gui::AffixPresetEditorPreset {
            name: "AddProbe".into(),
            entries: vec![
                any_type_entry(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "ProbeAspect",
                    "Probe Aspect",
                    "#3B82F6",
                ),
                any_type_entry(
                    d4lc_gui::PresetEntryKind::Sigil,
                    "ProbeSigil",
                    "Probe Sigil",
                    "#F59E0B",
                ),
                any_type_entry(
                    d4lc_gui::PresetEntryKind::Unique,
                    "ProbeUnique",
                    "Probe Unique",
                    "#111111",
                ),
                any_type_entry(
                    d4lc_gui::PresetEntryKind::Rune,
                    "ProbeRune",
                    "Probe Rune",
                    "#8B5CF6",
                ),
            ],
        },
        d4lc_gui::AffixPresetEditorPreset {
            name: "AddEmpty".into(),
            entries: Vec::new(),
        },
    ];
    on_ui(move |_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(presets);
        state.affix_preset_editor.selected_preset_name = Some("AddProbe".into());
        state.shell.affix_presets.preset_count = state.affix_preset_editor.presets.len();
        state.shell.affix_presets.selected_preset =
            state.affix_preset_editor.selected_preset_name.clone();
    });
    reproject_affixes();
}

/// Real-input regression for the affix-screen dead controls
///
/// 1. Per family (aspect/sigil/unique/rune): the "+" button
///    forwards through the real click path to the kind-aware add
///    handler without opening any dialog (upstream Set* commands
///    add to the wanted list, they do not open dialogs); a fresh
///    key fired through the same wired root callback appends the
///    entry to the matching wanted list, visible immediately,
///    after a preset away-and-back re-projection, and in the
///    persisted presets file; remove still unprojects it.
///
///    The fresh-key leg drives the root callback directly because
///    a wanted-list card for an entry that is not yet wanted
///    cannot exist on this screen — the same justification as the AppState
///    seeding; the existing-card "+" leg is a real click.
/// 2. Overlay power button: a real click flips the persisted
///    overlay flag through `confirm_overlay_config` (the same path
///    as the Ctrl+F12 hotkey arm), the indicator follows, and a
///    dispatched ToggleOverlay hotkey event flips it back — button
///    and hotkey stay consistent and both route into the screen
///    property.
pub(super) fn dead_controls_scenarios(
    client: &McpClient,
    root: McpHandle,
    config_root: &std::path::Path,
) {
    eprintln!("[dead-controls] starting dead-controls scenarios");

    click_on_ui("Current Item");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        0,
        "dead-controls: nav click must switch to the AffixScreen tab"
    );
    seed();

    // ── (1) per-family "+" add paths ─────────────────────────────
    for (tab, name, fresh_id, kind_prefix) in [
        ("Aspects", "Probe Aspect", "FreshAspect", "aspect"),
        ("Sigils", "Probe Sigil", "FreshSigil", "sigil"),
        ("Uniques", "Probe Unique", "FreshUnique", "unique"),
        ("Runes", "Probe Rune", "FreshRune", "rune"),
    ] {
        click_control_by_label(tab);

        // Real "+" click on the existing card: no dialog opens and
        // the already-wanted entry stays (upstream Add* skips
        // duplicates).
        click_on_label(&format!("Add entry: {name}"));
        let (visible, dialog_id) = dialog_state();
        assert!(
            !visible,
            "dead-controls: the {tab} add (+) click must not open a dialog (saw id={dialog_id:?})"
        );
        assert_label_findable(&format!("Set color: {name}"));

        // Fresh key through the same wired root callback: the
        // entry is appended to the matching wanted list and is
        // visible immediately.
        let key = format!("{kind_prefix}:{fresh_id}:");
        on_ui(move |app| match kind_prefix {
            "aspect" => app.invoke_set_aspect_entry(key.as_str().into()),
            "sigil" => app.invoke_set_sigil_entry(key.as_str().into()),
            "unique" => app.invoke_set_unique_entry(key.as_str().into()),
            _ => app.invoke_set_rune_entry(key.as_str().into()),
        });
        assert_label_findable(&format!("Set color: {fresh_id}"));

        // The add persisted through the same seam as every preset
        // mutation (the preset-persistence presets file).
        let presets_text = std::fs::read_to_string(config_root.join("Config/AffixPresets-v2.json"))
            .unwrap_or_default();
        assert!(
            presets_text.contains(fresh_id),
            "dead-controls: the {tab} add must persist into the presets file"
        );

        // Visible after the re-projection: switch preset away and
        // back, the added entry must come back with the list.
        select_preset(client, root, "AddEmpty");
        select_preset(client, root, "AddProbe");
        assert_label_findable(&format!("Set color: {fresh_id}"));

        // Remove still works on the added entry.
        click_on_label(&format!("Remove {fresh_id}"));
        assert_eq!(
            label_count(&format!("Set color: {fresh_id}")),
            0,
            "dead-controls: removing the added {tab} entry must unproject its cells"
        );
    }

    // ── (2) overlay power button + hotkey consistency ────────────
    // Start from a known off state; the indicator must mirror it.
    on_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state.bump_rules_input_revision();
        state.shell.settings.overlay_enabled = false;
        app.set_affix_overlay_enabled(false);
    });

    click_on_ui("Toggle affix overlay");
    let (visible, dialog_id) = dialog_state();
    assert!(
        !visible,
        "dead-controls: the overlay power click must not open a dialog (saw id={dialog_id:?})"
    );
    let (state_on, indicator_on) = read_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        (
            state.shell.settings.overlay_enabled,
            app.get_affix_overlay_enabled(),
        )
    });
    assert!(
        state_on,
        "dead-controls: the overlay power click must flip the persisted overlay flag on"
    );
    assert!(
        indicator_on,
        "dead-controls: the screen overlay indicator must follow the power click"
    );
    let persisted = d4lc_config::load_app_settings(config_root)
        .expect("dead-controls: settings must persist after the power toggle");
    assert!(
        persisted.overlay_enabled,
        "dead-controls: the power click must persist through confirm_overlay_config"
    );

    // Ctrl+F12 consistency: the hotkey arm runs the same confirm
    // path and routes the new state back into the screen property.
    let hotkey_root = config_root.to_path_buf();
    on_ui(move |app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        crate::hotkeys::dispatch_hotkey_event(
            app,
            &mut state,
            &hotkey_root,
            &crate::hotkeys::HotkeyEvent::new("ToggleOverlay"),
        );
    });
    let (state_off, indicator_off) = read_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        (
            state.shell.settings.overlay_enabled,
            app.get_affix_overlay_enabled(),
        )
    });
    assert!(
        !state_off,
        "dead-controls: the Ctrl+F12 arm must flip the overlay flag off again"
    );
    assert!(
        !indicator_off,
        "dead-controls: the Ctrl+F12 arm must route the toggle back into the screen indicator"
    );
    let persisted = d4lc_config::load_app_settings(config_root)
        .expect("dead-controls: settings must persist after the hotkey toggle");
    assert!(
        !persisted.overlay_enabled,
        "dead-controls: the hotkey toggle must persist the off state"
    );

    // Restore the standard seed so the families that follow see
    // the usual state.
    on_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        seed_standard_editor_presets(app, arc);
    });
    eprintln!("[dead-controls] dead-controls scenarios complete");
}

// ── rename preset keyboard scenario ──────────────────────

/// Real-input regression for the RenamePreset dialog keyboard path:
/// create a preset through the SetPresetName dialog with dispatched
/// keys, open the RenamePreset dialog through the selection-gated
/// pencil button (upstream `CanRenameAffixPresetCommandExecute`
/// parity), verify the seeded current/new names, rename via
/// dispatched keys in the shared DarkLineEdit field, and assert the
/// renamed preset is selected afterwards.
pub(super) fn rename_keyboard_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    // Create the preset under rename through the inline presets-panel
    // row, typing the name with dispatched key events (no
    // set_element_value shortcut for the name itself).
    let new_input = find_text_input(client, root, "Enter name");
    client
        .set_element_value(new_input, "")
        .expect("rename-keyboard: clear the new-preset input");
    client
        .click_element(new_input, None)
        .expect("rename-keyboard: click into the new-preset input");
    for character in "JourneyPreset".chars() {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| harness_panic("rename-keyboard: type preset name", &error));
    }
    assert_eq!(
        read_ui(|app| app.get_new_preset_name().to_string()),
        "JourneyPreset",
        "rename-keyboard: dispatched keys must land in the inline preset-name field"
    );
    click_on_ui("Create preset");
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        "JourneyPreset",
        "rename-keyboard: creating a preset must select it"
    );

    // The rename gate (upstream CanRenameAffixPresetCommandExecute):
    // creating a preset selects it, so the gated pencil button must
    // now be enabled.
    let pencil = find_button_by_label(client, root, "Rename preset");
    let pencil_properties = client
        .get_element_properties(pencil)
        .expect("rename-keyboard: pencil properties");
    assert!(
        accessible_enabled(&pencil_properties),
        "rename-keyboard: the selection-gated rename button must be enabled after \
         creation: {pencil_properties:.300}"
    );

    // Open the rename dialog and verify the seeded form values.
    click_on_ui("Rename preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "rename_preset",
        "rename-keyboard: pencil click must open rename_preset"
    );
    assert_eq!(
        read_ui(|app| app.get_rename_preset_current_name().to_string()),
        "JourneyPreset",
        "rename-keyboard: the rename dialog must seed the current preset name"
    );
    assert_eq!(
        read_ui(|app| app.get_rename_preset_new_name().to_string()),
        "JourneyPreset",
        "rename-keyboard: the rename dialog must pre-fill the new name"
    );

    // The dialog input is the shared DarkLineEdit field (the
    // std-widgets LineEdit was replaced): the placeholder is exposed
    // as the accessible label on the inner TextInput.
    let rename_input = find_text_input(client, root, "e.g. Speed, Boss, AoE");
    let input_properties = client
        .get_element_properties(rename_input)
        .expect("rename-keyboard: rename input properties");
    assert!(
        str_property(&input_properties, "accessibleLabel") == Some("e.g. Speed, Boss, AoE"),
        "rename-keyboard: the rename input must be the shared DarkLineEdit field: \
         {input_properties:.300}"
    );

    // Rename via dispatched keys: clear the pre-fill, type the new
    // name, confirm.
    client
        .click_element(rename_input, None)
        .expect("rename-keyboard: click into the rename input");
    client
        .set_element_value(rename_input, "")
        .expect("rename-keyboard: clear the pre-filled rename input");
    for character in "RenamedViaKeys".chars() {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| harness_panic("rename-keyboard: type new preset name", &error));
    }
    assert_eq!(
        read_ui(|app| app.get_rename_preset_new_name().to_string()),
        "RenamedViaKeys",
        "rename-keyboard: dispatched keys must land in the rename input"
    );
    click_on_ui("Apply");
    assert!(
        !dialog_state().0,
        "rename-keyboard: rename confirmed, dialog closed"
    );
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        "RenamedViaKeys",
        "rename-keyboard: the renamed preset must be the selected preset"
    );
    // The scenario runs last (only the final screenshot follows), so
    // the renamed preset is intentionally left selected.
}
