//! Affix ViewModel — pure state + command methods returning typed effects.
//!
//! ## Architecture
//!
//! `AffixViewModel` wraps state from `d4lc-gui` and provides command methods
//! that return `Vec<AffixEffect>`.
//!
//! ## Boundary rules
//!
//! - No Slint handles, no file I/O, no platform state, no runtime handles.

pub mod catalog;
pub(crate) mod editor;

use crate::{AffixPresetEditorState, PresetEntry, PresetEntryKind, SettingsPanelState};

/// Effect emitted by an [`AffixViewModel`] command.
#[derive(Debug, Clone, PartialEq)]
pub enum AffixEffect {
    /// Open a config dialog
    OpenDialog(AffixDialogKind),
    /// Persist affix state
    PersistConfig,
    /// Toggle the live overlay on/off through the app adapter's
    /// overlay-config confirm path (upstream `ToggleOverlayCommand`
    /// → `IsAffixOverlayEnabled` setter → `ToggleOverlayFromGUIMessage`,
    /// AffixViewModel.cs:260-272,978-981; the adapter flips the
    /// current `overlay_enabled` and persists it).
    ToggleOverlay,
}

/// Identifies which config dialog to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffixDialogKind {
    Affix,
    Aspect,
    Sigil,
    Unique,
    Rune,
}

/// ViewModel for the Affix screen.
///
/// Pure state + command methods wrapping [`AffixPresetEditorState`].
#[derive(Debug, Clone, PartialEq)]
pub struct AffixViewModel {
    pub state: AffixPresetEditorState,
}

impl AffixViewModel {
    pub fn new(state: AffixPresetEditorState) -> Self {
        Self { state }
    }

    // ── Config dialog commands ───────────────────────────────────────

    pub fn open_affix_config(&mut self) -> Vec<AffixEffect> {
        vec![AffixEffect::OpenDialog(AffixDialogKind::Affix)]
    }

    pub fn open_aspect_config(&mut self) -> Vec<AffixEffect> {
        vec![AffixEffect::OpenDialog(AffixDialogKind::Aspect)]
    }

    pub fn open_sigil_config(&mut self) -> Vec<AffixEffect> {
        vec![AffixEffect::OpenDialog(AffixDialogKind::Sigil)]
    }

    pub fn open_unique_config(&mut self) -> Vec<AffixEffect> {
        vec![AffixEffect::OpenDialog(AffixDialogKind::Unique)]
    }

    pub fn open_rune_config(&mut self) -> Vec<AffixEffect> {
        vec![AffixEffect::OpenDialog(AffixDialogKind::Rune)]
    }

    // ── Entry commands ───────────────────────────────────────────────

    /// Remove an affix entry by index from the currently selected preset.
    pub fn remove_affix_entry(&mut self, index: usize) -> Vec<AffixEffect> {
        let preset_name = match self.state.selected_preset_name.clone() {
            Some(name) => name,
            None => return Vec::new(),
        };
        if let Ok(preset) = self.state.preset_mut(&preset_name) {
            if index < preset.entries.len() {
                preset.entries.remove(index);
                return vec![AffixEffect::PersistConfig];
            }
        }
        Vec::new()
    }

    /// Toggle the affix overlay on/off.
    ///
    /// The overlay state is app-shell config, not preset-editor state,
    /// so this command only emits the typed effect; the `d4lc-app`
    /// adapter flips the current `overlay_enabled` through the shared
    /// `confirm_overlay_config` path (the same path the Ctrl+F12
    /// hotkey arm uses). Upstream: `ToggleOverlayExecute` sets
    /// `IsAffixOverlayEnabled`, whose setter broadcasts
    /// `ToggleOverlayFromGUIMessage` (AffixViewModel.cs:260-272,
    /// 978-981).
    pub fn toggle_affix_overlay(&mut self) -> Vec<AffixEffect> {
        vec![AffixEffect::ToggleOverlay]
    }
}

// ── SetAffix dialog ────────────────────────────────

/// Draft of the `SetAffixDialog` form values, sourced from the
/// root in-out props on the `AppWindow` host.
///
/// Upstream parity
/// (`upstream/Diablo4Companion/D4Companion/ViewModels/Dialogs/SetAffixViewModel.cs`):
/// the dialog configures a single
/// `PresetEntry` (affix kind) with id, item-type filter, color,
/// and three boolean flags (`is_greater`, `is_implicit`,
/// `is_tempered`). The Rust `PresetEntry` struct
/// (`d4lc-gui/src/lib.rs:58-68`) maps directly to these fields.
///
/// `edit_index` is `-1` for a new entry and `>= 0` for editing
/// an existing entry in the currently selected preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetAffixDraft {
    pub edit_index: i32,
    pub id: String,
    pub item_type: Option<String>,
    pub color: String,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
}

impl SetAffixDraft {
    /// Construct a draft from the form values the Slint form posts
    /// to Rust. `id` and `color` are trimmed; `item_type` is `None`
    /// when blank.
    pub fn new(
        edit_index: i32,
        id: &str,
        item_type: &str,
        color: &str,
        is_greater: bool,
        is_implicit: bool,
        is_tempered: bool,
    ) -> Self {
        let trimmed_type = item_type.trim();
        Self {
            edit_index,
            id: id.trim().to_string(),
            item_type: if trimmed_type.is_empty() {
                None
            } else {
                Some(trimmed_type.to_string())
            },
            color: color.trim().to_string(),
            is_greater,
            is_implicit,
            is_tempered,
        }
    }

    /// Convert the draft into a `PresetEntry` with `PresetEntryKind::Affix`.
    ///
    /// The form edits id, item type, color and the flags; `IsAnyType`
    /// and `TuningPrisms` are not part of it. A new entry starts with
    /// the entity defaults; the edit path preserves an existing entry's
    /// values through `preserve_entry_metadata_on_edit`.
    pub fn into_preset_entry(self) -> PresetEntry {
        PresetEntry {
            kind: PresetEntryKind::Affix,
            id: self.id,
            label: None,
            item_type: self.item_type,
            color: self.color,
            is_any_type: false,
            is_greater: self.is_greater,
            is_implicit: self.is_implicit,
            is_tempered: self.is_tempered,
            tuning_prisms: Vec::new(),
        }
    }
}

/// Validate the `SetAffixDialog` form draft and return the
/// canonical effects the caller should dispatch.
///
/// On success returns `Ok(vec![AffixEffect::PersistConfig])` so
/// the `d4lc-app` adapter knows the preset state changed.
/// On validation failure returns `Err(message)` without mutating
/// any state; the caller surfaces the error through the shared
/// `dialog-error` contract and keeps the dialog open.
///
/// ## Validation rules
///
/// - `id` must be non-empty and at most 64 characters.
/// - `color` must be non-empty and at most 16 characters.
/// - `edit_index` must be either `-1` (new entry) or a valid
///   index into the selected preset's entries. For a new entry
///   (`edit_index == -1`), no collision check is done here —
///   `edit_entry` upserts by key so re-adding the same id just
///   replaces the existing entry. For an edit, the index must
///   be in range.
pub fn confirm_set_affix_draft(
    draft: &SetAffixDraft,
    selected_preset: Option<&str>,
    current_entry_count: usize,
) -> Result<Vec<AffixEffect>, String> {
    if draft.id.is_empty() {
        return Err("Affix id is required.".to_string());
    }
    if draft.id.chars().count() > 64 {
        return Err("Affix id must be 64 characters or fewer.".to_string());
    }
    if draft.color.is_empty() {
        return Err("Color is required.".to_string());
    }
    if draft.color.chars().count() > 16 {
        return Err("Color must be 16 characters or fewer.".to_string());
    }
    if selected_preset.is_none() {
        return Err("No preset selected.".to_string());
    }
    if draft.edit_index < 0 {
        // New entry — no further index check needed.
    } else if (draft.edit_index as usize) >= current_entry_count {
        return Err(format!(
            "Entry index {} out of range ({} entries).",
            draft.edit_index, current_entry_count
        ));
    }
    Ok(vec![AffixEffect::PersistConfig])
}

// ── SetAffixColor dialog ───────────────────────────

/// Draft of the `SetAffixColorDialog` form values.
///
/// Upstream parity
/// (`upstream/Diablo4Companion/D4Companion/ViewModels/Dialogs/SetAffixColorViewModel.cs`):
/// the dialog picks a color for an existing
/// `PresetEntry`. The Rust side only needs the color value and
/// the target entry index; `target_index` identifies which
/// entry in the currently selected preset to update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetAffixColorDraft {
    pub target_index: i32,
    pub color: String,
}

impl SetAffixColorDraft {
    /// Construct a draft from the form values the Slint form posts
    /// to Rust. `color` is trimmed.
    pub fn new(target_index: i32, color: &str) -> Self {
        Self {
            target_index,
            color: color.trim().to_string(),
        }
    }
}

/// Predefined color palette used by the `SetAffixColorDialog`.
///
/// Mirrors the upstream color palette in
/// `SetAffixColorView.xaml` (a small set of named colors for
/// quick selection). The palette is exposed as a public constant
/// so the Slint dialog can render it without duplicating the
/// list.
pub const AFFIX_COLOR_PALETTE: &[(&str, &str)] = &[
    ("White", "#FFFFFF"),
    ("Red", "#FF0000"),
    ("Orange", "#FF8800"),
    ("Yellow", "#FFFF00"),
    ("Green", "#00FF00"),
    ("Cyan", "#00FFFF"),
    ("Blue", "#0088FF"),
    ("Purple", "#8800FF"),
    ("Magenta", "#FF00FF"),
    ("Gray", "#888888"),
];

/// Validate the `SetAffixColorDialog` form draft and return the
/// canonical effects the caller should dispatch.
///
/// On success returns `Ok(vec![AffixEffect::PersistConfig])`.
/// On validation failure returns `Err(message)` without mutating
/// any state; the caller surfaces the error through the shared
/// `dialog-error` contract and keeps the dialog open.
///
/// ## Validation rules
///
/// - `color` must be non-empty and at most 16 characters.
/// - `target_index` must be a valid index into the selected
///   preset's entries.
pub fn confirm_set_affix_color_draft(
    draft: &SetAffixColorDraft,
    selected_preset: Option<&str>,
    current_entry_count: usize,
) -> Result<Vec<AffixEffect>, String> {
    if draft.color.is_empty() {
        return Err("Color is required.".to_string());
    }
    if draft.color.chars().count() > 16 {
        return Err("Color must be 16 characters or fewer.".to_string());
    }
    if selected_preset.is_none() {
        return Err("No preset selected.".to_string());
    }
    if draft.target_index < 0 || (draft.target_index as usize) >= current_entry_count {
        return Err(format!(
            "Target index {} out of range ({} entries).",
            draft.target_index, current_entry_count
        ));
    }
    Ok(vec![AffixEffect::PersistConfig])
}

// ── Affix-family config dialogs ────────────────────────────
//
// The five per-family config dialogs (`AffixConfigView.xaml`,
// `AspectConfigView.xaml`, `SigilConfigView.xaml`, `UniqueConfigView.xaml`,
// `RuneConfigView.xaml`) edit persisted rule settings, and the SetAffix
// dialog edits the selected preset's per-slot entries plus the affix's
// minimal value. The dialog-facing logic lives here so the `d4lc-app`
// adapter stays a thin Slint reader, and so every rule below is testable
// without a window.

/// The ten upstream affix item types, in the upstream button order
/// (`D4Companion.Constants.ItemTypeConstants`; the per-slot add buttons of
/// `SetAffixView.xaml:31-98`).
pub const AFFIX_SLOT_TOKENS: [&str; 10] = [
    "helm", "chest", "gloves", "pants", "boots", "amulet", "ring", "weapon", "ranged", "offhand",
];

/// The upstream per-slot button labels (the `ToolTip` values of
/// `SetAffixView.xaml:35-95`), index-aligned with [`AFFIX_SLOT_TOKENS`].
pub const AFFIX_SLOT_LABELS: [&str; 10] = [
    "Helm",
    "Chest",
    "Gloves",
    "Pants",
    "Boots",
    "Amulet",
    "Ring",
    "Weapon",
    "Ranged Weapon",
    "Offhand",
];

/// The index of an upstream item-type token in [`AFFIX_SLOT_TOKENS`].
pub fn affix_slot_index(token: &str) -> Option<usize> {
    AFFIX_SLOT_TOKENS
        .iter()
        .position(|candidate| *candidate == token)
}

/// Append one per-slot affix entry to a preset, mirroring upstream
/// `AffixManager.AddAffix` (AffixManager.cs:141-158): the entry carries the
/// clicked item type and the normal-color default, and every click appends
/// a new entry (upstream does not dedupe — the SetAffix dialog's Done gate
/// rejects the resulting duplicates instead).
///
/// `state.rules_revision` is bumped like every other editor mutation so the
/// rule projection rebuilds after the append.
pub fn append_affix_slot_entry(
    state: &mut AffixPresetEditorState,
    preset_name: &str,
    affix_id: &str,
    slot_token: &str,
    color: &str,
) -> Result<PresetEntry, String> {
    if affix_slot_index(slot_token).is_none() {
        return Err(format!("Unknown affix item type {slot_token:?}."));
    }
    let preset = state
        .presets
        .iter_mut()
        .find(|preset| preset.name == preset_name)
        .ok_or_else(|| format!("preset '{preset_name}' not found"))?;
    let entry = PresetEntry {
        kind: PresetEntryKind::Affix,
        id: affix_id.to_string(),
        label: None,
        item_type: Some(slot_token.to_string()),
        color: color.to_string(),
        is_any_type: false,
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    };
    preset.entries.push(entry.clone());
    state.rules_revision = state.rules_revision.saturating_add(1);
    Ok(entry)
}

/// The upstream per-slot counters (`SetAffixViewModel.AffixCounterHead` …
/// `AffixCounterOffHand`, SetAffixViewModel.cs:88-176): how many entries of
/// the given affix id carry each item type.
pub fn affix_slot_counters(entries: &[PresetEntry], affix_id: &str) -> [usize; 10] {
    let mut counters = [0_usize; AFFIX_SLOT_TOKENS.len()];
    for entry in entries {
        if entry.kind != PresetEntryKind::Affix || entry.id != affix_id {
            continue;
        }
        if let Some(index) = entry.item_type.as_deref().and_then(affix_slot_index) {
            counters[index] += 1;
        }
    }
    counters
}

/// One duplicate group of the selected affixes, keyed like the upstream
/// Done gate (`SetAffixViewModel.CanSetAffixDoneExecute`,
/// SetAffixViewModel.cs:274-280): the same affix id grouped by
/// `(Type, IsImplicit, IsGreater, IsTempered)` with a count above one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixDuplicateGroup {
    pub item_type: Option<String>,
    pub is_implicit: bool,
    pub is_greater: bool,
    pub is_tempered: bool,
    pub count: usize,
}

/// The duplicate groups that block the SetAffix dialog's Done (upstream
/// disables the button; the d4lc adapter surfaces the same condition as a
/// dialog error because the gate reads the persisted preset).
pub fn affix_duplicate_groups(entries: &[PresetEntry], affix_id: &str) -> Vec<AffixDuplicateGroup> {
    let mut groups: Vec<AffixDuplicateGroup> = Vec::new();
    for entry in entries {
        if entry.kind != PresetEntryKind::Affix || entry.id != affix_id {
            continue;
        }
        match groups.iter_mut().find(|group| {
            group.item_type == entry.item_type
                && group.is_implicit == entry.is_implicit
                && group.is_greater == entry.is_greater
                && group.is_tempered == entry.is_tempered
        }) {
            Some(group) => group.count += 1,
            None => groups.push(AffixDuplicateGroup {
                item_type: entry.item_type.clone(),
                is_implicit: entry.is_implicit,
                is_greater: entry.is_greater,
                is_tempered: entry.is_tempered,
                count: 1,
            }),
        }
    }
    groups.retain(|group| group.count > 1);
    groups
}

/// Upper bound of the upstream minimal-value field
/// (`SetAffixView.xaml:107-112`: `Minimum="0" Maximum="9999"`).
pub const MINIMAL_AFFIX_VALUE_MAX: f64 = 9999.0;

/// Clamp one minimal-value field entry to the upstream `NumericUpDown`
/// bounds (`Minimum="0" Maximum="9999" Interval="0.1" StringFormat="N1"`,
/// SetAffixView.xaml:107-112).
///
/// `None` means the text is not a complete number yet — blank, a lone
/// sign, a trailing separator — which the live-apply path ignores exactly
/// like the `NumericUpDown` refuses an incomplete entry instead of
/// committing it. A complete number is clamped to the control's bounds and
/// rounded to its one-decimal step, mirroring how the control's own value
/// coercion behaves.
pub fn clamp_minimal_affix_value(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: f64 = trimmed.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    let clamped = value.clamp(0.0, MINIMAL_AFFIX_VALUE_MAX);
    Some((clamped * 10.0).round() / 10.0)
}

/// The upstream sigil dungeon tier list in cycle order
/// (`SigilInfoVM.Tiers`, SigilInfoVM.cs:67-76).
pub const SIGIL_DUNGEON_TIERS: [&str; 7] = ["S", "A", "B", "C", "D", "E", "F"];

/// The tier upstream reports for a sigil that has no stored tier
/// (`AffixManager.GetSigilDungeonTier`, AffixManager.cs:744-747).
pub const SIGIL_DUNGEON_TIER_DEFAULT: &str = "F";

/// The next tier of the upstream cycle
/// (`SigilInfoVM.SetSigilDungeonTierToNextExecute`, SigilInfoVM.cs:139-149):
/// `(index + 1) % count`, where an unknown current tier lands on `S` —
/// .NET's `List.IndexOf` returning `-1` wraps to index `0`.
pub fn next_sigil_dungeon_tier(current: &str) -> &'static str {
    let count = SIGIL_DUNGEON_TIERS.len();
    let index = SIGIL_DUNGEON_TIERS
        .iter()
        .position(|tier| tier.eq_ignore_ascii_case(current.trim()))
        .map(|index| index + 1)
        .unwrap_or(0);
    SIGIL_DUNGEON_TIERS[index % count]
}

/// The rule-setting keys the affix-family config dialogs report. Each key
/// names the persisted `rule_settings` field the toggle writes, and is the
/// `name` argument of the dialogs' `rule-toggle-changed` /
/// `rule-selection-changed` callbacks.
pub const RULE_SETTING_MINIMAL_AFFIX_VALUE_FILTER: &str = "minimal_affix_value_filter_enabled";
pub const RULE_SETTING_MULTI_BUILD_MODE: &str = "multi_build_mode_enabled";
pub const RULE_SETTING_TEMPERED_AFFIX_DETECTION: &str = "tempered_affix_detection_enabled";
pub const RULE_SETTING_ASPECT_DETECTION: &str = "aspect_detection_enabled";
pub const RULE_SETTING_UNIQUE_DETECTION: &str = "unique_detection_enabled";
pub const RULE_SETTING_RUNE_DETECTION: &str = "rune_detection_enabled";
pub const RULE_SETTING_TUNING_PRISM_HINTS: &str = "tuning_prism_hints_enabled";
pub const RULE_SETTING_DUNGEON_TIERS: &str = "dungeon_tiers_enabled";
pub const RULE_SETTING_SIGIL_DISPLAY_MODE: &str = "sigil_display_mode";

/// One dialog interaction's rule-setting patch: every field the dialogs can
/// report, with `None` for the fields the interaction does not touch. The
/// dialogs build one through [`rule_setting_toggle_patch`] /
/// [`rule_setting_selection_patch`], and the adapter writes the patch into
/// the settings panel state.
///
/// Which upstream control belongs to which dialog, and why only these
/// fields are exposed (each one has a live d4lc consumer):
///
/// | Dialog | Upstream controls | Consumer |
/// |--------|-------------------|----------|
/// | Affix  | tempered, tuning-prism, minimal-value filter, multi-build | runtime OCR segment plan, overlay descriptors, rule value filter, build-import mirror |
/// | Aspect | aspect detection, multi-build | runtime OCR segment plan (no aspect OCR for non-unique items), multi-build |
/// | Sigil  | dungeon tiers, multi-build, sigil display mode | sigil marker tier text, build-import mirror, sigil marker color |
/// | Unique | unique detection, multi-build | runtime OCR segment plan (no aspect OCR for unique and mythic items), multi-build |
/// | Rune   | rune detection, multi-build | runtime OCR segment plan (no affix OCR for rune tooltips), multi-build |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AffixFamilyConfigPatch {
    pub minimal_affix_value_filter_enabled: Option<bool>,
    pub multi_build_enabled: Option<bool>,
    pub tempered_affix_detection_enabled: Option<bool>,
    pub aspect_detection_enabled: Option<bool>,
    pub unique_detection_enabled: Option<bool>,
    pub rune_detection_enabled: Option<bool>,
    pub tuning_prism_hints_enabled: Option<bool>,
    pub dungeon_tiers_enabled: Option<bool>,
    pub sigil_display_mode: Option<d4lc_config::SigilDisplayModeSetting>,
}

impl AffixFamilyConfigPatch {
    /// Write the patch's fields into the settings panel state — the same
    /// projection the settings screen uses, so the panel and the persisted
    /// settings stay in step.
    pub fn apply_to_panel_state(&self, panel: &mut SettingsPanelState) {
        if let Some(value) = self.minimal_affix_value_filter_enabled {
            panel.minimal_affix_value_filter_enabled = value;
        }
        if let Some(value) = self.multi_build_enabled {
            panel.multi_build_mode_enabled = value;
        }
        if let Some(value) = self.tempered_affix_detection_enabled {
            panel.tempered_detection_enabled = value;
        }
        if let Some(value) = self.aspect_detection_enabled {
            panel.aspect_detection_enabled = value;
        }
        if let Some(value) = self.unique_detection_enabled {
            panel.unique_detection_enabled = value;
        }
        if let Some(value) = self.rune_detection_enabled {
            panel.rune_detection_enabled = value;
        }
        if let Some(value) = self.tuning_prism_hints_enabled {
            panel.is_tuning_prism_hints_enabled = value;
        }
        if let Some(value) = self.dungeon_tiers_enabled {
            panel.dungeon_tiers_enabled = value;
        }
        if let Some(value) = self.sigil_display_mode {
            panel.sigil_display_mode = value;
        }
    }
}

/// The one-field patch for a `rule-toggle-changed(name, value)` report, or
/// `None` for an unknown key (the adapter then ignores the report instead
/// of writing an arbitrary field).
pub fn rule_setting_toggle_patch(key: &str, value: bool) -> Option<AffixFamilyConfigPatch> {
    let patch = match key {
        RULE_SETTING_MINIMAL_AFFIX_VALUE_FILTER => AffixFamilyConfigPatch {
            minimal_affix_value_filter_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        RULE_SETTING_MULTI_BUILD_MODE => AffixFamilyConfigPatch {
            multi_build_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        RULE_SETTING_TEMPERED_AFFIX_DETECTION => AffixFamilyConfigPatch {
            tempered_affix_detection_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        RULE_SETTING_ASPECT_DETECTION => AffixFamilyConfigPatch {
            aspect_detection_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        RULE_SETTING_UNIQUE_DETECTION => AffixFamilyConfigPatch {
            unique_detection_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        RULE_SETTING_RUNE_DETECTION => AffixFamilyConfigPatch {
            rune_detection_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        RULE_SETTING_TUNING_PRISM_HINTS => AffixFamilyConfigPatch {
            tuning_prism_hints_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        RULE_SETTING_DUNGEON_TIERS => AffixFamilyConfigPatch {
            dungeon_tiers_enabled: Some(value),
            ..AffixFamilyConfigPatch::default()
        },
        _ => return None,
    };
    Some(patch)
}

/// The one-field patch for a `rule-selection-changed(name, value)` report
/// (the Sigil config dialog's display-mode selector,
/// `SigilConfigViewModel.cs:85-98`).
pub fn rule_setting_selection_patch(key: &str, value: &str) -> Option<AffixFamilyConfigPatch> {
    if key != RULE_SETTING_SIGIL_DISPLAY_MODE {
        return None;
    }
    Some(AffixFamilyConfigPatch {
        sigil_display_mode: Some(d4lc_config::SigilDisplayModeSetting::from_upstream(value)),
        ..AffixFamilyConfigPatch::default()
    })
}

/// The values the SetAffix dialog shows for the affix it edits: the
/// upstream per-slot counters and the stored minimal value
/// (`SetAffixViewModel.cs:88-182`). The adapter seeds the dialog's
/// `slot-counters` model and `minimal-value` field from this projection.
#[derive(Debug, Clone, PartialEq)]
pub struct SetAffixDialogSeed {
    pub slot_counters: [usize; 10],
    pub minimal_affix_value: Option<f64>,
}

/// Project the SetAffix dialog's seed values from the selected preset's
/// entries and the stored minimal values.
pub fn set_affix_dialog_seed(
    entries: &[PresetEntry],
    affix_id: &str,
    minimal_affix_values: &std::collections::BTreeMap<String, f64>,
) -> SetAffixDialogSeed {
    SetAffixDialogSeed {
        slot_counters: affix_slot_counters(entries, affix_id),
        minimal_affix_value: minimal_affix_values.get(affix_id).copied(),
    }
}

/// One sigil entry of the Sigil config dialog's tier list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigilTierRow {
    pub id: String,
    pub tier: String,
}

/// The Sigil config dialog's per-entry tier rows: every sigil entry of the
/// preset with the tier upstream would show it — the stored
/// `Config/DungeonTiers.json` value verbatim (upstream
/// `AffixManager.GetSigilDungeonTier`, AffixManager.cs:744-747, whose
/// values are free-form tier labels like `S2`/`T30`), or the `F` default
/// for a sigil without a stored tier.
pub fn sigil_tier_rows(
    entries: &[PresetEntry],
    dungeon_tiers: &std::collections::BTreeMap<String, String>,
) -> Vec<SigilTierRow> {
    entries
        .iter()
        .filter(|entry| entry.kind == PresetEntryKind::Sigil)
        .map(|entry| SigilTierRow {
            id: entry.id.clone(),
            tier: dungeon_tiers
                .get(&entry.id)
                .cloned()
                .unwrap_or_else(|| SIGIL_DUNGEON_TIER_DEFAULT.to_string()),
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_editor() -> AffixPresetEditorState {
        let mut editor = AffixPresetEditorState::new(Vec::new());
        editor.create_preset("Test").unwrap();
        editor
    }

    /// A settings panel in the persisted-default state (`AppSettings`
    /// defaults), the projection the config dialogs' patches write into.
    fn settings_panel() -> crate::SettingsPanelState {
        crate::SettingsPanelState::from_config(&d4lc_config::AppSettings::default(), Vec::new())
    }

    #[test]
    fn new_wraps_editor_state() {
        let editor = sample_editor();
        let vm = AffixViewModel::new(editor.clone());
        assert_eq!(vm.state, editor);
    }

    #[test]
    fn open_affix_config_returns_dialog_effect() {
        let mut vm = AffixViewModel::new(sample_editor());
        assert_eq!(
            vm.open_affix_config(),
            vec![AffixEffect::OpenDialog(AffixDialogKind::Affix)]
        );
    }

    #[test]
    fn open_aspect_config_returns_dialog_effect() {
        let mut vm = AffixViewModel::new(sample_editor());
        assert_eq!(
            vm.open_aspect_config(),
            vec![AffixEffect::OpenDialog(AffixDialogKind::Aspect)]
        );
    }

    #[test]
    fn open_sigil_config_returns_dialog_effect() {
        let mut vm = AffixViewModel::new(sample_editor());
        assert_eq!(
            vm.open_sigil_config(),
            vec![AffixEffect::OpenDialog(AffixDialogKind::Sigil)]
        );
    }

    #[test]
    fn open_unique_config_returns_dialog_effect() {
        let mut vm = AffixViewModel::new(sample_editor());
        assert_eq!(
            vm.open_unique_config(),
            vec![AffixEffect::OpenDialog(AffixDialogKind::Unique)]
        );
    }

    #[test]
    fn open_rune_config_returns_dialog_effect() {
        let mut vm = AffixViewModel::new(sample_editor());
        assert_eq!(
            vm.open_rune_config(),
            vec![AffixEffect::OpenDialog(AffixDialogKind::Rune)]
        );
    }

    #[test]
    fn remove_affix_entry_removes_entry() {
        let mut editor = sample_editor();
        let entry = crate::PresetEntry {
            kind: crate::PresetEntryKind::Affix,
            id: "test_affix".to_string(),
            label: Some("Test affix".to_string()),
            item_type: None,
            color: "#00FF00".to_string(),
            is_any_type: true,
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        };
        editor.preset_mut("Test").unwrap().entries.push(entry);
        assert_eq!(editor.preset_mut("Test").unwrap().entries.len(), 1);

        let mut vm = AffixViewModel::new(editor);
        let effects = vm.remove_affix_entry(0);
        assert!(vm.state.preset_mut("Test").unwrap().entries.is_empty());
        assert_eq!(effects, vec![AffixEffect::PersistConfig]);
    }

    #[test]
    fn remove_affix_entry_out_of_range_returns_empty() {
        let mut vm = AffixViewModel::new(sample_editor());
        let effects = vm.remove_affix_entry(99);
        assert!(effects.is_empty());
    }

    // ── SetAffix draft tests ─────────────────────────────

    #[test]
    fn set_affix_draft_new_trims_and_clears_item_type() {
        let draft = SetAffixDraft::new(
            -1,
            "  AttackSpeed  ",
            "  Amulet  ",
            "#FF0000",
            true,
            false,
            false,
        );
        assert_eq!(draft.id, "AttackSpeed");
        assert_eq!(draft.item_type.as_deref(), Some("Amulet"));
        assert_eq!(draft.color, "#FF0000");
        assert!(draft.is_greater);
        assert!(!draft.is_implicit);
        assert!(!draft.is_tempered);
    }

    #[test]
    fn set_affix_draft_new_empty_item_type_becomes_none() {
        let draft = SetAffixDraft::new(-1, "x", "   ", "#FF0000", false, false, false);
        assert!(draft.item_type.is_none());
    }

    #[test]
    fn toggle_affix_overlay_emits_toggle_effect() {
        let mut vm = AffixViewModel::new(AffixPresetEditorState::new(Vec::new()));
        assert_eq!(vm.toggle_affix_overlay(), vec![AffixEffect::ToggleOverlay]);
    }

    #[test]
    fn set_affix_draft_into_preset_entry_uses_affix_kind() {
        let draft = SetAffixDraft::new(0, "x", "y", "#00FF00", true, true, false);
        let entry = draft.into_preset_entry();
        assert_eq!(entry.kind, PresetEntryKind::Affix);
        assert_eq!(entry.id, "x");
        assert_eq!(entry.item_type.as_deref(), Some("y"));
        assert_eq!(entry.color, "#00FF00");
        assert!(entry.is_greater);
        assert!(entry.is_implicit);
        assert!(!entry.is_tempered);
    }

    #[test]
    fn confirm_set_affix_draft_accepts_valid_new_entry() {
        let draft = SetAffixDraft::new(-1, "AttackSpeed", "", "#FF0000", true, false, false);
        let effects = confirm_set_affix_draft(&draft, Some("Test"), 0)
            .expect("valid new entry should succeed");
        assert_eq!(effects, vec![AffixEffect::PersistConfig]);
    }

    #[test]
    fn confirm_set_affix_draft_accepts_valid_edit() {
        let draft = SetAffixDraft::new(0, "AttackSpeed", "Amulet", "#00FF00", false, true, true);
        let effects =
            confirm_set_affix_draft(&draft, Some("Test"), 3).expect("valid edit should succeed");
        assert_eq!(effects, vec![AffixEffect::PersistConfig]);
    }

    #[test]
    fn confirm_set_affix_draft_rejects_empty_id() {
        let draft = SetAffixDraft::new(-1, "   ", "", "#FF0000", false, false, false);
        let result = confirm_set_affix_draft(&draft, Some("Test"), 0);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("id is required"));
    }

    #[test]
    fn confirm_set_affix_draft_rejects_oversized_id() {
        let oversized = "A".repeat(65);
        let draft = SetAffixDraft::new(-1, &oversized, "", "#FF0000", false, false, false);
        let result = confirm_set_affix_draft(&draft, Some("Test"), 0);
        assert!(result.is_err());
    }

    #[test]
    fn confirm_set_affix_draft_rejects_empty_color() {
        let draft = SetAffixDraft::new(-1, "x", "", "   ", false, false, false);
        let result = confirm_set_affix_draft(&draft, Some("Test"), 0);
        assert!(result.is_err());
    }

    #[test]
    fn confirm_set_affix_draft_rejects_oversized_color() {
        let draft = SetAffixDraft::new(-1, "x", "", &"C".repeat(17), false, false, false);
        let result = confirm_set_affix_draft(&draft, Some("Test"), 0);
        assert!(result.is_err());
    }

    #[test]
    fn confirm_set_affix_draft_rejects_no_selected_preset() {
        let draft = SetAffixDraft::new(-1, "x", "", "#FF0000", false, false, false);
        let result = confirm_set_affix_draft(&draft, None, 0);
        assert!(result.is_err());
    }

    #[test]
    fn confirm_set_affix_draft_rejects_out_of_range_edit_index() {
        let draft = SetAffixDraft::new(5, "x", "", "#FF0000", false, false, false);
        let result = confirm_set_affix_draft(&draft, Some("Test"), 3);
        assert!(result.is_err());
    }

    // ── SetAffixColor draft tests ────────────────────────

    #[test]
    fn set_affix_color_draft_new_trims_color() {
        let draft = SetAffixColorDraft::new(0, "  #FF0000  ");
        assert_eq!(draft.target_index, 0);
        assert_eq!(draft.color, "#FF0000");
    }

    #[test]
    fn confirm_set_affix_color_draft_accepts_valid() {
        let draft = SetAffixColorDraft::new(1, "#00FF00");
        let effects = confirm_set_affix_color_draft(&draft, Some("Test"), 3)
            .expect("valid color draft should succeed");
        assert_eq!(effects, vec![AffixEffect::PersistConfig]);
    }

    #[test]
    fn confirm_set_affix_color_draft_rejects_empty_color() {
        let draft = SetAffixColorDraft::new(0, "   ");
        assert!(confirm_set_affix_color_draft(&draft, Some("Test"), 1).is_err());
    }

    #[test]
    fn confirm_set_affix_color_draft_rejects_oversized_color() {
        let draft = SetAffixColorDraft::new(0, &"C".repeat(17));
        assert!(confirm_set_affix_color_draft(&draft, Some("Test"), 1).is_err());
    }

    #[test]
    fn confirm_set_affix_color_draft_rejects_no_selected_preset() {
        let draft = SetAffixColorDraft::new(0, "#FF0000");
        assert!(confirm_set_affix_color_draft(&draft, None, 1).is_err());
    }

    #[test]
    fn confirm_set_affix_color_draft_rejects_negative_target() {
        let draft = SetAffixColorDraft::new(-1, "#FF0000");
        assert!(confirm_set_affix_color_draft(&draft, Some("Test"), 1).is_err());
    }

    #[test]
    fn confirm_set_affix_color_draft_rejects_out_of_range_target() {
        let draft = SetAffixColorDraft::new(5, "#FF0000");
        assert!(confirm_set_affix_color_draft(&draft, Some("Test"), 2).is_err());
    }

    #[test]
    fn affix_color_palette_has_ten_entries() {
        assert_eq!(AFFIX_COLOR_PALETTE.len(), 10);
        // All entries must be non-empty name and hex color.
        for (name, hex) in AFFIX_COLOR_PALETTE {
            assert!(!name.is_empty());
            assert!(hex.starts_with('#'));
        }
    }

    // ── Affix-family config dialog tests ─────────────

    fn affix_entry(id: &str, item_type: Option<&str>) -> PresetEntry {
        PresetEntry {
            kind: PresetEntryKind::Affix,
            id: id.to_string(),
            label: None,
            item_type: item_type.map(str::to_string),
            color: "#FFFFFF".to_string(),
            is_any_type: item_type.is_none(),
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        }
    }

    #[test]
    fn affix_slot_tables_are_index_aligned() {
        assert_eq!(AFFIX_SLOT_TOKENS.len(), AFFIX_SLOT_LABELS.len());
        for (index, token) in AFFIX_SLOT_TOKENS.iter().enumerate() {
            assert_eq!(affix_slot_index(token), Some(index));
            assert!(!AFFIX_SLOT_LABELS[index].is_empty());
        }
        assert_eq!(affix_slot_index("weapon"), Some(7));
        assert_eq!(affix_slot_index("charm"), None);
        assert_eq!(affix_slot_index("Helm"), None);
    }

    #[test]
    fn append_affix_slot_entry_appends_a_typed_entry_and_bumps_the_revision() {
        let mut editor = sample_editor();
        let revision = editor.rules_revision;
        let entry = append_affix_slot_entry(&mut editor, "Test", "AttackSpeed", "helm", "#FF0000")
            .expect("known slot should append");
        assert_eq!(entry.kind, PresetEntryKind::Affix);
        assert_eq!(entry.id, "AttackSpeed");
        assert_eq!(entry.item_type.as_deref(), Some("helm"));
        assert_eq!(editor.presets[0].entries.len(), 1);
        assert_eq!(editor.rules_revision, revision + 1);

        // Upstream appends on every click — no dedupe (AffixManager.cs:141-158).
        append_affix_slot_entry(&mut editor, "Test", "AttackSpeed", "helm", "#FF0000")
            .expect("second click appends again");
        assert_eq!(editor.presets[0].entries.len(), 2);
    }

    #[test]
    fn append_affix_slot_entry_rejects_unknown_slots_and_presets() {
        let mut editor = sample_editor();
        assert!(append_affix_slot_entry(&mut editor, "Test", "x", "charm", "#FF0000").is_err());
        assert!(append_affix_slot_entry(&mut editor, "Missing", "x", "helm", "#FF0000").is_err());
        assert!(editor.presets[0].entries.is_empty());
    }

    #[test]
    fn affix_slot_counters_count_only_the_requested_affix() {
        let entries = vec![
            affix_entry("AttackSpeed", Some("helm")),
            affix_entry("AttackSpeed", Some("helm")),
            affix_entry("AttackSpeed", Some("ring")),
            affix_entry("AttackSpeed", None),
            affix_entry("CritChance", Some("helm")),
            PresetEntry::non_affix(PresetEntryKind::Sigil, "Sigil", None, "#FF0000"),
        ];
        let counters = affix_slot_counters(&entries, "AttackSpeed");
        assert_eq!(counters[affix_slot_index("helm").unwrap()], 2);
        assert_eq!(counters[affix_slot_index("ring").unwrap()], 1);
        assert_eq!(counters[affix_slot_index("chest").unwrap()], 0);
        assert_eq!(counters.iter().sum::<usize>(), 3);
    }

    #[test]
    fn affix_duplicate_groups_follow_the_upstream_done_gate() {
        let mut duplicate = affix_entry("AttackSpeed", Some("helm"));
        duplicate.is_implicit = true;
        let entries = vec![
            affix_entry("AttackSpeed", Some("helm")),
            // Same id/type but a different flag set: not a duplicate
            // (SetAffixViewModel.cs:274-280 groups by all four).
            duplicate,
            affix_entry("AttackSpeed", Some("ring")),
            affix_entry("CritChance", Some("helm")),
        ];
        assert!(affix_duplicate_groups(&entries, "AttackSpeed").is_empty());
        assert!(affix_duplicate_groups(&entries, "CritChance").is_empty());

        let mut entries_with_duplicate = entries.clone();
        entries_with_duplicate.push(affix_entry("AttackSpeed", Some("ring")));
        let groups = affix_duplicate_groups(&entries_with_duplicate, "AttackSpeed");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].item_type.as_deref(), Some("ring"));
        assert_eq!(groups[0].count, 2);
        assert!(!groups[0].is_greater && !groups[0].is_implicit && !groups[0].is_tempered);
    }

    #[test]
    fn clamp_minimal_affix_value_ignores_incomplete_entries() {
        assert_eq!(clamp_minimal_affix_value(""), None);
        assert_eq!(clamp_minimal_affix_value("   "), None);
        assert_eq!(clamp_minimal_affix_value("-"), None);
        assert_eq!(clamp_minimal_affix_value("abc"), None);
        assert_eq!(clamp_minimal_affix_value("NaN"), None);
        assert_eq!(clamp_minimal_affix_value("inf"), None);
        // A trailing separator is still a complete Rust float.
        assert_eq!(clamp_minimal_affix_value("1."), Some(1.0));
    }

    #[test]
    fn clamp_minimal_affix_value_clamps_to_the_upstream_grid() {
        assert_eq!(clamp_minimal_affix_value("0"), Some(0.0));
        assert_eq!(clamp_minimal_affix_value("12.34"), Some(12.3));
        assert_eq!(clamp_minimal_affix_value("12.36"), Some(12.4));
        assert_eq!(clamp_minimal_affix_value("9999"), Some(9999.0));
        assert_eq!(clamp_minimal_affix_value(" 40 "), Some(40.0));
        // The control clamps out-of-range entries instead of rejecting them.
        assert_eq!(clamp_minimal_affix_value("-5"), Some(0.0));
        assert_eq!(
            clamp_minimal_affix_value("10000"),
            Some(MINIMAL_AFFIX_VALUE_MAX)
        );
    }

    #[test]
    fn sigil_dungeon_tier_cycle_wraps_like_upstream() {
        assert_eq!(next_sigil_dungeon_tier("S"), "A");
        assert_eq!(next_sigil_dungeon_tier("E"), "F");
        assert_eq!(next_sigil_dungeon_tier("F"), "S");
        assert_eq!(next_sigil_dungeon_tier(SIGIL_DUNGEON_TIER_DEFAULT), "S");
        // Unknown tier: .NET's IndexOf returns -1, which wraps to S.
        assert_eq!(next_sigil_dungeon_tier("n/a"), "S");
        assert_eq!(next_sigil_dungeon_tier("b"), "C");
    }

    #[test]
    fn affix_family_config_patch_applies_only_the_reported_fields() {
        let mut panel = settings_panel();
        panel.minimal_affix_value_filter_enabled = false;
        panel.multi_build_mode_enabled = false;
        panel.tempered_detection_enabled = false;
        panel.is_tuning_prism_hints_enabled = false;
        panel.dungeon_tiers_enabled = false;

        // Every affix-dialog toggle reports its own field and nothing else:
        // the affix dialog has no dungeon-tiers control, so the tier flag
        // keeps its value.
        for key in [
            RULE_SETTING_MINIMAL_AFFIX_VALUE_FILTER,
            RULE_SETTING_MULTI_BUILD_MODE,
            RULE_SETTING_TEMPERED_AFFIX_DETECTION,
            RULE_SETTING_TUNING_PRISM_HINTS,
        ] {
            rule_setting_toggle_patch(key, true)
                .expect("a known key maps to a patch")
                .apply_to_panel_state(&mut panel);
        }
        assert!(panel.minimal_affix_value_filter_enabled);
        assert!(panel.multi_build_mode_enabled);
        assert!(panel.tempered_detection_enabled);
        assert!(panel.is_tuning_prism_hints_enabled);
        assert!(!panel.dungeon_tiers_enabled);

        // The multi-build toggle carried by the aspect/unique/rune dialogs
        // writes the same field and leaves the detection flags alone.
        let mut aspect_panel = settings_panel();
        aspect_panel.multi_build_mode_enabled = true;
        aspect_panel.dungeon_tiers_enabled = true;
        aspect_panel.aspect_detection_enabled = true;
        rule_setting_toggle_patch(RULE_SETTING_MULTI_BUILD_MODE, false)
            .unwrap()
            .apply_to_panel_state(&mut aspect_panel);
        assert!(!aspect_panel.multi_build_mode_enabled);
        assert!(aspect_panel.dungeon_tiers_enabled);
        assert!(aspect_panel.aspect_detection_enabled);
    }

    #[test]
    fn detection_toggles_write_only_their_own_flag() {
        for (key, read) in [
            (
                RULE_SETTING_ASPECT_DETECTION,
                (|panel: &crate::SettingsPanelState| panel.aspect_detection_enabled)
                    as fn(&crate::SettingsPanelState) -> bool,
            ),
            (RULE_SETTING_UNIQUE_DETECTION, |panel| {
                panel.unique_detection_enabled
            }),
            (RULE_SETTING_RUNE_DETECTION, |panel| {
                panel.rune_detection_enabled
            }),
        ] {
            let mut panel = settings_panel();
            panel.aspect_detection_enabled = true;
            panel.unique_detection_enabled = true;
            panel.rune_detection_enabled = true;
            rule_setting_toggle_patch(key, false)
                .unwrap()
                .apply_to_panel_state(&mut panel);
            assert!(!read(&panel), "{key} must clear its own flag");
            let others_on = [
                panel.aspect_detection_enabled,
                panel.unique_detection_enabled,
                panel.rune_detection_enabled,
            ]
            .into_iter()
            .filter(|on| *on)
            .count();
            assert_eq!(others_on, 2, "{key} must not touch the other flags");
        }
    }

    #[test]
    fn sigil_config_patch_writes_the_display_mode() {
        let mut panel = settings_panel();
        // The persisted defaults: dungeon tiers on, multi-build off,
        // Whitelisting (`RuleSettingsConfig::default`).
        assert!(panel.dungeon_tiers_enabled);
        assert!(!panel.multi_build_mode_enabled);
        assert_eq!(
            panel.sigil_display_mode,
            d4lc_config::SigilDisplayModeSetting::Whitelisting
        );

        rule_setting_toggle_patch(RULE_SETTING_DUNGEON_TIERS, false)
            .unwrap()
            .apply_to_panel_state(&mut panel);
        rule_setting_toggle_patch(RULE_SETTING_MULTI_BUILD_MODE, true)
            .unwrap()
            .apply_to_panel_state(&mut panel);
        rule_setting_selection_patch(RULE_SETTING_SIGIL_DISPLAY_MODE, "Blacklisting")
            .unwrap()
            .apply_to_panel_state(&mut panel);

        assert!(!panel.dungeon_tiers_enabled);
        assert!(panel.multi_build_mode_enabled);
        assert_eq!(
            panel.sigil_display_mode,
            d4lc_config::SigilDisplayModeSetting::Blacklisting
        );
    }

    #[test]
    fn rule_setting_reports_map_to_single_field_patches() {
        assert_eq!(
            rule_setting_toggle_patch(RULE_SETTING_MINIMAL_AFFIX_VALUE_FILTER, true),
            Some(AffixFamilyConfigPatch {
                minimal_affix_value_filter_enabled: Some(true),
                ..AffixFamilyConfigPatch::default()
            })
        );
        assert_eq!(
            rule_setting_toggle_patch(RULE_SETTING_MULTI_BUILD_MODE, false),
            Some(AffixFamilyConfigPatch {
                multi_build_enabled: Some(false),
                ..AffixFamilyConfigPatch::default()
            })
        );
        assert_eq!(
            rule_setting_toggle_patch(RULE_SETTING_TEMPERED_AFFIX_DETECTION, true),
            Some(AffixFamilyConfigPatch {
                tempered_affix_detection_enabled: Some(true),
                ..AffixFamilyConfigPatch::default()
            })
        );
        assert_eq!(
            rule_setting_toggle_patch(RULE_SETTING_TUNING_PRISM_HINTS, true),
            Some(AffixFamilyConfigPatch {
                tuning_prism_hints_enabled: Some(true),
                ..AffixFamilyConfigPatch::default()
            })
        );
        assert_eq!(
            rule_setting_toggle_patch(RULE_SETTING_DUNGEON_TIERS, true),
            Some(AffixFamilyConfigPatch {
                dungeon_tiers_enabled: Some(true),
                ..AffixFamilyConfigPatch::default()
            })
        );
        assert_eq!(rule_setting_toggle_patch("unknown_key", true), None);
        assert_eq!(
            rule_setting_toggle_patch(RULE_SETTING_SIGIL_DISPLAY_MODE, true),
            None
        );

        assert_eq!(
            rule_setting_selection_patch(RULE_SETTING_SIGIL_DISPLAY_MODE, "Blacklisting"),
            Some(AffixFamilyConfigPatch {
                sigil_display_mode: Some(d4lc_config::SigilDisplayModeSetting::Blacklisting),
                ..AffixFamilyConfigPatch::default()
            })
        );
        assert_eq!(
            rule_setting_selection_patch("unknown_key", "Blacklisting"),
            None
        );
        // An unknown key never produces a patch that writes anything.
        assert_eq!(
            AffixFamilyConfigPatch::default(),
            AffixFamilyConfigPatch {
                minimal_affix_value_filter_enabled: None,
                multi_build_enabled: None,
                tempered_affix_detection_enabled: None,
                aspect_detection_enabled: None,
                unique_detection_enabled: None,
                rune_detection_enabled: None,
                tuning_prism_hints_enabled: None,
                dungeon_tiers_enabled: None,
                sigil_display_mode: None,
            }
        );
    }

    #[test]
    fn set_affix_dialog_seed_projects_counters_and_minimal_value() {
        let entries = vec![
            affix_entry("AttackSpeed", Some("helm")),
            affix_entry("AttackSpeed", Some("helm")),
            affix_entry("AttackSpeed", Some("ring")),
        ];
        let mut minimal_values = std::collections::BTreeMap::new();
        minimal_values.insert("AttackSpeed".to_string(), 12.5_f64);

        let seed = set_affix_dialog_seed(&entries, "AttackSpeed", &minimal_values);
        assert_eq!(seed.slot_counters[affix_slot_index("helm").unwrap()], 2);
        assert_eq!(seed.slot_counters[affix_slot_index("ring").unwrap()], 1);
        assert_eq!(seed.minimal_affix_value, Some(12.5));

        let missing = set_affix_dialog_seed(&entries, "CritChance", &minimal_values);
        assert_eq!(missing.slot_counters, [0; 10]);
        assert_eq!(missing.minimal_affix_value, None);
    }

    #[test]
    fn sigil_tier_rows_show_the_upstream_default_for_missing_tiers() {
        let mut stored = std::collections::BTreeMap::new();
        stored.insert("Sigil_Kehj".to_string(), "S2".to_string());
        stored.insert("Sigil_Lowercase".to_string(), "a".to_string());
        let entries = vec![
            PresetEntry::non_affix(PresetEntryKind::Sigil, "Sigil_Kehj", None, "#FF0000"),
            PresetEntry::non_affix(PresetEntryKind::Sigil, "Sigil_Lowercase", None, "#FF0000"),
            PresetEntry::non_affix(PresetEntryKind::Sigil, "Sigil_Missing", None, "#FF0000"),
            PresetEntry::non_affix(PresetEntryKind::Affix, "AttackSpeed", None, "#FF0000"),
        ];
        let rows = sigil_tier_rows(&entries, &stored);
        assert_eq!(rows.len(), 3);
        // Stored values are shown verbatim (upstream keeps free-form tier
        // labels like `S2`); a missing entry falls back to `F`.
        assert_eq!(rows[0].id, "Sigil_Kehj");
        assert_eq!(rows[0].tier, "S2");
        assert_eq!(rows[1].tier, "a");
        assert_eq!(rows[2].tier, SIGIL_DUNGEON_TIER_DEFAULT);
    }
}
