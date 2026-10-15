use super::*;

// ── preset persistence scenario ──────────────────────────

/// Real-input regression for affix-preset persistence:
///
/// 1. Create a preset through the real SetPresetName dialog and
///    assert `Config/AffixPresets-v2.json` plus the
///    `SelectedAffixPreset` settings key appear on disk.
/// 2. Simulate a restart through the production startup seam
///    (`hydrate_affix_presets_from_config`): empty the editor, then
///    hydrate — the created preset must come back selected.
/// 3. Hotkey-switch leg: seed two in-memory presets, cycle
///    the selection through `handle_switch_preset`, and assert the
///    per-frame descriptor rebuild (the same projection the live
///    loop runs) picks up the switched preset without any explicit
///    notify, and that the switch persisted to Settings.json.
pub(super) fn preset_persistence_scenario(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
    config_root: &std::path::Path,
) {
    eprintln!("[preset-persistence] starting preset persistence scenario");

    // Real click through the nav to the screen hosting the affix
    // presets UI. The content mapping is the established one: the
    // nav slot at tab 0 ("Current Item") instantiates AffixScreen
    // (main_window.slint `if active-tab == 0: AffixScreen`), which
    // is where every affix scenario drives the preset chips.
    click_on_ui("Current Item");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        0,
        "preset-persistence: nav click must switch to the AffixScreen tab"
    );

    // Create the probe preset through the inline presets-panel row
    // (upstream `AffixView.xaml:117-127`: the "Enter name" field plus
    // the add button; the port-only SetPresetName opener is gone).
    let new_input = find_text_input(client, root, "Enter name");
    client
        .set_element_value(new_input, "")
        .expect("preset-persistence: clear the new-preset input");
    client
        .click_element(new_input, None)
        .expect("preset-persistence: click into the new-preset input");
    for character in "PersistenceProbe".chars() {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| harness_panic("preset-persistence: type preset name", &error));
    }
    assert_eq!(
        read_ui(|app| app.get_new_preset_name().to_string()),
        "PersistenceProbe",
        "preset-persistence: dispatched keys must land in the inline preset-name field"
    );
    click_on_ui("Create preset");
    assert_eq!(
        read_ui(|app| { app.get_selected_preset().to_string() }),
        "PersistenceProbe",
        "preset-persistence: creating a preset must select it"
    );

    // The mutation persisted: the preset file and the selection
    // exist on disk in the upstream shapes.
    let presets_file = config_root.join("Config/AffixPresets-v2.json");
    let presets_text = std::fs::read_to_string(&presets_file)
        .expect("preset-persistence: the preset file must exist after a create");
    assert!(
        presets_text.contains("PersistenceProbe"),
        "preset-persistence: the created preset must be persisted to the presets file"
    );
    assert!(
        presets_text.contains("\"ItemAffixes\""),
        "preset-persistence: the presets file must keep the upstream AffixPreset shape"
    );
    let settings_text = std::fs::read_to_string(config_root.join("Config/Settings.json"))
        .expect("preset-persistence: settings must exist after the selection persist");
    assert!(
        settings_text.contains("\"SelectedAffixPreset\": \"PersistenceProbe\""),
        "preset-persistence: the selection must persist to Settings.json: {settings_text:.400}"
    );

    // Simulated restart: a fresh editor hydrates through the same
    // startup seam the wiring uses (`hydrate_affix_presets_from_config`).
    let restart_root = config_root.to_path_buf();
    let (names, selected) = read_ui(move |_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Restart: the editor starts from the empty state,
        // exactly like a fresh process, then the startup seam
        // hydrates from the persisted files.
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(Vec::new());
        state.hydrate_affix_presets_from_config(&restart_root);
        (
            state
                .affix_preset_editor
                .presets
                .iter()
                .map(|preset| preset.name.clone())
                .collect::<Vec<String>>(),
            state
                .affix_preset_editor
                .selected_preset_name
                .clone()
                .unwrap_or_default(),
        )
    });
    assert!(
        names.iter().any(|name| name == "PersistenceProbe"),
        "preset-persistence: the created preset must survive a reload: {names:?}"
    );
    assert_eq!(
        selected, "PersistenceProbe",
        "preset-persistence: the persisted selection must be restored"
    );

    // Hotkey-switch leg: the live loop rebuilds the
    // descriptor state from AppState every frame, so the switched
    // preset must appear in the rebuilt projection with no explicit
    // notify. Seed two in-memory presets with any-type affix entries
    // plus a parsed two-row current item, so the per-row evaluation
    // has real rows to evaluate against each probe preset.
    let switch_root = config_root.to_path_buf();
    let (selected_after_switch, required_by) = read_ui(move |app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(vec![
            d4lc_gui::AffixPresetEditorPreset {
                name: "ProbeA".into(),
                entries: vec![d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Affix,
                    "ProbeAffixA",
                    None,
                    "#22C55E",
                )],
            },
            d4lc_gui::AffixPresetEditorPreset {
                name: "ProbeB".into(),
                entries: vec![d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Affix,
                    "ProbeAffixB",
                    None,
                    "#EF4444",
                )],
            },
        ]);
        state.affix_preset_editor.selected_preset_name = Some("ProbeA".into());
        // The descriptor rebuild this leg asserts on is gated on the
        // overlay power switch (upstream OverlayHandler.cs:133), and
        // the dead-controls dead-controls family deliberately leaves the
        // persisted flag off. Seed the on state the projection needs
        // (the same setup the app_state descriptor unit test uses).
        state.shell.settings.overlay_enabled = true;
        // The evaluation inputs the per-frame rebuild consumes: one
        // parsed row per probe preset, each with row geometry and the
        // detected tooltip rect, exactly like the live event path
        // transports them.
        {
            let current = state.current_item_mut();
            current.item_type = Some("Amulet".into());
            current.affixes = vec![
                d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(0),
                    affix_id: Some("ProbeAffixA".into()),
                    label: "Probe Affix A".into(),
                    value_text: None,
                    source_text: "+10.0% Probe Affix A".into(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                },
                d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(1),
                    affix_id: Some("ProbeAffixB".into()),
                    label: "Probe Affix B".into(),
                    value_text: None,
                    source_text: "+12.0% Probe Affix B".into(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                },
            ];
            current.affix_row_geometry = vec![
                d4lc_gui::CurrentItemAffixRowGeometryView {
                    row_id: d4lc_core::AffixRowId(0),
                    source_segment_index: 0,
                    source_line_index: 4,
                    global_line_index: 5,
                    marker_ordinal: None,
                    text_rect: d4lc_core::Rect {
                        x: 600,
                        y: 300,
                        width: 240,
                        height: 22,
                    },
                    affix_area: d4lc_core::Rect {
                        x: 600,
                        y: 300,
                        width: 240,
                        height: 22,
                    },
                    marker_anchor: None,
                },
                d4lc_gui::CurrentItemAffixRowGeometryView {
                    row_id: d4lc_core::AffixRowId(1),
                    source_segment_index: 0,
                    source_line_index: 5,
                    global_line_index: 6,
                    marker_ordinal: None,
                    text_rect: d4lc_core::Rect {
                        x: 600,
                        y: 322,
                        width: 260,
                        height: 22,
                    },
                    affix_area: d4lc_core::Rect {
                        x: 600,
                        y: 322,
                        width: 260,
                        height: 22,
                    },
                    marker_anchor: None,
                },
            ];
            current.tooltip_window_relative = Some(d4lc_core::Rect {
                x: 580,
                y: 120,
                width: 400,
                height: 320,
            });
        }
        state.rebuild_overlay_descriptor_state();
        let before = state
            .overlay_descriptor_state
            .as_ref()
            .and_then(|descriptor| descriptor.rule.as_ref())
            .map(|rule| {
                rule.result
                    .matched_affixes
                    .iter()
                    .find(|affix| affix.affix_id == "ProbeAffixA")
                    .is_some_and(|affix| affix.required_by == ["ProbeA"])
            })
            .unwrap_or(false);
        assert!(
            before,
            "preset-persistence: the descriptor projection must start on ProbeA"
        );

        crate::hotkeys::handle_switch_preset(app, &mut state, &switch_root);

        let selected = state
            .affix_preset_editor
            .selected_preset_name
            .clone()
            .unwrap_or_default();
        // The exact projection the live loop re-runs per frame.
        state.rebuild_overlay_descriptor_state();
        let required_by = state
            .overlay_descriptor_state
            .as_ref()
            .and_then(|descriptor| descriptor.rule.as_ref())
            .map(|rule| {
                rule.result
                    .matched_affixes
                    .iter()
                    .find(|affix| affix.affix_id == "ProbeAffixB")
                    .is_some_and(|affix| affix.required_by == ["ProbeB"])
            })
            .unwrap_or(false);
        let _ = app;
        (selected, required_by)
    });
    assert_eq!(
        selected_after_switch, "ProbeB",
        "preset-persistence: the hotkey switch must cycle to the next preset"
    );
    assert!(
        required_by,
        "preset-persistence: the per-frame descriptor rebuild must pick up the \
         hotkey preset switch without an explicit notify"
    );
    let settings_text = std::fs::read_to_string(config_root.join("Config/Settings.json"))
        .expect("preset-persistence: settings must exist after the hotkey switch");
    assert!(
        settings_text.contains("\"SelectedAffixPreset\": \"ProbeB\""),
        "preset-persistence: the hotkey switch must persist the new selection"
    );

    // Restore the standard seed so the final screenshot sees the
    // standard preset state.
    on_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        seed_standard_editor_presets(app, arc);
    });
}
