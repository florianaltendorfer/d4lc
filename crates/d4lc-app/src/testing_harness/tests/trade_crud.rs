use super::*;

// ── Trade CRUD through the real UI ─────────────────────────────────────

/// Replace the harness state with the deterministic trade seed: two
/// wanted rows (one multi-affix), a parsed current item, and the
/// "Alpha"/"Beta" presets the multi-build rows reference by index —
/// then re-project the root models. Non-UI seeding is the same
/// justified pattern as `seed_affix_presets`: the add-dialog has
/// no affix create path and the multi-build rows have no direct UI
/// editor, so seeds that must exist before the flow cannot come
/// from clicks. Every interaction under test is still driven by
/// real clicks and keys.
pub(super) fn seed_trade_items() {
    on_ui(move |_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");

        // Deterministic reset: earlier scenarios leave rows (and a
        // confirmed font size) behind, so the seed replaces them.
        state.trade_imports_mut().trade_items = Vec::new();
        state.trade_imports_mut().selected_trade_item_id = None;
        state.shell.settings.overlay_font_size = 18;

        state
            .trade_imports_mut()
            .upsert_trade_item(d4lc_gui::TradeItemRow {
                trade_item_id: "Trade_Amulet".into(),
                item_type: "Amulet".into(),
                value_text: "10M".into(),
                affixes: vec![d4lc_gui::TradeItemAffixView {
                    affix_id: "Affix_AttackSpeed".into(),
                    is_greater: false,
                }],
            });
        state
            .trade_imports_mut()
            .upsert_trade_item(d4lc_gui::TradeItemRow {
                trade_item_id: "Trade_Helm".into(),
                item_type: "Helm".into(),
                value_text: "750k".into(),
                affixes: vec![
                    d4lc_gui::TradeItemAffixView {
                        affix_id: "Affix_MaxLife".into(),
                        is_greater: true,
                    },
                    d4lc_gui::TradeItemAffixView {
                        affix_id: "Affix_Armor".into(),
                        is_greater: false,
                    },
                ],
            });

        let current = state.current_item_mut();
        current.item_type = Some("Boots".into());
        current.base_type = Some("Boots".into());
        current.item_power = Some(862);
        current.affixes = vec![
            d4lc_gui::CurrentItemAffixView {
                row_id: d4lc_core::AffixRowId(0),
                affix_id: Some("Affix_MoveSpeed".into()),
                label: "Movement Speed".into(),
                value_text: None,
                source_text: String::new(),
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                matched_by_rule: false,
            },
            d4lc_gui::CurrentItemAffixView {
                row_id: d4lc_core::AffixRowId(1),
                affix_id: Some("Affix_Armor".into()),
                label: "Armor".into(),
                value_text: None,
                source_text: String::new(),
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                matched_by_rule: false,
            },
        ];

        // Presets the multi-build rows reference by index.
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(vec![
            d4lc_gui::AffixPresetEditorPreset {
                name: "Alpha".into(),
                entries: Vec::new(),
            },
            d4lc_gui::AffixPresetEditorPreset {
                name: "Beta".into(),
                entries: Vec::new(),
            },
        ]);
        state.affix_preset_editor.selected_preset_name = Some("Alpha".into());
        state.shell.affix_presets.preset_count = 2;
        state.shell.affix_presets.selected_preset = Some("Alpha".into());
        state.build_import.multi_builds = vec![
            crate::app_state::MultiBuildRow {
                name: "Alpha rotation".into(),
                index: 0,
                color: "#CC3333".into(),
            },
            crate::app_state::MultiBuildRow {
                name: "Beta rotation".into(),
                index: 1,
                color: "#22C55E".into(),
            },
        ];
    });
    reproject_trade();
}

/// Re-project the current trade state onto the root — the same
/// update the wired handlers perform after trade-item mutations.
/// This is the setup half of a seed, never a substitute for an
/// interaction under test.
pub(super) fn reproject_trade() {
    on_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        crate::actions::trade::project_trade_state(app, &state);
        let names: Vec<slint::SharedString> = state
            .affix_preset_editor
            .presets
            .iter()
            .map(|preset| slint::SharedString::from(preset.name.as_str()))
            .collect();
        app.set_preset_names(Rc::new(slint::VecModel::from(names)).into());
        app.set_selected_preset(slint::SharedString::from(
            state
                .affix_preset_editor
                .selected_preset_name
                .as_deref()
                .unwrap_or(""),
        ));
    });
}

/// Wanted row lookup by id through the root model, flattened to
/// `Send` primitives (`TradeItem` holds an `Rc` model).
pub(super) fn wanted_row(id: &'static str) -> Option<(String, String, usize)> {
    read_ui(move |app| {
        let model = app.get_trade_items();
        (0..model.row_count()).find_map(|i| {
            let row = model.row_data(i)?;
            (row.item_id.as_str() == id).then(|| {
                (
                    row.item_type.to_string(),
                    row.value.to_string(),
                    row.affixes.row_count(),
                )
            })
        })
    })
}

/// Wanted list row count through the root model.
pub(super) fn wanted_row_count() -> usize {
    read_ui(|app| app.get_trade_items().row_count())
}

fn state_trade_items() -> Vec<(String, String, String)> {
    read_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state
            .trade_imports()
            .trade_items
            .iter()
            .map(|row| {
                (
                    row.trade_item_id.clone(),
                    row.item_type.clone(),
                    row.value_text.clone(),
                )
            })
            .collect()
    })
}

fn state_selected_id() -> Option<String> {
    read_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state.trade_imports().selected_trade_item_id.clone()
    })
}

fn settings_font_size() -> u32 {
    read_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state.shell.settings.overlay_font_size
    })
}

/// (a) Add flow: the seeded wanted rows and the current-item card
/// render from the projection without any UI create path; a real
/// click on "Add trade item" opens the dialog, typed inputs land in
/// the form props, and Confirm projects the new row. The empty
/// item-type edge surfaces the dialog error without changing the
/// model.
fn add_flow_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[trade-crud] (a) add flow with typed inputs");
    seed_trade_items();
    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);

    // Initial projection of the seed.
    assert_eq!(
        wanted_row_count(),
        2,
        "trade-crud: the seeded wanted rows must render without any UI create path"
    );
    assert_label_findable("Remove Amulet");
    assert_label_findable("Edit Helm");
    assert_eq!(
        read_ui(|app| app.get_current_item_type().to_string()),
        "Boots",
        "trade-crud: the current-item card must project the parsed item type"
    );
    assert_eq!(
        read_ui(|app| app.get_current_item_power()),
        862,
        "trade-crud: the current-item card must project the item power"
    );
    assert_eq!(
        read_ui(|app| app.get_current_affixes().row_count()),
        2,
        "trade-crud: the current-item card must project the parsed affix labels"
    );
    take_evidence_screenshot(client, window, "trade-crud", "01-wanted-cards-seeded.png");

    // Real click on the plus card opens the dialog.
    click_on_ui("Add trade item");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "add_trade_item",
        "trade-crud: Add trade item must open add_trade_item \
         (saw visible={visible}, id={dialog_id:?})"
    );
    take_evidence_screenshot(client, window, "trade-crud", "02-add-dialog-open.png");

    // Typed inputs through the real key pipeline (lowercase, the
    // character class the backend is proven with).
    let type_input = find_text_input(client, root, "e.g. Amulet, Two-Handed Sword");
    client
        .click_element(type_input, None)
        .expect("click into the item type input");
    type_text(window, client, "ring");
    let value_input = find_text_input(client, root, "e.g. 10M, 5b");
    client
        .click_element(value_input, None)
        .expect("click into the value input");
    type_text(window, client, "25b");
    assert_eq!(
        read_ui(|app| app.get_trade_form_item_type().to_string()),
        "ring",
        "trade-crud: the typed item type must land in the root form property"
    );
    assert_eq!(
        read_ui(|app| app.get_trade_form_value_text().to_string()),
        "25b",
        "trade-crud: the typed value must land in the root form property"
    );

    click_on_ui("Confirm");
    assert!(!dialog_state().0, "Confirm must close add_trade_item");
    assert_eq!(
        wanted_row_count(),
        3,
        "trade-crud: the confirmed item must project as a wanted card"
    );
    assert_eq!(
        wanted_row("Trade_ring"),
        Some(("ring".to_string(), "25b".to_string(), 0)),
        "trade-crud: the new row must carry the typed values (no affixes yet)"
    );
    assert_label_findable("Remove ring");
    take_evidence_screenshot(client, window, "trade-crud", "03-added-row.png");

    // Validation edge: an empty item type must surface the dialog
    // error and leave the model unchanged.
    click_on_ui("Add trade item");
    assert!(dialog_state().0, "trade-crud: the add dialog must reopen");
    let value_input = find_text_input(client, root, "e.g. 10M, 5b");
    client
        .click_element(value_input, None)
        .expect("click into the value input for the invalid draft");
    type_text(window, client, "77");
    click_on_ui("Confirm");
    assert!(
        dialog_state().0 && read_ui(|app| app.get_dialog_error_visible()),
        "trade-crud: an empty item type must surface the dialog error"
    );
    assert_eq!(
        wanted_row_count(),
        3,
        "trade-crud: the failed confirm must not change the wanted list"
    );
    take_evidence_screenshot(client, window, "trade-crud", "04-add-validation-error.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close add_trade_item");
}

/// (b) Edit flow: the pencil click opens the dialog seeded from the
/// addressed row; the edited value confirms in place (same id,
/// preserved affixes, updated value).
fn edit_flow_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[trade-crud] (b) edit flow");
    seed_trade_items();
    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);

    click_on_label("Edit Helm");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "add_trade_item",
        "trade-crud: Edit Helm must open add_trade_item \
         (saw visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_trade_form_item_type().to_string()),
        "Helm",
        "trade-crud: the edit dialog must be seeded from the clicked row's item type"
    );
    assert_eq!(
        read_ui(|app| app.get_trade_form_value_text().to_string()),
        "750k",
        "trade-crud: the edit dialog must be seeded from the clicked row's value"
    );
    take_evidence_screenshot(client, window, "trade-crud", "05-edit-dialog-seeded.png");

    // Edit only the value; the item type stays.
    let value_input = find_text_input(client, root, "e.g. 10M, 5b");
    client
        .set_element_value(value_input, "")
        .expect("clear the seeded value");
    client
        .set_element_value(value_input, "800k")
        .expect("type the edited value");
    click_on_ui("Confirm");
    assert!(!dialog_state().0, "Confirm must close the edit dialog");

    assert_eq!(
        wanted_row_count(),
        2,
        "trade-crud: the edit must update in place, not append"
    );
    assert_eq!(
        wanted_row("Trade_Helm"),
        Some(("Helm".to_string(), "800k".to_string(), 2)),
        "trade-crud: the edited row must keep its id, item type and affixes, \
         and carry the new value"
    );
    assert_label_findable("Edit Helm");
}

/// (c) Wanted-list remove edges: selected/non-selected removal,
/// duplicate item type (unique ids, two cards), remove down to the
/// empty list — which then accepts a fresh add again.
fn remove_edges_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[trade-crud] (c) wanted-list remove edges");
    seed_trade_items();
    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);

    // (c) wanted-list remove edges: selected/non-selected removal,
    // duplicate item type (unique ids, two cards), remove down to
    // the empty list — which then accepts a fresh add again. The
    // seed's last upsert selected the Helm row, so the first click
    // below removes the selected row.
    assert_eq!(
        state_selected_id().as_deref(),
        Some("Trade_Helm"),
        "trade-crud: the seed must select the last upserted row"
    );
    click_on_label("Remove Helm");
    assert_eq!(
        state_selected_id(),
        None,
        "trade-crud: removing the selected row must clear the selection"
    );
    assert_eq!(
        wanted_row_count(),
        1,
        "trade-crud: removing the Helm row must drop it from the list"
    );
    assert_eq!(
        label_count("Remove Helm"),
        0,
        "trade-crud: the removed row's card must unproject"
    );
    assert_label_findable("Remove Amulet");
    take_evidence_screenshot(client, window, "trade-crud", "06-removed-row.png");

    // Re-add a Helm row, then a duplicate Amulet: the same item
    // type is allowed — the generated ids disambiguate and both
    // Amulet cards render.
    add_via_dialog(client, root, "Helm", "750k");
    add_via_dialog(client, root, "Amulet", "99M");
    assert_eq!(
        wanted_row_count(),
        3,
        "trade-crud: the duplicate item type must add its own row"
    );
    assert_eq!(
        label_count("Remove Amulet"),
        2,
        "trade-crud: a duplicate item type must render as two distinct cards"
    );
    assert!(
        wanted_row("Trade_Helm").is_some() && wanted_row("Trade_Amulet_2").is_some(),
        "trade-crud: the re-added Helm reuses the freed base id while the duplicate \
         Amulet gets a unique generated id"
    );

    // The duplicate confirm selected the new Amulet; clicking the
    // first Amulet card removes the NON-selected sibling, so the
    // selection must be preserved.
    assert_eq!(
        state_selected_id().as_deref(),
        Some("Trade_Amulet_2"),
        "trade-crud: the confirm must select the new row"
    );
    click_on_label("Remove Amulet");
    assert_eq!(
        state_selected_id().as_deref(),
        Some("Trade_Amulet_2"),
        "trade-crud: removing a non-selected row must preserve the selection"
    );

    // Removing the selected duplicate now clears the selection.
    click_on_label("Remove Amulet");
    assert_eq!(
        state_selected_id(),
        None,
        "trade-crud: removing the selected row must clear the selection"
    );

    // Remove down to the empty list.
    click_on_label("Remove Helm");
    assert_eq!(
        wanted_row_count(),
        0,
        "trade-crud: removing the last row must empty the wanted list: {:?}",
        state_trade_items()
    );
    assert_eq!(
        label_count("Remove Amulet") + label_count("Remove Helm"),
        0,
        "trade-crud: the empty list must render no cards"
    );
    take_evidence_screenshot(client, window, "trade-crud", "07-wanted-list-empty.png");

    // The emptied list accepts a fresh add again. The confirm path
    // enforces the fixed trade item-type enumeration (upstream
    // offers the 11 types in the dialog's ComboBox), so the fresh
    // add uses a known type.
    add_via_dialog(client, root, "Ring", "1b");
    assert_eq!(
        wanted_row_count(),
        1,
        "trade-crud: the emptied list must accept a fresh add"
    );
}

/// Drive one add through the real dialog path: click "Add trade
/// item", fill both inputs through the MCP input pipeline
/// (`set_element_value` on the dialog's LineEdits), Confirm.
fn add_via_dialog(client: &McpClient, root: McpHandle, item_type: &str, value: &str) {
    click_on_ui("Add trade item");
    assert!(
        dialog_state().0 && dialog_state().1 == "add_trade_item",
        "trade-crud: Add trade item must open add_trade_item"
    );
    let type_input = find_text_input(client, root, "e.g. Amulet, Two-Handed Sword");
    client
        .set_element_value(type_input, item_type)
        .unwrap_or_else(|error| harness_panic("type the add-dialog item type", &error));
    let value_input = find_text_input(client, root, "e.g. 10M, 5b");
    client
        .set_element_value(value_input, value)
        .unwrap_or_else(|error| harness_panic("type the add-dialog value", &error));
    click_on_ui("Confirm");
    assert!(
        !dialog_state().0,
        "trade-crud: Confirm must close add_trade_item"
    );
}

/// (d) Trade config dialog flow: open (seeded), edit the numeric
/// field through real stepper clicks, confirm persists into
/// AppState and the config file, reopen re-seeds, cancel discards.
/// The two toggles inside the dialog are deliberately not flipped
/// here — the toggle sweep covers them.
fn config_dialog_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[trade-crud] (d) trade config dialog flow");
    seed_trade_items();
    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);

    // Open, seeded from the settings panel state.
    click_on_ui("Trade config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "trade_config",
        "trade-crud: Trade config must open trade_config \
         (saw visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_trade_config_font_size()),
        18,
        "trade-crud: the dialog must seed the font size from the settings"
    );
    // The overlay toggle seeds from the settings panel state
    // (default config: enabled), so assert the seeded value equals
    // the settings rather than a hardcoded bool.
    let overlay_seed = read_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state.shell.settings.trade_overlay_enabled
    });
    assert_eq!(
        read_ui(|app| app.get_trade_config_overlay_enabled()),
        overlay_seed,
        "trade-crud: the dialog must seed the overlay toggle from the settings"
    );
    take_evidence_screenshot(client, window, "trade-crud", "08-trade-config-open.png");

    // Edit the numeric field with real stepper clicks.
    click_on_ui("Overlay font size increment");
    click_on_ui("Overlay font size increment");
    assert_eq!(
        read_ui(|app| app.get_trade_config_font_size()),
        20,
        "trade-crud: two increment clicks must step the draft font size"
    );

    // Confirm persists into AppState and the config file.
    click_on_ui("Apply");
    assert!(!dialog_state().0, "Apply must close trade_config");
    assert_eq!(
        settings_font_size(),
        20,
        "trade-crud: the confirmed font size must reach the app settings state"
    );
    let persisted =
        d4lc_config::load_app_settings(&crate::actions::settings::resolve_config_root())
            .expect("trade-crud: the confirmed trade config must be readable from the config root");
    assert_eq!(
        persisted.overlay_font_size, 20,
        "trade-crud: the confirmed font size must persist to the config file"
    );

    // Reopen: re-seeded from the persisted value.
    click_on_ui("Trade config");
    assert!(
        dialog_state().0 && dialog_state().1 == "trade_config",
        "trade-crud: Trade config must reopen"
    );
    assert_eq!(
        read_ui(|app| app.get_trade_config_font_size()),
        20,
        "trade-crud: the reopen must re-seed from the persisted value"
    );

    // Cancel discards the draft.
    click_on_ui("Overlay font size decrement");
    click_on_ui("Overlay font size decrement");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close trade_config");
    assert_eq!(
        settings_font_size(),
        20,
        "trade-crud: the cancelled draft must not reach the app settings"
    );
    assert_eq!(
        read_ui(|app| app.get_trade_config_font_size()),
        18,
        "trade-crud: the cancel must reset the root form props for the next open"
    );
}

/// (e) Multi-build rows referencing presets: the seeded rotation
/// rows project into the dialog and each row's index selects the
/// preset it is named after (upstream MultiBuildList x AffixPresets
/// relation), asserted through the same root properties the dialog
/// two-way binds (its row Texts carry no accessible labels).
fn multi_build_relation_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[trade-crud] (e) multi-build rows referencing presets");
    seed_trade_items();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    click_on_ui("Affix config");
    click_on_ui("Multi-build config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "multi_build_config",
        "trade-crud: the AffixConfig dialog's Multi-build config must open multi_build_config \
         (saw visible={visible}, id={dialog_id:?})"
    );
    take_evidence_screenshot(
        client,
        window,
        "trade-crud",
        "09-multi-build-preset-rows.png",
    );

    let names = read_ui(|app| {
        let model = app.get_multi_build_names();
        (0..model.row_count())
            .map(|i| model.row_data(i).unwrap().to_string())
            .collect::<Vec<_>>()
    });
    let indices = read_ui(|app| {
        let model = app.get_multi_build_indices();
        (0..model.row_count())
            .map(|i| model.row_data(i).unwrap())
            .collect::<Vec<_>>()
    });
    let colors = read_ui(|app| {
        let model = app.get_multi_build_colors();
        (0..model.row_count())
            .map(|i| model.row_data(i).unwrap().to_string())
            .collect::<Vec<_>>()
    });
    let presets = preset_names();

    assert_eq!(
        names,
        vec!["Alpha rotation".to_string(), "Beta rotation".to_string()],
        "trade-crud: the seeded rotation rows must project into the dialog"
    );
    assert_eq!(
        indices,
        vec![0, 1],
        "trade-crud: the row indices must project"
    );
    assert_eq!(
        colors,
        vec!["#CC3333".to_string(), "#22C55E".to_string()],
        "trade-crud: the row colors must project"
    );

    for (row, index) in names.iter().zip(&indices) {
        let referenced = presets
            .get(*index as usize)
            .map(String::as_str)
            .unwrap_or("");
        assert!(
            !referenced.is_empty() && row.starts_with(referenced),
            "trade-crud: multi-build row {row:?} must reference the preset at index \
             {index} (preset list: {presets:?})"
        );
    }

    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close multi_build_config");
}

/// The trade CRUD family: add/edit/remove through real clicks and
/// typed inputs, wanted-list edges, the trade config dialog flow
/// and the multi-build preset-referencing rows. Every scenario
/// re-seeds its own state (see `seed_trade_items`).
pub(super) fn trade_crud_scenarios(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[trade-crud] starting trade CRUD scenarios");
    add_flow_scenario(client, window, root);
    edit_flow_scenario(client, window, root);
    remove_edges_scenario(client, window, root);
    config_dialog_scenario(client, window);
    multi_build_relation_scenario(client, window);
    eprintln!("[trade-crud] trade CRUD scenarios complete");
}
