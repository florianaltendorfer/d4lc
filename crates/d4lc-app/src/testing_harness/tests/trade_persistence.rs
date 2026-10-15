use super::*;

// ── trade persistence + live lookup scenario ─────────────

/// Real-input regression for the trade screen:
///
/// 1. Persistence: a real Add-trade-item click/confirm persists
///    the whole wanted list (seeded rows + the new row) to
///    `Config/TradeItems.json` in the harness config root. A
///    simulated restart through the production startup seam
///    (`hydrate_trade_items`) restores every row with its item
///    type, value, and affix `IsGreater` flags intact.
/// 2. Live lookup (documented harness proof): the
///    runtime `CurrentItemUpdate` event carries the parsed item
///    type/power/affixes, the current-item card re-projects from
///    it, and the per-frame overlay descriptor rebuild
///    derives the trade value panel from
///    `apply_trade_item_lookup` — shown on a parsed match, absent
///    when the parsed item matches no wanted row. The event is a
///    synthetic offline payload (no real OCR in the harness); it
///    exercises the exact production event application path.
pub(super) fn trade_persistence_scenario(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
    config_root: &std::path::Path,
) {
    eprintln!("[trade-items] starting trade persistence + live lookup scenario");
    click_on_ui("Affix Presets");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        1,
        "trade-items: nav click must switch to the TradeScreen tab"
    );

    // ── Leg 1: persistence across a simulated restart ─────────────
    seed_trade_items();
    reproject_trade();

    // Real add through the dialog (same interaction shape the
    // trade-crud add flow proves).
    click_on_ui("Add trade item");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "add_trade_item",
        "trade-items: Add trade item must open add_trade_item"
    );
    let type_input = find_text_input(client, root, "e.g. Amulet, Two-Handed Sword");
    client
        .click_element(type_input, None)
        .expect("trade-items: click into the item type input");
    type_text(window, client, "amulet");
    let value_input = find_text_input(client, root, "e.g. 10M, 5b");
    client
        .click_element(value_input, None)
        .expect("trade-items: click into the value input");
    type_text(window, client, "12m");
    click_on_ui("Confirm");
    assert!(
        !dialog_state().0,
        "trade-items: Confirm must close the dialog"
    );
    assert_eq!(
        wanted_row("Trade_amulet"),
        Some(("amulet".to_string(), "12m".to_string(), 0)),
        "trade-items: the confirmed row must project before the restart"
    );

    // The confirm persisted the whole wanted list to the harness
    // config root in the upstream shape.
    let trade_file = config_root.join("Config/TradeItems.json");
    let trade_text = std::fs::read_to_string(&trade_file)
        .expect("trade-items: the trade items file must exist after the confirm");
    assert!(trade_text.trim_start().starts_with('['));
    for fragment in [
        "\"Affixes\"",
        "\"Type\"",
        "\"Value\"",
        "\"IsGreater\": true",
        "12m",
    ] {
        assert!(
            trade_text.contains(fragment),
            "trade-items: the trade items file must contain {fragment}: {trade_text:.400}"
        );
    }

    // Simulated restart: fresh in-memory trade state, then the
    // startup seam hydrates from the persisted file.
    let restart_root = config_root.to_path_buf();
    let restarted_rows = read_ui(move |_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state.trade_imports_mut().trade_items = Vec::new();
        state.trade_imports_mut().selected_trade_item_id = None;
        crate::actions::trade::hydrate_trade_items(&mut state, &restart_root);
        state
            .trade_imports()
            .trade_items
            .iter()
            .map(|row| {
                (
                    row.trade_item_id.clone(),
                    row.item_type.clone(),
                    row.value_text.clone(),
                    row.affixes
                        .iter()
                        .map(|affix| (affix.affix_id.clone(), affix.is_greater))
                        .collect::<Vec<(String, bool)>>(),
                )
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        restarted_rows.len(),
        3,
        "trade-items: every wanted row must survive the restart: {restarted_rows:?}"
    );
    assert!(
        restarted_rows
            .iter()
            .any(|(id, item_type, value, affixes)| {
                id == "Trade_amulet"
                    && item_type == "amulet"
                    && value == "12m"
                    && affixes.is_empty()
            }),
        "trade-items: the UI-created row must come back with its typed fields: {restarted_rows:?}"
    );
    assert!(
        restarted_rows.iter().any(|(id, _, _, affixes)| {
            id == "Trade_Helm"
                && affixes
                    == &[
                        ("Affix_MaxLife".to_string(), true),
                        ("Affix_Armor".to_string(), false),
                    ]
        }),
        "trade-items: multi-affix rows must keep their IsGreater flags: {restarted_rows:?}"
    );

    // ── Leg 2: live lookup through the runtime event path ─────────
    // Deterministic lookup seed: one wanted Amulet row and no
    // parsed item yet.
    on_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state.trade_imports_mut().trade_items = Vec::new();
        state.trade_imports_mut().selected_trade_item_id = None;
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
        state.shell.settings.trade_overlay_enabled = true;
        // The rule-marker projection is gated on the persisted overlay
        // power switch (`project_rule_overlay_state`): with it off the
        // descriptor carries no rule and this leg would assert against a
        // marker the projection never builds. The scenario sets that gate
        // itself instead of inheriting whichever state ran before it.
        state.shell.settings.overlay_enabled = true;
        let current = state.current_item_mut();
        current.item_type = None;
        current.item_power = None;
        current.affixes = Vec::new();
        current.affix_row_geometry = Vec::new();
        current.tooltip_window_relative = None;
    });
    reproject_trade();

    // A parsed tooltip matching the wanted row: the real event
    // application path updates the card and the trade marker
    // derives from `apply_trade_item_lookup`.
    let (card_type, card_power, card_affixes, marker, wanted_rows, resolved_affix_ids) =
        on_ui(|app| {
            let arc = HARNESS_APP_STATE.get().expect("harness app state");
            let mut state = arc.lock().expect("app state lock");
            crate::actions::runtime::apply_event_to_ui(
                app,
                &mut state,
                &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
                    item_name: Some("Amulet of Speed".into()),
                    parse_status: "Parsed".into(),
                    item_type: Some("Amulet".into()),
                    base_type: Some("Amulet".into()),
                    item_power: Some(925),
                    affixes: vec![d4lc_gui::CurrentItemAffixView {
                        row_id: d4lc_core::AffixRowId(0),
                        affix_id: Some("Affix_AttackSpeed".into()),
                        label: "Attack Speed".into(),
                        value_text: Some("18.0%".into()),
                        source_text: "+18.0% Attack Speed".into(),
                        is_greater: false,
                        is_implicit: false,
                        is_tempered: false,
                        matched_by_rule: false,
                    }],
                    affix_row_geometry: Vec::new(),
                    non_affix_rows: Vec::new(),
                    tooltip_window_relative: None,
                },
            );
            // The exact projection the live loop re-runs per frame.
            state.rebuild_overlay_descriptor_state();
            let marker = state
                .overlay_descriptor_state
                .as_ref()
                .and_then(|descriptor| descriptor.rule.as_ref())
                .and_then(|rule| rule.result.trade_marker.clone());
            let wanted_rows: Vec<(String, String)> = state
                .trade_imports()
                .trade_items
                .iter()
                .map(|row| (row.trade_item_id.clone(), row.item_type.clone()))
                .collect();
            let resolved_affix_ids: Vec<Option<String>> = state
                .current_item()
                .affixes
                .iter()
                .map(|affix| affix.affix_id.clone())
                .collect();
            (
                app.get_current_item_type().to_string(),
                app.get_current_item_power(),
                app.get_current_affixes().row_count(),
                marker.map(|marker| (marker.trade_item_id, marker.value_text)),
                wanted_rows,
                resolved_affix_ids,
            )
        });
    assert_eq!(
        card_type, "Amulet",
        "trade-items: the CurrentItemUpdate event must re-project the current-item card"
    );
    assert_eq!(
        card_power, 925,
        "trade-items: the event payload must carry the item power to the card"
    );
    assert_eq!(
        card_affixes, 1,
        "trade-items: the event payload must carry the parsed affixes to the card"
    );
    assert_eq!(
        marker
            .as_ref()
            .map(|(id, value)| (id.as_str(), value.as_str())),
        Some(("Trade_Amulet", "10M")),
        "trade-items: the trade marker must derive from the lookup over the wanted rows \
         (wanted rows: {wanted_rows:?}; parsed affix ids: {resolved_affix_ids:?})"
    );

    // Without a match: a parsed Ring matches no wanted row, so no
    // trade marker is projected.
    let no_match_marker = on_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        crate::actions::runtime::apply_event_to_ui(
            app,
            &mut state,
            &crate::runtime_bridge::RuntimeEvent::CurrentItemUpdate {
                item_name: Some("Ring of Nothing".into()),
                parse_status: "Parsed".into(),
                item_type: Some("Ring".into()),
                base_type: Some("Ring".into()),
                item_power: Some(880),
                affixes: vec![d4lc_gui::CurrentItemAffixView {
                    row_id: d4lc_core::AffixRowId(0),
                    affix_id: Some("Affix_MoveSpeed".into()),
                    label: "Movement Speed".into(),
                    value_text: None,
                    source_text: "+12% Movement Speed".into(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    matched_by_rule: false,
                }],
                affix_row_geometry: Vec::new(),
                non_affix_rows: Vec::new(),
                tooltip_window_relative: None,
            },
        );
        state.rebuild_overlay_descriptor_state();
        let marker = state
            .overlay_descriptor_state
            .as_ref()
            .and_then(|descriptor| descriptor.rule.as_ref())
            .and_then(|rule| rule.result.trade_marker.clone());
        let card = app.get_current_item_type().to_string();
        (card, marker.is_some())
    });
    assert_eq!(
        no_match_marker.0, "Ring",
        "trade-items: the card must follow the latest parsed item"
    );
    assert!(
        !no_match_marker.1,
        "trade-items: a parsed item that matches no wanted row must show no marker"
    );

    // Restore the standard state so the following scenarios (and
    // the final screenshot) see the usual seed: the affix presets
    // are re-seeded, the trade rows are emptied, the parsed card
    // carries the trade-crud seed values again, and the app returns
    // to the tab-0 AffixScreen the rename-keyboard scenario after this one
    // expects.
    on_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state.trade_imports_mut().trade_items = Vec::new();
        state.trade_imports_mut().selected_trade_item_id = None;
        let current = state.current_item_mut();
        // The seed carries both the type identity the evaluation reads and
        // the display derivation the current-item card renders.
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
        state.rebuild_overlay_descriptor_state();
        drop(state);
        seed_standard_editor_presets(app, arc);
    });
    click_on_ui("Current Item");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        0,
        "trade-items: the cleanup must return to the AffixScreen tab"
    );
}

// ── add-dialog picker scenario ────────────────────────────

/// Flatten the staged AddTradeItem picker draft to Send-safe
/// `(affix_id, label, is_greater)` triples.
fn staged_affixes() -> Vec<(String, String, bool)> {
    read_ui(|app| {
        let model = app.get_trade_item_affixes();
        (0..model.row_count())
            .filter_map(|index| model.row_data(index))
            .map(|affix| {
                (
                    affix.affix_id.to_string(),
                    affix.label.to_string(),
                    affix.is_greater,
                )
            })
            .collect()
    })
}

/// Read one wanted row's affixes `(affix_id, is_greater)` from the
/// app state.
fn row_affixes(row_id: &'static str) -> Vec<(String, bool)> {
    read_ui(move |_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state
            .trade_imports()
            .trade_items
            .iter()
            .find(|row| row.trade_item_id == row_id)
            .map(|row| {
                row.affixes
                    .iter()
                    .map(|affix| (affix.affix_id.clone(), affix.is_greater))
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Real-input regression for the AddTradeItem picker wiring: the dialog's candidate / rune
/// models, the `;`-keyword filter box, and the add / IsGreater /
/// remove gestures reach the `d4lc_gui::trade` editing API through
/// the wired root callbacks.
///
/// 1. Add: a picker-selected candidate lands on the confirmed row
///    (staged draft → confirm → `add_trade_affix`).
/// 2. Strict types: an unknown item type keeps the dialog open with
///    the supported-types error (upstream only ever saves one of
///    the 11 fixed ComboBox types, InitItemTypes).
/// 3. Edit: the IsGreater checkbox persists through the confirm;
///    typing a rune type clears the staged draft (upstream
///    `SelectedItemType` setter) and switches to rune candidates
///    where the max-1 gate holds (upstream `CanAddAffixRuneExecute`).
pub(super) fn trade_item_editing_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[trade-items] starting add-dialog picker scenario");

    // Fresh-config proof for the ShowCurrentItem gate (upstream
    // default `true`, SettingsD4.cs:83): the suite runs against a
    // fresh config root, so the startup seed must project `true`
    // onto the gate.
    assert!(
        read_ui(|app| app.get_trade_show_current_item()),
        "trade-items: the ShowCurrentItem gate must seed true from a fresh config"
    );

    seed_trade_items();
    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);
    // With the gate on, the current-item card renders its content.
    assert_eq!(
        read_ui(|app| app.get_current_item_type().to_string()),
        "Boots",
        "trade-items: the current-item card must project while the gate is on"
    );

    // ── Leg 1: add via picker selection ────────────────────────────
    click_on_ui("Add trade item");
    assert!(
        dialog_state().0 && dialog_state().1 == "add_trade_item",
        "trade-items: Add trade item must open add_trade_item"
    );
    // The candidates project unfiltered from the vendored catalog.
    let full_count = read_ui(|app| app.get_trade_affix_candidates().row_count());
    assert!(
        full_count > 100,
        "trade-items: the vendored affix catalog must project as candidates: {full_count}"
    );

    let type_input = find_text_input(client, root, "e.g. Amulet, Two-Handed Sword");
    client
        .click_element(type_input, None)
        .expect("trade-items: click into the item type input");
    type_text(window, client, "amulet");
    let value_input = find_text_input(client, root, "e.g. 10M, 5b");
    client
        .click_element(value_input, None)
        .expect("trade-items: click into the value input");
    type_text(window, client, "7m");

    // The filter box narrows the candidate model through the real
    // key pipeline (upstream FilterAffixes on the `;`-keywords).
    let filter_input = find_text_input(client, root, "Enter filter");
    client
        .click_element(filter_input, None)
        .expect("trade-items: click into the affix filter input");
    type_text(window, client, "attack speed");
    let filtered_count = read_ui(|app| app.get_trade_affix_candidates().row_count());
    assert!(
        filtered_count > 0 && filtered_count < full_count,
        "trade-items: the filter must narrow the candidate list: {filtered_count} of {full_count}"
    );

    // Pick the first filtered candidate through its real add
    // button; the staged draft row appears with the resolved label.
    let (candidate_id, candidate_label) = read_ui(|app| {
        let model = app.get_trade_affix_candidates();
        let row = model.row_data(0).expect("trade-items: filtered candidate");
        (row.affix_id.to_string(), row.label.to_string())
    });
    click_on_label(&format!("Add affix {candidate_label}"));
    assert_eq!(
        staged_affixes(),
        vec![(candidate_id.clone(), candidate_label.clone(), false)],
        "trade-items: the picked candidate must appear on the staged draft"
    );

    click_on_ui("Confirm");
    assert!(
        !dialog_state().0,
        "trade-items: Confirm must close the dialog"
    );
    // The standard seed owns `Trade_Amulet` (capitalized type), so
    // the typed lowercase row synthesizes the base id `Trade_amulet`.
    assert_eq!(
        wanted_row("Trade_amulet"),
        Some(("amulet".to_string(), "7m".to_string(), 1)),
        "trade-items: the confirmed row must carry the picker-selected affix"
    );
    assert_eq!(
        row_affixes("Trade_amulet"),
        vec![(candidate_id, false)],
        "trade-items: the staged draft must reach the row through the ViewModel API"
    );

    // ── Leg 2: unknown types are rejected on the confirm path ──────
    click_on_ui("Add trade item");
    assert!(dialog_state().0, "trade-items: the add dialog must reopen");
    let type_input = find_text_input(client, root, "e.g. Amulet, Two-Handed Sword");
    client
        .set_element_value(type_input, "Belt")
        .expect("trade-items: type the unknown item type");
    let value_input = find_text_input(client, root, "e.g. 10M, 5b");
    client
        .set_element_value(value_input, "1b")
        .expect("trade-items: type the rejected draft value");
    click_on_ui("Confirm");
    assert!(
        dialog_state().0 && read_ui(|app| app.get_dialog_error_visible()),
        "trade-items: an unknown type must keep the dialog open with the supported-types error"
    );
    assert_eq!(
        wanted_row_count(),
        3,
        "trade-items: the rejected confirm must not change the wanted list"
    );
    click_on_ui("Cancel");
    assert!(
        !dialog_state().0,
        "trade-items: Cancel must close the dialog"
    );

    // ── Leg 3: edit — IsGreater toggle, rune boundary, max-1 ───────
    click_on_label("Edit amulet");
    assert!(
        dialog_state().0 && dialog_state().1 == "add_trade_item",
        "trade-items: Edit amulet must open add_trade_item"
    );
    assert!(
        read_ui(|app| app.get_trade_edit_mode()),
        "trade-items: the edit flow must seed edit mode"
    );
    let staged = staged_affixes();
    assert_eq!(
        staged.len(),
        1,
        "trade-items: the edit draft seeds from the row"
    );
    // The per-row checkbox is the only element carrying this label.
    click_on_label(&format!("Greater affix {}", staged[0].1));
    assert_eq!(
        staged_affixes(),
        vec![(staged[0].0.clone(), staged[0].1.clone(), true)],
        "trade-items: the IsGreater checkbox must flip the staged flag"
    );
    click_on_ui("Confirm");
    assert!(
        !dialog_state().0,
        "trade-items: Confirm must close the edit dialog"
    );
    assert_eq!(
        row_affixes("Trade_amulet"),
        vec![(staged[0].0.clone(), true)],
        "trade-items: the IsGreater flag must persist through the confirm"
    );

    // Crossing the rune boundary clears the draft (upstream
    // SelectedItemType setter) and switches to the rune candidates.
    click_on_label("Edit amulet");
    assert!(dialog_state().0, "trade-items: the edit dialog must reopen");
    let type_input = find_text_input(client, root, "e.g. Amulet, Two-Handed Sword");
    client
        .set_element_value(type_input, "rune")
        .expect("trade-items: retype the item type across the rune boundary");
    assert!(
        read_ui(|app| app.get_trade_item_type_is_rune()),
        "trade-items: the rune flag must follow the typed type"
    );
    assert!(
        staged_affixes().is_empty(),
        "trade-items: crossing the rune boundary must clear the staged draft"
    );
    let rune_count = read_ui(|app| app.get_trade_rune_candidates().row_count());
    assert!(
        rune_count > 0,
        "trade-items: the rune candidates must project for a rune type"
    );
    let (rune_id, rune_label) = read_ui(|app| {
        let model = app.get_trade_rune_candidates();
        let row = model.row_data(0).expect("trade-items: rune candidate");
        (row.affix_id.to_string(), row.label.to_string())
    });
    click_on_label(&format!("Add rune {rune_label}"));
    assert_eq!(
        staged_affixes().len(),
        1,
        "trade-items: the rune add must land exactly one draft row"
    );
    // The max-1 gate: a second rune add must not grow the draft
    // (the button is disabled and the host refuses a bypass).
    let (_, second_rune_label) = read_ui(|app| {
        let model = app.get_trade_rune_candidates();
        let row = model
            .row_data(1)
            .expect("trade-items: second rune candidate");
        (row.affix_id.to_string(), row.label.to_string())
    });
    click_on_label(&format!("Add rune {second_rune_label}"));
    assert_eq!(
        staged_affixes().len(),
        1,
        "trade-items: a rune item holds at most one affix"
    );
    click_on_ui("Confirm");
    assert!(
        !dialog_state().0,
        "trade-items: Confirm must close the rune edit"
    );
    assert_eq!(
        wanted_row("Trade_amulet"),
        Some(("rune".to_string(), "7m".to_string(), 1)),
        "trade-items: the crossed edit must keep the row id and value"
    );
    assert_eq!(
        row_affixes("Trade_amulet"),
        vec![(rune_id, false)],
        "trade-items: the rune edit lands the picked rune and drops the cleared affix"
    );

    // Restore the standard seed and the AffixScreen tab the
    // following scenarios expect.
    seed_trade_items();
    click_on_ui("Current Item");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        0,
        "trade-items: the picker scenario cleanup must return to the AffixScreen tab"
    );
}
