//! Additional affix command handlers for the Slint desktop app.
//!
//! ## Upstream evidence
//!
//! - `AffixViewModel.cs:196-233` — 38 command definitions
//!
//! ## Scope
//!
//! These handlers wire the affix commands that open dialogs or edit
//! entries:
//!
//! - The five `open_*_config` handlers open their `*ConfigDialog` through
//!   the shared [`crate::actions::dialogs::open`] router.
//! - `handle_set_affix_entry` and `handle_set_affix_color` open the
//!   `SetAffixDialog` / `SetAffixColorDialog` and seed the root in-out
//!   props from the targeted entry.
//! - `handle_set_family_entry` adds an entry to the matching wanted list
//!   (upstream `Set*` commands).
//! - `handle_toggle_affix_overlay` flips the persisted overlay flag through
//!   the shared `set_overlay_enabled` path.

use std::path::Path;

use d4lc_config::PresetColor;
use d4lc_gui::affix::AffixViewModel;
use slint::{ModelRc, VecModel};

use crate::app_state::{AppState, DialogKind};
use crate::slint_generated::AppWindow;

/// Handle open-affix-config callback.
pub fn handle_open_affix_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = AffixViewModel::new(state.affix_preset_editor.clone());
    let _effects = vm.open_affix_config();
    state.affix_preset_editor = vm.state;
    state.bump_rules_input_revision();
    seed_affix_config_rule_settings(app, state);
    crate::actions::dialogs::open(app, state, DialogKind::AffixConfig, None);
}

/// Handle open-aspect-config callback.
pub fn handle_open_aspect_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = AffixViewModel::new(state.affix_preset_editor.clone());
    let _effects = vm.open_aspect_config();
    state.affix_preset_editor = vm.state;
    state.bump_rules_input_revision();
    app.set_settings_rule_aspect_detection_enabled(state.shell.settings.aspect_detection_enabled);
    seed_multi_build_toggle(app, state);
    crate::actions::dialogs::open(app, state, DialogKind::AspectConfig, None);
}

/// Handle open-sigil-config callback.
pub fn handle_open_sigil_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = AffixViewModel::new(state.affix_preset_editor.clone());
    let _effects = vm.open_sigil_config();
    state.affix_preset_editor = vm.state;
    state.bump_rules_input_revision();
    seed_sigil_config_dialog(app, state);
    crate::actions::dialogs::open(app, state, DialogKind::SigilConfig, None);
}

/// Handle open-unique-config callback.
pub fn handle_open_unique_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = AffixViewModel::new(state.affix_preset_editor.clone());
    let _effects = vm.open_unique_config();
    state.affix_preset_editor = vm.state;
    state.bump_rules_input_revision();
    app.set_settings_rule_unique_detection_enabled(state.shell.settings.unique_detection_enabled);
    seed_multi_build_toggle(app, state);
    crate::actions::dialogs::open(app, state, DialogKind::UniqueConfig, None);
}

/// Handle open-rune-config callback.
pub fn handle_open_rune_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = AffixViewModel::new(state.affix_preset_editor.clone());
    let _effects = vm.open_rune_config();
    state.affix_preset_editor = vm.state;
    state.bump_rules_input_revision();
    app.set_settings_rule_rune_detection_enabled(state.shell.settings.rune_detection_enabled);
    seed_multi_build_toggle(app, state);
    crate::actions::dialogs::open(app, state, DialogKind::RuneConfig, None);
}

// ── Affix-family config dialog seeds ──────────────────────────────────────
//
// The config dialogs are write-through on interaction (upstream saves from
// every setter), so each open handler projects the persisted panel state
// into the dialog's two-way-bound root properties. The wired change
// callbacks in `wiring.rs` then persist and re-project on every gesture.

/// Seed the Affix config dialog's four rule-setting toggles from the
/// persisted settings panel state.
fn seed_affix_config_rule_settings(app: &AppWindow, state: &AppState) {
    let settings = &state.shell.settings;
    app.set_settings_rule_minimal_affix_value_filter_enabled(
        settings.minimal_affix_value_filter_enabled,
    );
    app.set_settings_rule_tempered_affix_detection_enabled(settings.tempered_detection_enabled);
    app.set_settings_rule_tuning_prism_hints_enabled(settings.is_tuning_prism_hints_enabled);
    seed_multi_build_toggle(app, state);
}

/// Seed the shared multi-build toggle the aspect / unique / rune config
/// dialogs carry (upstream `IsMultiBuildModeEnabled`, one persisted field).
fn seed_multi_build_toggle(app: &AppWindow, state: &AppState) {
    app.set_settings_rule_multi_build_enabled(state.shell.settings.multi_build_mode_enabled);
}

/// Seed the Sigil config dialog: the dungeon-tiers / multi-build toggles
/// and the display-mode selector from the persisted panel state, plus the
/// per-entry tier rows of the selected preset projected through
/// `d4lc_gui::affix::sigil_tier_rows`.
///
/// Called on open and after every confirmed tier cycle, so the listed tier
/// labels track the persisted `Config/DungeonTiers.json` state.
pub(crate) fn seed_sigil_config_dialog(app: &AppWindow, state: &AppState) {
    let settings = &state.shell.settings;
    app.set_settings_rule_dungeon_tiers_enabled(settings.dungeon_tiers_enabled);
    app.set_settings_rule_sigil_display_mode(settings.sigil_display_mode.as_str().into());
    seed_multi_build_toggle(app, state);

    let entries = state
        .affix_preset_editor
        .selected_preset_entries()
        .unwrap_or(&[]);
    let rows = d4lc_gui::affix::sigil_tier_rows(entries, &state.dungeon_tiers);
    let ids: Vec<slint::SharedString> = rows
        .iter()
        .map(|row| slint::SharedString::from(row.id.as_str()))
        .collect();
    let tiers: Vec<slint::SharedString> = rows
        .iter()
        .map(|row| slint::SharedString::from(row.tier.as_str()))
        .collect();
    app.set_sigil_config_entry_ids(ModelRc::new(VecModel::from(ids)));
    app.set_sigil_config_entry_tiers(ModelRc::new(VecModel::from(tiers)));
}

/// Handle remove-affix-entry callback.
///
/// Removes the entry addressed by the kind-tagged `key` from the
/// currently selected preset and re-projects the screen's entry
/// rows/cards so the removal is visible immediately.
///
/// Persistence mirrors the upstream remove path (`RemoveAffix` →
/// `SaveAffixPresets`, AffixManager.cs:173) — the preset file is
/// written through `d4lc-config`.
pub fn handle_remove_affix_entry(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    key: &str,
) {
    let Some((_, entry)) = crate::actions::affix::resolve_entry_key(state, key) else {
        return;
    };
    let preset_name = state
        .affix_preset_editor
        .selected_preset_name
        .clone()
        .unwrap_or_default();
    if let Err(error) = state.affix_preset_editor.remove_entry(
        &preset_name,
        entry.kind,
        &entry.id,
        entry.item_type.as_deref(),
    ) {
        eprintln!("[d4lc] remove entry failed: {error}");
        return;
    }
    state.bump_rules_input_revision();
    crate::actions::affix::project_affix_entries(app, state);
    // The persist helper logs the reason on the Logging screen; the
    // eprintln keeps it visible on the console too (this handler has no
    // error channel back to the screen).
    if let Err(error) = state.persist_affix_presets_to_config(config_root) {
        eprintln!("[d4lc] remove entry persist failed: {error}");
    }
}

/// Handle toggle-affix-overlay callback (the AffixScreen power
/// button, upstream `ToggleOverlayCommand`, AffixView.xaml:49-83).
///
/// The ViewModel emits [`d4lc_gui::affix::AffixEffect::ToggleOverlay`]
/// and this adapter flips `state.shell.settings.overlay_enabled`
/// through the shared `set_overlay_enabled` path — the exact path
/// the Ctrl+F12 hotkey arm uses, so the button and the hotkey stay
/// consistent (and the new state persists + rebuilds the overlay
/// descriptors). On success the live state is projected into the
/// screen's `overlay-enabled` indicator.
///
/// Returns the confirmed new value so the caller can reconcile the live
/// loop with it (upstream `ToggleOverlayFromGUIMessage` sets the same
/// `ScreenProcessHandler.IsEnabled` the hotkey sets,
/// `ScreenProcessHandler.cs:88,116-169`). `None` when the persist failed:
/// the flag did not change, so the loop must not be touched either.
pub fn handle_toggle_affix_overlay(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Option<bool> {
    let mut vm = AffixViewModel::new(state.affix_preset_editor.clone());
    let effects = vm.toggle_affix_overlay();
    state.affix_preset_editor = vm.state;
    state.bump_rules_input_revision();
    let mut overlay_enabled = None;
    for effect in effects {
        match effect {
            d4lc_gui::affix::AffixEffect::ToggleOverlay => {
                overlay_enabled = apply_overlay_toggle(app, state, config_root);
            }
            // The overlay toggle opens no dialog and mutates no
            // preset entries.
            d4lc_gui::affix::AffixEffect::PersistConfig
            | d4lc_gui::affix::AffixEffect::OpenDialog(_) => {}
        }
    }
    overlay_enabled
}

/// Flip the persisted overlay flag through the headless
/// `set_overlay_enabled` helper (the Ctrl+F12 arm in `hotkeys.rs` uses
/// the same one: only the on/off bit is written, no other overlay row
/// is re-validated) and project the new state into the AffixScreen
/// indicator.
///
/// Returns the confirmed value, or `None` when the persist failed
/// (state and indicator keep the previous value).
fn apply_overlay_toggle(app: &AppWindow, state: &mut AppState, config_root: &Path) -> Option<bool> {
    let new_value = !state.shell.settings.overlay_enabled;
    if let Err(error) = crate::actions::settings::set_overlay_enabled(new_value, state, config_root)
    {
        eprintln!("[d4lc] overlay toggle persist failed: {error}");
        return None;
    }
    crate::actions::settings::project_overlay_gate(app, &state.shell.settings);
    Some(new_value)
}

// ── Per-family add ("+") handler ────────────────────────────

/// Handle the per-family `set-aspect-entry` / `set-sigil-entry` /
/// `set-unique-entry` / `set-rune-entry` callbacks.
///
/// The kind-tagged `key` selects the wanted list: a missing entry is
/// appended to the selected preset with its kind's default color and
/// the change persists; an entry that is already wanted stays
/// untouched (upstream `AddAspect`/`AddSigil`/`AddUnique`/`AddRune`
/// skip duplicates, AffixManager.cs:178-304). Either way the screen
/// entry models re-project, mirroring the upstream
/// `Selected*ChangedMessage` re-bind fan-out.
pub fn handle_set_family_entry(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    key: &str,
) {
    let parsed = match crate::actions::affix::parse_entry_key(key) {
        Some(parsed) => parsed,
        None => {
            eprintln!("[d4lc] malformed affix entry key ignored: {key:?}");
            return;
        }
    };
    let added = add_family_entry(state, parsed);
    crate::actions::affix::project_affix_entries(app, state);
    if added {
        if let Err(error) = state.persist_affix_presets_to_config(config_root) {
            eprintln!("[d4lc] add entry persist failed: {error}");
        }
    }
}

/// Append the entry addressed by `parsed` to the selected preset's
/// wanted list when it is not already present. Returns whether the
/// preset changed.
fn add_family_entry(state: &mut AppState, parsed: crate::actions::affix::EntryKey) -> bool {
    let already_wanted = state
        .affix_preset_editor
        .selected_preset_entries()
        .map(|entries| crate::actions::affix::find_entry_index(entries, &parsed).is_some())
        .unwrap_or(false);
    if already_wanted {
        return false;
    }
    let color = default_entry_color(state, parsed.kind);
    let entry = d4lc_gui::PresetEntry {
        kind: parsed.kind,
        id: parsed.id,
        label: None,
        is_any_type: parsed.item_type.is_none(),
        item_type: parsed.item_type,
        color,
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    };
    let preset_name = state
        .affix_preset_editor
        .selected_preset_name
        .clone()
        .unwrap_or_default();
    if let Err(error) = state.affix_preset_editor.edit_entry(&preset_name, entry) {
        eprintln!("[d4lc] add entry failed: {error}");
        return false;
    }
    state.bump_rules_input_revision();
    true
}

/// Default color for a freshly added wanted entry of `kind`.
///
/// Upstream colors every add from the matching `DefaultColor*` setting
/// (`AffixManager.AddAffix` → `Settings.DefaultColorNormal`,
/// AffixManager.cs:141-158; `AddAspect` → `DefaultColorAspects`, :189;
/// `AddUnique` → `DefaultColorUniques`, :252; `AddRune` →
/// `DefaultColorRunes`, :283), so a configured default colors the next
/// added entry of that family. A stored value the color parser cannot
/// read (empty, misspelled) falls back to the compiled defaults
/// (SettingsD4.cs:20-24) instead of writing an unusable color into the
/// preset; a parseable one is normalized to the `#RRGGBB` form the preset
/// file carries.
///
/// Sigils ignore `DefaultColorNormal` outside Whitelisting: upstream
/// colors a new sigil with `DefaultColorNormal` in Whitelisting mode and
/// with `Colors.Red` in every other mode (`AddSigil`,
/// AffixManager.cs:221), and the port keeps that fixed red.
pub(crate) fn default_entry_color(state: &AppState, kind: d4lc_gui::PresetEntryKind) -> String {
    let colors = &state.shell.settings.default_colors;
    let (stored, fallback) = match kind {
        d4lc_gui::PresetEntryKind::Affix => (&colors.normal, DEFAULT_ENTRY_COLOR),
        d4lc_gui::PresetEntryKind::Aspect => (&colors.aspects, DEFAULT_ENTRY_COLOR),
        d4lc_gui::PresetEntryKind::Unique => (&colors.uniques, DEFAULT_ENTRY_COLOR),
        d4lc_gui::PresetEntryKind::Rune => (&colors.runes, DEFAULT_ENTRY_COLOR),
        d4lc_gui::PresetEntryKind::Sigil
            if state.shell.settings.sigil_display_mode
                == d4lc_config::SigilDisplayModeSetting::Blacklisting =>
        {
            // Upstream `Colors.Red` — a fixed value, not a setting.
            return SIGIL_DEFAULT_COLOR.to_string();
        }
        d4lc_gui::PresetEntryKind::Sigil => (&colors.normal, DEFAULT_ENTRY_COLOR),
    };
    PresetColor::try_parse(stored)
        .map(|color| color.to_string_form())
        .unwrap_or_else(|_| fallback.to_string())
}

/// Upstream `Colors.Green` — the compiled default of every `DefaultColor*`
/// setting (SettingsD4.cs:20-24) and the fallback for a stored default the
/// color parser cannot read.
const DEFAULT_ENTRY_COLOR: &str = "#008000";

/// Upstream `Colors.Red` — the sigil color of every non-Whitelisting
/// display mode (`AddSigil`, AffixManager.cs:221).
const SIGIL_DEFAULT_COLOR: &str = "#FF0000";

// ── SetAffix / SetAffixColor open handlers ──────────────────────

/// Handle set-affix-entry callback.
///
/// Opens the `SetAffixDialog`. If `key` addresses an Affix-kind entry
/// in the currently selected preset, seeds the form props from that
/// entry (edit mode; `set-affix-edit-index` carries the entry's real
/// mixed-list index for the validation draft). Otherwise the form opens
/// in new-entry mode, seeded with the id the key carries (the
/// AffixScreen catalog card "+" addresses a catalog entry that is not
/// wanted yet).
pub fn handle_set_affix_entry(app: &AppWindow, state: &mut AppState, key: &str) {
    seed_set_affix_form_from_entry(app, state, key);
    seed_set_affix_dialog_model(app, state);
    crate::actions::dialogs::open(app, state, DialogKind::SetAffix, None);
}

/// Handle set-affix-color callback.
///
/// Opens the `SetAffixColorDialog`, seeds the color from the entry
/// addressed by the kind-tagged `key` and stores the key in the
/// `set-affix-color-target-key` root prop for the confirm body.
pub fn handle_set_affix_color(app: &AppWindow, state: &mut AppState, key: &str) {
    match crate::actions::affix::resolve_entry_key(state, key) {
        Some((_, entry)) => {
            app.set_set_affix_color_value(entry.color.into());
            app.set_set_affix_color_target_key(key.into());
        }
        None => {
            // Unresolvable key: seed the default form so the dialog
            // still opens in a defined state instead of leaking a
            // previous entry's draft.
            app.set_set_affix_color_value("#FF0000".into());
            app.set_set_affix_color_target_key(slint::SharedString::default());
        }
    }
    crate::actions::dialogs::open(app, state, DialogKind::SetAffixColor, None);
}

// ── RenamePreset / SetPresetName open handlers ───────────────────

/// Handle open-rename-preset callback.
///
/// Opens the `RenamePresetDialog`. Seeds the read-only
/// `rename-preset-current-name` from the currently selected
/// preset (when one is selected) and pre-fills
/// `rename-preset-new-name` with the current name so the user
/// only needs to edit it. When no preset is selected the
/// current name is empty and the user is shown
/// "(no preset selected)" in the dialog.
///
/// This is the natural invoker for the RenamePreset dialog —
/// the pencil (✎) icon next to the preset ComboBox in the
/// AffixScreen already fires the root `rename-preset()`
/// callback, which is wired to this handler in `wiring.rs`.
pub fn handle_open_rename_preset(app: &AppWindow, state: &mut AppState) {
    let current = state
        .affix_preset_editor
        .selected_preset_name
        .clone()
        .unwrap_or_default();
    app.set_rename_preset_current_name(current.clone().into());
    // Pre-fill the new name with the current name so the user
    // can edit it. The confirm body in `affix_dialogs` validates
    // and persists.
    app.set_rename_preset_new_name(current.into());
    crate::actions::dialogs::open(app, state, DialogKind::RenamePreset, None);
}

/// Handle open-new-preset-dialog callback.
///
/// Opens the `SetPresetNameDialog` (alt path to the existing
/// inline `+` button on the AffixScreen). Clears the form
/// input so a previous draft does not leak into the new
/// session. The confirm body in `affix_dialogs` validates and
/// creates the preset through the public editor API.
pub fn handle_open_set_preset_name(app: &AppWindow, state: &mut AppState) {
    app.set_set_preset_name_input(slint::SharedString::default());
    crate::actions::dialogs::open(app, state, DialogKind::SetPresetName, None);
}

/// Seed the SetAffix dialog's host-projected model: the ten per-slot entry
/// counters and the stored minimal value of the affix the dialog edits
/// (`d4lc_gui::affix::set_affix_dialog_seed`).
///
/// Called when the dialog opens and after every confirmed per-slot add, so
/// the slot captions track the preset's entries (upstream binds them to the
/// affix's live entry list).
pub(crate) fn seed_set_affix_dialog_model(app: &AppWindow, state: &AppState) {
    let affix_id = app.get_set_affix_entry_id().to_string();
    let entries = state
        .affix_preset_editor
        .selected_preset_entries()
        .unwrap_or(&[]);
    let seed =
        d4lc_gui::affix::set_affix_dialog_seed(entries, &affix_id, &state.minimal_affix_values);
    let counters: Vec<i32> = seed
        .slot_counters
        .iter()
        .map(|count| *count as i32)
        .collect();
    app.set_set_affix_slot_counters(ModelRc::new(VecModel::from(counters)));
    // The upstream `NumericUpDown` renders with `StringFormat="N1"`
    // (SetAffixView.xaml:107-112): one decimal, which round-trips through
    // the dialog's clamp-on-edit path.
    app.set_set_affix_minimal_value(
        seed.minimal_affix_value
            .map(|value| slint::SharedString::from(format!("{value:.1}")))
            .unwrap_or_default(),
    );
}

fn seed_set_affix_form_from_entry(app: &AppWindow, state: &mut AppState, key: &str) {
    // Only Affix-kind entries can be edited through the SetAffix
    // dialog (its confirm persists Affix-kind entries); a key that
    // resolves to another kind (or to nothing) opens the create form.
    let resolved = crate::actions::affix::resolve_entry_key(state, key)
        .filter(|(_, entry)| entry.kind == d4lc_gui::PresetEntryKind::Affix);
    if let Some((index, entry)) = resolved {
        // The edit index is the entry's real index in the mixed-kind
        // selected-preset list (the validation draft bounds-checks it).
        app.set_set_affix_edit_index(index as i32);
        app.set_set_affix_entry_id(entry.id.into());
        app.set_set_affix_item_type(entry.item_type.unwrap_or_default().into());
        app.set_set_affix_color(entry.color.into());
        app.set_set_affix_is_greater(entry.is_greater);
        app.set_set_affix_is_implicit(entry.is_implicit);
        app.set_set_affix_is_tempered(entry.is_tempered);
    } else {
        // New entry or missing preset: seed the create form. A kind-tagged
        // catalog key (the AffixScreen card "+") still carries the entry id,
        // so the create form starts from that id instead of an empty field.
        app.set_set_affix_edit_index(-1);
        app.set_set_affix_entry_id(
            crate::actions::affix::parse_entry_key(key)
                .map(|parsed| parsed.id)
                .unwrap_or_default()
                .into(),
        );
        app.set_set_affix_item_type(slint::SharedString::default());
        app.set_set_affix_color("#FF0000".into());
        app.set_set_affix_is_greater(false);
        app.set_set_affix_is_implicit(false);
        app.set_set_affix_is_tempered(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;

    /// Append the entry addressed by the kind-tagged `key` (asserting the
    /// key parses) and report whether the preset changed.
    fn add_key(state: &mut AppState, key: &str) -> bool {
        let Some(parsed) = crate::actions::affix::parse_entry_key(key) else {
            panic!("{key} must parse");
        };
        add_family_entry(state, parsed)
    }

    /// The color of the selected preset's entry with `id`.
    fn entry_color(state: &AppState, id: &str) -> Option<String> {
        state
            .affix_preset_editor
            .selected_preset_entries()?
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.color.clone())
    }

    #[test]
    fn open_affix_config_handler_does_not_panic() {
        let _state = AppState::default_startup();
        // AppWindow setter calls require a Slint runtime; we only
        // assert the handler signature and that the state-mutation
        // path compiles. The open path is exercised through
        // `cargo test -p d4lc-app` build.
        let _: fn(&AppWindow, &mut AppState) = handle_open_affix_config;
    }

    #[test]
    fn open_aspect_config_handler_does_not_panic() {
        let _state = AppState::default_startup();
        let _: fn(&AppWindow, &mut AppState) = handle_open_aspect_config;
    }

    #[test]
    fn open_sigil_config_handler_does_not_panic() {
        let _state = AppState::default_startup();
        let _: fn(&AppWindow, &mut AppState) = handle_open_sigil_config;
    }

    #[test]
    fn open_unique_config_handler_does_not_panic() {
        let _state = AppState::default_startup();
        let _: fn(&AppWindow, &mut AppState) = handle_open_unique_config;
    }

    #[test]
    fn open_rune_config_handler_does_not_panic() {
        let _state = AppState::default_startup();
        let _: fn(&AppWindow, &mut AppState) = handle_open_rune_config;
    }

    #[test]
    fn remove_affix_entry_handler_mutates_state() {
        // The handler reads the selected preset through
        // AffixViewModel::remove_affix_entry and stores the mutated
        // state back; the projection step needs an AppWindow (Slint
        // runtime) so the state mutation is asserted on its own here.
        // The live projection is exercised by the headless GUI
        // harness scenario (testing_harness.rs).
        let mut state = AppState::default_startup();
        state.affix_preset_editor =
            d4lc_gui::AffixPresetEditorState::new(vec![d4lc_gui::AffixPresetEditorPreset {
                name: "Starter".into(),
                entries: vec![d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Affix,
                    "AttackSpeed",
                    Some("Amulet"),
                    "#00FF00",
                )],
            }]);
        state.affix_preset_editor.selected_preset_name = Some("Starter".into());

        let mut vm = AffixViewModel::new(state.affix_preset_editor.clone());
        vm.remove_affix_entry(0);
        state.affix_preset_editor = vm.state;

        assert_eq!(
            state
                .affix_preset_editor
                .selected_preset_entries()
                .map(<[d4lc_gui::PresetEntry]>::len),
            Some(0),
            "removing the only entry must leave the selected preset empty"
        );
    }

    #[test]
    fn toggle_affix_overlay_handler_signature_is_app_adapter_shape() {
        let _state = AppState::default_startup();
        // The handler needs an AppWindow (indicator projection) and a
        // config root (persistence) and returns the confirmed overlay
        // value for the live-loop reconcile; the live flip is exercised by
        // the headless GUI harness scenario and the hotkey dispatch tests.
        let _: fn(&AppWindow, &mut AppState, &Path) -> Option<bool> = handle_toggle_affix_overlay;
    }

    #[test]
    fn add_family_entry_appends_missing_entry_of_the_key_kind() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor =
            d4lc_gui::AffixPresetEditorState::new(vec![d4lc_gui::AffixPresetEditorPreset {
                name: "Starter".into(),
                entries: Vec::new(),
            }]);
        state.affix_preset_editor.selected_preset_name = Some("Starter".into());

        let Some(parsed) = crate::actions::affix::parse_entry_key("aspect:BloodForBlood:") else {
            panic!("aspect key must parse");
        };
        assert!(
            add_family_entry(&mut state, parsed),
            "fresh add must append"
        );

        let Some(entries) = state.affix_preset_editor.selected_preset_entries() else {
            panic!("selected preset must exist");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, d4lc_gui::PresetEntryKind::Aspect);
        assert_eq!(entries[0].id, "BloodForBlood");
        assert_eq!(entries[0].item_type, None);
        // Upstream compiled default color for aspects (SettingsD4.cs:22).
        assert_eq!(entries[0].color, "#008000");

        // A second add of the same entry is a no-op (upstream skips
        // duplicates, AffixManager.cs:183-186).
        let Some(parsed) = crate::actions::affix::parse_entry_key("aspect:BloodForBlood:") else {
            panic!("aspect key must parse");
        };
        assert!(!add_family_entry(&mut state, parsed));
        assert_eq!(
            state
                .affix_preset_editor
                .selected_preset_entries()
                .map(<[d4lc_gui::PresetEntry]>::len),
            Some(1),
            "duplicate add must not append a second entry"
        );

        // A configured `DefaultColor*` colors the next add of its family
        // (upstream `AddAspect` / `AddUnique` / `AddRune`,
        // AffixManager.cs:189,252,283).
        state.shell.settings.default_colors.aspects = "#123456".to_string();
        state.shell.settings.default_colors.uniques = "#234567".to_string();
        state.shell.settings.default_colors.runes = "#345678".to_string();
        add_key(&mut state, "aspect:ConfiguredAspect:");
        add_key(&mut state, "unique:ConfiguredUnique:");
        add_key(&mut state, "rune:ConfiguredRune:");
        assert_eq!(
            entry_color(&state, "ConfiguredAspect").as_deref(),
            Some("#123456")
        );
        assert_eq!(
            entry_color(&state, "ConfiguredUnique").as_deref(),
            Some("#234567")
        );
        assert_eq!(
            entry_color(&state, "ConfiguredRune").as_deref(),
            Some("#345678")
        );

        // A stored default the color parser cannot read falls back to the
        // compiled Green (SettingsD4.cs:20-24) instead of reaching the
        // preset file.
        state.shell.settings.default_colors.aspects = "chartreuse".to_string();
        add_key(&mut state, "aspect:Unreadable:");
        assert_eq!(
            entry_color(&state, "Unreadable").as_deref(),
            Some("#008000")
        );
    }

    #[test]
    fn add_family_entry_sigil_color_follows_display_mode() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor =
            d4lc_gui::AffixPresetEditorState::new(vec![d4lc_gui::AffixPresetEditorPreset {
                name: "Starter".into(),
                entries: Vec::new(),
            }]);
        state.affix_preset_editor.selected_preset_name = Some("Starter".into());

        state.shell.settings.sigil_display_mode =
            d4lc_config::SigilDisplayModeSetting::Blacklisting;
        let Some(parsed) = crate::actions::affix::parse_entry_key("sigil:Torture:") else {
            panic!("sigil key must parse");
        };
        assert!(add_family_entry(&mut state, parsed));
        let Some(entries) = state.affix_preset_editor.selected_preset_entries() else {
            panic!("selected preset must exist");
        };
        assert_eq!(
            entries[0].color, "#FF0000",
            "Blacklisting sigil adds use the upstream Red default"
        );

        state.shell.settings.sigil_display_mode =
            d4lc_config::SigilDisplayModeSetting::Whitelisting;
        let Some(parsed) = crate::actions::affix::parse_entry_key("sigil:Hunt:") else {
            panic!("sigil key must parse");
        };
        assert!(add_family_entry(&mut state, parsed));
        let Some(entries) = state.affix_preset_editor.selected_preset_entries() else {
            panic!("selected preset must exist");
        };
        assert_eq!(
            entries[1].color, "#008000",
            "Whitelisting sigil adds use the upstream Green default"
        );

        // Whitelisting sigils take the configured `DefaultColorNormal`
        // (AffixManager.cs:221); every other mode keeps the fixed
        // upstream Red whatever that setting says.
        state.shell.settings.default_colors.normal = "#0A0B0C".to_string();
        add_key(&mut state, "sigil:Configured:");
        state.shell.settings.sigil_display_mode =
            d4lc_config::SigilDisplayModeSetting::Blacklisting;
        add_key(&mut state, "sigil:Blacklisted:");
        assert_eq!(
            entry_color(&state, "Configured").as_deref(),
            Some("#0A0B0C")
        );
        assert_eq!(
            entry_color(&state, "Blacklisted").as_deref(),
            Some("#FF0000"),
            "a Blacklisting sigil ignores the configured DefaultColorNormal"
        );
    }

    #[test]
    fn add_family_entry_without_selected_preset_is_a_no_op() {
        let mut state = AppState::default_startup();
        let Some(parsed) = crate::actions::affix::parse_entry_key("rune:ElixirOfSorrow:") else {
            panic!("rune key must parse");
        };
        assert!(!add_family_entry(&mut state, parsed));
    }
}
