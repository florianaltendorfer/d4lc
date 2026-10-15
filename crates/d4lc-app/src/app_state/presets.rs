use super::AppState;
use d4lc_gui::{AffixPresetEditorState, PresetEntryKind};
use d4lc_rules::{ImportedParagonBoard, ImportedParagonRoute};
use std::path::Path;

impl AppState {
    // -------------------------------------------------------------------
    // Affix-preset persistence
    //
    // Upstream reference: `AffixManager.LoadAffixPresets` /
    // `SaveAffixPresets` (AffixManager.cs:913-926, 1049-1058) plus the
    // `SelectedAffixPreset` restore in `AffixViewModel.cs:1686-1689`.
    // -------------------------------------------------------------------

    /// Load the affix presets from `Config/AffixPresets-v2.json` and
    /// the persisted `SelectedAffixPreset` from `Config/Settings.json`
    /// into the editor.
    ///
    /// Startup seam: called from the affix wiring (`wire_affix_filters_actions`)
    /// once the config root is known, before the AffixScreen's initial
    /// entry projection. A missing file keeps the empty editor;
    /// an unreadable file is surfaced on the Logging screen and the
    /// editor keeps its current (empty) state — user state we could not
    /// read is never silently replaced. A persisted selection is
    /// restored only when a preset with that name exists, mirroring the
    /// upstream `FirstOrDefault` restore.
    pub fn hydrate_affix_presets_from_config(&mut self, config_root: &Path) {
        // The presets, the selection and the overlay rule data below are
        // rule inputs.
        self.bump_rules_input_revision();
        // An interrupted multi-file change is reported here, visibly and
        // without adopting anything: whatever reached the disk is loaded
        // read-only, the recovery command is logged, and the write path
        // keeps refusing the affected files until the operator decides.
        // The one exception is a change that never replaced any file: it
        // was never applied nor reported as saved, so discarding its
        // staged copies loses nothing and is logged.
        match d4lc_config::interrupted_transactions(config_root) {
            Ok(records) => {
                for record in records {
                    let message = interrupted_change_message(config_root, record);
                    self.append_log_message(message, crate::clock::hh_mm_now());
                }
            }
            Err(error) => {
                self.append_log_message(
                    format!("config change check failed: {error}"),
                    crate::clock::hh_mm_now(),
                );
            }
        }

        let mut loaded = match d4lc_config::load_affix_presets(config_root) {
            Ok(presets) => presets,
            Err(error) => {
                self.append_log_message(
                    format!("affix presets load failed: {error}"),
                    crate::clock::hh_mm_now(),
                );
                return;
            }
        };
        // The stored ids are migrated onto the canonical catalog identities
        // before the editor adopts them: the pre-fix imports stored a
        // shortened row key that the evaluation can never match. The migration
        // is visible (one log entry per finding), lossless (only unambiguous
        // keys are rewritten) and persisted through the transactional write.
        self.migrate_preset_entry_ids(&mut loaded, config_root);

        self.affix_preset_editor =
            AffixPresetEditorState::new(loaded.iter().map(editor_preset_from_config).collect());

        // Imported paragon survives restarts through the preset file:
        // seed the editor's keyed paragon store from the loaded config
        // presets. Empty lists are not stored — a missing entry projects
        // no boards either way.
        self.affix_preset_editor.paragon_boards_by_preset = loaded
            .iter()
            .map(|preset| {
                (
                    preset.name.clone(),
                    paragon_routes_from_preset_boards(&preset.paragon_boards_list),
                )
            })
            .filter(|(_, routes)| !routes.is_empty())
            .collect();

        let selected = d4lc_config::load_app_settings(config_root)
            .map(|settings| settings.selected_affix_preset)
            .unwrap_or_default();
        if !selected.is_empty()
            && self
                .affix_preset_editor
                .presets
                .iter()
                .any(|preset| preset.name == selected)
        {
            self.affix_preset_editor.selected_preset_name = Some(selected);
        }

        // Overlay rule data that lives next to the presets in the config
        // root. The two maps mirror upstream
        // `AffixManager.InitSigilDungeonTierData` /
        // `InitAffixMinimalValueData` — a missing file keeps the
        // empty map, an unreadable one is logged and the previous value
        // stays (user state we could not read is never silently replaced).
        match d4lc_config::load_affix_minimal_values(config_root) {
            Ok(minimal_values) => self.minimal_affix_values = minimal_values,
            Err(error) => {
                self.append_log_message(
                    format!("minimal affix values load failed: {error}"),
                    crate::clock::hh_mm_now(),
                );
            }
        }
        match d4lc_config::load_dungeon_tiers(config_root) {
            Ok(dungeon_tiers) => self.dungeon_tiers = dungeon_tiers,
            Err(error) => {
                self.append_log_message(
                    format!("dungeon tiers load failed: {error}"),
                    crate::clock::hh_mm_now(),
                );
            }
        }

        // Keep the shell panel projection in step with the editor.
        self.shell.affix_presets.preset_count = self.affix_preset_editor.presets.len();
        self.shell.affix_presets.selected_preset =
            self.affix_preset_editor.selected_preset_name.clone();

        // `LoadAffixPresets` ends with `SaveAffixPresets` (AffixManager.cs:926):
        // the file is rewritten sorted, and a fresh install gets its empty
        // list. A file that is already up to date is left alone; a refused or
        // failed write is logged and leaves the loaded state in place.
        let presets = self.config_presets_for_save();
        match d4lc_config::save_affix_presets_if_changed(config_root, &presets) {
            Ok(Some(revision)) => self.applied_config_revision = Some(revision),
            Ok(None) => {}
            Err(error) => self.append_log_message(
                format!("affix presets save failed: {error}"),
                crate::clock::hh_mm_now(),
            ),
        }
    }

    /// Rewrite the loaded preset entry ids onto the canonical catalog
    /// identities of the active language and persist the migrated file.
    ///
    /// The import paths stored a shortened key of a catalog row before the
    /// canonical identity work — the first `IdNameList` entry of the row, or
    /// the first `;` segment of its `IdName` — while the OCR path, the imports
    /// and the rule evaluation only ever use the complete `IdName`. An id that
    /// already is a canonical identity stays unchanged, an id that resolves
    /// onto exactly one identity is rewritten to it, and an ambiguous or
    /// unknown id stays exactly as stored: every finding names the preset, the
    /// entry kind, the id and the reason, and nothing is guessed. The oracle is
    /// the pinned catalog file (`Affixes.{language}.json`, ...) of the language
    /// the preset file was written in; an unreadable catalog leaves every id of
    /// that kind as stored and says so.
    ///
    /// A run that rewrote at least one id persists the result through the
    /// transactional write ([`d4lc_config::save_affix_presets`]): only a
    /// successful commit advances [`Self::applied_config_revision`], and a
    /// refused or failed write keeps the migrated ids in memory, logs the
    /// reason and leaves the stored file unchanged (visible, lossless, no
    /// silent takeover).
    fn migrate_preset_entry_ids(
        &mut self,
        presets: &mut [d4lc_config::AffixPreset],
        config_root: &Path,
    ) {
        let language = self.shell.settings.language.clone();
        // Only the kinds the file actually carries need their catalog file.
        let mut kinds: Vec<PresetEntryKind> = Vec::new();
        for preset in presets.iter() {
            for kind in PRESET_ENTRY_KINDS {
                if kinds.contains(&kind) {
                    continue;
                }
                if preset_entry_list(preset, kind)
                    .iter()
                    .any(|entry| !entry.id.is_empty())
                {
                    kinds.push(kind);
                }
            }
        }
        if kinds.is_empty() {
            return;
        }

        let repo =
            crate::actions::affix::catalog_repo_root().map(d4lc_assets::AssetRepository::new);
        let mut findings: Vec<String> = Vec::new();
        let mut migrated = 0usize;
        let mut ambiguous = 0usize;
        let mut unknown = 0usize;

        for kind in kinds {
            let (category, label_field) = preset_entry_catalog(kind);
            let catalog = match &repo {
                Some(repo) => d4lc_assets::catalog::IdentityCatalog::load(
                    repo,
                    &language,
                    category,
                    label_field,
                ),
                None => Err(format!(
                    "the pinned catalog data ({category}.{language}.json) was not found"
                )),
            };
            let catalog = match catalog {
                Ok(catalog) => catalog,
                Err(reason) => {
                    // Without the oracle every id of this kind keeps its
                    // stored form; the missing prerequisite stays visible.
                    self.append_log_message(
                        format!(
                            "preset id migration skipped for {} entries: {reason}",
                            preset_entry_kind_label(kind)
                        ),
                        crate::clock::hh_mm_now(),
                    );
                    continue;
                }
            };

            for preset in presets.iter_mut() {
                let preset_name = preset.name.clone();
                for entry in preset_entry_list_mut(preset, kind).iter_mut() {
                    match resolve_preset_entry_id(&catalog, &entry.id) {
                        PresetEntryIdResolution::Canonical => {}
                        PresetEntryIdResolution::Rewrite(canonical) => {
                            findings.push(format!(
                                "preset id migrated: '{}' -> '{canonical}' ({} entry in preset '{preset_name}')",
                                entry.id,
                                preset_entry_kind_label(kind)
                            ));
                            entry.id = canonical.to_string();
                            migrated += 1;
                        }
                        PresetEntryIdResolution::Ambiguous(candidates) => {
                            ambiguous += 1;
                            findings.push(format!(
                                "preset id not migrated: '{}' resolves to {} canonical identities \
                                 [{}] ({} entry in preset '{preset_name}')",
                                entry.id,
                                candidates.len(),
                                candidates.join(", "),
                                preset_entry_kind_label(kind)
                            ));
                        }
                        PresetEntryIdResolution::Unknown => {
                            unknown += 1;
                            findings.push(format!(
                                "preset id not migrated: '{}' is not a key of the pinned {category} \
                                 catalog for '{language}' ({} entry in preset '{preset_name}')",
                                entry.id,
                                preset_entry_kind_label(kind)
                            ));
                        }
                    }
                }
            }
        }

        if migrated > 0 {
            // The previous bytes are the only way back from a migration, so the
            // stored file is kept before it is rewritten; without that copy the
            // file stays untouched.
            let persisted =
                d4lc_config::keep_affix_presets_migration_backup(config_root, "pre-id-migration")
                    .and_then(|backup| {
                        if let Some(backup) = backup {
                            findings.push(format!("preset id migration backup: {backup}"));
                        }
                        d4lc_config::save_affix_presets(config_root, presets)
                    });
            match persisted {
                Ok(revision) => {
                    self.applied_config_revision = Some(revision);
                    findings.push(format!(
                        "preset id migration persisted: {migrated} entry id(s) rewritten to their \
                         canonical identity ({language} catalog state)"
                    ));
                }
                Err(error) => findings.push(format!(
                    "preset id migration could not be persisted: {error}; the migrated ids stay \
                     active in memory and the stored file keeps the previous ids"
                )),
            }
        }
        if !findings.is_empty() {
            findings.push(format!(
                "preset id migration: {migrated} migrated, {ambiguous} ambiguous, {unknown} \
                 unknown ({language} catalog state)"
            ));
        }
        for message in findings {
            self.append_log_message(message, crate::clock::hh_mm_now());
        }
    }

    /// Persist the editor's presets to `Config/AffixPresets-v2.json`
    /// and report the committed revision.
    ///
    /// Every preset/entry mutation calls this after the in-memory change
    /// (the upstream `SaveAffixPresets` trigger points). The write is:
    /// it refuses to replace a preset file this build cannot
    /// read, only a successful write advances
    /// [`Self::applied_config_revision`], and the failure is returned so
    /// the caller surfaces it instead of reporting success.
    ///
    /// # Errors
    ///
    /// Returns the refused or failed write and leaves the previously
    /// applied revision untouched. The reason is also logged.
    pub fn persist_affix_presets_to_config(
        &mut self,
        config_root: &Path,
    ) -> Result<d4lc_config::CommittedRevision, d4lc_config::ConfigError> {
        let presets = self.config_presets_for_save();
        match d4lc_config::save_affix_presets(config_root, &presets) {
            Ok(revision) => {
                self.applied_config_revision = Some(revision.clone());
                Ok(revision)
            }
            Err(error) => {
                self.append_log_message(
                    format!("affix presets save failed: {error}"),
                    crate::clock::hh_mm_now(),
                );
                Err(error)
            }
        }
    }

    /// Persist the editor's presets and the selected preset name as one
    /// transaction (the shape an import or merge leaves behind).
    ///
    /// Both files are staged and recorded before either is replaced, so
    /// an interrupted change is visible at the next start instead of
    /// leaving a preset file and a selection that silently disagree.
    /// Only a successful commit advances
    /// [`Self::applied_config_revision`]; a failure is logged and
    /// returned, and the caller must not report success or close its
    /// dialog on it.
    ///
    /// # Errors
    ///
    /// Returns the refused or interrupted write; a failure between the
    /// two replacements leaves the transaction record in place for an
    /// explicit recovery.
    pub fn persist_affix_presets_and_selection_to_config(
        &mut self,
        config_root: &Path,
    ) -> Result<d4lc_config::CommittedRevision, d4lc_config::ConfigError> {
        let presets = self.config_presets_for_save();
        let selected = self.affix_preset_editor.selected_preset_name.clone();
        match d4lc_config::save_affix_presets_with_selection(
            config_root,
            &presets,
            selected.as_deref(),
        ) {
            Ok(revision) => {
                self.applied_config_revision = Some(revision.clone());
                Ok(revision)
            }
            Err(error) => {
                self.append_log_message(
                    format!("affix presets/selection save failed: {error}"),
                    crate::clock::hh_mm_now(),
                );
                Err(error)
            }
        }
    }

    /// The editor's presets in the persisted shape, with the keyed
    /// paragon boards merged in (upstream keeps them inside the preset;
    /// the gui entry list cannot hold them).
    fn config_presets_for_save(&self) -> Vec<d4lc_config::AffixPreset> {
        self.affix_preset_editor
            .presets
            .iter()
            .map(|preset| {
                let mut config = config_preset_from_editor(preset);
                if let Some(routes) = self.affix_preset_editor.paragon_routes_for(&preset.name) {
                    config.paragon_boards_list = preset_boards_from_paragon_routes(routes);
                }
                config
            })
            .collect()
    }

    /// Persist the currently selected preset name into
    /// `Config/Settings.json` (upstream `SelectedAffixPreset`,
    /// `AffixViewModel.cs:372-373` + `AffixManager.RenamePreset`).
    ///
    /// Fire-and-forget like the preset saves; a failure is logged, the
    /// in-memory selection stays applied, and the applied revision is
    /// only advanced by a successful write.
    pub fn persist_selected_affix_preset_to_config(&mut self, config_root: &Path) {
        let selected = self.affix_preset_editor.selected_preset_name.as_deref();
        match d4lc_config::save_selected_affix_preset(config_root, selected) {
            Ok(revision) => self.applied_config_revision = Some(revision),
            Err(error) => {
                self.append_log_message(
                    format!("selected affix preset save failed: {error}"),
                    crate::clock::hh_mm_now(),
                );
            }
        }
    }

    /// Apply an explicit recovery decision to an interrupted config
    /// change and log the outcome (the operator surface is the
    /// `d4lc-debug config-recover` command).
    ///
    /// This is the "offer" side of the interrupted-change protocol:
    /// nothing recovers automatically, [`Self::hydrate_affix_presets_from_config`]
    /// only reports the record, and the caller (an operator surface or
    /// a test) decides between [`d4lc_config::Recovery::Complete`] and
    /// [`d4lc_config::Recovery::Rollback`].
    ///
    /// # Errors
    ///
    /// Returns the precise failure when there is no interrupted change
    /// or the chosen recovery cannot be completed safely.
    #[cfg(test)]
    pub fn recover_interrupted_config_transaction(
        &mut self,
        config_root: &Path,
        action: d4lc_config::Recovery,
    ) -> Result<d4lc_config::RecoveryOutcome, String> {
        let record = d4lc_config::interrupted_transactions(config_root)
            .map_err(|error| error.to_string())?
            .into_iter()
            .next()
            .ok_or_else(|| "no interrupted configuration change to recover".to_string())?;
        let record_path = record.record_path().display().to_string();
        let outcome = record.recover(action).map_err(|error| error.to_string())?;
        self.append_log_message(
            format!(
                "config change recovered ({action}): {} ({record_path})",
                outcome.files.join(", ")
            ),
            crate::clock::hh_mm_now(),
        );
        Ok(outcome)
    }

    /// Record the paragon routes an import produced for `preset_name`.
    ///
    /// The add-as-preset path (`handle_name_done`) calls this after the
    /// editor import so the boards reach both the live projection and
    /// the next preset save. An empty route list removes a previous
    /// entry: re-importing a preset without paragon must not keep the
    /// stale boards of an earlier import under the same name.
    pub fn set_imported_paragon_routes(
        &mut self,
        preset_name: &str,
        routes: Vec<ImportedParagonRoute>,
    ) {
        self.affix_preset_editor
            .set_paragon_routes(preset_name, routes);
        // The import supersedes any download-only boards (upstream
        // keeps paragon on the import, and the trade-imports tab now
        // reports the import result).
        self.downloaded_paragon_routes = None;
        self.bump_rules_input_revision();
    }

    /// Add a preset imported through the Local tab's file import.
    ///
    /// Upstream `ImportAffixPresetCommandExecute`
    /// (ImportAffixPresetViewModel.cs:1498-1538): the imported name is
    /// de-duplicated case-INSENSITIVELY against the existing presets by
    /// appending `-1`, `-2`, …, then the preset is added through the
    /// editor's silent-overwrite import (which sorts and selects). The
    /// file's `ParagonBoardsList` travels with the preset the same way a
    /// build import's boards do. Returns the (possibly de-duplicated)
    /// name the preset was stored under.
    pub fn add_imported_affix_preset(&mut self, preset: d4lc_config::AffixPreset) -> String {
        let base_name = preset.name.clone();
        let mut name = base_name.clone();
        let mut counter = 0u32;
        while self
            .affix_preset_editor
            .presets
            .iter()
            .any(|existing| existing.name.eq_ignore_ascii_case(&name))
        {
            counter += 1;
            name = format!("{base_name}-{counter}");
        }

        let mut editor_preset = editor_preset_from_config(&preset);
        editor_preset.name = name.clone();
        self.affix_preset_editor
            .import_preset(editor_preset, Vec::new());
        self.affix_preset_editor.set_paragon_routes(
            &name,
            paragon_routes_from_preset_boards(&preset.paragon_boards_list),
        );
        self.bump_rules_input_revision();
        name
    }

    /// The persisted-form preset for the Local tab's export: the editor
    /// preset converted into the `Config/AffixPresets-v2.json` entity
    /// shape and merged with its stored paragon boards — the same
    /// projection [`Self::persist_affix_presets_to_config`] writes, so
    /// an exported file is exactly the preset slice of the preset file.
    pub fn affix_preset_for_export(&self, name: &str) -> Result<d4lc_config::AffixPreset, String> {
        let preset = self
            .affix_preset_editor
            .presets
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| format!("preset '{name}' not found"))?;
        let mut config = config_preset_from_editor(preset);
        if let Some(routes) = self.affix_preset_editor.paragon_routes_for(name) {
            config.paragon_boards_list = preset_boards_from_paragon_routes(routes);
        }
        Ok(config)
    }
}

/// The startup log line for one interrupted config change.
///
/// A change that never replaced any file is discarded here; the decision
/// is made under the journal lock
/// ([`d4lc_config::InterruptedTransaction::discard_if_untouched`]), so a
/// change that advanced since it was listed is never rolled back. Every
/// other change is only reported, with the exact operator command and the
/// recovery options the files currently allow.
fn interrupted_change_message(
    config_root: &Path,
    record: d4lc_config::InterruptedTransaction,
) -> String {
    let files = record.files().join(", ");
    let id = record.id().to_string();
    let root = config_root.display();
    let discarded = if record.is_untouched() {
        record.discard_if_untouched()
    } else {
        Ok(None)
    };
    match discarded {
        // Not (or no longer, under the lock) untouched: report it below.
        Ok(None) => {}
        Ok(Some(_)) => {
            return format!(
                "config change interrupted before any file was replaced: discarded the staged, \
                 never-applied change to {files} (record {id}, config root {root}); every file \
                 keeps its previous content"
            );
        }
        Err(error) => {
            return format!(
                "config change interrupted before any file was replaced ({files}, record {id}): \
                 discarding it failed: {error}; saving these files stays blocked — run \
                 `d4lc-debug config-recover --rollback --record {id}` with config root {root}"
            );
        }
    }
    let option = |action: d4lc_config::Recovery| match record.check(action) {
        Ok(()) => format!("--{action} possible"),
        Err(reason) => format!("--{action} not possible ({reason})"),
    };
    format!(
        "config change interrupted: {files} (record {id}, config root {root}); nothing was \
         applied automatically and saving these files is blocked until you run \
         `d4lc-debug config-recover --complete --record {id}` or \
         `d4lc-debug config-recover --rollback --record {id}` for config root {root} \
         (set D4LC_CONFIG_ROOT when it is not the default); {}; {}",
        option(d4lc_config::Recovery::Complete),
        option(d4lc_config::Recovery::Rollback),
    )
}

// ---------------------------------------------------------------------------
// Affix-preset editor <-> file-shape mapping
//
// d4lc stores one mixed-kind entry list per preset
// (`AffixPresetEditorPreset`, kind-tagged `PresetEntry`); the upstream
// file shape (`AffixPreset.cs`) stores one list per kind. The mapping
// routes each entry into the list named by its kind. The d4lc-local
// display `label` has no upstream counterpart — it persists through the
// d4lc-extension `Label` field (upstream deserialization skips it), so
// marker labels survive restarts.
// ---------------------------------------------------------------------------

/// Convert an editor preset into the upstream-shaped config model.
fn config_preset_from_editor(
    preset: &d4lc_gui::AffixPresetEditorPreset,
) -> d4lc_config::AffixPreset {
    let mut config = d4lc_config::AffixPreset {
        name: preset.name.clone(),
        ..d4lc_config::AffixPreset::default()
    };
    for entry in &preset.entries {
        let target = match entry.kind {
            PresetEntryKind::Affix => &mut config.item_affixes,
            PresetEntryKind::Aspect => &mut config.item_aspects,
            PresetEntryKind::Sigil => &mut config.item_sigils,
            PresetEntryKind::Unique => &mut config.item_uniques,
            PresetEntryKind::Rune => &mut config.item_runes,
        };
        target.push(config_entry_from_editor(entry));
    }
    config
}

/// Convert an upstream-shaped config preset into the editor model. The
/// upstream list an entry came from determines its kind; an empty
/// upstream `Type` means "any item type" (no item-type constraint).
fn editor_preset_from_config(
    preset: &d4lc_config::AffixPreset,
) -> d4lc_gui::AffixPresetEditorPreset {
    let mut entries = Vec::new();
    let lists = [
        (PresetEntryKind::Affix, &preset.item_affixes),
        (PresetEntryKind::Aspect, &preset.item_aspects),
        (PresetEntryKind::Sigil, &preset.item_sigils),
        (PresetEntryKind::Unique, &preset.item_uniques),
        (PresetEntryKind::Rune, &preset.item_runes),
    ];
    for (kind, list) in lists {
        for entry in list {
            entries.push(editor_entry_from_config(kind, entry));
        }
    }
    d4lc_gui::AffixPresetEditorPreset {
        name: preset.name.clone(),
        entries,
    }
}

/// The decision of the preset-id migration for one stored id.
#[derive(Debug, PartialEq, Eq)]
enum PresetEntryIdResolution<'a> {
    /// The stored id is already a canonical catalog identity.
    Canonical,
    /// The id resolves onto exactly this canonical identity, which differs.
    Rewrite(&'a str),
    /// Several canonical identities carry the key: the candidates in catalog
    /// order, so a finding can name them instead of picking one.
    Ambiguous(Vec<&'a str>),
    /// No canonical identity carries the key.
    Unknown,
}

/// Resolve one stored preset entry id onto the canonical identity space.
///
/// The catalog's own precedence applies (`IdentityCatalog::resolve`: an exact
/// canonical `IdName` first, then the `IdNameList` names and the `IdSnoList`
/// SNOs as lookup keys). An ambiguous or unknown key never produces a rewrite:
/// the caller keeps the stored id and reports the finding.
fn resolve_preset_entry_id<'a>(
    catalog: &'a d4lc_assets::catalog::IdentityCatalog,
    id: &str,
) -> PresetEntryIdResolution<'a> {
    match catalog.resolve(id) {
        d4lc_assets::catalog::IdentityResolution::Resolved(row) if row.id_name == id => {
            PresetEntryIdResolution::Canonical
        }
        d4lc_assets::catalog::IdentityResolution::Resolved(row) => {
            PresetEntryIdResolution::Rewrite(row.id_name.as_str())
        }
        d4lc_assets::catalog::IdentityResolution::Ambiguous(candidates) => {
            PresetEntryIdResolution::Ambiguous(
                candidates
                    .iter()
                    .map(|candidate| candidate.id_name.as_str())
                    .collect(),
            )
        }
        d4lc_assets::catalog::IdentityResolution::Unknown => PresetEntryIdResolution::Unknown,
    }
}

/// The preset entry lists of one config preset, in the upstream list order.
const PRESET_ENTRY_KINDS: [PresetEntryKind; 5] = [
    PresetEntryKind::Affix,
    PresetEntryKind::Aspect,
    PresetEntryKind::Unique,
    PresetEntryKind::Rune,
    PresetEntryKind::Sigil,
];

/// The catalog file one preset entry kind resolves through, with the label
/// field of that file (`IdentityCatalog::load` shape).
fn preset_entry_catalog(kind: PresetEntryKind) -> (&'static str, &'static str) {
    match kind {
        PresetEntryKind::Affix => ("Affixes", d4lc_assets::catalog::AFFIX_LABEL_FIELD),
        PresetEntryKind::Aspect => ("Aspects", "Name"),
        PresetEntryKind::Unique => ("Uniques", "Name"),
        PresetEntryKind::Rune => ("Runes", "Name"),
        PresetEntryKind::Sigil => ("Sigils", "Name"),
    }
}

fn preset_entry_kind_label(kind: PresetEntryKind) -> &'static str {
    match kind {
        PresetEntryKind::Affix => "affix",
        PresetEntryKind::Aspect => "aspect",
        PresetEntryKind::Unique => "unique",
        PresetEntryKind::Rune => "rune",
        PresetEntryKind::Sigil => "sigil",
    }
}

fn preset_entry_list(
    preset: &d4lc_config::AffixPreset,
    kind: PresetEntryKind,
) -> &Vec<d4lc_config::PresetItemAffix> {
    match kind {
        PresetEntryKind::Affix => &preset.item_affixes,
        PresetEntryKind::Aspect => &preset.item_aspects,
        PresetEntryKind::Unique => &preset.item_uniques,
        PresetEntryKind::Rune => &preset.item_runes,
        PresetEntryKind::Sigil => &preset.item_sigils,
    }
}

fn preset_entry_list_mut(
    preset: &mut d4lc_config::AffixPreset,
    kind: PresetEntryKind,
) -> &mut Vec<d4lc_config::PresetItemAffix> {
    match kind {
        PresetEntryKind::Affix => &mut preset.item_affixes,
        PresetEntryKind::Aspect => &mut preset.item_aspects,
        PresetEntryKind::Unique => &mut preset.item_uniques,
        PresetEntryKind::Rune => &mut preset.item_runes,
        PresetEntryKind::Sigil => &mut preset.item_sigils,
    }
}

fn config_entry_from_editor(entry: &d4lc_gui::PresetEntry) -> d4lc_config::PresetItemAffix {
    d4lc_config::PresetItemAffix {
        id: entry.id.clone(),
        item_type: entry.item_type.clone().unwrap_or_default(),
        color: d4lc_config::PresetColor::parse(&entry.color),
        // The upstream `IsAnyType` flag and the `TuningPrisms` list are
        // carried through the editor instead of being derived/dropped
        // so an import survives edit + save unchanged.
        is_any_type: entry.is_any_type,
        is_greater: entry.is_greater,
        is_implicit: entry.is_implicit,
        is_tempered: entry.is_tempered,
        tuning_prisms: entry.tuning_prisms.clone(),
        label: entry.label.clone(),
    }
}

fn editor_entry_from_config(
    kind: PresetEntryKind,
    entry: &d4lc_config::PresetItemAffix,
) -> d4lc_gui::PresetEntry {
    d4lc_gui::PresetEntry {
        kind,
        id: entry.id.clone(),
        label: entry.label.clone(),
        item_type: if entry.item_type.is_empty() {
            None
        } else {
            Some(entry.item_type.clone())
        },
        color: entry.color.to_string_form(),
        is_any_type: entry.is_any_type,
        is_greater: entry.is_greater,
        is_implicit: entry.is_implicit,
        is_tempered: entry.is_tempered,
        tuning_prisms: entry.tuning_prisms.clone(),
    }
}

/// Convert the persisted preset paragon list
/// (`d4lc_config::AffixPreset::paragon_boards_list`) into the rules'
/// route shape the overlay consumes. Journey steps have no persisted
/// name (the upstream file shape has no field for one), so hydrated
/// steps project unnamed; maxroll's named steps stay named only for
/// the live session sourced from the import result.
fn paragon_routes_from_preset_boards(
    steps: &[Vec<d4lc_config::PresetParagonBoard>],
) -> Vec<ImportedParagonRoute> {
    steps
        .iter()
        .map(|step| ImportedParagonRoute {
            step_name: None,
            boards: step
                .iter()
                .map(|board| ImportedParagonBoard {
                    index: board.index,
                    name: board.name.clone(),
                    glyph: board.glyph.clone(),
                    rotation: board.rotation.clone(),
                    active_nodes: board.active_nodes.clone(),
                })
                .collect(),
        })
        .collect()
}

/// Convert the rules' route shape into the persisted preset paragon
/// list. A board keeps the `Index` it was imported or loaded with (upstream
/// sets it for D2Core only; every other provider leaves the entity default
/// `0`), and the list position carries the board order.
fn preset_boards_from_paragon_routes(
    routes: &[ImportedParagonRoute],
) -> Vec<Vec<d4lc_config::PresetParagonBoard>> {
    routes
        .iter()
        .map(|route| {
            route
                .boards
                .iter()
                .map(|board| d4lc_config::PresetParagonBoard {
                    name: board.name.clone(),
                    glyph: board.glyph.clone(),
                    rotation: board.rotation.clone(),
                    active_nodes: board.active_nodes.clone(),
                    index: board.index,
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
