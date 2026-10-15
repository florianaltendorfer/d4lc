use super::*;

// ── Modal + keyboard edge scenarios (shared dialog host) ──────────

/// Evidence directory under the gitignored `debug-output/` for the
/// modal/keyboard edge-case family screenshots.
const MODAL_EVIDENCE_DIR: &str = "pilots/modal";

fn write_modal_screenshot(name: &str, png: &[u8]) {
    let dir = workspace_root()
        .join("debug-output")
        .join(MODAL_EVIDENCE_DIR);
    std::fs::create_dir_all(&dir).expect("create the evidence directory");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write modal screenshot");
    eprintln!("[gui-modal] screenshot written: {}", path.display());
}

fn modal_screenshot(client: &McpClient, window: McpHandle, name: &str) {
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic("modal screenshot", &error));
    assert_png_and_dimensions(&png);
    write_modal_screenshot(name, &png);
}

/// Find the new-preset-name TextInput while the SetPresetName dialog
/// is open. The MCP surface has no find-by-placeholder, so this
/// composes `query_element_descendants` + `get_element_properties`.
fn find_preset_name_input(client: &McpClient, root: McpHandle) -> McpHandle {
    let text_inputs = client
        .query_element_descendants(root, descendants_with_role("TextInput"), true)
        .unwrap_or_else(|error| harness_panic("query TextInput descendants", &error));
    text_inputs
        .into_iter()
        .find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessiblePlaceholderText")
                        == Some("e.g. Speed, Boss, AoE")
                })
        })
        .unwrap_or_else(|| {
            panic!("real-input GUI harness: preset-name TextInput (placeholder) not found")
        })
}

/// Modal + keyboard edge cases across the shared dialog host.
/// Runs inside the single-process pilot suite, right after the
/// coverage matrix (which leaves the Settings tab active and the
/// window at 1150x1200):
///
/// - Escape/Enter route through the dialog keyboard host, and the
///   verified upstream parity holds: no upstream dialog binds any
///   key (`actions::dialogs::dialog_key_action`), so routed keys
///   never confirm or cancel — asserted per key and per family
///   through `dialog-last-key` + the unchanged dialog state.
/// - Modal races: a double click on the dialog confirm button
///   applies exactly once; a click on a blank host area while a
///   dialog is open is consumed by the host pointer guard (an
///   underlying opener stays inert, upstream parity); a cancelled
///   draft does not survive a reopen; `dialog-error-visible`
///   clears on reopen.
pub(super) fn run_modal_keyboard_scenarios(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[gui-modal] starting modal/keyboard edge scenarios");

    // The overlay-config opener lives on the Settings tab. The scenario
    // establishes that itself instead of inheriting the tab an earlier
    // scenario happened to leave selected.
    click_on_ui("Settings");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        4,
        "modal scenarios: the dialog openers live on the Settings tab"
    );

    // ── Escape routes on a Settings-family dialog (upstream parity:
    //    ignored) ─────────────────────────────────────────────────
    click_on_ui("Open overlay config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "overlay_config",
        "modal scenarios: Open overlay config must open overlay_config"
    );
    client
        .dispatch_key_event(window, "\u{1b}")
        .expect("dispatch Escape on overlay_config");
    assert_eq!(
        read_ui(|app| app.get_dialog_last_key().to_string()),
        "\u{1b}",
        "the keyboard host must route Escape on the settings-family dialog"
    );
    assert!(
        dialog_state().0,
        "upstream parity: Escape must not close overlay_config"
    );
    modal_screenshot(client, window, "escape-overlay-config-ignored.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close overlay_config");

    // ── Escape routes on a build-import-family dialog; single host
    //    instance ─────────────────────────────────────────────────
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "import_preset");
    // The tabbed import dialog closes through its Done button
    // (upstream parity: no Cancel); the Done button must still be
    // hosted exactly once while the keyboard host is live. The
    // label matches twice like the Cancel rows (button + Text
    // child), pinning the single-instance hosting.
    let dones = read_ui(|app| ElementHandle::find_by_accessible_label(app, "Done").count());
    assert_eq!(
        dones, CANCEL_MATCHES_PER_INSTANCE,
        "import_preset must host exactly one Done button while its keyboard host is live"
    );
    client
        .dispatch_key_event(window, "\u{1b}")
        .expect("dispatch Escape on import_preset");
    assert_eq!(
        read_ui(|app| app.get_dialog_last_key().to_string()),
        "\u{1b}",
        "the keyboard host must route Escape on the build-import-family dialog"
    );
    assert!(
        dialog_state().0,
        "upstream parity: Escape must not close import_preset"
    );
    assert!(
        !read_ui(|app| app.get_dialog_error_visible()),
        "a routed (ignored) Escape must not surface a dialog error"
    );
    modal_screenshot(client, window, "escape-import-preset-ignored.png");
    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close import_preset");
    client
        .dispatch_key_event(window, "\u{1b}")
        .expect("dispatch Escape with no dialog open");
    assert!(
        !dialog_state().0,
        "Escape outside a dialog must stay a no-op"
    );

    // ── Enter does not confirm (upstream parity): a typed name must
    //    not be applied by pressing Enter ────────────────────────────
    seed_affix_presets();
    click_on_ui("Current Item");
    click_on_ui("Rename preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "rename_preset");
    let names_before = preset_names();
    let preset_input = find_preset_name_input(client, root);
    client
        .set_element_value(preset_input, "EnterMustNotApply")
        .expect("type the Enter-parity preset name");
    client
        .dispatch_key_event(window, "\u{000a}")
        .expect("dispatch Enter on rename_preset");
    assert_eq!(
        read_ui(|app| app.get_dialog_last_key().to_string()),
        "\u{000a}",
        "the keyboard host must route Enter on the open dialog"
    );
    assert!(
        dialog_state().0,
        "upstream parity: Enter must not confirm rename_preset"
    );
    assert_eq!(
        preset_names(),
        names_before,
        "upstream parity: Enter must not rename the preset: {:?}",
        preset_names()
    );
    modal_screenshot(client, window, "enter-rename-preset-ignored.png");
    click_on_ui("Cancel");
    assert!(
        !dialog_state().0 && preset_names() == names_before,
        "Cancel must close without applying the Enter-parity draft"
    );

    // ── Escape reaches the host even while a dialog input has focus:
    //    the host wraps the dialog box, so the key the TextInput
    //    rejects bubbles up to the same route (the typed-then-Escape
    //    flow a user actually performs). ────────────────────────────
    click_on_ui("Rename preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "rename_preset");
    let preset_input = find_preset_name_input(client, root);
    client
        .click_element(preset_input, None)
        .expect("click into the rename input (moves focus off the host)");
    client
        .dispatch_key_event(window, "x")
        .expect("type into the focused input");
    // The rename field is pre-filled with the current preset name, so
    // the typed character appends to the seeded text.
    let seeded_name = read_ui(|app| app.get_rename_preset_current_name().to_string());
    assert_eq!(
        read_ui(|app| app.get_rename_preset_new_name().to_string()),
        format!("{seeded_name}x"),
        "the focused input must receive the typed character first"
    );
    client
        .dispatch_key_event(window, "\u{1b}")
        .expect("dispatch Escape with the dialog input focused");
    assert_eq!(
        read_ui(|app| app.get_dialog_last_key().to_string()),
        "\u{1b}",
        "Escape must bubble from the focused input to the keyboard host"
    );
    assert!(
        dialog_state().0,
        "upstream parity: Escape must not close rename_preset"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0);

    // ── The generic confirm prompt: the one recorded
    //    keyboard exception in the router's key table. Upstream's
    //    MahApps `ShowMessageAsync` message dialog answers Yes on Enter
    //    (its default button) and No on Escape; the host guard blocks
    //    the window content behind the prompt like every other modal.
    seed_affix_presets();
    click_on_ui("Current Item");
    click_on_ui("Remove preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "the prompt must open for the seeded selection (observed: visible={visible}, \
         id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_dialog_message().to_string()),
        "Are you sure you want to delete preset \"Alpha\"",
        "the prompt must carry the upstream copy for the selected preset"
    );
    modal_screenshot(client, window, "confirm-prompt-delete.png");

    // The backdrop guard: an underlying opener behind the prompt is
    // consumed by the host TouchArea and must not stack dialog B.
    click_on_ui("Rename preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "a click on an underlying opener behind the prompt must be consumed by the host \
         guard (observed: visible={visible}, id={dialog_id:?})"
    );

    // Escape answers No: the prompt closes, the pending action is
    // dropped and the preset survives.
    client
        .dispatch_key_event(window, "\u{1b}")
        .expect("dispatch Escape on the confirm prompt");
    assert_eq!(
        read_ui(|app| app.get_dialog_last_key().to_string()),
        "\u{1b}",
        "the keyboard host must route Escape on the prompt"
    );
    assert!(
        !dialog_state().0,
        "Escape must answer No and close the prompt"
    );
    assert!(
        preset_names().contains(&"Alpha".to_string()),
        "No must keep the preset: {:?}",
        preset_names()
    );

    // Enter answers Yes: the prompt closes and the pending delete runs.
    click_on_ui("Remove preset");
    client
        .dispatch_key_event(window, "\u{000a}")
        .expect("dispatch Enter on the confirm prompt");
    assert_eq!(
        read_ui(|app| app.get_dialog_last_key().to_string()),
        "\u{000a}",
        "the keyboard host must route Enter on the prompt"
    );
    assert!(
        !dialog_state().0,
        "Enter must answer Yes and close the prompt"
    );
    assert!(
        !preset_names().contains(&"Alpha".to_string()),
        "Yes must delete the preset: {:?}",
        preset_names()
    );
    modal_screenshot(client, window, "confirm-prompt-answered.png");

    // Restore the standard seed (in memory and on disk) for the
    // scenarios after this one.
    restore_standard_presets();
    click_on_ui("Current Item");

    // ── Modal race: double click on the dialog confirm button. The
    //    first Apply click renames the preset and closes the dialog;
    //    the second click of the gesture lands after the host is
    //    gone and must not apply anything again. ───────────────────
    click_on_ui("Rename preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "rename_preset");
    let preset_input = find_preset_name_input(client, root);
    client
        .set_element_value(preset_input, "RacePreset")
        .expect("type the double-apply preset name");
    let apply = find_button_by_label(client, root, "Apply");
    client
        .click_element(apply, Some("DoubleClick"))
        .expect("double click the dialog Apply button");
    assert_eq!(
        preset_names()
            .iter()
            .filter(|name| name.as_str() == "RacePreset")
            .count(),
        1,
        "the double click must apply exactly once: {:?}",
        preset_names()
    );
    assert!(
        !dialog_state().0,
        "the first Apply click must close the dialog"
    );
    modal_screenshot(client, window, "race-double-click-apply.png");

    // ── Modal pointer guard: a click on a blank host area must be
    //    consumed by the host TouchArea, not fall through to the
    //    screen behind the modal. Clicking an underlying opener (the
    //    website row's Import button) while a dialog is open targets a
    //    point carrying no dialog TouchArea, so the guard is the
    //    topmost TouchArea there: the opener's dialog-open handler must
    //    not fire, and dialog B must not replace A (upstream MahApps
    //    dialogs block underlying clicks). ──────────────────────────
    select_website(client, root, "D2Core.com");
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "import_preset");
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "import_preset",
        "a click on an underlying opener behind the modal must be consumed by the \
         host guard (observed: visible={visible}, id={dialog_id:?})"
    );
    let dones = read_ui(|app| ElementHandle::find_by_accessible_label(app, "Done").count());
    assert_eq!(
        dones, CANCEL_MATCHES_PER_INSTANCE,
        "the blocked click must not stack dialog B into the single host"
    );
    // Real dialog controls keep working above the guard.
    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close import_preset");
    // The blocked opener keeps working once no modal is above it —
    // Import re-opens the tabbed dialog at the selected provider's tab.
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "import_preset",
        "the same opener must still open its dialog without the modal above it \
         (observed: visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        2,
        "the D2Core.com selection must land on the D2Core tab"
    );
    modal_screenshot(client, window, "modal-guard-blocks-underlying-opener.png");
    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close import_preset");

    // ── Reopen semantics: a cancelled draft does not survive the
    //    reopen (upstream constructs a fresh view model per open).
    click_on_ui("Rename preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "rename_preset");
    let seeded_name = read_ui(|app| app.get_rename_preset_current_name().to_string());
    let preset_input = find_preset_name_input(client, root);
    client
        .set_element_value(preset_input, "GhostDraft")
        .expect("type the ghost draft");
    assert_eq!(
        read_ui(|app| app.get_rename_preset_new_name().to_string()),
        "GhostDraft",
        "the draft must reach the dialog's root form property"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0);
    click_on_ui("Rename preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "rename_preset");
    assert_eq!(
        read_ui(|app| app.get_rename_preset_new_name().to_string()),
        seeded_name,
        "reopen must reset the cancelled draft (upstream re-seeds per open)"
    );
    modal_screenshot(client, window, "reopen-resets-draft.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0);

    // ── Reopen semantics: dialog-error-visible clears on reopen ──
    // The deterministic offline error path: Update on a stored
    // Mobalytics row fails (the offline legacy fixture rows
    // carry no stored Url to re-download) and surfaces the shared
    // dialog error — no browser, no network.
    select_website(client, root, "Mobalytics.gg");
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "import_preset");
    assert_eq!(read_ui(|app| app.get_import_tab_index()), 6);
    click_on_ui("Upd");
    assert!(
        read_ui(|app| app.get_dialog_error_visible()),
        "the Mobalytics update gate must surface the dialog error"
    );
    click_on_ui("Done");
    assert!(!dialog_state().0);
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible && dialog_id == "import_preset");
    assert!(
        !read_ui(|app| app.get_dialog_error_visible())
            && read_ui(|app| app.get_dialog_error().to_string()).is_empty(),
        "reopen must clear the dialog error and its visible flag"
    );
    modal_screenshot(client, window, "reopen-clears-error.png");
    click_on_ui("Done");
    assert!(!dialog_state().0, "cleanup: every dialog must be closed");

    eprintln!("[gui-modal] modal/keyboard edge scenarios complete");
}
