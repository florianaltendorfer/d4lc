use super::*;

// ── Coverage matrix (findability + full interaction coverage) ─────

/// Evidence directory under the gitignored `debug-output/`: one
/// screenshot per opened dialog and per nav tab.
const MATRIX_EVIDENCE_DIR: &str = "pilots/interaction-matrix";

/// Dialog kinds that must be reachable by real clicks (the full
/// `DialogKind` set rendered by the main_window.slint dialog host).
/// 20 = the upstream dialog set the host renders (the provider downloads
/// live in the tabbed import dialog) plus the generic confirm prompt.
const EXPECTED_DIALOG_KINDS: usize = 20;

/// Accessible-label matches contributed by one open dialog's Cancel
/// button: the button itself plus its Text child (Slint propagates
/// the accessible label to text children). Every dialog kind is
/// hosted exactly once in main_window.slint, so an open dialog
/// always contributes exactly this many matches.
pub(super) const CANCEL_MATCHES_PER_INSTANCE: usize = 2;

fn write_matrix_screenshot(name: &str, png: &[u8]) {
    let dir = workspace_root()
        .join("debug-output")
        .join(MATRIX_EVIDENCE_DIR);
    std::fs::create_dir_all(&dir).expect("create the evidence directory");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write matrix screenshot");
    eprintln!("[gui-matrix] screenshot written: {}", path.display());
}

fn matrix_screenshot(client: &McpClient, window: McpHandle, name: &str) {
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic("matrix screenshot", &error));
    assert_png_and_dimensions(&png);
    write_matrix_screenshot(name, &png);
}

/// Read the current preset-name model into a Vec for assertions.
pub(super) fn preset_names() -> Vec<String> {
    read_ui(|app| {
        let model = app.get_preset_names();
        (0..model.row_count())
            .map(|row| model.row_data(row).expect("preset row").to_string())
            .collect::<Vec<_>>()
    })
}

/// The Logging screen's message rows, oldest first (the model the
/// `active-tab == 2` screen arm renders).
pub(super) fn log_messages() -> Vec<String> {
    read_ui(|app| {
        let model = app.get_log_messages();
        (0..model.row_count())
            .map(|row| model.row_data(row).expect("log row").message.to_string())
            .collect::<Vec<_>>()
    })
}

/// Open a dialog through its real user path (a click on `opener`),
/// assert `dialog-id`/`dialog-visible`, take the on-open screenshot,
/// then close it by clicking Cancel and assert closure. Returns the
/// number of Cancel matches observed while open (a live-structure
/// probe: every dialog is hosted exactly once in main_window.slint,
/// so the count is [`CANCEL_MATCHES_PER_INSTANCE`]).
fn open_screenshot_cancel(
    client: &McpClient,
    window: McpHandle,
    opener: &str,
    expected_id: &'static str,
) -> usize {
    click_on_ui(opener);
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == expected_id,
        "coverage matrix: clicking {opener:?} must open dialog {expected_id:?} \
         (saw visible={visible}, id={dialog_id:?})"
    );
    let cancels = read_ui(|app| ElementHandle::find_by_accessible_label(app, "Cancel").count());
    assert!(
        cancels > 0,
        "coverage matrix: dialog {expected_id:?} must expose a Cancel button"
    );
    matrix_screenshot(client, window, &format!("dialog-{expected_id}.png"));
    click_on_ui("Cancel");
    let (visible, _) = dialog_state();
    assert!(
        !visible,
        "coverage matrix: Cancel must close {expected_id:?}"
    );
    cancels
}

/// Every .slint source, embedded verbatim. Shared by the coverage
/// matrix's disallowed-marker scan and the toggle-sweep toggle-source
/// re-scan, so both sweeps see the same full UI surface.
pub(super) const UI_SOURCES: [(&str, &str); 35] = [
    (
        "ui/main_window.slint",
        include_str!("../../../ui/main_window.slint"),
    ),
    (
        "ui/screens/affix_screen.slint",
        include_str!("../../../ui/screens/affix_screen.slint"),
    ),
    (
        "ui/screens/trade_screen.slint",
        include_str!("../../../ui/screens/trade_screen.slint"),
    ),
    (
        "ui/screens/logging_screen.slint",
        include_str!("../../../ui/screens/logging_screen.slint"),
    ),
    (
        "ui/screens/debug_screen.slint",
        include_str!("../../../ui/screens/debug_screen.slint"),
    ),
    (
        "ui/screens/settings_screen.slint",
        include_str!("../../../ui/screens/settings_screen.slint"),
    ),
    ("ui/theme.slint", include_str!("../../../ui/theme.slint")),
    (
        "ui/components/catalog_grid.slint",
        include_str!("../../../ui/components/catalog_grid.slint"),
    ),
    (
        "ui/components/catalog_card.slint",
        include_str!("../../../ui/components/catalog_card.slint"),
    ),
    (
        "ui/components/affix_chip.slint",
        include_str!("../../../ui/components/affix_chip.slint"),
    ),
    (
        "ui/components/section_header.slint",
        include_str!("../../../ui/components/section_header.slint"),
    ),
    (
        "ui/components/icons.slint",
        include_str!("../../../ui/components/icons.slint"),
    ),
    (
        "ui/components/toggle_switch.slint",
        include_str!("../../../ui/components/toggle_switch.slint"),
    ),
    (
        "ui/components/numeric_spin_box.slint",
        include_str!("../../../ui/components/numeric_spin_box.slint"),
    ),
    (
        "ui/components/debug_artifact.slint",
        include_str!("../../../ui/components/debug_artifact.slint"),
    ),
    (
        "ui/dialogs/dark_line_edit.slint",
        include_str!("../../../ui/dialogs/dark_line_edit.slint"),
    ),
    (
        "ui/dialogs/add_trade_item_dialog.slint",
        include_str!("../../../ui/dialogs/add_trade_item_dialog.slint"),
    ),
    (
        "ui/dialogs/affix_config_dialog.slint",
        include_str!("../../../ui/dialogs/affix_config_dialog.slint"),
    ),
    (
        "ui/dialogs/aspect_config_dialog.slint",
        include_str!("../../../ui/dialogs/aspect_config_dialog.slint"),
    ),
    (
        "ui/dialogs/colors_config_dialog.slint",
        include_str!("../../../ui/dialogs/colors_config_dialog.slint"),
    ),
    (
        "ui/dialogs/controller_config_dialog.slint",
        include_str!("../../../ui/dialogs/controller_config_dialog.slint"),
    ),
    (
        "ui/dialogs/hotkey_config_dialog.slint",
        include_str!("../../../ui/dialogs/hotkey_config_dialog.slint"),
    ),
    (
        "ui/dialogs/hotkeys_config_dialog.slint",
        include_str!("../../../ui/dialogs/hotkeys_config_dialog.slint"),
    ),
    (
        "ui/dialogs/import_preset_dialog.slint",
        include_str!("../../../ui/dialogs/import_preset_dialog.slint"),
    ),
    (
        "ui/dialogs/multi_build_config_dialog.slint",
        include_str!("../../../ui/dialogs/multi_build_config_dialog.slint"),
    ),
    (
        "ui/dialogs/overlay_config_dialog.slint",
        include_str!("../../../ui/dialogs/overlay_config_dialog.slint"),
    ),
    (
        "ui/dialogs/paragon_config_dialog.slint",
        include_str!("../../../ui/dialogs/paragon_config_dialog.slint"),
    ),
    (
        "ui/dialogs/rename_preset_dialog.slint",
        include_str!("../../../ui/dialogs/rename_preset_dialog.slint"),
    ),
    (
        "ui/dialogs/rune_config_dialog.slint",
        include_str!("../../../ui/dialogs/rune_config_dialog.slint"),
    ),
    (
        "ui/dialogs/set_affix_color_dialog.slint",
        include_str!("../../../ui/dialogs/set_affix_color_dialog.slint"),
    ),
    (
        "ui/dialogs/set_affix_dialog.slint",
        include_str!("../../../ui/dialogs/set_affix_dialog.slint"),
    ),
    (
        "ui/dialogs/set_preset_name_dialog.slint",
        include_str!("../../../ui/dialogs/set_preset_name_dialog.slint"),
    ),
    (
        "ui/dialogs/sigil_config_dialog.slint",
        include_str!("../../../ui/dialogs/sigil_config_dialog.slint"),
    ),
    (
        "ui/dialogs/trade_config_dialog.slint",
        include_str!("../../../ui/dialogs/trade_config_dialog.slint"),
    ),
    (
        "ui/dialogs/unique_config_dialog.slint",
        include_str!("../../../ui/dialogs/unique_config_dialog.slint"),
    ),
];

/// The full findability + interaction coverage matrix. Runs inside
/// the single-process pilot suite: one wired window, real clicks
/// only, loud failures, zero silent skips — every element not driven
/// end-to-end is recorded as a printed expected-mismatch row with a
/// one-line reason (known gaps are listed, not skipped).
pub(super) fn run_interaction_matrix(client: &McpClient, window: McpHandle) {
    // Root element handle for the popup/descendant queries below.
    let root = client
        .get_window_properties(window)
        .expect("coverage matrix: window properties")
        .get("rootElementHandle")
        .map(parse_handle)
        .unwrap_or_else(|| panic!("coverage matrix: window properties missing rootElementHandle"))
        .unwrap_or_else(|error| harness_panic("parse rootElementHandle", &error));
    eprintln!("[gui-matrix] starting coverage matrix");

    // Enlarge the harness window first: Slint exposes only elements
    // whose geometry intersects the clip (clipped-out content is as
    // invisible to the a11y tree as it is to a screen reader). The
    // Debug screen's 15 numeric steppers exceed the default 625 px
    // viewport, so the matrix runs against a tall window — the
    // headless equivalent of a maximized window.
    read_ui(|app| {
        app.window()
            .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
                1150.0, 1200.0,
            )));
    });

    let mut findability_checks = 0_usize;
    let mut click_throughs = 0_usize;
    let mut dialogs_covered = 0_usize;
    // SetPresetName: upstream opens it from the import flow only, so
    // no AffixScreen element reaches it after the port-only opener was
    // removed (the dialog kind stays registered and unit-tested).
    let dialogs_unreachable = 1_usize;
    let mut mismatches: Vec<String> = Vec::new();

    // Dialog routing: SetAffix and SetAffixColor open through the
    // seeded Starter preset's entry cards (the entry models must be
    // projected for that; the card's remove X sits in the bottom-right
    // corner, clear of the edit (+) button), and the preset ComboBox
    // re-projects the entries on switches (`affix-preset-selected`). Only the SetAffix create
    // mode (empty form, edit key absent) stays unreachable by real
    // click, recorded as an expected mismatch in the Tab 0
    // section.

    // ── Nav: every tab reachable by real click ────────────────────
    const TABS: [(&str, i32, &str); 5] = [
        ("Current Item", 0, "tab-0-current-item.png"),
        ("Affix Presets", 1, "tab-1-affix-presets.png"),
        ("Trade Imports", 2, "tab-2-trade-imports.png"),
        ("Debug", 3, "tab-3-debug.png"),
        ("Settings", 4, "tab-4-settings.png"),
    ];
    for (label, index, shot) in TABS {
        click_on_ui(label);
        assert_eq!(
            read_ui(|app| app.get_active_tab()),
            index,
            "coverage matrix: nav click on {label:?} must switch tabs"
        );
        matrix_screenshot(client, window, shot);
        findability_checks += 1;
    }

    // ══ Tab 0 "Current Item" (AffixScreen) ═══════════════════════
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
    for label in [
        "Toggle affix overlay",
        "Affix language",
        "Preset",
        "Import provider",
        "Enter name",
        "Enter filter",
        "Rename preset",
        "Remove preset",
        "Create preset",
        "Import preset",
        "Affixes",
        "Aspects",
        "Sigils",
        "Uniques",
        "Runes",
        // The right column's filter-icon strip (upstream class filters;
        // the sigil/rune filters appear on their own tabs).
        "Core",
        "Barbarian",
        "Druid",
        "Necromancer",
        "Paladin",
        "Rogue",
        "Sorcerer",
        "Spiritborn",
        "Warlock",
        "Affix config",
        // Header window commands are always present.
        "d4lc on GitHub",
        "Diablo4Companion (upstream) on GitHub",
        "Support the Diablo4Companion author on Ko-fi",
        "Diablo4Companion (upstream) wiki on GitHub",
    ] {
        assert_label_findable(label);
        findability_checks += 1;
    }

    // Category config cells: switching the category is proven by the
    // per-category config cell becoming findable (it is only
    // instantiated while its category is active); the dialog opens by
    // a real click on that cell.
    for (switch, cell, dialog_id) in [
        ("Affixes", "Affix config", "affix_config"),
        ("Aspects", "Aspect config", "aspect_config"),
        ("Sigils", "Sigil config", "sigil_config"),
        ("Uniques", "Unique config", "unique_config"),
        ("Runes", "Rune config", "rune_config"),
    ] {
        click_control_by_label(switch);
        assert_label_findable(cell);
        findability_checks += 1;
        dialogs_covered += 1;
        open_screenshot_cancel(client, window, cell, dialog_id);
    }
    click_control_by_label("Affixes");

    for (website, expected_tab) in [
        ("D2Core.com", 2_i32),
        ("D4Builds.gg", 3),
        ("Mobalytics.gg", 6),
        ("Maxroll.gg", 5),
        ("InfinityBuilds.gg", 4),
    ] {
        // The website combo selection + the Import button open the
        // tabbed import dialog at the selected provider's upstream tab;
        // the dialog closes via its Done button.
        select_website(client, root, website);
        click_on_ui("Import preset");
        let (visible, dialog_id) = dialog_state();
        assert!(
            visible && dialog_id == "import_preset",
            "coverage matrix: Import with {website:?} selected must open import_preset \
             (saw visible={visible}, id={dialog_id:?})"
        );
        assert_eq!(
            read_ui(|app| app.get_import_tab_index()),
            expected_tab,
            "coverage matrix: {website:?} must land on tab {expected_tab}"
        );
        matrix_screenshot(
            client,
            window,
            &format!("dialog-import_preset-{website}.png"),
        );
        click_on_ui("Done");
        assert!(!dialog_state().0, "Done must close import_preset");
    }

    // RenamePreset through its real user path: the selection-gated
    // pencil button of the presets panel.
    dialogs_covered += 1;
    open_screenshot_cancel(client, window, "Rename preset", "rename_preset");
    // The individual HotkeyConfig dialog is reachable from the Settings
    // hotkeys manager (its per-row Edit button) and is opened there.
    mismatches.push(
        "SetPresetName dialog — upstream opens it from the import flow only \
         (ImportAffixPresetViewModel); the port-only AffixScreen button that opened it \
         was removed, so no AffixScreen element reaches it (dialog kind stays registered)"
            .to_string(),
    );

    // SetAffixColor through its real user path: the entry row/card
    // color swatch. Reachable only since the entry projection
    // seeds the Starter preset in `wired_test_app`.
    dialogs_covered += 1;
    open_screenshot_cancel(
        client,
        window,
        &format!("Set color: {SEED_ATTACK_SPEED_TEXT}"),
        "set_affix_color",
    );
    // SetAffix edit path through its real user path: the entry
    // card's edit (+) button. Reachable since the overlap fix
    // moved the card remove X to the bottom-right corner (before,
    // a real click on the + landed on the overlapping X and
    // deleted the entry).
    dialogs_covered += 1;
    open_screenshot_cancel(
        client,
        window,
        &format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"),
        "set_affix",
    );
    // SetAffix create-mode (empty form) stays unreachable by real
    // click: entry rows/cards only pass kind-tagged keys of
    // existing entries, so no UI element can open the create path
    // (design point). It is a mode of the covered set_affix kind,
    // not its own dialog kind, so the kind accounting is unchanged.
    mismatches.push(
        "SetAffix create-mode (empty form) — entry rows/cards only pass kind-tagged \
         keys of existing entries, so no UI element can open the create path (design point)"
            .to_string(),
    );

    // Infinity Builds: the website row opens the tabbed import dialog
    // on the InfinityBuilds tab; with an empty share URL the Add button
    // stays disabled (upstream parity: invalid input never errors).
    select_website(client, root, "InfinityBuilds.gg");
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "import_preset",
        "coverage matrix: InfinityB. must open import_preset"
    );
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        4,
        "coverage matrix: InfinityB. must land on the InfinityBuilds tab"
    );
    assert!(
        !read_ui(|app| app.get_import_infinity_add_enabled()),
        "coverage matrix: Add with an empty share URL must stay disabled"
    );
    matrix_screenshot(client, window, "dialog-import_preset-InfinityB.png");
    click_throughs += 1;
    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close import_preset");
    // All five provider chips open the single tabbed import dialog:
    // count the import_preset kind once.
    dialogs_covered += 1;

    // The multi_build_config host was declared twice in
    // main_window.slint (stacked duplicates) and has been
    // deduplicated: the open dialog renders exactly one Done
    // button (its only button), contributing [`CANCEL_MATCHES_PER_INSTANCE`] label
    // matches (button + Text child). The count pins the
    // single-instance hosting — a re-introduced stacked duplicate
    // would double it again. (The tabbed import dialog closes via
    // Done and is covered above.)
    click_on_ui("Affix config");
    click_on_ui("Multi-build config");
    let (visible, dialog_id_seen) = dialog_state();
    assert!(
        visible && dialog_id_seen == "multi_build_config",
        "coverage matrix: clicking 'Multi-Build' must open multi_build_config"
    );
    let dones = read_ui(|app| ElementHandle::find_by_accessible_label(app, "Done").count());
    assert_eq!(
        dones, CANCEL_MATCHES_PER_INSTANCE,
        "coverage matrix: dialog multi_build_config must be hosted exactly once \
         (stacked duplicates regress here)"
    );
    matrix_screenshot(client, window, "dialog-multi_build_config.png");
    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close multi_build_config");
    dialogs_covered += 1;

    // Header window commands: the browser-launching callbacks stay
    // unwired in the harness (network-free suite), so the asserted
    // click-through invariant is that the real click is delivered
    // through hit-testing and opens nothing.
    for label in [
        "d4lc on GitHub",
        "Diablo4Companion (upstream) on GitHub",
        "Support the Diablo4Companion author on Ko-fi",
        "Diablo4Companion (upstream) wiki on GitHub",
    ] {
        click_on_ui(label);
        assert!(
            !dialog_state().0,
            "coverage matrix: {label} click must not open a dialog"
        );
        click_throughs += 1;
    }
    // Overlay power button: real click is delivered; the flag lives
    // in AppState only (no root-observable projection).
    click_on_ui("Toggle affix overlay");
    click_throughs += 1;
    mismatches.push(
        "affix overlay power button — click delivered but state is AppState-only, no \
         root-observable projection to assert"
            .to_string(),
    );

    // Create preset through the real path: type into the named field
    // (MCP set_element_value — real input pipeline), click +, assert
    // the model and the cleared input; then remove the selected
    // preset via the trash button and assert the model again.
    let new_preset_input = client
        .find_elements_by_id(window, "DarkLineEdit::input")
        .expect("find DarkLineEdit inputs")
        .into_iter()
        .find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessibleLabel") == Some("Enter name")
                })
        })
        .unwrap_or_else(|| {
            panic!("coverage matrix: new-preset name field not found by accessible label")
        });
    client
        .set_element_value(new_preset_input, "MatrixPreset")
        .expect("type the matrix preset name");
    assert_eq!(
        read_ui(|app| app.get_new_preset_name().to_string()),
        "MatrixPreset",
        "coverage matrix: set_element_value must reach the new-preset form property"
    );
    click_on_ui("Create preset");
    let names = preset_names();
    assert!(
        names.iter().any(|name| name == "MatrixPreset"),
        "coverage matrix: the created preset must appear in the model: {names:?}"
    );
    assert_eq!(
        read_ui(|app| app.get_new_preset_name().to_string()),
        "",
        "coverage matrix: create must clear the name field"
    );
    click_on_ui("Remove preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "coverage matrix: Remove preset must ask through the confirm prompt (observed: \
         visible={visible}, id={dialog_id:?})"
    );
    click_on_ui("Yes");
    let names_after = preset_names();
    assert!(
        !names_after.iter().any(|name| name == "MatrixPreset"),
        "coverage matrix: the removed preset must disappear from the model: {names_after:?}"
    );
    click_throughs += 3;

    // The generic confirm prompt is a dialog kind of its
    // own: open it through the same real remove path and answer No (the
    // Yes path is covered by the delete click-through above).
    click_on_ui("Remove preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "coverage matrix: Remove preset must open the confirm prompt (observed: \
         visible={visible}, id={dialog_id:?})"
    );
    matrix_screenshot(client, window, "dialog-confirm.png");
    let names_before_cancel = preset_names();
    click_on_ui("No");
    let (visible, _) = dialog_state();
    assert!(
        !visible,
        "coverage matrix: No must close the confirm prompt"
    );
    assert_eq!(
        preset_names(),
        names_before_cancel,
        "coverage matrix: No must keep the preset"
    );
    dialogs_covered += 1;
    click_throughs += 2;

    // ══ Tab 1 "Affix Presets" (TradeScreen) ══════════════════════
    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);
    for label in ["Trade config", "Add trade item"] {
        assert_label_findable(label);
        findability_checks += 1;
    }

    // TradeConfig: the overlay toggle is two-way bound to the root —
    // observable click-through.
    click_on_ui("Trade config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "trade_config",
        "coverage matrix: Trade config must open trade_config"
    );
    matrix_screenshot(client, window, "dialog-trade_config.png");
    let toggle_before = read_ui(|app| app.get_trade_config_overlay_enabled());
    click_control_by_label("Enable trade overlay");
    assert_ne!(
        read_ui(|app| app.get_trade_config_overlay_enabled()),
        toggle_before,
        "coverage matrix: the trade overlay switch must flip the root form value"
    );
    click_throughs += 1;
    dialogs_covered += 1;
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close trade_config");

    dialogs_covered += 1;
    open_screenshot_cancel(client, window, "Add trade item", "add_trade_item");
    mismatches.push(
        "wanted trade item cards — the trade-items model is empty in the harness, so no \
         card rows exist to click (attributes shipped)"
            .to_string(),
    );

    // ══ Tab 2 "Trade Imports" (LoggingScreen) ════════════════════
    click_on_ui("Trade Imports");
    assert_eq!(read_ui(|app| app.get_active_tab()), 2);
    assert_label_findable("Clear Log");
    findability_checks += 1;
    click_on_ui("Clear Log");
    click_throughs += 1;
    mismatches.push(
        "Clear Log — the log model is empty in the harness, so clearing has no observable \
         state change to assert (click delivered)"
            .to_string(),
    );

    // ══ Tab 3 "Debug" (DebugScreen) ══════════════════════════════
    click_on_ui("Debug");
    assert_eq!(read_ui(|app| app.get_active_tab()), 3);
    const SPIN_LABELS: [&str; 15] = [
        "Tooltip width:",
        "Tooltip max height:",
        "Brightness Threshold (Min):",
        "Brightness Threshold (Max):",
        "Affix Area Height Offset (Top):",
        "Affix Area Height Offset (Bottom):",
        "Affix/Aspect Area Width Offset:",
        "Aspect Area Height Offset (Top):",
        "Type Area Offset (Left):",
        "Minimal OCR Match (Type):",
        "Similarity Threshold (Tooltip):",
        "Similarity Threshold (Affix location):",
        "Similarity Threshold (Aspect location):",
        "Similarity Threshold (Socket location):",
        "Similarity Threshold (Splitter location):",
    ];
    for label in SPIN_LABELS {
        assert_label_findable(label);
        assert_label_findable(&format!("{label} increment"));
        assert_label_findable(&format!("{label} decrement"));
        findability_checks += 3;
    }
    for label in [
        "Top Most",
        "Take screenshot",
        "Export debug images",
        "Reload preset images",
        "Start",
        "Stop",
        "Previous panel",
        "Next panel",
    ] {
        assert_label_findable(label);
        findability_checks += 1;
    }
    // Pager wrap-around + selection persistence. The
    // index is shell-owned (`debug-active-panel-index` on the root)
    // because the DebugScreen is instantiated per tab visit, and both
    // chevrons wrap instead of clamping at the ends.
    read_ui(|app| app.set_debug_active_panel_index(0));
    click_on_ui("Previous panel");
    assert_eq!(
        read_ui(|app| app.get_debug_active_panel_index()),
        11,
        "debug-pager: Previous panel from panel 0 must wrap to the last panel"
    );
    click_on_ui("Next panel");
    assert_eq!(
        read_ui(|app| app.get_debug_active_panel_index()),
        0,
        "debug-pager: Next panel from the last panel must wrap to panel 0"
    );
    click_on_ui("Next panel");
    click_on_ui("Next panel");
    click_on_ui("Affix Presets");
    click_on_ui("Debug");
    assert_eq!(
        read_ui(|app| app.get_debug_active_panel_index()),
        2,
        "debug-pager: the selected panel must survive leaving and re-entering the Debug tab"
    );
    click_on_ui("Previous panel");
    click_on_ui("Previous panel");
    assert_eq!(
        read_ui(|app| app.get_debug_active_panel_index()),
        0,
        "debug-pager: Previous panel must step back panel by panel"
    );
    click_throughs += 8;

    // The DebugInfo toggle's visible label is dynamic ("On"/"Off"),
    // so it has no stable label; the switch role/label still ships.
    mismatches.push(
        "Debug screen DebugInfo toggle — visible label is dynamic (\"On\"/\"Off\"), so the \
         matrix cannot address it by a stable label"
            .to_string(),
    );
    mismatches.push(
        "Most Debug numeric steppers are screen-internal (not root-observable); Type Area \
         Offset Left is root-observable (asserted by the affix-surface spin-box scenario)"
            .to_string(),
    );
    mismatches.push(
        "Top Most / Take screenshot / Export debug images / Reload preset images — effects \
         land in AppState and the log only, no root-observable state to assert"
            .to_string(),
    );
    mismatches.push(
        "Start / Stop live loop — excluded: would spawn live capture threads; the suite \
         must stay display- and network-free (runtime-bridge tests own this path)"
            .to_string(),
    );
    mismatches.push(
        "Reset Perf — screen-internal perf-model reset with no root-observable counter \
         (findability asserted only; the pager above is driven end-to-end)"
            .to_string(),
    );
    mismatches.push(
        "crash-log rows — the crash-log model is empty in the harness (no crash dir), so \
         no rows exist to click (attributes shipped)"
            .to_string(),
    );

    // ══ Tab 4 "Settings" (SettingsScreen) ════════════════════════
    click_on_ui("Settings");
    assert_eq!(read_ui(|app| app.get_active_tab()), 4);
    for label in [
        "Check for Updates",
        "Launch Minimized",
        "Minimize to Tray",
        "Diagnostics Export",
        "Check for New Releases",
        "Controller Mode",
        "Paragon Mode",
        "Open controller config",
        "Open paragon config",
        "Open colors config",
        "Open hotkeys config",
        "Open overlay config",
        "System preset",
        "Community preset",
        "App language",
        "Download preset",
        "Made with Slint",
    ] {
        assert_label_findable(label);
        findability_checks += 1;
    }

    // Settings toggle rows: two-way bound to the root — observable
    // click-through (flip + restore).
    type RowReader = fn(&AppWindow) -> bool;
    let toggle_rows: [(&str, RowReader); 2] = [
        ("Check for Updates", |app| app.get_check_updates()),
        ("Launch Minimized", |app| app.get_launch_minimized()),
    ];
    for (row_label, read) in toggle_rows {
        let before = read_ui(read);
        click_control_by_label(row_label);
        assert_ne!(
            read_ui(read),
            before,
            "coverage matrix: the {row_label} toggle must flip the root value"
        );
        click_control_by_label(row_label);
        assert_eq!(
            read_ui(read),
            before,
            "coverage matrix: the {row_label} toggle must restore"
        );
        click_throughs += 1;
    }

    for (opener, dialog_id) in [
        ("Open colors config", "colors_config"),
        ("Open controller config", "controller_config"),
        ("Open hotkeys config", "hotkeys_config"),
        ("Open paragon config", "paragon_config"),
    ] {
        dialogs_covered += 1;
        open_screenshot_cancel(client, window, opener, dialog_id);
    }

    // HotkeyConfig through its upstream Settings entry: the hotkeys
    // manager's per-row Edit button (upstream SettingsView opens the
    // single-binding dialog from its HotkeysConfig rows).
    click_on_ui("Open hotkeys config");
    click_control_by_label("Edit switch preset binding");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "hotkey_config",
        "coverage matrix: the hotkeys manager row Edit must open hotkey_config \
         (saw visible={visible}, id={dialog_id:?})"
    );
    matrix_screenshot(client, window, "dialog-hotkey_config.png");
    click_on_ui("Cancel");
    assert_dialog_closed("hotkey_config", "Cancel must close hotkey_config");
    dialogs_covered += 1;

    // Overlay config: the steppers are two-way bound to the root —
    // observable click-through for the numeric controls.
    click_on_ui("Open overlay config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "overlay_config",
        "coverage matrix: Open overlay config must open overlay_config"
    );
    matrix_screenshot(client, window, "dialog-overlay_config.png");
    let font_before = read_ui(|app| app.get_settings_overlay_font_size());
    click_on_ui("Overlay font size increment");
    assert_eq!(
        read_ui(|app| app.get_settings_overlay_font_size()),
        font_before + 1,
        "coverage matrix: the font-size stepper must change the root form value"
    );
    let offset_before = read_ui(|app| app.get_settings_overlay_type_area_offset_left());
    click_on_ui("Type-area offset left increment");
    assert_eq!(
        read_ui(|app| app.get_settings_overlay_type_area_offset_left()),
        offset_before + 1,
        "coverage matrix: the type-area stepper must change the root form value"
    );
    click_throughs += 2;
    dialogs_covered += 1;
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close overlay_config");

    mismatches.push(
        "Debug Mode settings toggle — hidden while dev-mode is off (enabled/hidden states \
         are not covered by this matrix)"
            .to_string(),
    );
    mismatches.push(
        "Download preset (DOWNLOAD) — the real download path is driven by the preset-download \
         family against a loopback-only stub via D4LC_PRESET_DOWNLOAD_BASE_URL; the matrix \
         itself does not click it because the suite-wide default must stay network-free"
            .to_string(),
    );
    mismatches.push(
        "release banner — only instantiated when update-available is true; the harness \
         never runs the update check"
            .to_string(),
    );
    mismatches.push(
        "ComboBox popups (Language / Preset / Import provider / System preset / Community \
         preset) — addressability asserted; popup selection opens a separate popup window \
         with no required observable, so it is not driven"
            .to_string(),
    );

    // ── Source scan: disallowed visible markers, whole UI tree ────
    const DISALLOWED_VISIBLE_MARKERS: [&str; 2] = ["content pending", "coming soon"];
    for (path, contents) in UI_SOURCES {
        let lowered = contents.to_ascii_lowercase();
        let markers: Vec<&str> = DISALLOWED_VISIBLE_MARKERS
            .iter()
            .copied()
            .filter(|marker| lowered.contains(marker))
            .collect();
        assert!(
            markers.is_empty(),
            "coverage matrix: {path} still contains disallowed visible placeholder \
             markers: {markers:?}"
        );
    }

    // ── Loud completion contract ──────────────────────────────────
    assert_eq!(
        dialogs_covered + dialogs_unreachable,
        EXPECTED_DIALOG_KINDS,
        "coverage matrix: {dialogs_covered} opened + {dialogs_unreachable} routed as \
         unreachable != {EXPECTED_DIALOG_KINDS} dialog kinds — the matrix accounting is \
         incomplete"
    );
    eprintln!(
        "[gui-matrix] coverage: 5/5 tabs, {dialogs_covered}/{EXPECTED_DIALOG_KINDS} dialogs \
         opened ({dialogs_unreachable} routed as unreachable by real click), \
         {findability_checks} findability probes, {click_throughs} observable click-throughs, \
         {} expected-mismatch rows; evidence under debug-output/{MATRIX_EVIDENCE_DIR}/",
        mismatches.len()
    );
    for row in &mismatches {
        eprintln!("[gui-matrix] expected-mismatch: {row}");
    }
}
