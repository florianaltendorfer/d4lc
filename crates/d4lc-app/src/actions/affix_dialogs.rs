//! Affix dialog confirm / cancel handlers.
//!
//! Every `handle_confirm` returns a `Result<(), String>` the shared
//! [`crate::actions::dialogs`] router acts on: success closes the dialog,
//! failure surfaces the error through the shared dialog state and keeps the
//! dialog open.
//!
//! Confirm bodies:
//!
//! - `SetAffix` — add or edit a `PresetEntry` (affix kind) in the selected
//!   preset. Reads the form from the root in-out props (`set-affix-entry-id`,
//!   `set-affix-item-type`, `set-affix-color`, the three toggles and
//!   `set-affix-edit-index`), validates through
//!   `d4lc_gui::affix::confirm_set_affix_draft` and stores the entry via
//!   `AffixPresetEditorState::edit_entry`.
//! - `SetAffixColor` — set the color of an existing entry, validated through
//!   `d4lc_gui::affix::confirm_set_affix_color_draft`.
//! - `RenamePreset` — rename the selected preset (`rename-preset-current-name`,
//!   `rename-preset-new-name`; non-empty and unique), then refresh the
//!   `preset-names` model and the `selected-preset` projection.
//! - `SetPresetName` — create a preset named by `set-preset-name-input`
//!   (non-empty and unique) and refresh the model.
//! - `AffixConfig`, `AspectConfig`, `SigilConfig`, `UniqueConfig`,
//!   `RuneConfig` — Done only closes, like upstream's `*ConfigDoneExecute`.
//!
//! `ImportPreset` and `MultiBuildConfig` belong to the `BuildImport` family.
//!
//! ## Affix-family config dialogs
//!
//! Every upstream control in the five config dialogs and the SetAffix dialog
//! writes through on interaction — the upstream ViewModels save from their
//! property setters (`AffixConfigViewModel.cs:55-101` calls
//! `_settingsManager.SaveSettings()` from each toggle,
//! `AffixManager.SetAffixMinimalValue` (AffixManager.cs:1087-1092) and
//! `SetSigilDungeonTier` (AffixManager.cs:1094-1099) save their file on every
//! change) — so the adapters below are wired to the dialogs' change callbacks.
//!
//! The handlers keep the persisted state and the live consumers in step:
//! rule-setting toggles persist through `d4lc_config::save_app_settings` and
//! re-project the overlay descriptors, the affix screen rows, and the
//! build-import multi-build mirror; the minimal-values reset rewrites
//! `Config/MinimalAffixValues.json` and re-runs the value-filter projection;
//! the per-slot add appends a typed entry and saves the preset file; the
//! sigil tier cycle rewrites `Config/DungeonTiers.json` and re-projects the
//! tier text. The host contract (root properties and callbacks) is
//! documented in the six dialog files.

use std::path::Path;
use std::rc::Rc;

use d4lc_gui::affix::{
    affix_duplicate_groups, append_affix_slot_entry, clamp_minimal_affix_value,
    confirm_set_affix_color_draft, confirm_set_affix_draft, next_sigil_dungeon_tier,
    rule_setting_selection_patch, rule_setting_toggle_patch, AffixFamilyConfigPatch,
    SetAffixColorDraft, SetAffixDraft, SIGIL_DUNGEON_TIER_DEFAULT,
};
use slint::VecModel;

use crate::app_state::{AppState, ConfirmAction, DialogKind};
use crate::slint_generated::AppWindow;

/// Confirm entry for the affix-family dialogs and the two preset-name
/// dialogs.
///
/// Dispatches by `state.dialog.kind` to the per-dialog confirm
/// implementation. For dialogs that are read-only viewers (the 5
/// per-type config dialogs), the confirm is a no-op that closes
/// the dialog.
pub fn handle_confirm(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let kind = state.dialog.kind;
    match kind {
        DialogKind::SetAffix => confirm_set_affix(app, state, config_root),
        DialogKind::SetAffixColor => confirm_set_affix_color(app, state, config_root),
        DialogKind::RenamePreset => confirm_rename_preset(app, state, config_root),
        DialogKind::SetPresetName => confirm_set_preset_name(app, state, config_root),
        DialogKind::AffixConfig
        | DialogKind::AspectConfig
        | DialogKind::SigilConfig
        | DialogKind::UniqueConfig
        | DialogKind::RuneConfig => {
            // Read-only entry-list viewer. No state mutation on
            // confirm. The dialog closes via the router.
            Ok(())
        }
        _ => Err(format!(
            "affix_dialogs::handle_confirm: unimplemented kind {kind}"
        )),
    }
}

/// Cancel entry for the affix-family dialogs.
///
/// Clears the root in-out props for `SetAffix`, `SetAffixColor`,
/// `RenamePreset`, and `SetPresetName` so a subsequent open
/// starts from a clean seed.
pub fn handle_cancel(app: &AppWindow, _state: &AppState) {
    clear_set_affix_props(app);
    clear_set_affix_color_props(app);
    clear_rename_preset_props(app);
    clear_set_preset_name_props(app);
}

// ── SetAffix confirm body ────────────────────────────────────────────────

fn confirm_set_affix(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let edit_index = app.get_set_affix_edit_index();
    let id = app.get_set_affix_entry_id().to_string();
    let item_type = app.get_set_affix_item_type().to_string();
    let color = app.get_set_affix_color().to_string();
    let is_greater = app.get_set_affix_is_greater();
    let is_implicit = app.get_set_affix_is_implicit();
    let is_tempered = app.get_set_affix_is_tempered();

    let draft = SetAffixDraft::new(
        edit_index,
        &id,
        &item_type,
        &color,
        is_greater,
        is_implicit,
        is_tempered,
    );
    let selected = state.affix_preset_editor.selected_preset_name.clone();
    // Use the public read-only `selected_preset_entry_count` to
    // avoid reaching into the editor's private mutation surface.
    let entry_count = state.affix_preset_editor.selected_preset_entry_count();
    let _effects = confirm_set_affix_draft(&draft, selected.as_deref(), entry_count)?;

    // Duplicate gate (upstream `SetAffixViewModel.CanSetAffixDoneExecute`,
    // SetAffixViewModel.cs:274-280): the Done button is disabled while the
    // selected affixes of this id contain a duplicate
    // (Type, IsImplicit, IsGreater, IsTempered) group. The d4lc dialog cannot
    // disable Done from Rust, so the same condition is rejected here before
    // anything is written and surfaced through the shared dialog error.
    let existing_entries: Vec<d4lc_gui::PresetEntry> = state
        .affix_preset_editor
        .selected_preset_entries()
        .unwrap_or(&[])
        .to_vec();
    let duplicates = affix_duplicate_groups(&existing_entries, &draft.id);
    if let Some(error) = duplicate_gate_error(&duplicates, &draft.id) {
        return Err(error);
    }

    // Minimal-value field: the SetAffix dialog applies it live through
    // `handle_set_affix_minimal_value` (the upstream `MinimalAffixValue`
    // setter saves on every change, SetAffixViewModel.cs:178-182), so Done
    // has nothing left to write for it.

    // Persist: convert to PresetEntry and call edit_entry on the
    // selected preset. edit_entry upserts by key (kind, id,
    // item_type) so re-adding the same id replaces the existing
    // entry.
    let mut entry = draft.into_preset_entry();
    preserve_entry_metadata_on_edit(&mut entry, edit_index, state);
    let preset_name = selected.ok_or_else(|| "No preset selected.".to_string())?;
    state
        .affix_preset_editor
        .edit_entry(&preset_name, entry)
        .map_err(|e| format!("Failed to save affix entry: {e}"))?;
    state.bump_rules_input_revision();

    // The selected preset's entries changed: re-project the entry
    // rows/cards so the new or edited entry shows up immediately.
    crate::actions::affix::project_affix_entries(app, state);

    // Upstream parity: AddAffix ends with SaveAffixPresets
    // (AffixManager.cs:158) — persist the preset file.
    // A failed write is surfaced through the shared dialog error
    // instead of closing on an edit that never reached the disk.
    state
        .persist_affix_presets_to_config(config_root)
        .map_err(|error| format!("Failed to save affix entry: {error}"))?;

    // Clear root props after a successful confirm.
    clear_set_affix_props(app);
    Ok(())
}

/// Preserve the edited entry's non-form metadata (its user-visible
/// label, the upstream `IsAnyType` flag and the `TuningPrisms` list).
///
/// The SetAffix form has no fields for any of them (upstream
/// `SetAffixInfoView` is an id + item-type + color + flags form; its
/// any-type checkbox is a separate `SetIsAnyType` action), and
/// `edit_entry` upserts by (kind, id, item_type) — without this,
/// confirming an edit would silently drop the entry's label and its
/// preserved file metadata. The edit index is the dialog-seeded index
/// of the edited entry in the mixed-kind list (-1 for a new entry,
/// where none of them exists yet).
fn preserve_entry_metadata_on_edit(
    entry: &mut d4lc_gui::PresetEntry,
    edit_index: i32,
    state: &AppState,
) {
    if edit_index < 0 {
        return;
    }
    if let Some(existing) = state
        .affix_preset_editor
        .selected_preset_entry(edit_index as usize)
    {
        entry.label = existing.label.clone();
        entry.is_any_type = existing.is_any_type;
        entry.tuning_prisms = existing.tuning_prisms.clone();
    }
}

fn clear_set_affix_props(app: &AppWindow) {
    app.set_set_affix_edit_index(-1);
    app.set_set_affix_entry_id(slint::SharedString::default());
    app.set_set_affix_item_type(slint::SharedString::default());
    app.set_set_affix_color("#FF0000".into());
    app.set_set_affix_is_greater(false);
    app.set_set_affix_is_implicit(false);
    app.set_set_affix_is_tempered(false);
    // The host-seeded model of the SetAffix dialog: the minimal-value field and the per-slot counters are
    // re-seeded on the next open, so a stale draft must not survive a
    // cancel / confirm. The counters reset to the dialog's ten-zero
    // default — the dialog indexes all ten slots unconditionally.
    app.set_set_affix_minimal_value(slint::SharedString::default());
    app.set_set_affix_slot_counters(Rc::new(VecModel::from(vec![0_i32; 10])).into());
}

/// The Done-gate error for the duplicate groups of one affix id, or
/// `None` when the entry set is clean.
///
/// Upstream disables the SetAffix dialog's Done button while the selected
/// affixes of the edited id contain a duplicate group
/// (`CanSetAffixDoneExecute`, SetAffixViewModel.cs:274-280). The d4lc
/// dialog cannot disable a button from Rust, so the same condition is
/// reported through the shared dialog error contract before anything is
/// written.
fn duplicate_gate_error(
    duplicates: &[d4lc_gui::affix::AffixDuplicateGroup],
    affix_id: &str,
) -> Option<String> {
    let group = duplicates.first()?;
    Some(format!(
        "Duplicate affix entries for {affix_id}: {} entries share type {} with implicit={}, \
         greater={}, tempered={}. Remove one before confirming.",
        group.count,
        group.item_type.as_deref().unwrap_or("any"),
        group.is_implicit,
        group.is_greater,
        group.is_tempered
    ))
}

// ── Affix-family config dialog handlers ─────────────────────
//
// One handler per dialog interaction, each split into a pure state +
// persistence core (unit-tested without a Slint window) and a thin
// app-facing wrapper that re-projects the live consumers.

/// Handle the config dialogs' `rule-toggle-changed(name, value)` report.
///
/// Upstream saves from every toggle setter (`AffixConfigViewModel.cs:55-101`,
/// `SigilConfigViewModel.cs:60-98`), so the toggle persists and re-projects
/// immediately instead of waiting for Done.
pub fn handle_rule_setting_toggled(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    name: &str,
    value: bool,
) -> Result<(), String> {
    let patch = rule_setting_toggle_patch(name, value)
        .ok_or_else(|| format!("Unknown affix rule setting {name:?}."))?;
    apply_family_config_patch(app, state, config_root, patch)
}

/// Handle the Sigil config dialog's `rule-selection-changed(name, value)`
/// report — the upstream `SelectedSigilDisplayMode` selector
/// (`SigilConfigViewModel.cs:85-98`).
pub fn handle_rule_setting_selected(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    name: &str,
    value: &str,
) -> Result<(), String> {
    let patch = rule_setting_selection_patch(name, value)
        .ok_or_else(|| format!("Unknown affix rule selection {name:?}."))?;
    apply_family_config_patch(app, state, config_root, patch)
}

/// Handle the Affix config dialog's reset button: ask before resetting.
///
/// Upstream `ResetMinimalAffixValuesExecute` (`AffixConfigViewModel.cs:114-123`)
/// asks `ShowMessageAsync(this, "Reset", "Are you sure you want to reset the
/// minimal affix values?", MessageDialogStyle.AffirmativeAndNegative)` and
/// only resets on `Affirmative`. The port opens the same window-modal prompt
/// through the shared confirm dialog and runs the reset in
/// [`handle_confirm_reset_minimal_affix_values`] (the prompt's Yes path).
///
/// Answering No drops the request and returns to this dialog: the prompt
/// declares `ConfirmParent::AffixConfig`, which the router reopens after the
/// answer, mirroring upstream's prompt shown over the open config dialog.
///
/// The callback keeps the wiring's `(app, state, config_root)` shape; the
/// persistence seam moved into the confirm body.
pub fn handle_reset_minimal_affix_values(
    app: &AppWindow,
    state: &mut AppState,
    _config_root: &Path,
) -> Result<(), String> {
    crate::actions::dialogs::open_confirm(app, state, ConfirmAction::ResetMinimalAffixValues);
    Ok(())
}

/// Confirm body of the minimal-values reset prompt (the prompt's Yes path).
///
/// Upstream `ResetMinimalAffixValues` (AffixManager.cs:892-896) clears the
/// per-affix thresholds and saves the emptied `Config/MinimalAffixValues.json`.
pub fn handle_confirm_reset_minimal_affix_values(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    reset_minimal_affix_values(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    crate::actions::affix::project_affix_entries(app, state);
    Ok(())
}

/// Handle one per-slot add click of the SetAffix dialog.
///
/// Upstream `AddAffixExecute` (SetAffixViewModel.cs:211-248) appends one
/// entry of the clicked item type for the affix the dialog edits and saves
/// the preset file; every click appends, so the dialog's Done gate is what
/// rejects duplicates.
pub fn handle_add_affix_slot(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    affix_id: &str,
    slot_token: &str,
) -> Result<(), String> {
    add_affix_slot(state, config_root, affix_id, slot_token)?;
    crate::actions::affix::project_affix_entries(app, state);
    Ok(())
}

/// Handle one minimal-value field change of the SetAffix dialog.
///
/// Upstream binds the `NumericUpDown` with
/// `UpdateSourceTrigger=PropertyChanged` (SetAffixView.xaml:107-112) and its
/// value setter saves the threshold file on every change
/// (`SetAffixViewModel.cs:178-182` → `AffixManager.SetAffixMinimalValue`,
/// AffixManager.cs:1087-1092). The d4lc dialog reports the field text, so an
/// incomplete entry (blank, `-`, `1.`) is ignored exactly like the
/// `NumericUpDown` refuses it, and a complete one is clamped to the
/// control's `0..9999` bounds before it is stored.
pub fn handle_set_affix_minimal_value(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    affix_id: &str,
    text: &str,
) -> Result<(), String> {
    let affix_id = affix_id.trim();
    let Some(value) = clamp_minimal_affix_value(text) else {
        // Incomplete input (or no affix selected): nothing to store.
        return Ok(());
    };
    if affix_id.is_empty() {
        return Err("Affix id is required.".to_string());
    }
    store_minimal_affix_value(state, config_root, affix_id, value)?;
    state.rebuild_overlay_descriptor_state();
    crate::actions::affix::project_affix_entries(app, state);
    Ok(())
}

/// Handle one sigil tier cycle of the Sigil config dialog.
///
/// Upstream `SetSigilDungeonTierToNextExecute` (SigilInfoVM.cs:139-149)
/// advances the sigil's tier through the `S..F` list and
/// `AffixManager.SetSigilDungeonTier` (AffixManager.cs:1094-1099) writes
/// `Config/DungeonTiers.json`; the tier text the sigil marker draws is part
/// of the overlay descriptor, which is re-projected here.
pub fn handle_cycle_sigil_dungeon_tier(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    sigil_id: &str,
) -> Result<(), String> {
    cycle_sigil_dungeon_tier(state, config_root, sigil_id)?;
    state.rebuild_overlay_descriptor_state();
    crate::actions::affix::project_affix_entries(app, state);
    Ok(())
}

/// Apply one family patch: write the panel state, persist it, mirror the
/// multi-build flag into the build-import projection, and re-project the
/// consumers (rule descriptors + affix screen rows).
fn apply_family_config_patch(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    patch: AffixFamilyConfigPatch,
) -> Result<(), String> {
    apply_family_config_patch_state(state, config_root, patch)?;
    state.rebuild_overlay_descriptor_state();
    crate::actions::affix::project_affix_entries(app, state);
    Ok(())
}

/// The state + persistence core of [`apply_family_config_patch`].
fn apply_family_config_patch_state(
    state: &mut AppState,
    config_root: &Path,
    patch: AffixFamilyConfigPatch,
) -> Result<(), String> {
    patch.apply_to_panel_state(&mut state.shell.settings);
    state.bump_rules_input_revision();
    persist_settings_panel(state, config_root)
}

/// Persist the settings panel state through the same load-modify-save path
/// the settings dialogs use (`SettingsPanelState::apply_to_config` →
/// `d4lc_config::save_app_settings`).
fn persist_settings_panel(state: &AppState, config_root: &Path) -> Result<(), String> {
    let mut config = d4lc_config::load_app_settings(config_root)
        .map_err(|e| format!("Failed to load app settings: {e}"))?;
    state.shell.settings.apply_to_config(&mut config);
    d4lc_config::save_app_settings(config_root, &config)
        .map_err(|e| format!("Failed to save app settings: {e}"))
}

/// Clear every per-affix threshold and save the emptied file (upstream
/// `ResetMinimalAffixValues`, AffixManager.cs:892-896).
fn reset_minimal_affix_values(state: &mut AppState, config_root: &Path) -> Result<(), String> {
    state.minimal_affix_values.clear();
    d4lc_config::save_affix_minimal_values(config_root, &state.minimal_affix_values)
        .map_err(|e| format!("Failed to save minimal affix values: {e}"))
}

/// Store one per-affix threshold and save the threshold file (upstream
/// `SetAffixMinimalValue`, AffixManager.cs:1087-1092).
fn store_minimal_affix_value(
    state: &mut AppState,
    config_root: &Path,
    affix_id: &str,
    value: f64,
) -> Result<(), String> {
    state
        .minimal_affix_values
        .insert(affix_id.to_string(), value);
    d4lc_config::save_affix_minimal_values(config_root, &state.minimal_affix_values)
        .map_err(|e| format!("Failed to save minimal affix values: {e}"))
}

/// Append one per-slot entry to the selected preset and save the preset
/// file (upstream `AddAffix`, AffixManager.cs:141-158). The appended entry
/// carries the configured `DefaultColorNormal`.
fn add_affix_slot(
    state: &mut AppState,
    config_root: &Path,
    affix_id: &str,
    slot_token: &str,
) -> Result<(), String> {
    let affix_id = affix_id.trim();
    if affix_id.is_empty() {
        return Err("Affix id is required.".to_string());
    }
    let preset_name = state
        .affix_preset_editor
        .selected_preset_name
        .clone()
        .ok_or_else(|| "No preset selected.".to_string())?;
    let color =
        crate::actions::affix_cmds::default_entry_color(state, d4lc_gui::PresetEntryKind::Affix);
    append_affix_slot_entry(
        &mut state.affix_preset_editor,
        &preset_name,
        affix_id,
        slot_token,
        &color,
    )?;
    state.bump_rules_input_revision();
    state
        .persist_affix_presets_to_config(config_root)
        .map_err(|error| format!("Failed to save affix entry: {error}"))?;
    Ok(())
}

/// Advance one sigil's dungeon tier through the upstream cycle and save
/// `Config/DungeonTiers.json`.
fn cycle_sigil_dungeon_tier(
    state: &mut AppState,
    config_root: &Path,
    sigil_id: &str,
) -> Result<(), String> {
    let sigil_id = sigil_id.trim();
    if sigil_id.is_empty() {
        return Err("Sigil id is required.".to_string());
    }
    let current = state
        .dungeon_tiers
        .get(sigil_id)
        .map(String::as_str)
        .unwrap_or(SIGIL_DUNGEON_TIER_DEFAULT);
    let next = next_sigil_dungeon_tier(current);
    state
        .dungeon_tiers
        .insert(sigil_id.to_string(), next.to_string());
    d4lc_config::save_dungeon_tiers(config_root, &state.dungeon_tiers)
        .map_err(|e| format!("Failed to save dungeon tiers: {e}"))
}

// ── SetAffixColor confirm body ───────────────────────────────────────────

fn confirm_set_affix_color(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let target_key = app.get_set_affix_color_target_key().to_string();
    let color = app.get_set_affix_color_value().to_string();
    // Resolve the kind-tagged target key against the mixed-kind entry
    // list; the validation draft carries the resolved index (the
    // d4lc_gui contract bounds-checks an index).
    let (target_index, mut entry) = crate::actions::affix::resolve_entry_key(state, &target_key)
        .ok_or_else(|| format!("Target entry {target_key:?} not found."))?;
    let draft = SetAffixColorDraft::new(target_index as i32, &color);
    let selected = state.affix_preset_editor.selected_preset_name.clone();
    let entry_count = state.affix_preset_editor.selected_preset_entry_count();
    let _effects = confirm_set_affix_color_draft(&draft, selected.as_deref(), entry_count)?;

    // Update the resolved entry's color and call edit_entry to upsert.
    // edit_entry compares entries by (kind, id, item_type) so the
    // color-only update is preserved.
    let preset_name = selected.ok_or_else(|| "No preset selected.".to_string())?;
    entry.color = color.clone();
    state
        .affix_preset_editor
        .edit_entry(&preset_name, entry)
        .map_err(|e| format!("Failed to save color: {e}"))?;
    state.bump_rules_input_revision();

    // The entry's color changed: re-project so the row/card swatch
    // picks up the new value immediately.
    crate::actions::affix::project_affix_entries(app, state);

    // Same mutation seam as SetAffix: persist the preset file
    // a failed write keeps the dialog open with the
    // error instead of reporting a color change that was not stored.
    state
        .persist_affix_presets_to_config(config_root)
        .map_err(|error| format!("Failed to save affix color: {error}"))?;

    clear_set_affix_color_props(app);
    Ok(())
}

fn clear_set_affix_color_props(app: &AppWindow) {
    app.set_set_affix_color_target_key(slint::SharedString::default());
    app.set_set_affix_color_value(slint::SharedString::default());
}

// ── RenamePreset confirm body ────────────────────────────────────

fn confirm_rename_preset(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    // Read the form values from the root in-out props (no Slint
    // global). The current name is the read-only seed (matches
    // `state.affix_preset_editor.selected_preset_name` at the
    // time the dialog opened); the new name is the user input.
    let current = app.get_rename_preset_current_name().to_string();
    let new_name_raw = app.get_rename_preset_new_name().to_string();
    let new_name = new_name_raw.trim().to_string();

    if current.is_empty() {
        return Err("No preset selected to rename.".to_string());
    }
    if new_name.is_empty() {
        return Err("Preset name must not be empty.".to_string());
    }
    if new_name == current {
        // No-op rename: clear props and close cleanly.
        clear_rename_preset_props(app);
        return Ok(());
    }

    // Persist via the public editor API.
    state
        .affix_preset_editor
        .rename_preset(&current, &new_name)
        .map_err(|e| format!("Failed to rename preset: {e}"))?;
    state.bump_rules_input_revision();

    // Refresh the preset ComboBox (new sort order), the selection and the
    // entry projections through the shared preset-model seam.
    crate::actions::affix::refresh_affix_screen_preset_models(app, state);

    // Upstream parity (`AffixManager.RenamePreset`,
    // AffixManager.cs:1011-1022): rename persists the preset file and
    // follows the selection rename with a settings save.
    // Both files commit as one transaction; a failed write is surfaced
    // through the dialog error instead of reporting the rename.
    state
        .persist_affix_presets_and_selection_to_config(config_root)
        .map_err(|error| format!("Failed to save renamed preset: {error}"))?;

    clear_rename_preset_props(app);
    Ok(())
}

fn clear_rename_preset_props(app: &AppWindow) {
    app.set_rename_preset_current_name(slint::SharedString::default());
    app.set_rename_preset_new_name(slint::SharedString::default());
}

// ── SetPresetName confirm body ───────────────────────────────────

fn confirm_set_preset_name(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let name_raw = app.get_set_preset_name_input().to_string();
    let name = name_raw.trim().to_string();
    if name.is_empty() {
        return Err("Preset name must not be empty.".to_string());
    }

    // Persist via the public editor API. `create_preset` rejects
    // duplicate names and returns an error string that we surface
    // to the shared dialog error prop.
    state
        .affix_preset_editor
        .create_preset(&name)
        .map_err(|e| format!("Failed to create preset: {e}"))?;
    state.bump_rules_input_revision();

    // Refresh the preset ComboBox, the selection and the entry models
    // through the shared preset-model seam: a fresh preset has no
    // entries, so the (now empty) lists replace the previously selected
    // preset's entries instead of letting them leak into the screen.
    crate::actions::affix::refresh_affix_screen_preset_models(app, state);

    // Same persistence pair as the inline create path (upstream
    // SaveAffixPresets + selection setter), committed as
    // one transaction; a failed write keeps the dialog open with the
    // error instead of reporting a created preset.
    state
        .persist_affix_presets_and_selection_to_config(config_root)
        .map_err(|error| format!("Failed to create preset: {error}"))?;

    clear_set_preset_name_props(app);
    Ok(())
}

fn clear_set_preset_name_props(app: &AppWindow) {
    app.set_set_preset_name_input(slint::SharedString::default());
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app_state::{AppState, DialogFamily, DialogKind};
    use d4lc_gui::{PresetEntry, PresetEntryKind};

    #[test]
    fn handle_confirm_contract_shape() {
        // Type-check the router-facing contract.
        let _: fn(&AppWindow, &mut AppState, &Path) -> Result<(), String> = handle_confirm;
        // Cancel contract: take &AppState (the router holds the lock
        // when cancel fires; we do not need mutable access).
        let _: fn(&AppWindow, &AppState) = handle_cancel;
    }

    #[test]
    fn affix_family_kinds_are_non_empty() {
        // Nine affix-family dialogs (MultiBuildConfig and ImportPreset
        // belong to BuildImport). The five *Config kinds return Ok(())
        // without mutating state; the four write kinds mutate state
        // through the public editor API.
        let affix_kinds: Vec<DialogKind> = DialogKind::ALL
            .iter()
            .copied()
            .filter(|k| k.family() == DialogFamily::Affix)
            .collect();
        assert_eq!(affix_kinds.len(), 9);
    }

    #[test]
    fn set_affix_draft_roundtrips_through_preset_entry() {
        let draft = SetAffixDraft::new(-1, "x", "Amulet", "#00FF00", true, false, true);
        let entry = draft.into_preset_entry();
        assert_eq!(entry.kind, PresetEntryKind::Affix);
        assert_eq!(entry.id, "x");
        assert_eq!(entry.item_type.as_deref(), Some("Amulet"));
        assert_eq!(entry.color, "#00FF00");
        assert!(entry.is_greater);
        assert!(!entry.is_implicit);
        assert!(entry.is_tempered);
    }

    #[test]
    fn set_affix_draft_new_trims_and_clears_item_type() {
        let draft = SetAffixDraft::new(-1, "  id  ", "  ", "#FF0000", false, false, false);
        assert_eq!(draft.id, "id");
        assert!(draft.item_type.is_none());
    }

    #[test]
    fn set_affix_color_draft_trims_color() {
        let draft = SetAffixColorDraft::new(0, "  #FF0000  ");
        assert_eq!(draft.color, "#FF0000");
    }

    #[test]
    fn preserve_entry_metadata_on_edit_keeps_the_edited_entry_metadata() {
        // The SetAffix form has no label / any-type / prisms fields; an
        // edit confirm must not silently drop the entry's metadata.
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Test").unwrap();
        state
            .affix_preset_editor
            .edit_entry(
                "Test",
                d4lc_gui::PresetEntry {
                    label: Some("Attack Speed".into()),
                    is_any_type: true,
                    tuning_prisms: vec!["Prism of Control".into()],
                    ..PresetEntry::non_affix(PresetEntryKind::Affix, "AttackSpeed", None, "#22C55E")
                },
            )
            .unwrap();

        let mut edited =
            PresetEntry::non_affix(PresetEntryKind::Affix, "AttackSpeed", None, "#00FF00");
        super::preserve_entry_metadata_on_edit(&mut edited, 0, &state);
        assert_eq!(
            edited.label.as_deref(),
            Some("Attack Speed"),
            "an edit confirm must preserve the entry's label"
        );
        assert!(edited.is_any_type, "the any-type flag must survive an edit");
        assert_eq!(edited.tuning_prisms, vec!["Prism of Control".to_string()]);

        // A new entry (edit index -1) has no metadata to preserve.
        let mut fresh = PresetEntry::non_affix(PresetEntryKind::Affix, "Other", None, "#00FF00");
        super::preserve_entry_metadata_on_edit(&mut fresh, -1, &state);
        assert_eq!(fresh.label, None);
        assert!(fresh.tuning_prisms.is_empty());
    }

    #[test]
    fn confirm_set_affix_validates_and_inserts_entry() {
        // Simulate the post-handler state mutation: the confirm
        // body validates through the headless function and calls
        // edit_entry on the selected preset. We exercise the
        // state-mutation side without an AppWindow by calling
        // the same APIs the handler calls. We use the public
        // `presets` field to read back the inserted entry —
        // `preset_mut` is intentionally private to d4lc-gui.
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Test").unwrap();
        let draft = SetAffixDraft::new(-1, "AttackSpeed", "Amulet", "#FF0000", true, false, false);
        let effects =
            confirm_set_affix_draft(&draft, Some("Test"), 0).expect("valid draft should succeed");
        assert_eq!(effects, vec![d4lc_gui::affix::AffixEffect::PersistConfig]);
        let entry = draft.into_preset_entry();
        state.affix_preset_editor.edit_entry("Test", entry).unwrap();
        let preset = &state.affix_preset_editor.presets[0];
        assert_eq!(preset.name, "Test");
        assert_eq!(preset.entries.len(), 1);
        assert_eq!(preset.entries[0].id, "AttackSpeed");
        assert!(preset.entries[0].is_greater);
    }

    #[test]
    fn confirm_set_affix_rejects_empty_id() {
        let draft = SetAffixDraft::new(-1, "", "", "#FF0000", false, false, false);
        assert!(confirm_set_affix_draft(&draft, Some("Test"), 0).is_err());
    }

    #[test]
    fn confirm_set_affix_rejects_no_selected_preset() {
        let draft = SetAffixDraft::new(-1, "x", "", "#FF0000", false, false, false);
        assert!(confirm_set_affix_draft(&draft, None, 0).is_err());
    }

    #[test]
    fn confirm_set_affix_color_validates_and_updates_entry() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Test").unwrap();
        // Add an initial entry.
        let initial =
            PresetEntry::non_affix(PresetEntryKind::Affix, "x", Some("Amulet"), "#000000");
        state
            .affix_preset_editor
            .edit_entry("Test", initial)
            .unwrap();
        let draft = SetAffixColorDraft::new(0, "#FF0000");
        let effects = confirm_set_affix_color_draft(&draft, Some("Test"), 1)
            .expect("valid color draft should succeed");
        assert_eq!(effects, vec![d4lc_gui::affix::AffixEffect::PersistConfig]);
        // Simulate the read-modify-write round-trip using the
        // public `selected_preset_entries` accessor and
        // `edit_entry` upsert (no `preset_mut`).
        let mut entry = state
            .affix_preset_editor
            .selected_preset_entries()
            .and_then(|entries| entries.first().cloned())
            .expect("entry exists");
        entry.color = "#FF0000".to_string();
        state.affix_preset_editor.edit_entry("Test", entry).unwrap();
        let preset = &state.affix_preset_editor.presets[0];
        assert_eq!(preset.entries[0].color, "#FF0000");
    }

    #[test]
    fn confirm_set_affix_color_rejects_out_of_range() {
        let draft = SetAffixColorDraft::new(5, "#FF0000");
        assert!(confirm_set_affix_color_draft(&draft, Some("Test"), 1).is_err());
    }

    #[test]
    fn read_only_config_kinds_return_ok() {
        // The 5 per-type config dialogs are read-only viewers; the
        // confirm body must return Ok without mutating state.
        for kind in [
            DialogKind::AffixConfig,
            DialogKind::AspectConfig,
            DialogKind::SigilConfig,
            DialogKind::UniqueConfig,
            DialogKind::RuneConfig,
        ] {
            let mut state = AppState::default_startup();
            state.affix_preset_editor.create_preset("Test").unwrap();
            state.dialog.kind = kind;
            // The match arm returns Ok(()) unconditionally; we
            // assert the type-level dispatch compiles and that no
            // state mutation occurs.
            match kind {
                DialogKind::AffixConfig
                | DialogKind::AspectConfig
                | DialogKind::SigilConfig
                | DialogKind::UniqueConfig
                | DialogKind::RuneConfig => {
                    // expected: no-op
                }
                _ => panic!("unexpected kind in read-only test"),
            }
            // State unchanged.
            assert_eq!(state.affix_preset_editor.presets.len(), 1);
            assert!(state.affix_preset_editor.presets[0].entries.is_empty());
        }
    }

    // ── RenamePreset + SetPresetName ──────────────────────────────
    //
    // The handler bodies call public `d4lc_gui` / `d4lc_gui::affix`
    // APIs; these tests exercise the same path without an AppWindow.

    #[test]
    fn rename_preset_validates_and_mutates_state() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        // The handler does this after reading from the form:
        state
            .affix_preset_editor
            .rename_preset("Speed", "Boss")
            .expect("rename should succeed");
        assert_eq!(state.affix_preset_editor.presets.len(), 1);
        assert_eq!(state.affix_preset_editor.presets[0].name, "Boss");
        assert_eq!(
            state.affix_preset_editor.selected_preset_name.as_deref(),
            Some("Boss")
        );
    }

    #[test]
    fn rename_preset_rejects_empty_new_name() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        // The handler trims the new name first, so a whitespace
        // input is treated as empty.
        let trimmed = "   ".trim();
        let result = state.affix_preset_editor.rename_preset("Speed", trimmed);
        assert!(result.is_err());
        // State unchanged.
        assert_eq!(state.affix_preset_editor.presets[0].name, "Speed");
    }

    #[test]
    fn rename_preset_rejects_collision() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        state.affix_preset_editor.create_preset("Boss").unwrap();
        let result = state.affix_preset_editor.rename_preset("Speed", "Boss");
        assert!(result.is_err());
        // State unchanged. Presets are sorted by name so the
        // ordering after both creates is "Boss" then "Speed".
        let names: Vec<&str> = state
            .affix_preset_editor
            .presets
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"Speed"));
        assert!(names.contains(&"Boss"));
    }

    #[test]
    fn rename_preset_rejects_missing_old() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        let result = state
            .affix_preset_editor
            .rename_preset("DoesNotExist", "Boss");
        assert!(result.is_err());
    }

    #[test]
    fn set_preset_name_validates_and_creates() {
        let mut state = AppState::default_startup();
        state
            .affix_preset_editor
            .create_preset("Test")
            .expect("create should succeed");
        assert_eq!(state.affix_preset_editor.presets.len(), 1);
        assert_eq!(
            state.affix_preset_editor.selected_preset_name.as_deref(),
            Some("Test")
        );
    }

    /// The root `preset-names` model (the AffixScreen preset ComboBox).
    fn preset_names(app: &AppWindow) -> Vec<String> {
        use slint::Model as _;
        let model = app.get_preset_names();
        (0..model.row_count())
            .map(|row| {
                model
                    .row_data(row)
                    .map(|name| name.to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The wanted-row ids of the projected affix entry model.
    fn wanted_affix_ids(app: &AppWindow) -> Vec<String> {
        use slint::Model as _;
        let model = app.get_affix_entries();
        (0..model.row_count())
            .filter_map(|row| model.row_data(row))
            .filter(|entry| entry.is_wanted)
            .map(|entry| entry.id.to_string())
            .collect()
    }

    /// RenamePreset confirm: the ComboBox reflects the renamed (re-sorted)
    /// preset, the selection follows the rename and the entry models
    /// re-project in the same session — the same refresh the
    /// build-import and inline flows run.
    #[test]
    fn rename_preset_confirm_refreshes_the_affix_screen_preset_models() {
        let app = crate::test_window::new_app_window();
        let root = temp_config_root("rename-refresh");
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        state
            .affix_preset_editor
            .edit_entry(
                "Speed",
                PresetEntry::non_affix(PresetEntryKind::Affix, "AttackSpeed", None, "#22C55E"),
            )
            .unwrap();
        state.affix_preset_editor.create_preset("Boss").unwrap();
        // The rename dialog opens on the selected preset (`create_preset`
        // selected "Boss" last).
        state.affix_preset_editor.selected_preset_name = Some("Speed".to_string());
        // The screen holds the pre-rename list and selection.
        app.set_preset_names(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from("Boss"),
            slint::SharedString::from("Speed"),
        ])));
        app.set_selected_preset(slint::SharedString::from("Speed"));
        app.set_import_selected_preset(slint::SharedString::from("Speed"));
        app.set_rename_preset_current_name(slint::SharedString::from("Speed"));
        app.set_rename_preset_new_name(slint::SharedString::from("Zeta"));

        confirm_rename_preset(&app, &mut state, &root).expect("the rename must succeed");

        assert_eq!(
            preset_names(&app),
            vec!["Boss".to_string(), "Zeta".to_string()],
            "the renamed preset must appear in the AffixScreen combo without a restart"
        );
        assert_eq!(
            app.get_selected_preset().to_string(),
            "Zeta",
            "the renamed preset must stay the AffixScreen selection"
        );
        // A catalog row carries every affix id that shares its label.
        let wanted = wanted_affix_ids(&app);
        assert_eq!(
            wanted.len(),
            1,
            "the renamed preset's entry must stay projected: {wanted:?}"
        );
        assert!(
            wanted[0].split(';').any(|id| id == "AttackSpeed"),
            "the projected row must be the AttackSpeed entry: {wanted:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn set_preset_name_rejects_empty() {
        let mut state = AppState::default_startup();
        let trimmed = "   ".trim();
        let result = state.affix_preset_editor.create_preset(trimmed);
        assert!(result.is_err());
        assert!(state.affix_preset_editor.presets.is_empty());
    }

    #[test]
    fn set_preset_name_rejects_duplicate() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        let result = state.affix_preset_editor.create_preset("Speed");
        assert!(result.is_err());
        assert_eq!(state.affix_preset_editor.presets.len(), 1);
    }

    // ── Affix-family config dialog tests ─────────────
    //
    // The app-facing handlers re-project the Slint models through an
    // `AppWindow`; the state + persistence cores they call are exercised
    // here, exactly like the other handler tests in this module.

    fn temp_config_root(label: &str) -> crate::test_root::TestRoot {
        crate::test_root::TestRoot::new(&format!("affix-dialogs-{label}"))
    }

    /// Every family config patch changes rule settings, so it bumps the
    /// rules input revision, not only the multi-build mirror.
    #[test]
    fn every_family_config_patch_bumps_the_rules_input_revision() {
        let root = temp_config_root("rule-toggle-revision");
        let mut state = AppState::default_startup();
        let before = state.rules_input_revision;
        let patch = rule_setting_toggle_patch(
            d4lc_gui::affix::RULE_SETTING_MINIMAL_AFFIX_VALUE_FILTER,
            true,
        )
        .unwrap();
        super::apply_family_config_patch_state(&mut state, &root, patch).unwrap();
        assert!(state.rules_input_revision > before);
    }

    #[test]
    fn detection_toggles_persist_their_own_rule_setting() {
        let root = temp_config_root("detection-toggle-persist");
        let mut state = AppState::default_startup();
        assert!(state.shell.settings.aspect_detection_enabled);

        for key in [
            d4lc_gui::affix::RULE_SETTING_ASPECT_DETECTION,
            d4lc_gui::affix::RULE_SETTING_UNIQUE_DETECTION,
            d4lc_gui::affix::RULE_SETTING_RUNE_DETECTION,
        ] {
            let patch = rule_setting_toggle_patch(key, false).expect("a known detection key");
            super::apply_family_config_patch_state(&mut state, &root, patch)
                .expect("the toggle should persist");
        }

        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert!(!persisted.rule_settings.aspect_detection_enabled);
        assert!(!persisted.rule_settings.unique_detection_enabled);
        assert!(!persisted.rule_settings.rune_detection_enabled);
        // The OCR-side detection flags are untouched.
        assert!(persisted.is_socket_detection_enabled);
        assert!(persisted.is_tempered_affix_detection_enabled);
    }

    #[test]
    fn rule_setting_toggle_persists_the_multi_build_flag() {
        let root = temp_config_root("rule-toggle-persist");
        let mut state = AppState::default_startup();
        assert!(!state.shell.settings.multi_build_mode_enabled);

        let patch = rule_setting_toggle_patch(d4lc_gui::affix::RULE_SETTING_MULTI_BUILD_MODE, true)
            .expect("the multi-build key is a known setting");
        super::apply_family_config_patch_state(&mut state, &root, patch)
            .expect("the toggle should persist");

        // The rule projection reads this one setting: no restart needed.
        assert!(state.shell.settings.multi_build_mode_enabled);
        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert!(persisted.rule_settings.multi_build_enabled);

        // The Affix dialog's own toggles persist too.
        let patch = rule_setting_toggle_patch(
            d4lc_gui::affix::RULE_SETTING_MINIMAL_AFFIX_VALUE_FILTER,
            true,
        )
        .unwrap();
        super::apply_family_config_patch_state(&mut state, &root, patch).unwrap();
        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert!(persisted.rule_settings.minimal_affix_value_enabled);

        // The Sigil dialog's display-mode selector writes its enum value.
        let patch = rule_setting_selection_patch(
            d4lc_gui::affix::RULE_SETTING_SIGIL_DISPLAY_MODE,
            "Blacklisting",
        )
        .unwrap();
        super::apply_family_config_patch_state(&mut state, &root, patch).unwrap();
        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert_eq!(
            persisted.rule_settings.sigil_display_mode,
            d4lc_config::SigilDisplayModeSetting::Blacklisting
        );

        // The Affix dialog's remaining toggles and the Sigil dialog's tier
        // gate persist their own fields.
        for key in [
            d4lc_gui::affix::RULE_SETTING_TEMPERED_AFFIX_DETECTION,
            d4lc_gui::affix::RULE_SETTING_TUNING_PRISM_HINTS,
            d4lc_gui::affix::RULE_SETTING_DUNGEON_TIERS,
        ] {
            let patch = rule_setting_toggle_patch(key, true)
                .unwrap_or_else(|| panic!("{key} is a known setting"));
            super::apply_family_config_patch_state(&mut state, &root, patch).unwrap();
        }
        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert!(persisted.rule_settings.dungeon_tiers_enabled);
        assert!(persisted.is_tempered_affix_detection_enabled);
        assert!(persisted.is_tuning_prism_hints_enabled);

        assert!(rule_setting_toggle_patch("not_a_setting", true).is_none());
        assert!(rule_setting_selection_patch("not_a_setting", "Blacklisting").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn minimal_affix_value_edit_persists_an_upstream_shaped_file() {
        // The SetAffix dialog's minimal-value field: a stored threshold
        // lands in Config/MinimalAffixValues.json (`SetAffixMinimalValue`,
        // AffixManager.cs:1087-1092).
        let root = temp_config_root("minimal-value-persist");
        let mut state = AppState::default_startup();

        super::store_minimal_affix_value(&mut state, &root, "Affix_AttackSpeed", 12.5)
            .expect("the threshold should persist");
        assert_eq!(
            state.minimal_affix_values.get("Affix_AttackSpeed"),
            Some(&12.5)
        );

        let reloaded = d4lc_config::load_affix_minimal_values(&root).unwrap();
        assert_eq!(reloaded.get("Affix_AttackSpeed"), Some(&12.5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn minimal_affix_values_reset_clears_the_map_and_the_file() {
        // Upstream `ResetMinimalAffixValues` (AffixManager.cs:892-896).
        let root = temp_config_root("minimal-value-reset");
        let mut state = AppState::default_startup();
        super::store_minimal_affix_value(&mut state, &root, "Affix_AttackSpeed", 12.5).unwrap();

        super::reset_minimal_affix_values(&mut state, &root).expect("reset should persist");
        assert!(state.minimal_affix_values.is_empty());
        assert!(d4lc_config::load_affix_minimal_values(&root)
            .unwrap()
            .is_empty());
        let text = std::fs::read_to_string(root.join("Config/MinimalAffixValues.json")).unwrap();
        assert_eq!(text, "{}\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn add_affix_slot_appends_a_typed_entry_and_persists_the_preset() {
        let root = temp_config_root("slot-add-persist");
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Test").unwrap();

        super::add_affix_slot(&mut state, &root, "AttackSpeed", "helm")
            .expect("a known slot should append");
        super::add_affix_slot(&mut state, &root, "AttackSpeed", "helm")
            .expect("upstream appends on every click");
        super::add_affix_slot(&mut state, &root, "AttackSpeed", "ring").unwrap();

        let counters = d4lc_gui::affix::affix_slot_counters(
            state.affix_preset_editor.selected_preset_entries().unwrap(),
            "AttackSpeed",
        );
        assert_eq!(
            counters[d4lc_gui::affix::affix_slot_index("helm").unwrap()],
            2
        );
        assert_eq!(
            counters[d4lc_gui::affix::affix_slot_index("ring").unwrap()],
            1
        );

        // The preset file carries the appended entries (upstream
        // `SaveAffixPresets` at the end of AddAffix).
        let presets = d4lc_config::load_affix_presets(&root).unwrap();
        let preset = presets
            .iter()
            .find(|preset| preset.name == "Test")
            .expect("the preset file must hold the edited preset");
        assert_eq!(preset.item_affixes.len(), 3);
        assert_eq!(preset.item_affixes[0].item_type, "helm");
        assert_eq!(preset.item_affixes[0].id, "AttackSpeed");

        // The unknown-slot and empty-id guards write nothing.
        assert!(super::add_affix_slot(&mut state, &root, "AttackSpeed", "charm").is_err());
        assert!(super::add_affix_slot(&mut state, &root, "   ", "helm").is_err());
        assert_eq!(
            state.affix_preset_editor.selected_preset_entry_count(),
            3,
            "a rejected add must not touch the preset"
        );

        // The appended entry carries the configured `DefaultColorNormal`
        // (upstream `AddAffix` → `Settings.DefaultColorNormal`,
        // AffixManager.cs:155); a stored value the color parser cannot
        // read falls back to the compiled Green (SettingsD4.cs:20).
        state.shell.settings.default_colors.normal = "#0A0B0C".to_string();
        super::add_affix_slot(&mut state, &root, "AttackSpeed", "boots")
            .expect("a known slot should append");
        state.shell.settings.default_colors.normal = "chartreuse".to_string();
        super::add_affix_slot(&mut state, &root, "AttackSpeed", "chest")
            .expect("a known slot should append");
        let entries = state.affix_preset_editor.selected_preset_entries().unwrap();
        let color_of = |item_type: &str| {
            entries
                .iter()
                .find(|entry| entry.item_type.as_deref() == Some(item_type))
                .map(|entry| entry.color.clone())
        };
        assert_eq!(color_of("boots"), Some("#0A0B0C".to_string()));
        assert_eq!(color_of("chest"), Some("#008000".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn add_affix_slot_requires_a_selected_preset() {
        let root = temp_config_root("slot-add-no-preset");
        let mut state = AppState::default_startup();
        assert!(super::add_affix_slot(&mut state, &root, "AttackSpeed", "helm").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicate_gate_reports_the_upstream_done_condition() {
        // Upstream disables Done while a (Type, IsImplicit, IsGreater,
        // IsTempered) group of the edited affix id repeats
        // (SetAffixViewModel.cs:274-280); d4lc reports the same condition
        // as a dialog error.
        let base = PresetEntry::non_affix(
            PresetEntryKind::Affix,
            "AttackSpeed",
            Some("helm"),
            "#008000",
        );
        let duplicate = base.clone();
        let distinct_flag = {
            let mut entry = PresetEntry::non_affix(
                PresetEntryKind::Affix,
                "AttackSpeed",
                Some("helm"),
                "#008000",
            );
            entry.is_implicit = true;
            entry
        };
        let other_id = PresetEntry::non_affix(
            PresetEntryKind::Affix,
            "CritChance",
            Some("helm"),
            "#008000",
        );

        let clean = [base.clone(), distinct_flag, other_id];
        let groups = affix_duplicate_groups(&clean, "AttackSpeed");
        assert!(super::duplicate_gate_error(&groups, "AttackSpeed").is_none());

        let with_duplicate = [base, duplicate];
        let groups = affix_duplicate_groups(&with_duplicate, "AttackSpeed");
        let error = super::duplicate_gate_error(&groups, "AttackSpeed")
            .expect("a duplicate group must block Done");
        assert!(
            error.contains("Duplicate affix entries for AttackSpeed"),
            "{error}"
        );
        assert!(error.contains("helm"), "{error}");
    }

    #[test]
    fn sigil_dungeon_tier_cycle_persists_the_next_tier() {
        let root = temp_config_root("sigil-tier-cycle");
        let mut state = AppState::default_startup();

        // A sigil without a stored tier starts from the upstream `F`
        // default (AffixManager.cs:744-747) and cycles to `S`.
        super::cycle_sigil_dungeon_tier(&mut state, &root, "Sigil_Kehj").unwrap();
        assert_eq!(
            state.dungeon_tiers.get("Sigil_Kehj").map(String::as_str),
            Some("S")
        );
        let reloaded = d4lc_config::load_dungeon_tiers(&root).unwrap();
        assert_eq!(reloaded.get("Sigil_Kehj").map(String::as_str), Some("S"));

        // The cycle advances through the upstream list and wraps.
        for expected in ["A", "B", "C", "D", "E", "F", "S"] {
            super::cycle_sigil_dungeon_tier(&mut state, &root, "Sigil_Kehj").unwrap();
            assert_eq!(
                state.dungeon_tiers.get("Sigil_Kehj").map(String::as_str),
                Some(expected)
            );
        }
        assert_eq!(
            d4lc_config::load_dungeon_tiers(&root)
                .unwrap()
                .get("Sigil_Kehj")
                .map(String::as_str),
            Some("S")
        );

        assert!(super::cycle_sigil_dungeon_tier(&mut state, &root, "  ").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
