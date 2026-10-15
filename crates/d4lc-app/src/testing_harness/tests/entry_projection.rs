use super::*;

// ── Entry projection scenario (AppState -> AffixScreen models) ────

/// Evidence directory for the entry-projection scenario.
const PROJECTION_EVIDENCE_DIR: &str = "pilots/projection";

fn write_projection_screenshot(name: &str, png: &[u8]) {
    let dir = workspace_root()
        .join("debug-output")
        .join(PROJECTION_EVIDENCE_DIR);
    std::fs::create_dir_all(&dir).expect("create the evidence directory");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write projection screenshot");
    eprintln!("[gui-projection] screenshot written: {}", path.display());
}

fn projection_screenshot(client: &McpClient, window: McpHandle, name: &str) {
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic("projection screenshot", &error));
    assert_png_and_dimensions(&png);
    write_projection_screenshot(name, &png);
}

/// Prove the AppState -> AffixScreen entry projection end to end:
/// the seeded preset's entries render as rows/cards without any UI
/// create path, SetAffixColor applies through real clicks and
/// persists, and a real remove click unprojects the entry's cells.
pub(super) fn run_entry_projection_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[gui-projection] starting entry projection scenario");
    let mut click_throughs = 0_usize;
    let mismatches: Vec<String> = Vec::new();

    // The matrix leaves the window on the Settings tab; the
    // projection scenario runs on the AffixScreen (tab 0).
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    // Initial projection: rows (left list, labeled by the localized index
    // text the converter resolves the entry id to) and cards (edit
    // buttons) exist without any UI create path. The label-less fixture
    // entries the data does not carry (Gamma's "MaxLife", the mixed-kind
    // seeds) keep the label.unwrap_or(id) path covered.
    assert_label_findable(SEED_ATTACK_SPEED_TEXT);
    assert_label_findable(SEED_LIFE_TEXT);
    assert_label_findable(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
    projection_screenshot(client, window, "01-entry-rows-and-cards.png");

    // SetAffixColor end to end: swatch click opens the dialog
    // seeded from the entry, palette swatch selects the color,
    // Apply persists the entry and re-projects.
    click_on_ui(&format!("Set color: {SEED_LIFE_TEXT}"));
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "set_affix_color",
        "entry projection: the row color swatch must open set_affix_color \
         (saw visible={visible}, id={dialog_id:?})"
    );
    projection_screenshot(client, window, "02-set-affix-color-dialog.png");
    click_on_ui("Blue");
    click_on_ui("Apply");
    assert!(!dialog_state().0, "Apply must close set_affix_color");
    click_throughs += 1;

    // Persistence proof: reopening the color dialog re-seeds its
    // form from the projected entry, so the applied palette hex
    // must come back through a real click path.
    click_on_ui(&format!("Set color: {SEED_LIFE_TEXT}"));
    assert!(
        dialog_state().0 && dialog_state().1 == "set_affix_color",
        "entry projection: the color dialog must reopen for the persistence read-back"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_color_value().to_string()),
        "#0088FF",
        "entry projection: the applied color must persist into the entry and \
         re-seed the dialog form"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close set_affix_color");
    click_throughs += 1;

    // Entry removal through the real click path: the remove button
    // must drop the entry from the projection while the sibling
    // entry stays.
    click_on_ui(&format!("Remove {SEED_LIFE_TEXT}"));
    assert_eq!(
        label_count(&format!("Set color: {SEED_LIFE_TEXT}")),
        0,
        "entry projection: removing the entry must unproject its row/card cells"
    );
    assert_label_findable(&format!("Set color: {SEED_ATTACK_SPEED_TEXT}"));
    click_throughs += 1;

    // No overlap (proven last, after the removal steps): the
    // AffixEntryCard edit (+) button opens SetAffix seeded from its
    // entry; the remove (X) button must not cover its center, or a
    // real click deletes the entry instead.
    click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "set_affix",
        "entry projection: the card edit (+) click must open set_affix \
         (saw visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_entry_id().to_string()),
        "AttackSpeed",
        "entry projection: the SetAffix dialog must be seeded from the clicked entry"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_item_type().to_string()),
        "Amulet",
        "entry projection: the SetAffix dialog must seed the entry's item type"
    );
    projection_screenshot(client, window, "03-set-affix-edit-path.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close set_affix");
    click_throughs += 1;

    eprintln!(
        "[gui-projection] coverage: initial projection, set_affix_color apply + \
         persistence, entry removal, and the fixed edit (+) path proven by real clicks \
         ({click_throughs} click-throughs, {} expected-mismatch rows); evidence under \
         debug-output/{PROJECTION_EVIDENCE_DIR}/",
        mismatches.len()
    );
    for row in &mismatches {
        eprintln!("[gui-projection] expected-mismatch: {row}");
    }
}
