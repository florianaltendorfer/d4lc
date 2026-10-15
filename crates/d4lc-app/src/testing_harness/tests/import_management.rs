use super::*;

/// Seed one import-shaped preset ("ImportedWhirlwind", two affix
/// entries) through `HARNESS_APP_STATE` and re-project — the
/// documented seed route for state whose UI create path is gated:
/// `confirm_import_preset` reaches
/// `AffixPresetEditorState::import_preset` only under the
/// `real-runtime` feature, so the headless harness pins the
/// import gap in the provider matrix and seeds the editor for the
/// list/select/delete half of the cycle. Every interaction under
/// test (list readback, ComboBox selection, Remove preset) stays
/// real input.
fn seed_imported_preset() {
    on_ui(move |_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state
            .affix_preset_editor
            .presets
            .push(d4lc_gui::AffixPresetEditorPreset {
                name: "ImportedWhirlwind".into(),
                entries: vec![
                    preset_entry(
                        d4lc_gui::PresetEntryKind::Affix,
                        "AttackSpeed",
                        Some("Attack Speed"),
                        Some("Amulet"),
                        "#22C55E",
                    ),
                    preset_entry(
                        d4lc_gui::PresetEntryKind::Affix,
                        "Life",
                        None,
                        None,
                        "#EF4444",
                    ),
                ],
            });
    });
    reproject_affixes();
}

/// One provider tab's offline seeding, driven through real
/// input: the chip opens the tabbed dialog on the provider's tab and
/// the left build list carries the fixture-corpus rows (upstream
/// `LoadAvailable{Provider}Builds` parity over the resolved builds
/// directory). No import confirm exists on the tabbed surface — the
/// add-as-preset flow owns it — so the offline matrix here is:
/// real-click open, fixture seeding, empty right list, Done close.
fn provider_tab_seeding_case(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
    website: &'static str,
    expected_tab: i32,
    provider: &'static str,
) {
    select_website(client, root, website);
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "import_preset",
        "local-merge-tabs: clicking {website:?} must open import_preset (saw visible={visible}, \
         id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        expected_tab,
        "local-merge-tabs: the {website:?} chip must land on tab {expected_tab} ({provider})"
    );
    let ids = seeded_build_rows(provider);
    assert!(
        !ids.is_empty(),
        "local-merge-tabs: the {provider} tab must seed its build rows offline from the \
         bundled fixture corpus"
    );
    take_evidence_screenshot(
        client,
        window,
        "import-delete-edges",
        &format!("dialog-import-{provider}-seeded.png"),
    );
    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "local-merge-tabs: Done must close import_preset"
    );
}

/// The Mobalytics nested "Builds"/"Profiles" sub-tab family on the
/// tabbed import dialog: the Profiles page seeds the stored profiles
/// from the bundled fixture corpus, a profile click lists its
/// published builds, and the variant Add fills the URL field through
/// the upstream `AddMobalyticsProfileBuildVariantExecute` parity (the
/// user then presses Add — no direct import). Offline: no browser, no
/// network.
pub(super) fn mobalytics_subtab_scenarios(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[mobalytics-subtabs] starting scenarios");
    select_website(client, root, "Mobalytics.gg");
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "import_preset",
        "mobalytics-subtabs: the website row must open import_preset"
    );
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        6,
        "mobalytics-subtabs: Mobalytics.gg must land on tab 6"
    );

    // A fresh open selects the upstream default "Builds" sub-tab and
    // seeds the profiles list (UpdateMobalyticsProfiles parity).
    assert_eq!(
        read_ui(|app| app.get_import_mobalytics_selected_sub_tab()),
        0,
        "mobalytics-subtabs: the nested control must open on Builds"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-mobalytics-builds-subtab.png",
    );

    click_on_ui("Profiles");
    assert_eq!(
        read_ui(|app| app.get_import_mobalytics_selected_sub_tab()),
        1,
        "mobalytics-subtabs: the Profiles tab click must switch the nested control"
    );
    let profile_names = read_ui(|app| {
        let model = app.get_import_mobalytics_profiles();
        (0..model.row_count())
            .map(|row| model.row_data(row).expect("profile row").name.to_string())
            .collect::<Vec<_>>()
    });
    assert!(
        profile_names.iter().any(|name| name == "Test Profile"),
        "mobalytics-subtabs: the stored profile fixture must seed the Profiles list: \
         {profile_names:?}"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-mobalytics-profiles-subtab.png",
    );

    // Selecting a profile lists its published builds.
    click_on_ui("Test Profile");
    let variant_names = read_ui(|app| {
        let model = app.get_import_mobalytics_profile_variants();
        (0..model.row_count())
            .map(|row| {
                model
                    .row_data(row)
                    .expect("profile variant row")
                    .name
                    .to_string()
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        variant_names,
        vec![
            "Spiritborn Endgame".to_string(),
            "Warlock Starter".to_string()
        ],
        "mobalytics-subtabs: the profile click must list its published builds"
    );

    // The variant Add fills the URL field (the `BuildIdMobalytics`
    // setter parity) and re-evaluates the Add gate — the user then
    // presses Add; nothing imports directly.
    click_on_ui("Add profile build to URL field");
    assert_eq!(
        read_ui(|app| app.get_import_mobalytics_url().to_string()),
        "https://mobalytics.gg/diablo-4/profile/test-profile/builds/spiritborn-endgame",
        "mobalytics-subtabs: the variant Add must fill the URL field with the variant Url"
    );
    assert_eq!(
        read_ui(|app| app.get_import_mobalytics_extracted_id().to_string()),
        "https://mobalytics.gg/diablo-4/profile/test-profile/builds/spiritborn-endgame",
        "mobalytics-subtabs: the filled URL must pass the upstream Add gate"
    );
    assert!(
        read_ui(|app| app.get_import_mobalytics_add_enabled()),
        "mobalytics-subtabs: the filled URL must enable Add"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-mobalytics-profile-add.png",
    );

    // Back to the Builds page through the real sub-tab strip.
    click_on_ui("Builds");
    assert_eq!(
        read_ui(|app| app.get_import_mobalytics_selected_sub_tab()),
        0,
        "mobalytics-subtabs: the Builds tab click must switch back"
    );

    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "mobalytics-subtabs: Done must close import_preset"
    );
    eprintln!("[mobalytics-subtabs] scenarios complete");
}

/// The import → preset-list → select → delete cycle with its clean
/// state proof, plus the garbage-import edge. The import step is
/// pinned at its headless boundary (seeded — see
/// `seed_imported_preset`); list visibility, selection,
/// deletion, and the no-orphan readback are driven by real input.
fn import_delete_cycle_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[import-delete-edges] starting seeded import/delete cycle");
    // Prior state (the affix-surface family leaves the standard seed on
    // Alpha).
    let names_before = preset_names();
    let selected_before = read_ui(|app| app.get_selected_preset().to_string());
    let rows_before = affix_row_count();
    assert!(
        !names_before.iter().any(|name| name == "ImportedWhirlwind"),
        "import-delete-edges: the ImportedWhirlwind seed must start absent: {names_before:?}"
    );

    // The imported preset appears in the projected affix preset list.
    seed_imported_preset();
    assert!(
        preset_names()
            .iter()
            .any(|name| name == "ImportedWhirlwind"),
        "import-delete-edges: the seeded imported preset must appear in the projected preset \
         list: {:?}",
        preset_names()
    );
    take_evidence_screenshot(
        client,
        window,
        "import-delete-edges",
        "imported-preset-in-list.png",
    );

    // Garbage input while the imported preset exists: the Mobalytics
    // tab's Add stays disabled (upstream parity: invalid input never
    // errors) and the presets stay untouched. This is the
    // headless analog of "garbage after a successful import" — no
    // import can complete headless, so the seeded preset stands in
    // for the successful one.
    click_on_ui("Import preset");
    assert!(
        dialog_state().0 && dialog_state().1 == "import_preset",
        "import-delete-edges: the Import click must open import_preset"
    );
    // The dialog opens on the tab mapped from the website selection;
    // switch to the Mobalytics tab through the real tab strip.
    click_on_ui("\u{1f310} Mobalytics");
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        6,
        "import-delete-edges: the Mobalytics tab click must land on tab 6"
    );
    type_into_field(
        client,
        window,
        root,
        super::build_import::MOBALYTICS_PLACEHOLDER,
        "https://evil.example/not-a-build",
    );
    assert!(
        !read_ui(|app| app.get_import_mobalytics_add_enabled()),
        "import-delete-edges: the garbage URL must keep Add disabled"
    );
    assert!(
        !read_ui(|app| app.get_dialog_error_visible()),
        "import-delete-edges: the garbage URL must surface no dialog error"
    );
    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "import-delete-edges: Done must close import_preset"
    );
    assert!(
        preset_names()
            .iter()
            .any(|name| name == "ImportedWhirlwind"),
        "import-delete-edges: the failed import must leave the imported preset untouched"
    );
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        selected_before,
        "import-delete-edges: the failed import must not move the selection"
    );

    // The imported preset is usable: selectable through the real
    // ComboBox path, its entries project onto the screen as the wanted
    // state of the catalog rows that list their ids.
    select_preset(client, root, "ImportedWhirlwind");
    assert_eq!(
        affix_row_count(),
        60 + 2,
        "import-delete-edges: the imported preset's entries must project when selected"
    );
    assert_eq!(
        wanted_affix_count(),
        2,
        "import-delete-edges: the imported preset's two wanted rows must project when selected"
    );
    take_evidence_screenshot(
        client,
        window,
        "import-delete-edges",
        "imported-preset-selected.png",
    );

    // Delete through the real remove path (the window-modal confirm
    // prompt answers Yes: confirm-prompt).
    click_on_ui("Remove preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "import-delete-edges: deleting the imported preset must ask through the confirm prompt \
         (observed: visible={visible}, id={dialog_id:?})"
    );
    click_on_ui("Yes");
    let names_after = preset_names();
    assert_eq!(
        names_after, names_before,
        "import-delete-edges: deleting the imported preset must restore the prior list: \
         {names_after:?}"
    );
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        selected_before,
        "import-delete-edges: the selection must fall back to the prior preset"
    );
    assert_eq!(
        affix_row_count(),
        rows_before,
        "import-delete-edges: the removed preset's entries must be unprojected"
    );

    // No orphan in AppState: editor + shell projection readback.
    let (editor_names, editor_selected, shell_count, shell_selected) = app_state_presets();
    assert_eq!(
        editor_names, names_before,
        "import-delete-edges: no orphan may remain in the editor presets: {editor_names:?}"
    );
    assert_eq!(
        editor_selected, selected_before,
        "import-delete-edges: the editor selection must fall back to the prior preset"
    );
    assert_eq!(
        (shell_count, shell_selected),
        (names_before.len(), Some(selected_before.clone())),
        "import-delete-edges: the shell projection must match the restored state"
    );
    // Config readback: affix presets have no config store
    // (the editor state above is the only store; the delete path
    // performs no config write), so no config readback exists to
    // assert.
    eprintln!(
        "[import-delete-edges] clean state: no orphan in AppState (editor + shell projection); \
         affix presets have no config store, so no config readback exists — the \
         delete path performs no config write"
    );
    take_evidence_screenshot(
        client,
        window,
        "import-delete-edges",
        "imported-preset-deleted.png",
    );
}

/// The import-delete-edges scenario family: import/delete end-to-end edge
/// cases through the real dialogs.
///
/// Offline import matrix (this headless build has no
/// `real-runtime` feature, so `build_import_dialogs::import_variant`
/// gates every provider's JSON→preset import behind an
/// feature-gap error):
///
/// - offline-capable for every provider dialog (D2Core, D4Builds,
///   Mobalytics, Maxroll, InfinityBuilds) and the cross-provider
///   import_preset dialog: the real-click open, the fixture-corpus
///   build-id seeding, the pasted-URL host gate, and the cached-id
///   → fixture source-path resolution;
/// - not offline-capable anywhere: the import itself (JSON → affix
///   preset). The confirm surfaces the `real-runtime` feature gap
///   as the dialog error and writes no preset — asserted per
///   provider in `provider_offline_case`; the InfinityBuilds
///   share-URL download is additionally gated (already pinned by
///   the build-import-url family);
/// - a live import test needs a `--features real-runtime` build
///   (asset data); only the InfinityBuilds live share-page fetch
///   would additionally leave the machine (`preset-download`).
///
/// Runs after the affix-surface family (this family also seeds and deletes presets) and
/// leaves the preset list as it found it.
pub(super) fn import_delete_edge_scenarios(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[import-delete-edges] starting import/delete edge scenarios");
    // The provider openers live in the AffixScreen presets panel
    // (the screen is only instantiated on its tab), so the family
    // runs on the AffixScreen like the other preset families.
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
    provider_tab_seeding_case(client, window, root, "D2Core.com", 2, "D2Core");
    provider_tab_seeding_case(client, window, root, "D4Builds.gg", 3, "D4Builds");
    provider_tab_seeding_case(
        client,
        window,
        root,
        "InfinityBuilds.gg",
        4,
        "InfinityBuilds",
    );
    provider_tab_seeding_case(client, window, root, "Maxroll.gg", 5, "Maxroll");
    provider_tab_seeding_case(client, window, root, "Mobalytics.gg", 6, "Mobalytics");
    import_delete_cycle_scenario(client, window, root);
    eprintln!(
        "[import-delete-edges] import/delete edge scenarios complete: 5 offline provider \
         seeding cases on the tabbed dialog (real-click open + fixture corpus rows + \
         Done close), seeded import→list→select→delete cycle with clean-state readback"
    );
}

// ── Local + Merge tab families ──────────────────────────────────────────

/// Click the FIRST button carrying `label` through the MCP click path
/// (server-side geometry). Used inside the freshly instantiated
/// overlay subtrees, where the synchronous mock click can compute a
/// stale element rect.
fn click_by_label(client: &McpClient, root: McpHandle, label: &str) {
    let target = client
        .query_element_descendants(root, descendants_with_role("Button"), true)
        .unwrap_or_else(|error| harness_panic("query buttons by label", &error))
        .into_iter()
        .find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessibleLabel") == Some(label)
                })
        })
        .unwrap_or_else(|| {
            panic!("local-merge-tabs: no button with accessible label {label:?} found")
        });
    client
        .click_element(target, None)
        .unwrap_or_else(|error| harness_panic("click the button", &error));
}

/// Click the LAST button carrying `label`. The tabbed dialog hosts a
/// bottom "Done" and its overlay layers host their own Done buttons;
/// the overlays are declared after the main layout, so the last match
/// is the overlay's control (the first match would close the whole
/// dialog).
fn click_last_by_label(client: &McpClient, root: McpHandle, label: &str) {
    let matches: Vec<McpHandle> = client
        .query_element_descendants(root, descendants_with_role("Button"), true)
        .unwrap_or_else(|error| harness_panic("query buttons by label", &error))
        .into_iter()
        .filter(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessibleLabel") == Some(label)
                })
        })
        .collect();
    let target = matches.last().unwrap_or_else(|| {
        panic!("local-merge-tabs: no button with accessible label {label:?} found")
    });
    client
        .click_element(*target, None)
        .unwrap_or_else(|error| harness_panic("click the overlay button", &error));
}

/// Real-click a ComboBox open (by its accessible label) and click the
/// option `option` in the opened popup (the affix-surface preset-ComboBox
/// popup pattern), then assert the selection landed through
/// `read_selected`.
fn select_combo_option(
    client: &McpClient,
    root: McpHandle,
    combo_label: &str,
    option: &str,
    read_selected: fn(&AppWindow) -> String,
) {
    let query_options = |client: &McpClient, root: McpHandle| -> Option<McpHandle> {
        let items = client
            .query_element_descendants(root, descendants_with_role("ListItem"), true)
            .unwrap_or_else(|error| harness_panic("query combo popup items", &error));
        items.into_iter().find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessibleLabel") == Some(option)
                })
        })
    };
    // A stale popup consumes the first outside click (Slint popup
    // semantics), so the open retries once.
    let mut target = None;
    for _ in 0..2 {
        click_on_label(combo_label);
        if let Some(found) = query_options(client, root) {
            target = Some(found);
            break;
        }
    }
    let item = target.unwrap_or_else(|| {
        panic!(
            "local-merge-tabs: option {option:?} not reachable in the open {combo_label:?} popup"
        )
    });
    client
        .click_element(item, None)
        .unwrap_or_else(|error| harness_panic("click the combo option", &error));
    assert_eq!(
        read_ui(read_selected),
        option,
        "local-merge-tabs: selecting {option:?} must update {combo_label:?}"
    );
}

/// The local-merge-tabs Local + Merge tab families, run after the import-delete-edges
/// family (same seed-and-restore contract):
///
/// - Local tab: a preset file is imported through the INJECTED
///   file-picker seam (the real picker needs a portal session; the
///   seam is the offline decision point), the name de-duplicates
///   case-insensitively on a second import, the Export click writes
///   `Exports/{name}.json` under the harness config root, and the
///   Remove click flows through the shared Delete confirmation
///   overlay.
/// - Merge tab: two seeded presets are selected through real combo
///   clicks, the Build 1 swatch opens the embedded color picker
///   seeded with the row's OWN color (the deliberate divergence from
///   upstream's Build-2 seeding bug), the picked color lands on that
///   row only, and the Merge click drives the shared Confirm-name
///   overlay with exactly the two suggestions; the merged preset
///   recolors Build 1's entries, appends Build 2's new keys, excludes
///   uniques/runes, silently overwrites, and re-selects.
///
/// Network-free and portal-free: Add is never clicked and the picker
/// seam is mocked. Restores the standard preset seed before returning.
pub(super) fn local_merge_tab_scenarios(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
    config_root: &std::path::Path,
) {
    eprintln!("[local-merge-tabs] starting Local + Merge tab scenarios");
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    // ── Local tab ────────────────────────────────────────────────
    seed_affix_presets();
    // The import file: exactly the persisted-format single preset
    // object the Local tab reads (produced by the same serializer the
    // export writes, so the file round-trips through upstream's shape).
    let import_dir = std::env::temp_dir().join(format!(
        "d4lc-local-merge-tabs-local-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&import_dir).expect("create import file dir");
    let import_file = import_dir.join("LocalProbe.json");
    let probe_preset = d4lc_config::AffixPreset {
        name: "LocalProbe".into(),
        item_affixes: vec![d4lc_config::PresetItemAffix {
            id: "AttackSpeed".into(),
            item_type: "Amulet".into(),
            color: d4lc_config::PresetColor::from_rgb(34, 197, 94),
            is_any_type: false,
            is_greater: true,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
            label: Some("Attack Speed".into()),
        }],
        ..d4lc_config::AffixPreset::default()
    };
    std::fs::write(
        &import_file,
        d4lc_config::affix_preset_to_indented_json(&probe_preset),
    )
    .expect("write the import probe file");

    struct ProbePicker(std::path::PathBuf);
    impl crate::actions::build_import_dialogs::PresetFileDialog for ProbePicker {
        fn pick_preset_json_file(&self) -> Result<Option<std::path::PathBuf>, String> {
            Ok(Some(self.0.clone()))
        }
    }
    crate::actions::build_import_dialogs::set_import_file_dialog(Some(Arc::new(ProbePicker(
        import_file.clone(),
    ))));

    select_website(client, root, "D2Core.com");
    click_on_ui("Import preset");
    assert!(dialog_state().0 && dialog_state().1 == "import_preset");
    click_on_ui("Local");
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        0,
        "local-merge-tabs: the Local tab title must switch to tab 0"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-local-tab.png",
    );

    let names_before = read_ui(|app| app.get_import_preset_names().row_count());
    click_on_ui("Import local preset");
    assert_eq!(
        read_ui(|app| app.get_import_selected_preset().to_string()),
        "LocalProbe",
        "local-merge-tabs: the imported preset must be selected in the combo"
    );
    assert_eq!(
        read_ui(|app| app.get_import_preset_names().row_count()),
        names_before + 1,
        "local-merge-tabs: the import must add exactly one combo entry"
    );
    let presets_file = config_root.join("Config/AffixPresets-v2.json");
    let persisted = std::fs::read_to_string(&presets_file)
        .expect("local-merge-tabs: presets file after import");
    assert!(
        persisted.contains("LocalProbe"),
        "local-merge-tabs: the imported preset must be persisted to the presets file"
    );

    // A second import of the same file de-duplicates
    // case-insensitively — "LocalProbe" already exists, so `-1`.
    click_on_ui("Import local preset");
    assert_eq!(
        read_ui(|app| app.get_import_selected_preset().to_string()),
        "LocalProbe-1",
        "local-merge-tabs: the re-imported preset name must de-duplicate with -1"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-local-imported.png",
    );

    // Export writes `Exports/{selected}.json` under the config root
    // and then opens the folder (upstream's `explorer.exe` step). The
    // open routes through the injected folder-open seam so a pilot run
    // never spawns a file manager on the live desktop: the recorder
    // must see exactly the Exports directory. A headless `xdg-open`
    // failure keeps surfacing the dialog error instead.
    struct RecordingFolderOpener(std::sync::Mutex<Vec<std::path::PathBuf>>);
    impl crate::actions::build_import_dialogs::ExportsFolderOpener for RecordingFolderOpener {
        fn open_exports_folder(&self, path: &std::path::Path) -> Result<(), String> {
            self.0
                .lock()
                .expect("folder-open recorder")
                .push(path.to_path_buf());
            Ok(())
        }
    }
    let opened_folders = Arc::new(RecordingFolderOpener(std::sync::Mutex::new(Vec::new())));
    crate::actions::build_import_dialogs::set_exports_folder_opener(Some(opened_folders.clone()));
    click_on_ui("Export local preset");
    let export_path = config_root.join("Exports").join("LocalProbe-1.json");
    let exported = std::fs::read_to_string(&export_path)
        .unwrap_or_else(|error| panic!("local-merge-tabs: export must write {error}"));
    assert!(
        exported.contains("\"Name\": \"LocalProbe-1\"") && exported.contains("\"AttackSpeed\""),
        "local-merge-tabs: the exported file must carry the selected preset"
    );
    assert_eq!(
        opened_folders
            .0
            .lock()
            .expect("folder-open recorder")
            .as_slice(),
        [config_root.join("Exports")],
        "local-merge-tabs: the export must open the Exports folder through the seam, \
         never the live desktop file manager"
    );
    crate::actions::build_import_dialogs::set_exports_folder_opener(None);
    let open_failed = read_ui(|app| app.get_dialog_error_visible());
    if open_failed {
        let error = read_ui(|app| app.get_dialog_error().to_string());
        assert!(
            error.contains("xdg-open"),
            "local-merge-tabs: the only tolerated export error is the missing xdg-open: {error}"
        );
    }

    // Remove flows through the shared Delete confirmation overlay.
    click_on_ui("Remove local preset");
    assert!(
        read_ui(|app| app.get_import_delete_visible()),
        "local-merge-tabs: the Remove click must open the Delete overlay"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-local-delete.png",
    );
    click_on_ui("OK");
    assert!(
        !read_ui(|app| app.get_import_delete_visible()),
        "local-merge-tabs: OK must close the Delete overlay"
    );
    assert_eq!(
        read_ui(|app| app.get_import_preset_names().row_count()),
        names_before + 1,
        "local-merge-tabs: removing LocalProbe-1 leaves the first import in place"
    );
    let persisted = std::fs::read_to_string(&presets_file)
        .expect("local-merge-tabs: presets file after remove");
    assert!(
        !persisted.contains("LocalProbe-1"),
        "local-merge-tabs: the removed preset must not persist"
    );

    crate::actions::build_import_dialogs::set_import_file_dialog(None);
    let _ = std::fs::remove_dir_all(&import_dir);
    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "local-merge-tabs: Done must close import_preset"
    );

    // ── Merge tab ────────────────────────────────────────────────
    // Two merge-specific presets (deterministic entry matrix):
    // MAlpha carries affix/aspect/sigil + unique + rune, MBeta
    // repeats MAlpha's affix key and adds a new affix and aspect plus
    // a unique and a rune (never merged).
    let merge_presets = vec![
        d4lc_gui::AffixPresetEditorPreset {
            name: "MAlpha".into(),
            entries: vec![
                preset_entry(
                    d4lc_gui::PresetEntryKind::Affix,
                    "AffixA",
                    None,
                    Some("Amulet"),
                    "#FF0000",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "AspectA",
                    None,
                    None,
                    "#00FF00",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Sigil,
                    "SigilA",
                    None,
                    None,
                    "#0000FF",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Unique,
                    "UniqueA",
                    None,
                    None,
                    "#111111",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Rune,
                    "RuneA",
                    None,
                    None,
                    "#8B5CF6",
                ),
            ],
        },
        d4lc_gui::AffixPresetEditorPreset {
            name: "MBeta".into(),
            entries: vec![
                preset_entry(
                    d4lc_gui::PresetEntryKind::Affix,
                    "AffixA",
                    None,
                    Some("Amulet"),
                    "#00FF00",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Affix,
                    "AffixB",
                    None,
                    None,
                    "#00FF00",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "AspectB",
                    None,
                    None,
                    "#00FF00",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Unique,
                    "UniqueB",
                    None,
                    None,
                    "#00FF00",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Rune,
                    "RuneB",
                    None,
                    None,
                    "#00FF00",
                ),
            ],
        },
    ];
    on_ui(move |_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(merge_presets);
        state.affix_preset_editor.selected_preset_name = Some("MAlpha".into());
        state.shell.affix_presets.preset_count = state.affix_preset_editor.presets.len();
        state.shell.affix_presets.selected_preset =
            state.affix_preset_editor.selected_preset_name.clone();
    });
    reproject_affixes();

    select_website(client, root, "D2Core.com");
    click_on_ui("Import preset");
    assert!(dialog_state().0 && dialog_state().1 == "import_preset");
    click_on_ui("Merge");
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        1,
        "local-merge-tabs: the Merge tab title must switch to tab 1"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-merge-tab.png",
    );

    // Select the two presets through real combo clicks.
    select_combo_option(client, root, "Merge Build 1 preset", "MAlpha", |app| {
        app.get_import_merge_build1().to_string()
    });
    select_combo_option(client, root, "Merge Build 2 preset", "MBeta", |app| {
        app.get_import_merge_build2().to_string()
    });

    // The Merge gate follows the selections (upstream
    // CanMergeBuildsExecute): enabled only with both names set.
    // Build 1's change-color checkbox gates its swatch; toggling it
    // is a real checkbox click.
    click_control_by_label("Change color Build 1");
    assert!(
        read_ui(|app| app.get_import_merge_change_color1()),
        "local-merge-tabs: the Build 1 change-color checkbox must toggle on"
    );
    // The enabled swatch click opens the embedded picker seeded with
    // the row's OWN color (divergence: upstream seeds Build 2's
    // dialog with ColorBuild1 — a bug the port does not copy).
    click_on_label("Color #008000");
    assert!(
        read_ui(|app| app.get_import_color_picker_visible()),
        "local-merge-tabs: the swatch click must open the color picker"
    );
    assert_eq!(
        read_ui(|app| app.get_import_color_picker_value().to_string()),
        "#008000",
        "local-merge-tabs: the picker must seed with the row's own color"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-merge-color-picker.png",
    );
    click_by_label(client, root, "Yellow");
    assert_eq!(
        read_ui(|app| app.get_import_color_picker_value().to_string()),
        "#FFFF00",
        "local-merge-tabs: a palette click must fill the picker value"
    );
    click_by_label(client, root, "Apply");
    assert!(
        !read_ui(|app| app.get_import_color_picker_visible()),
        "local-merge-tabs: Apply must close the color picker"
    );
    assert_eq!(
        read_ui(|app| app.get_import_merge_color1().to_string()),
        "#FFFF00",
        "local-merge-tabs: Apply must write the color back to the clicked row"
    );

    // Merge click: the shared Confirm-name overlay with exactly the
    // two suggestions and Build 1's name pre-filled.
    click_last_by_label(client, root, "Merge");
    assert!(
        read_ui(|app| app.get_import_name_visible()),
        "local-merge-tabs: the Merge click must open the Confirm-name overlay"
    );
    assert_eq!(
        read_ui(|app| app.get_import_name_text().to_string()),
        "MAlpha",
        "local-merge-tabs: the confirm name must seed with Build 1's name"
    );
    let (suggestions, warning): (Vec<String>, bool) = read_ui(|app| {
        let model = app.get_import_name_suggestions();
        let rows: Vec<String> = (0..model.row_count())
            .filter_map(|index| model.row_data(index).map(|row| row.to_string()))
            .collect();
        (rows, app.get_import_name_overwrite_warning())
    });
    assert_eq!(
        suggestions,
        vec!["MAlpha".to_string(), "MBeta".to_string()],
        "local-merge-tabs: the merge confirm name must offer exactly the two preset names"
    );
    assert!(
        warning,
        "local-merge-tabs: Build 1's own name exists, so the overwrite warning shows"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-merge-confirm-name.png",
    );

    // Done runs the merge (first pass: ChangeColorBuild1 only).
    click_last_by_label(client, root, "Done");
    assert!(
        !read_ui(|app| app.get_import_name_visible()),
        "local-merge-tabs: Done must close the Confirm-name overlay"
    );
    fn kind_label(kind: d4lc_gui::PresetEntryKind) -> String {
        match kind {
            d4lc_gui::PresetEntryKind::Affix => "Affix",
            d4lc_gui::PresetEntryKind::Aspect => "Aspect",
            d4lc_gui::PresetEntryKind::Sigil => "Sigil",
            d4lc_gui::PresetEntryKind::Unique => "Unique",
            d4lc_gui::PresetEntryKind::Rune => "Rune",
        }
        .to_string()
    }
    let merged_colors: Vec<(String, String, String)> = on_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        let preset = state
            .affix_preset_editor
            .presets
            .iter()
            .find(|preset| preset.name == "MAlpha")
            .expect("local-merge-tabs: the merged preset must exist")
            .clone();
        preset
            .entries
            .into_iter()
            .map(|entry| (kind_label(entry.kind), entry.id, entry.color))
            .collect()
    });
    // Build 1's affixes/aspects/sigils recolored to #FFFF00; its
    // unique and rune are NOT merged; Build 2's new keys appended
    // with their own colors (ChangeColorBuild2 off); the duplicate
    // keeps Build 1's (recolored) color with ChangeColorBuild12 off.
    assert_eq!(
        merged_colors,
        vec![
            ("Affix".to_string(), "AffixA".to_string(), "#FFFF00".to_string()),
            ("Aspect".to_string(), "AspectA".to_string(), "#FFFF00".to_string()),
            ("Sigil".to_string(), "SigilA".to_string(), "#FFFF00".to_string()),
            ("Affix".to_string(), "AffixB".to_string(), "#00FF00".to_string()),
            ("Aspect".to_string(), "AspectB".to_string(), "#00FF00".to_string()),
        ],
        "local-merge-tabs: the merge must recolor Build 1, append Build 2's new keys, and drop uniques/runes"
    );
    assert_eq!(
        read_ui(|app| app.get_import_selected_preset().to_string()),
        "MAlpha",
        "local-merge-tabs: the merged preset must be selected in the combo"
    );
    let persisted = std::fs::read_to_string(config_root.join("Config/AffixPresets-v2.json"))
        .expect("local-merge-tabs: presets file after merge");
    assert!(
        persisted.contains("MAlpha") && !persisted.contains("UniqueA"),
        "local-merge-tabs: the merged preset must persist without Build 1's unique"
    );

    // Second pass: ChangeColorBuild12 recolors the DUPLICATE entry to
    // the Build 1+2 swatch; the silent overwrite replaces the first
    // merged preset under the same name.
    click_control_by_label("Change color Build 1+2");
    click_last_by_label(client, root, "Merge");
    assert!(
        read_ui(|app| app.get_import_name_visible()),
        "local-merge-tabs: the second Merge click must reopen the Confirm-name overlay"
    );
    click_last_by_label(client, root, "Done");
    let duplicate_color: Option<String> = on_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state
            .affix_preset_editor
            .presets
            .iter()
            .find(|preset| preset.name == "MAlpha")
            .and_then(|preset| {
                preset
                    .entries
                    .iter()
                    .find(|entry| entry.id == "AffixA")
                    .map(|entry| entry.color.clone())
            })
    });
    assert_eq!(
        duplicate_color.as_deref(),
        Some("#008000"),
        "local-merge-tabs: ChangeColorBuild12 must recolor the duplicate to the Build 1+2 swatch"
    );
    let preset_count = on_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state.affix_preset_editor.presets.len()
    });
    assert_eq!(
        preset_count, 2,
        "local-merge-tabs: the silent overwrite must not grow the preset list"
    );
    take_evidence_screenshot(
        client,
        window,
        "local-merge-tabs",
        "dialog-import-merge-merged.png",
    );

    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "local-merge-tabs: Done must close import_preset"
    );
    eprintln!("[local-merge-tabs] Local + Merge tab scenarios complete");
    seed_affix_presets();
}
