//! Local tab (upstream tab 0): preset file import and export.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::dialog::seed_preset_models;
use super::row_actions::open_url;
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

// ---------------------------------------------------------------------------
// Local tab (upstream tab 0): file import, export, remove.
//
// Upstream `ImportAffixPresetCommandExecute` opens an OpenFileDialog and
// deserializes one `AffixPreset` from the picked file; every failure
// (unreadable file, invalid JSON) lands on the Logging screen as
// `Failed to import {fileName}` (exact copy, no trailing period). Export
// writes `Exports/{SelectedAffixPreset.Name}.json` (indented JSON, the
// directory created on demand) and opens the folder in the file manager.
// The remove button is the same `RemoveAffixPresetNameCommand` the
// provider tabs bind, so it flows through the shared Delete overlay.
// ---------------------------------------------------------------------------

/// Injectable file-picker seam for the Local tab's Import button.
///
/// The real implementation opens the XDG desktop-portal file chooser
/// through `rfd`; tests inject a mock so the default suite stays offline,
/// and headless environments get the structured portal error instead of a
/// panic or a silent no-op.
pub trait PresetFileDialog: Send + Sync {
    /// Pick an existing JSON preset file. `Ok(None)` = the user cancelled;
    /// `Err` = the picker is unavailable (structured, user-presentable).
    fn pick_preset_json_file(&self) -> Result<Option<PathBuf>, String>;
}

/// The production picker: `rfd`'s XDG desktop-portal backend.
///
/// `rfd` collapses portal errors and cancellation into `None`; the
/// pre-flight `DBUS_SESSION_BUS_ADDRESS` check separates the missing
/// portal stack (headless / CI sessions) from a user cancel so headless
/// environments surface the capability error.
pub struct PortalFileDialog;

impl PresetFileDialog for PortalFileDialog {
    fn pick_preset_json_file(&self) -> Result<Option<PathBuf>, String> {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS")
            .unwrap_or_default()
            .is_empty()
        {
            return Err(
                "the file picker is unavailable: no D-Bus session bus is configured, \
                 so the XDG file-chooser portal cannot be reached"
                    .to_string(),
            );
        }
        Ok(rfd::FileDialog::new().pick_file())
    }
}

/// Process-wide picker seam. `None` = the production portal picker; tests
/// inject a mock through `set_import_file_dialog` and reset it after.
static IMPORT_FILE_DIALOG: Mutex<Option<Arc<dyn PresetFileDialog>>> = Mutex::new(None);

/// Replace the Local tab's file-picker seam (`None` restores the
/// production portal picker).
#[cfg(test)]
pub fn set_import_file_dialog(dialog: Option<Arc<dyn PresetFileDialog>>) {
    match IMPORT_FILE_DIALOG.lock() {
        Ok(mut guard) => *guard = dialog,
        Err(poisoned) => *poisoned.into_inner() = dialog,
    }
}

fn import_file_dialog() -> Arc<dyn PresetFileDialog> {
    match IMPORT_FILE_DIALOG.lock() {
        Ok(guard) => guard.clone().unwrap_or_else(|| Arc::new(PortalFileDialog)),
        Err(poisoned) => poisoned
            .into_inner()
            .clone()
            .unwrap_or_else(|| Arc::new(PortalFileDialog)),
    }
}

/// Injectable folder-open seam for the Local tab's Export button.
///
/// The production implementation hands the exports directory to the
/// desktop default handler (`xdg-open`, upstream's `explorer.exe`); the
/// pilot suite injects a recorder so a test run never spawns a file
/// manager on the live desktop session, and headless environments keep
/// surfacing the `xdg-open` error.
pub trait ExportsFolderOpener: Send + Sync {
    /// Open the exports directory in the desktop file manager.
    fn open_exports_folder(&self, path: &Path) -> Result<(), String>;
}

/// The production opener: `xdg-open` on the directory path.
pub struct XdgExportsFolderOpener;

impl ExportsFolderOpener for XdgExportsFolderOpener {
    fn open_exports_folder(&self, path: &Path) -> Result<(), String> {
        open_url(&path.display().to_string())
    }
}

/// Process-wide folder-open seam. `None` = the production `xdg-open`
/// opener; tests inject a recorder through `set_exports_folder_opener`
/// and reset it after.
static EXPORTS_FOLDER_OPENER: Mutex<Option<Arc<dyn ExportsFolderOpener>>> = Mutex::new(None);

/// Replace the Local tab's exports-folder-open seam (`None` restores the
/// production `xdg-open` opener).
#[cfg(test)]
pub fn set_exports_folder_opener(opener: Option<Arc<dyn ExportsFolderOpener>>) {
    match EXPORTS_FOLDER_OPENER.lock() {
        Ok(mut guard) => *guard = opener,
        Err(poisoned) => *poisoned.into_inner() = opener,
    }
}

fn exports_folder_opener() -> Arc<dyn ExportsFolderOpener> {
    match EXPORTS_FOLDER_OPENER.lock() {
        Ok(guard) => guard
            .clone()
            .unwrap_or_else(|| Arc::new(XdgExportsFolderOpener)),
        Err(poisoned) => poisoned
            .into_inner()
            .clone()
            .unwrap_or_else(|| Arc::new(XdgExportsFolderOpener)),
    }
}

/// Read and validate one imported preset file (upstream deserializes a
/// single `AffixPreset`; a file without a usable `Name` is a failure —
/// upstream silently ignores it, the port logs the same failure line so
/// the import never no-ops silently).
fn read_import_preset_file(path: &Path) -> Result<d4lc_config::AffixPreset, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let preset = d4lc_config::parse_affix_preset_json(&text)?;
    if preset.name.trim().is_empty() {
        return Err("the preset file carries no Name".to_string());
    }
    Ok(preset)
}

/// Local tab Import: pick a JSON file, deserialize one preset, add it
/// under a de-duplicated name, persist, and select it.
pub fn handle_local_import(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let path = match import_file_dialog().pick_preset_json_file() {
        Ok(Some(path)) => path,
        Ok(None) => return Ok(()),
        Err(error) => return Err(error),
    };

    match read_import_preset_file(&path) {
        Ok(preset) => {
            // Snapshot the editor so a failed commit restores it: the
            // import must not leave a preset that only exists in RAM.
            let editor_before = state.affix_preset_editor.clone();
            let downloaded_before = state.downloaded_paragon_routes.clone();
            let name = state.add_imported_affix_preset(preset);
            // Validate → persist → success: the added preset and the new
            // selection reach the disk as one transaction before the
            // import reports success; a failed write is returned so the
            // caller shows it and the dialog keeps the failure visible.
            if let Err(error) = state.persist_affix_presets_and_selection_to_config(config_root) {
                state.affix_preset_editor = editor_before;
                state.bump_rules_input_revision();
                state.downloaded_paragon_routes = downloaded_before;
                state.bump_rules_input_revision();
                return Err(format!("preset \"{name}\" could not be saved: {error}"));
            }
            // The import selects the added preset and is a preset-list
            // mutation like any other: refresh the AffixScreen models too.
            crate::actions::affix::refresh_affix_screen_preset_models(app, state);
            seed_preset_models(app, state);
            app.set_import_selected_preset(name.clone().into());
            eprintln!(
                "[d4lc] local import: {} added as preset \"{name}\"",
                path.display()
            );
            Ok(())
        }
        Err(error) => {
            eprintln!("[d4lc] local import failed: {error}");
            state.append_log_message(
                format!("Failed to import {}", path.display()),
                crate::clock::hh_mm_now(),
            );
            // A picked file that cannot be read or parsed is an import
            // failure, not a silent no-op: the caller reports it.
            Err(format!("Failed to import {}: {error}", path.display()))
        }
    }
}

/// Local tab Export: write the selected preset as indented JSON to
/// `Exports/{name}.json` under the config root and open the folder in the
/// file manager (upstream writes `Exports/{name}.json` relative to the
/// app directory and opens it with `explorer.exe`).
pub fn handle_local_export(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let name = app.get_import_selected_preset().trim().to_string();
    if name.is_empty() {
        return Err("No preset selected.".to_string());
    }
    // The name is the file name (upstream has no guard because Windows
    // `File.Create` rejects separators; the port checks first so a
    // hostile preset name cannot write outside the Exports directory).
    if name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(format!(
            "preset name '{name}' cannot be used as an export file name"
        ));
    }
    let config_preset = state.affix_preset_for_export(&name)?;
    let exports_dir = config_root.join("Exports");
    std::fs::create_dir_all(&exports_dir)
        .map_err(|error| format!("create {}: {error}", exports_dir.display()))?;
    let path = exports_dir.join(format!("{name}.json"));
    std::fs::write(
        &path,
        d4lc_config::affix_preset_to_indented_json(&config_preset),
    )
    .map_err(|error| format!("write {}: {error}", path.display()))?;
    exports_folder_opener().open_exports_folder(&exports_dir)?;
    eprintln!(
        "[d4lc] local export: \"{name}\" written to {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::actions::build_import_dialogs::test_support::{
        local_tab_root, preset_names, reference_item_rows, reference_item_type, wanted_affix_ids,
        write_import_file, IMPORT_FILE_BODY, IMPORT_FILE_DIALOG_TEST_LOCK,
    };
    use crate::app_state::AppState;

    // -----------------------------------------------------------------------
    // Local tab (upstream tab 0): file import / export
    // -----------------------------------------------------------------------

    /// The Local tab's Import confirm is a preset-list mutation: the
    /// AffixScreen preset ComboBox (`preset-names`), the selection and the
    /// entry models must update in the same session, not after a restart.
    #[test]
    fn local_import_refreshes_the_affix_screen_preset_models() {
        let app = crate::test_window::new_app_window();
        let _guard = IMPORT_FILE_DIALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = local_tab_root("local-import-refresh");
        let file = write_import_file(&root, "import.json", IMPORT_FILE_BODY);
        struct ScriptedPicker(std::path::PathBuf);
        impl PresetFileDialog for ScriptedPicker {
            fn pick_preset_json_file(&self) -> Result<Option<PathBuf>, String> {
                Ok(Some(self.0.clone()))
            }
        }
        set_import_file_dialog(Some(Arc::new(ScriptedPicker(file))));

        let mut state = AppState::default_startup();
        state
            .affix_preset_editor
            .create_preset("Alpha")
            .expect("seed preset");
        // The pre-import screen state: Alpha is listed and selected.
        app.set_preset_names(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from("Alpha"),
        ])));
        app.set_selected_preset(slint::SharedString::from("Alpha"));
        app.set_import_selected_preset(slint::SharedString::from("Alpha"));

        handle_local_import(&app, &mut state, &root).expect("the import confirm must succeed");
        set_import_file_dialog(None);

        assert_eq!(
            preset_names(&app),
            vec!["Alpha".to_string(), "Speed".to_string()],
            "the imported preset must appear in the AffixScreen combo without a restart"
        );
        assert_eq!(
            app.get_selected_preset().to_string(),
            "Speed",
            "the imported preset must be the AffixScreen selection"
        );
        assert_eq!(
            app.get_import_selected_preset().to_string(),
            "Speed",
            "the dialog's own preset combo still re-selects the import"
        );
        // The wanted rail row carries the localized catalog row the entry
        // resolves to — the canonical `IdName`, which lists the preset file's
        // own key as one of its members — not the raw stored key
        // (`resolve_catalog_index` merges the entry into its index row).
        let wanted = wanted_affix_ids(&app);
        assert_eq!(
            wanted.len(),
            1,
            "the import must project exactly one wanted affix: {wanted:?}"
        );
        assert!(
            wanted[0]
                .split(';')
                .any(|member| member.trim() == "AttackSpeed"),
            "the wanted row must be the catalog row listing AttackSpeed: {wanted:?}"
        );

        // The chain ends at the active evaluation: a restart through the
        // startup seam restores the imported preset from the preset file, and
        // a reference item carrying that preset's own entries matches them.
        // The imported file stores the legacy short id, so this also proves
        // the restart leaves an identity the evaluation can match.
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(Vec::new());
        state.hydrate_affix_presets_from_config(&root);
        let projection = state.affix_preset_editor.to_rule_presets();
        let preset = projection
            .affix_presets
            .iter()
            .find(|preset| preset.name == "Speed")
            .expect("the imported preset must survive a restart");
        let item_type = reference_item_type(preset);
        let rows = reference_item_rows(preset, &item_type);
        let item = d4lc_rules::ParsedItemForRules {
            item_type,
            rarity: Some("legendary".to_string()),
            affixes: rows.clone(),
        };
        let evaluation =
            d4lc_rules::evaluate_affix_preset(&item, "Speed", &projection.affix_presets);
        assert_eq!(
            evaluation.matched_affixes.len(),
            rows.len(),
            "the imported entry must match a reference item after a restart: {:?}",
            evaluation.missing_affixes
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn imported_preset_names_deduplicate_case_insensitively() {
        // Upstream `ImportAffixPresetCommandExecute`: a name that already
        // exists (case-insensitively) gets `-1`, `-2`, … appended.
        let root = local_tab_root("dedup");
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("SPEED").unwrap();
        let file = write_import_file(&root, "preset.json", IMPORT_FILE_BODY);

        let preset = read_import_preset_file(&file).expect("the file must parse");
        assert_eq!(preset.name, "Speed");
        let stored = state.add_imported_affix_preset(preset);
        assert_eq!(stored, "Speed-1");
        assert!(
            state
                .affix_preset_editor
                .presets
                .iter()
                .any(|preset| preset.name == "Speed-1"),
            "the de-duplicated preset is stored"
        );
        assert_eq!(
            state.affix_preset_editor.selected_preset_name.as_deref(),
            Some("Speed-1"),
            "the import selects the added preset"
        );

        // A second import of the same file becomes `Speed-2`.
        let preset = read_import_preset_file(&file).expect("the file must parse");
        let stored = state.add_imported_affix_preset(preset);
        assert_eq!(stored, "Speed-2");
    }

    #[test]
    fn local_import_rejects_broken_and_nameless_files() {
        let root = local_tab_root("broken");
        let broken = write_import_file(&root, "broken.json", "{ broken");
        assert!(
            read_import_preset_file(&broken).is_err(),
            "invalid JSON is an import failure (upstream logs `Failed to import …`)"
        );
        let array = write_import_file(&root, "array.json", "[]");
        assert!(
            read_import_preset_file(&array).is_err(),
            "the import format is one preset object, not the preset-file array"
        );
        let nameless = write_import_file(&root, "nameless.json", "{\"Name\": \"  \"}");
        assert!(
            read_import_preset_file(&nameless).is_err(),
            "a preset without a usable Name never imports silently"
        );
        let missing = root.join("does-not-exist.json");
        assert!(read_import_preset_file(&missing).is_err());
    }

    #[test]
    fn local_import_parse_error_is_reported_not_swallowed() {
        // A picked file that cannot be parsed is an import failure
        // the caller must see — never a silent `Ok` that looks like a
        // successful import.
        let app = crate::test_window::new_app_window();
        let _guard = IMPORT_FILE_DIALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = local_tab_root("local-import-parse-error");
        let file = write_import_file(&root, "broken.json", "{ broken");
        struct ScriptedPicker(std::path::PathBuf);
        impl PresetFileDialog for ScriptedPicker {
            fn pick_preset_json_file(&self) -> Result<Option<PathBuf>, String> {
                Ok(Some(self.0.clone()))
            }
        }
        set_import_file_dialog(Some(Arc::new(ScriptedPicker(file))));

        let mut state = AppState::default_startup();
        state
            .affix_preset_editor
            .create_preset("Alpha")
            .expect("seed preset");
        let error = handle_local_import(&app, &mut state, &root)
            .expect_err("a parse failure must not return Ok");
        set_import_file_dialog(None);

        assert!(
            error.contains("Failed to import"),
            "the error must name the failed import: {error}"
        );
        assert!(
            state
                .affix_preset_editor
                .presets
                .iter()
                .all(|preset| preset.name != "Speed"),
            "a failed import adds no preset"
        );
        assert!(
            state
                .logging
                .messages
                .iter()
                .any(|message| message.message.contains("Failed to import")),
            "the failure is logged like upstream's `Failed to import {{fileName}}`"
        );
        assert!(
            !root.join(d4lc_config::AFFIX_PRESETS_PATH).exists(),
            "a failed import writes no preset file"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_import_write_failure_before_replacement_reports_failure_without_a_new_revision() {
        // Phase 1 (failure before any replacement): the preset
        // file is obstructed, so the transaction fails at its first
        // replacement. The caller sees the error, the in-memory editor is
        // restored, the previously applied revision is unchanged and no
        // interrupted record exists because nothing was replaced.
        let app = crate::test_window::new_app_window();
        let _guard = IMPORT_FILE_DIALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = local_tab_root("local-import-write-failure-before");
        let file = write_import_file(&root, "import.json", IMPORT_FILE_BODY);
        struct ScriptedPicker(std::path::PathBuf);
        impl PresetFileDialog for ScriptedPicker {
            fn pick_preset_json_file(&self) -> Result<Option<PathBuf>, String> {
                Ok(Some(self.0.clone()))
            }
        }
        set_import_file_dialog(Some(Arc::new(ScriptedPicker(file))));

        let mut state = AppState::default_startup();
        state
            .affix_preset_editor
            .create_preset("Alpha")
            .expect("seed preset");
        state
            .persist_affix_presets_to_config(&root)
            .expect("seed persist");
        let previous = state
            .applied_config_revision
            .clone()
            .expect("the seed write is an applied revision");
        let presets_target = root.join(d4lc_config::AFFIX_PRESETS_PATH);
        std::fs::remove_file(&presets_target).expect("remove presets file");
        std::fs::create_dir_all(&presets_target).expect("directory in place of the presets file");

        let error = handle_local_import(&app, &mut state, &root)
            .expect_err("a failed write must not report success");
        set_import_file_dialog(None);

        assert!(
            error.contains("could not be saved"),
            "the error must name the failed save: {error}"
        );
        assert_eq!(
            state.applied_config_revision.as_ref(),
            Some(&previous),
            "a failed write keeps the previously applied revision"
        );
        assert!(
            state
                .affix_preset_editor
                .presets
                .iter()
                .all(|preset| preset.name != "Speed"),
            "the editor is restored, so no preset exists only in RAM"
        );
        assert!(
            d4lc_config::interrupted_transactions(&root)
                .expect("detect")
                .is_empty(),
            "a change that replaced nothing leaves no interrupted record"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_import_write_failure_between_replacements_is_visible_and_recoverable() {
        // Phase 2 (failure between the two replacements): the
        // preset file is replaced and the settings replacement fails. The
        // caller sees the error, the editor is restored, the applied
        // revision is unchanged, and the partial change is reported
        // explicitly instead of being adopted; rollback restores the
        // previous preset file.
        let app = crate::test_window::new_app_window();
        let _guard = IMPORT_FILE_DIALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = local_tab_root("local-import-write-failure-between");
        let file = write_import_file(&root, "import.json", IMPORT_FILE_BODY);
        struct ScriptedPicker(std::path::PathBuf);
        impl PresetFileDialog for ScriptedPicker {
            fn pick_preset_json_file(&self) -> Result<Option<PathBuf>, String> {
                Ok(Some(self.0.clone()))
            }
        }
        set_import_file_dialog(Some(Arc::new(ScriptedPicker(file))));

        let mut state = AppState::default_startup();
        state
            .affix_preset_editor
            .create_preset("Alpha")
            .expect("seed preset");
        state
            .persist_affix_presets_to_config(&root)
            .expect("seed persist");
        state.persist_selected_affix_preset_to_config(&root);
        let previous = state
            .applied_config_revision
            .clone()
            .expect("the seed writes apply revisions");
        // The settings target becomes a directory after the change was
        // prepared, so the presets replacement succeeds and the settings
        // replacement fails.
        let settings_target = root.join(d4lc_config::SETTINGS_PATH);
        std::fs::remove_file(&settings_target).expect("remove settings file");
        std::fs::create_dir_all(&settings_target).expect("directory in place of settings");

        let error = handle_local_import(&app, &mut state, &root)
            .expect_err("an interrupted write must not report success");
        set_import_file_dialog(None);

        assert!(
            error.contains("could not be saved"),
            "the error must name the failed save: {error}"
        );
        assert_eq!(
            state.applied_config_revision.as_ref(),
            Some(&previous),
            "the previous applied revision is unchanged"
        );
        assert!(
            state
                .affix_preset_editor
                .presets
                .iter()
                .all(|preset| preset.name != "Speed"),
            "the editor is restored, so the partial change is not adopted"
        );

        let records = d4lc_config::interrupted_transactions(&root).expect("detect");
        assert_eq!(records.len(), 1, "the partial change stays visible");
        assert_eq!(
            records[0].files(),
            vec![d4lc_config::AFFIX_PRESETS_PATH, d4lc_config::SETTINGS_PATH],
            "the record names exactly the changed files"
        );

        // Explicit recovery (never automatic): rollback restores the
        // previous preset file and clears the record.
        state
            .recover_interrupted_config_transaction(&root, d4lc_config::Recovery::Rollback)
            .expect("rollback should succeed");
        let restored = d4lc_config::load_affix_presets(&root).expect("load restored presets");
        assert_eq!(
            restored
                .iter()
                .map(|preset| preset.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha"],
            "rollback restores the previously persisted preset file"
        );
        assert!(
            d4lc_config::interrupted_transactions(&root)
                .expect("list")
                .is_empty(),
            "recovery clears the record"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_export_writes_the_selected_preset_with_boards() {
        // Export produces exactly the persisted-format slice for the
        // selected preset (upstream writes `Exports/{Name}.json`
        // indented and opens the folder).
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        state
            .affix_preset_editor
            .edit_entry(
                "Speed",
                d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "RapidAspect",
                    Some("Gloves"),
                    "#00FF00",
                ),
            )
            .expect("the aspect is added");

        let config = state
            .affix_preset_for_export("Speed")
            .expect("the selected preset exports");
        assert_eq!(config.name, "Speed");
        assert_eq!(config.item_aspects.len(), 1);
        assert_eq!(config.item_aspects[0].id, "RapidAspect");

        let text = d4lc_config::affix_preset_to_indented_json(&config);
        assert!(text.contains("\"Name\": \"Speed\""));
        assert!(text.contains("\"RapidAspect\""));
        assert!(
            state.affix_preset_for_export("Ghost").is_err(),
            "a preset that does not exist cannot export"
        );
    }

    #[test]
    fn file_dialog_seam_defaults_to_the_portal_picker() {
        // The injected seam wins over the production picker; resetting
        // it restores the portal-backed default.
        let _guard = IMPORT_FILE_DIALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        struct MockPicker;
        impl PresetFileDialog for MockPicker {
            fn pick_preset_json_file(&self) -> Result<Option<PathBuf>, String> {
                Ok(Some(PathBuf::from("/tmp/mock-preset.json")))
            }
        }
        set_import_file_dialog(Some(Arc::new(MockPicker)));
        assert_eq!(
            import_file_dialog()
                .pick_preset_json_file()
                .expect("mock pick"),
            Some(PathBuf::from("/tmp/mock-preset.json"))
        );
        // Reset to the production portal picker so later scenarios (and
        // the other tests in this suite) never see the mock.
        set_import_file_dialog(None);
    }

    #[test]
    fn exports_folder_seam_defaults_to_the_xdg_opener() {
        // The injected opener wins over the production `xdg-open` opener;
        // resetting it restores the production default, which this test
        // never invokes — no test run may spawn a file manager.
        struct StubOpener;
        impl ExportsFolderOpener for StubOpener {
            fn open_exports_folder(&self, _path: &Path) -> Result<(), String> {
                Err("stub opener".to_string())
            }
        }
        set_exports_folder_opener(Some(Arc::new(StubOpener)));
        assert_eq!(
            exports_folder_opener()
                .open_exports_folder(Path::new("/tmp/mock-exports"))
                .expect_err("stub opener"),
            "stub opener"
        );
        // Reset to the production xdg-open opener so later scenarios (and
        // the other tests in this suite) never see the stub.
        set_exports_folder_opener(None);
    }

    #[test]
    fn portal_picker_reports_a_missing_session_bus_instead_of_no_op() {
        // Headless: without a D-Bus session bus the XDG
        // file-chooser portal cannot open, so the picker fails with a
        // structured error rather than silently reporting "cancelled".
        let previous = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
        let result = PortalFileDialog.pick_preset_json_file();
        match previous {
            Some(value) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", value),
            None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
        }
        let error = result.expect_err("no session bus must be a structured error");
        assert!(
            error.contains("file picker is unavailable"),
            "the error names the missing portal stack: {error}"
        );
    }
}
