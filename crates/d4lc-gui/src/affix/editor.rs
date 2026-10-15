//! Affix preset editor state operations and the build merge.

use crate::{
    AffixPresetEditorPreset, AffixPresetEditorState, ImportWarningView, PresetEntry,
    PresetEntryKind, PresetExportRequest, RulePresetProjection,
};

impl AffixPresetEditorState {
    pub fn new(mut presets: Vec<AffixPresetEditorPreset>) -> Self {
        sort_presets(&mut presets);
        let selected_preset_name = presets.first().map(|preset| preset.name.clone());
        Self {
            presets,
            selected_preset_name,
            import_warnings: Vec::new(),
            export_requests: Vec::new(),
            rules_revision: 0,
            paragon_boards_by_preset: Vec::new(),
        }
    }

    pub fn create_preset(&mut self, name: &str) -> Result<(), String> {
        let name = normalized_preset_name(name)?;
        if self.presets.iter().any(|preset| preset.name == name) {
            return Err(format!("preset '{name}' already exists"));
        }
        self.presets.push(AffixPresetEditorPreset {
            name: name.clone(),
            entries: Vec::new(),
        });
        sort_presets(&mut self.presets);
        self.selected_preset_name = Some(name);
        self.bump_rules_revision();
        Ok(())
    }

    pub fn rename_preset(&mut self, old_name: &str, new_name: &str) -> Result<(), String> {
        let new_name = normalized_preset_name(new_name)?;
        if self
            .presets
            .iter()
            .any(|preset| preset.name == new_name && preset.name != old_name)
        {
            return Err(format!("preset '{new_name}' already exists"));
        }
        let preset = self
            .presets
            .iter_mut()
            .find(|preset| preset.name == old_name)
            .ok_or_else(|| format!("preset '{old_name}' not found"))?;
        preset.name.clone_from(&new_name);
        sort_presets(&mut self.presets);
        // Upstream stores the paragon boards INSIDE the preset
        // (`AffixPreset.ParagonBoardsList`), so a rename moves them with
        // it; the keyed store mirrors that by moving the entry's key.
        if let Some((name, _)) = self
            .paragon_boards_by_preset
            .iter_mut()
            .find(|(name, _)| name == old_name)
        {
            *name = new_name.clone();
        }
        if self.selected_preset_name.as_deref() == Some(old_name) {
            self.selected_preset_name = Some(new_name);
        }
        self.bump_rules_revision();
        Ok(())
    }

    pub fn remove_preset(&mut self, name: &str) -> Result<(), String> {
        let initial_len = self.presets.len();
        self.presets.retain(|preset| preset.name != name);
        if self.presets.len() == initial_len {
            return Err(format!("preset '{name}' not found"));
        }
        // Removing the preset deletes its stored boards with it (upstream
        // keeps paragon inside the preset).
        self.paragon_boards_by_preset.retain(|(key, _)| key != name);
        if self.selected_preset_name.as_deref() == Some(name) {
            self.selected_preset_name = self.presets.first().map(|preset| preset.name.clone());
        }
        self.bump_rules_revision();
        Ok(())
    }

    /// Record the paragon routes an import produced for `preset_name`.
    ///
    /// An empty route list removes a previous entry: re-importing a
    /// preset without paragon must not keep the stale boards of an
    /// earlier import under the same name.
    pub fn set_paragon_routes(
        &mut self,
        preset_name: &str,
        routes: Vec<d4lc_rules::ImportedParagonRoute>,
    ) {
        self.paragon_boards_by_preset
            .retain(|(name, _)| name != preset_name);
        if !routes.is_empty() {
            self.paragon_boards_by_preset
                .push((preset_name.to_string(), routes));
        }
    }

    /// The stored paragon routes of one preset, or `None` when the preset
    /// carries no boards (the empty-list shape is never stored).
    pub fn paragon_routes_for(
        &self,
        preset_name: &str,
    ) -> Option<&[d4lc_rules::ImportedParagonRoute]> {
        self.paragon_boards_by_preset
            .iter()
            .find(|(name, _)| name == preset_name)
            .map(|(_, routes)| routes.as_slice())
    }

    pub fn import_preset(
        &mut self,
        preset: AffixPresetEditorPreset,
        warnings: Vec<ImportWarningView>,
    ) {
        self.presets.retain(|existing| existing.name != preset.name);
        self.selected_preset_name = Some(preset.name.clone());
        self.presets.push(preset);
        sort_presets(&mut self.presets);
        self.import_warnings = warnings;
        self.bump_rules_revision();
    }

    pub fn export_preset(&mut self, name: &str) -> Result<PresetExportRequest, String> {
        if !self.presets.iter().any(|preset| preset.name == name) {
            return Err(format!("preset '{name}' not found"));
        }
        let request = PresetExportRequest {
            preset_name: name.to_string(),
            suggested_path: format!("Exports/{name}.json"),
        };
        self.export_requests.push(request.clone());
        Ok(request)
    }

    /// Upsert `entry` into `preset_name`, keyed by (kind, id, item type).
    ///
    /// ## Aspect color sharing
    ///
    /// Upstream's aspect color dialog writes the chosen color to the
    /// targeted wanted entry, copies it to every wanted aspect of the
    /// selected preset that shares the id ("Set same color to all other gear
    /// slots", `AffixViewModel.cs:886-894`) and saves. Adding an aspect is a
    /// different path: `AffixManager.AddAspect` appends the new slot with
    /// `DefaultColorAspects` and leaves the existing same-id entries
    /// untouched (`AffixManager.cs:178-195`). This upsert therefore shares
    /// the incoming color across same-id aspects only when it *updates* an
    /// existing (kind, id, item type) key — the color dialog's path — and
    /// never re-colors when it appends a new slot.
    ///
    /// The other kinds keep per-entry colors: upstream's
    /// `SetAffixColorCommand` targets exactly one `ItemAffix` in the affix
    /// slot lists and in the sigil/unique/rune lists
    /// (`AffixView.xaml:194-196,951,1018,1085`), so their entries are not
    /// re-colored.
    pub fn edit_entry(&mut self, preset_name: &str, entry: PresetEntry) -> Result<(), String> {
        let preset = self.preset_mut(preset_name)?;
        let updates_existing_key = preset
            .entries
            .iter()
            .any(|existing| same_entry_key(existing, &entry));
        let shared_aspect_color = (entry.kind == PresetEntryKind::Aspect && updates_existing_key)
            .then(|| entry.color.clone());
        preset
            .entries
            .retain(|existing| !same_entry_key(existing, &entry));
        if let Some(color) = shared_aspect_color {
            for existing in preset.entries.iter_mut() {
                if existing.kind == PresetEntryKind::Aspect && existing.id == entry.id {
                    existing.color.clone_from(&color);
                }
            }
        }
        preset.entries.push(entry);
        self.bump_rules_revision();
        Ok(())
    }

    pub fn remove_entry(
        &mut self,
        preset_name: &str,
        kind: PresetEntryKind,
        id: &str,
        item_type: Option<&str>,
    ) -> Result<(), String> {
        let preset = self.preset_mut(preset_name)?;
        let initial_len = preset.entries.len();
        preset.entries.retain(|entry| {
            !(entry.kind == kind && entry.id == id && entry.item_type.as_deref() == item_type)
        });
        if preset.entries.len() == initial_len {
            return Err(format!("entry '{id}' not found in preset '{preset_name}'"));
        }
        self.bump_rules_revision();
        Ok(())
    }

    pub fn to_rule_presets(&self) -> RulePresetProjection {
        RulePresetProjection {
            affix_presets: self
                .presets
                .iter()
                .map(|preset| d4lc_rules::AffixPreset {
                    name: preset.name.clone(),
                    affixes: preset
                        .entries
                        .iter()
                        .filter(|entry| entry.kind == PresetEntryKind::Affix)
                        .map(affix_requirement_from_entry)
                        .collect(),
                })
                .collect(),
            non_affix_presets: self
                .presets
                .iter()
                .map(non_affix_preset_from_editor_preset)
                .collect(),
        }
    }

    pub(super) fn preset_mut(
        &mut self,
        name: &str,
    ) -> Result<&mut AffixPresetEditorPreset, String> {
        self.presets
            .iter_mut()
            .find(|preset| preset.name == name)
            .ok_or_else(|| format!("preset '{name}' not found"))
    }

    /// Public read-only accessor for the currently selected
    /// preset's entries. Returns
    /// `None` if no preset is selected.
    ///
    /// The mutation helper `preset_mut` is visible only to the affix
    /// view-model; this public accessor gives the Slint adapter
    /// read-only access to seed dialog form values and count
    /// entries without exposing mutation internals.
    pub fn selected_preset_entries(&self) -> Option<&[PresetEntry]> {
        let name = self.selected_preset_name.as_deref()?;
        self.presets
            .iter()
            .find(|preset| preset.name == name)
            .map(|preset| preset.entries.as_slice())
    }

    /// Public read-only accessor for the number of entries in
    /// the currently selected preset.
    /// Returns 0 if no preset is selected.
    pub fn selected_preset_entry_count(&self) -> usize {
        self.selected_preset_entries()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    /// Public read-only accessor for a specific entry in the
    /// currently selected preset by index. Returns `None` if the index is out of
    /// range or no preset is selected.
    pub fn selected_preset_entry(&self, index: usize) -> Option<&PresetEntry> {
        self.selected_preset_entries()?.get(index)
    }

    fn bump_rules_revision(&mut self) {
        self.rules_revision = self.rules_revision.saturating_add(1);
    }
}

fn sort_presets(presets: &mut [AffixPresetEditorPreset]) {
    // `string.Compare(..., StringComparison.Ordinal)` (AffixManager.cs:120-122).
    presets.sort_by(|left, right| d4lc_config::compare_ordinal(&left.name, &right.name));
}

fn normalized_preset_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        Err("preset name must not be empty".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn same_entry_key(left: &PresetEntry, right: &PresetEntry) -> bool {
    left.kind == right.kind && left.id == right.id && left.item_type == right.item_type
}

// ── Build merge (upstream `MergeBuildsExecute`,
//    ImportAffixPresetViewModel.cs:904-1024) ───────────────────────────────

/// The per-row color settings of the import dialog's Merge tab.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeColorOptions {
    /// `ChangeColorBuild1`: recolor every entry taken from Build 1 to
    /// [`Self::color_build1`].
    pub change_color_build1: bool,
    pub color_build1: String,
    /// `ChangeColorBuild2`: recolor every entry appended from Build 2 to
    /// [`Self::color_build2`].
    pub change_color_build2: bool,
    pub color_build2: String,
    /// `ChangeColorBuild12`: recolor entries present in BOTH builds to
    /// [`Self::color_build12`] (otherwise the Build 1 color stays).
    pub change_color_build12: bool,
    pub color_build12: String,
}

/// Merge two presets the way upstream's Merge tab does.
///
/// `requested_name` is the confirmed Confirm-name entry; an empty or
/// whitespace name falls back to Build 1's preset name (upstream
/// `buildName = string.IsNullOrWhiteSpace(buildName) ? …Build1.Name`).
///
/// The result starts from Build 1's affixes, aspects, and sigils —
/// uniques and runes are NOT merged from either side, exactly like
/// upstream, which only touches `ItemAffixes`/`ItemAspects`/`ItemSigils`.
/// Build 1's entries are recolored when `ChangeColorBuild1` is set. Then
/// Build 2's affixes/aspects/sigils are appended when their
/// (kind, id, item-type) key is new — the upstream `(Id, Type)` pair per
/// entity list — recolored when `ChangeColorBuild2` is set; on a
/// duplicate the EXISTING Build 1 entry is recolored to the Build 1+2
/// color when `ChangeColorBuild12` is set, otherwise its color stays.
pub fn merge_affix_presets(
    build1: &AffixPresetEditorPreset,
    build2: &AffixPresetEditorPreset,
    requested_name: &str,
    options: &MergeColorOptions,
) -> AffixPresetEditorPreset {
    let name = requested_name.trim();
    let name = if name.is_empty() {
        build1.name.as_str()
    } else {
        name
    };

    let mut merged = AffixPresetEditorPreset {
        name: name.to_string(),
        entries: Vec::new(),
    };
    // Build 1: affixes (with their flags), aspects, and sigils — with
    // the Build 1 swatch color where the row checkbox is set. Uniques
    // and runes are not merged from either side (upstream only touches
    // ItemAffixes / ItemAspects / ItemSigils).
    for entry in &build1.entries {
        let mergeable = matches!(
            entry.kind,
            PresetEntryKind::Affix | PresetEntryKind::Aspect | PresetEntryKind::Sigil
        );
        if !mergeable {
            continue;
        }
        let mut entry = entry.clone();
        if options.change_color_build1 {
            entry.color = options.color_build1.clone();
        }
        merged.entries.push(entry);
    }
    // Build 2: append new (kind, id, item-type) keys, recolor existing
    // ones to the Build 1+2 swatch where that checkbox is set.
    for entry in &build2.entries {
        let mergeable = matches!(
            entry.kind,
            PresetEntryKind::Affix | PresetEntryKind::Aspect | PresetEntryKind::Sigil
        );
        if !mergeable {
            continue;
        }
        match merged
            .entries
            .iter_mut()
            .find(|existing| same_entry_key(existing, entry))
        {
            Some(existing) => {
                if options.change_color_build12 {
                    existing.color = options.color_build12.clone();
                }
            }
            None => {
                let mut entry = entry.clone();
                if options.change_color_build2 {
                    entry.color = options.color_build2.clone();
                }
                merged.entries.push(entry);
            }
        }
    }
    merged
}

fn affix_requirement_from_entry(entry: &PresetEntry) -> d4lc_rules::AffixRequirement {
    d4lc_rules::AffixRequirement {
        affix_id: entry.id.clone(),
        label: entry.label.clone(),
        item_type: entry.item_type.clone().unwrap_or_default(),
        color: entry.color.clone(),
        // A type-less editor entry has always meant "any item type" in
        // this model; the explicit upstream flag additionally covers a
        // typed entry that also matches any type. The fallback
        // keeps the pre-existing rule-match decision for type-less
        // entries unchanged.
        is_any_type: entry.is_any_type || entry.item_type.is_none(),
        is_greater: entry.is_greater,
        is_implicit: entry.is_implicit,
        is_tempered: entry.is_tempered,
        tuning_prisms: entry.tuning_prisms.clone(),
    }
}

fn non_affix_preset_from_editor_preset(
    preset: &AffixPresetEditorPreset,
) -> d4lc_rules::NonAffixPreset {
    let mut powers = Vec::new();
    let mut runes = Vec::new();
    let mut sigils = Vec::new();
    for entry in &preset.entries {
        match entry.kind {
            PresetEntryKind::Affix => {}
            PresetEntryKind::Aspect | PresetEntryKind::Unique => {
                powers.push(non_affix_requirement_from_entry(entry))
            }
            PresetEntryKind::Rune => runes.push(non_affix_requirement_from_entry(entry)),
            PresetEntryKind::Sigil => sigils.push(non_affix_requirement_from_entry(entry)),
        }
    }
    d4lc_rules::NonAffixPreset {
        name: preset.name.clone(),
        powers,
        runes,
        sigils,
    }
}

fn non_affix_requirement_from_entry(entry: &PresetEntry) -> d4lc_rules::NonAffixRequirement {
    d4lc_rules::NonAffixRequirement {
        id: entry.id.clone(),
        item_type: entry.item_type.clone(),
        color: entry.color.clone(),
        kind: match entry.kind {
            PresetEntryKind::Affix => d4lc_rules::RuleNonAffixKind::Aspect,
            PresetEntryKind::Aspect => d4lc_rules::RuleNonAffixKind::Aspect,
            PresetEntryKind::Unique => d4lc_rules::RuleNonAffixKind::Unique,
            PresetEntryKind::Rune => d4lc_rules::RuleNonAffixKind::Rune,
            PresetEntryKind::Sigil => d4lc_rules::RuleNonAffixKind::Sigil,
        },
    }
}
