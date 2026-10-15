//! Affix preset CRUD action handlers for the Slint desktop app.
//!
//! ## Upstream evidence
//!
//! - `AffixViewModel.cs:919-931` — `CanAddAffixPresetNameExecute`, `AddAffixPresetNameExecute`
//! - `AffixViewModel.cs:1746-1761` — `CanRemoveAffixPresetNameExecute`, `RemoveAffixPresetNameExecute`
//!
//! ## Design notes
//!
//! - `create-preset` and `remove-preset` have real handlers.
//! - `rename-preset` opens the
//!   `RenamePresetDialog` via the affix_cmds open handler and
//!   delegates the dialog-driven rename flow to the shared
//!   `actions::dialogs` router.
//! - `import-preset` delegates to the existing offline build-import dialog flow.
//! - The AffixScreen's five entry models are root-declared and
//!   projected from the selected preset's entries by
//!   [`project_affix_entries`]; every preset-entry mutation path
//!   (preset create/remove, SetAffix / SetAffixColor confirms,
//!   entry removal, hotkey preset switches) re-projects through it,
//!   mirroring the upstream WPF binding where the wanted-entry
//!   lists observe the view model directly.
//! - Every preset-list mutation (create, rename, remove, build
//!   import, local import, merge) refreshes the AffixScreen preset
//!   ComboBox and the selected preset's entries through the single
//!   [`refresh_affix_screen_preset_models`] seam, mirroring the
//!   observable `AffixPresets` / `SelectedAffixPreset` updates of
//!   `AffixViewModel`.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use slint::{Model, SharedString, VecModel};

use crate::app_state::{AppState, ConfirmAction, DEFAULT_CATALOG_WINDOW};
use crate::slint_generated::{
    AffixEntry, AffixRailRow, AppWindow, AspectEntry, RuneEntry, SigilEntry, UniqueEntry,
};

/// The catalog window step in rows: the page the AffixScreen's bottom-edge
/// trigger asks for (`AffixScreen.catalog-page-size`) and the most one
/// request may grow the window by.
pub const CATALOG_WINDOW_PAGE: usize = 60;

/// Projections run by this process, for the real-input harness: a scroll
/// gesture that reaches the grid's bottom edge must re-project the windowed
/// models a bounded number of times instead of once per scroll event.
#[cfg(test)]
pub static AFFIX_PROJECTION_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Wanted-entry rail rebuilds, for the same assertion: the rail is a
/// function of the preset alone, so a render-window growth must not touch it.
#[cfg(test)]
pub static AFFIX_RAIL_BUILD_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
thread_local! {
    /// The same two counters for the current thread only: unit tests run in
    /// parallel in one process, so a test that asserts exact counts reads
    /// these instead of the process-wide totals above.
    static THREAD_PROJECTION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static THREAD_RAIL_BUILD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Handle the `create-preset()` callback.
///
/// Reads `new-preset-name` from AppWindow, creates the preset via
/// `AffixPresetEditorState::create_preset()`, and updates the
/// `preset-names` model and `selected-preset` on AppWindow.
///
/// Persistence mirrors the upstream create path (`AddAffixPreset` →
/// `SaveAffixPresets`, then the `SelectedAffixPreset` setter →
/// `SaveSettings`): the preset file and the new selection are both
/// written through `d4lc-config`.
pub fn handle_create_preset(app: &AppWindow, state: &mut AppState, config_root: &Path) {
    let name = app.get_new_preset_name().trim().to_string();
    if name.is_empty() {
        return;
    }
    match state.affix_preset_editor.create_preset(&name) {
        Ok(()) => {
            state.bump_rules_input_revision();
            app.set_new_preset_name(SharedString::default());
            // The preset list and the selected preset both changed: the
            // screen ComboBox and the entry projections refresh together.
            refresh_affix_screen_preset_models(app, state);
            // Preset file and selection are persisted together; only a
            // successful commit is a created preset on disk (the persist
            // helper logs the failure).
            if let Err(error) = state.persist_affix_presets_and_selection_to_config(config_root) {
                eprintln!("[d4lc] create preset persist failed: {error}");
            }
        }
        Err(e) => {
            eprintln!("[d4lc] create preset failed: {e}");
        }
    }
}

/// Handle the `remove-preset()` callback.
///
/// Reads `selected-preset` from AppWindow and opens the shared window-modal
/// confirm prompt (`DialogKind::Confirm`) on a `DeletePreset` action:
/// upstream asks
/// `ShowMessageAsync(this, "Delete", $"Are you sure you want to delete preset \"{name}\"", MessageDialogStyle.AffirmativeAndNegative)`
/// before `RemoveAffixPreset` (`AffixViewModel.cs:1762-1772`), so the preset
/// is only removed on an affirmative answer
/// ([`handle_confirm_remove_preset`], the prompt's Yes path) and an empty
/// selection stays a no-op (the upstream `CanRemoveAffixPresetNameExecute`
/// gate, `AffixViewModel.cs:1757-1760`).
///
/// The callback keeps the wiring's `(app, state, config_root)` shape; the
/// persistence seam moved into the confirm body, which is where the delete
/// now runs.
pub fn handle_remove_preset(app: &AppWindow, state: &mut AppState, _config_root: &Path) {
    let name = app.get_selected_preset().to_string();
    if name.is_empty() {
        return;
    }
    crate::actions::dialogs::open_confirm(app, state, ConfirmAction::DeletePreset { name });
}

/// Confirm body of the delete-preset prompt (the prompt's Yes path).
///
/// Removes the preset via `AffixPresetEditorState::remove_preset()`, updates
/// the `preset-names` model and `selected-preset` (the editor's selection
/// fallback stands in for the upstream re-selection), re-projects the entry
/// rows/cards of the fallback preset, persists the preset file and the
/// selection, and logs the removal the way upstream does
/// (`AffixViewModel.cs:1768`: `_logger.LogInformation($"Deleted preset
/// \"{name}\"")`).
///
/// A failure is surfaced through the Logging screen and returned so the
/// prompt shows it in its error row: upstream's command pre-gates its
/// `CanExecute` and never fails silently, while a stale `selected-preset`
/// (a name the editor no longer holds) can reach this body because the
/// screen's gate only checks that a name is selected.
pub fn handle_confirm_remove_preset(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    name: &str,
) -> Result<(), String> {
    if let Err(error) = state.affix_preset_editor.remove_preset(name) {
        let message = format!("remove preset failed: {error}");
        state.append_log_message(message.clone(), crate::clock::hh_mm_now());
        crate::actions::logging::project_log_messages(app, &state.logging.messages);
        return Err(message);
    }
    state.bump_rules_input_revision();
    // The removed preset's entries must leave the screen and the
    // fallback preset's entries (if any) take their place; the preset
    // ComboBox and the selection refresh with the same call.
    refresh_affix_screen_preset_models(app, state);
    // Removal and fallback selection persist together; a failed write is
    // surfaced through the prompt's error row instead of reporting a
    // deletion that never reached the disk.
    if let Err(error) = state.persist_affix_presets_and_selection_to_config(config_root) {
        let message = format!("remove preset persist failed: {error}");
        state.append_log_message(message.clone(), crate::clock::hh_mm_now());
        crate::actions::logging::project_log_messages(app, &state.logging.messages);
        return Err(message);
    }
    state.append_log_message(
        format!("Deleted preset \"{name}\""),
        crate::clock::hh_mm_now(),
    );
    crate::actions::logging::project_log_messages(app, &state.logging.messages);
    Ok(())
}

/// Handle the `affix-preset-selected()` callback (the AffixScreen
/// preset ComboBox switch).
///
/// Upstream parity: switching the preset ComboBox re-binds the
/// wanted-entry lists (`AffixView.xaml` preset ComboBox binding). The
/// handler selects the preset in the editor and re-projects the entry
/// rows/cards — the same seam the hotkey preset switch uses in
/// `hotkeys::sync_selected_preset_projection`. An unknown name (the
/// ComboBox two-way binding moves the visible text before this
/// handler runs) snaps the selection back to the editor's current
/// selection.
/// Persistence mirrors the upstream selection setter
/// (`AffixViewModel.cs:372-373`): the new selection is stored in
/// `Config/Settings.json`. A switch changes no preset
/// data, so the preset file is untouched.
pub fn handle_affix_preset_selected(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    name: &str,
) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    if !state
        .affix_preset_editor
        .presets
        .iter()
        .any(|preset| preset.name == name)
    {
        eprintln!("[d4lc] affix preset switch: unknown preset '{name}' ignored");
        app.set_selected_preset(SharedString::from(
            state
                .affix_preset_editor
                .selected_preset_name
                .as_deref()
                .unwrap_or(""),
        ));
        return;
    }
    state.affix_preset_editor.selected_preset_name = Some(name.to_string());
    state.bump_rules_input_revision();
    // The selected preset's entries changed: re-project the rows/cards
    // through the shared preset-model refresh.
    refresh_affix_screen_preset_models(app, state);
    // Upstream parity: the selection setter persists the new preset
    // name into the settings (AffixViewModel.cs:372-373).
    state.persist_selected_affix_preset_to_config(config_root);
}

/// Handle the `rename-preset()` callback.
///
/// The pencil (✎) icon next to the
/// preset ComboBox on the AffixScreen now opens the
/// `RenamePresetDialog` instead of stub-eprintlining. The
/// wiring in `wire_affix_actions` dispatches the
/// `rename-preset` root callback to this handler, which
/// delegates to `actions::affix_cmds::handle_open_rename_preset`
/// (kept here as a thin pass-through so the existing
/// `wire_affix_actions` call site does not need to change).
pub fn handle_rename_preset(app: &AppWindow, state: &mut AppState) {
    crate::actions::affix_cmds::handle_open_rename_preset(app, state);
}

/// Handle the `import-preset()` callback.
///
/// Reuses the existing offline build-import dialog path so the
/// AffixScreen Import button opens the shared ImportPreset dialog.
pub fn handle_import_preset(app: &AppWindow, state: &mut AppState) {
    crate::actions::build_import_dialogs::handle_open_import_preset(app, state);
}

// ── Entry projection (AppState -> AffixScreen entry models) ────────────────

/// A kind-tagged entry key parsed from an AffixScreen callback string.
///
/// The screen addresses entries with `"<kind>:<id>:<item-type>"`
/// keys (`kind`: affix|aspect|sigil|unique|rune; the item-type segment
/// stays empty when the entry has none or the kind does not project
/// one). Keys — not per-kind view indices — are the callback contract:
/// the screen models are per-kind, but the preset stores one
/// mixed-kind entry list, so a view index addresses the wrong entry
/// whenever kinds mix. Ids and item types must not contain `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryKey {
    pub kind: d4lc_gui::PresetEntryKind,
    pub id: String,
    pub item_type: Option<String>,
}

/// Parse a kind-tagged entry key. Returns `None` for a malformed key
/// or unknown kind prefix (callers log and ignore — keys are built by
/// the screen itself).
pub fn parse_entry_key(key: &str) -> Option<EntryKey> {
    let mut parts = key.splitn(3, ':');
    let kind = match parts.next()? {
        "affix" => d4lc_gui::PresetEntryKind::Affix,
        "aspect" => d4lc_gui::PresetEntryKind::Aspect,
        "sigil" => d4lc_gui::PresetEntryKind::Sigil,
        "unique" => d4lc_gui::PresetEntryKind::Unique,
        "rune" => d4lc_gui::PresetEntryKind::Rune,
        _ => return None,
    };
    let id = parts.next()?;
    if id.is_empty() {
        return None;
    }
    let item_type = parts.next()?;
    Some(EntryKey {
        kind,
        id: id.to_string(),
        item_type: if item_type.is_empty() {
            None
        } else {
            Some(item_type.to_string())
        },
    })
}

/// Find the index of the entry matching `key` in the selected
/// preset's mixed-kind entry list. Affix entries match on kind + id +
/// item type (the screen projects both fields); the non-affix kinds
/// project no item type, so they match on kind + id, first match.
pub fn find_entry_index(entries: &[d4lc_gui::PresetEntry], key: &EntryKey) -> Option<usize> {
    entries.iter().position(|entry| {
        entry.kind == key.kind
            && entry.id == key.id
            && match key.kind {
                d4lc_gui::PresetEntryKind::Affix => entry.item_type == key.item_type,
                _ => true,
            }
    })
}

/// Resolve a screen callback key against the selected preset's
/// entries, logging and returning `None` on a malformed key or an
/// entry that no longer exists (e.g. removed by a concurrent path).
pub fn resolve_entry_key(state: &AppState, key: &str) -> Option<(usize, d4lc_gui::PresetEntry)> {
    let parsed = match parse_entry_key(key) {
        Some(parsed) => parsed,
        None => {
            eprintln!("[d4lc] malformed affix entry key ignored: {key:?}");
            return None;
        }
    };
    let entries = state
        .affix_preset_editor
        .selected_preset_entries()
        .unwrap_or(&[]);
    let index = match find_entry_index(entries, &parsed) {
        Some(index) => index,
        None => {
            eprintln!("[d4lc] affix entry key not found in selected preset: {key:?}");
            return None;
        }
    };
    entries.get(index).map(|entry| (index, entry.clone()))
}

/// Parse an upstream `#RRGGBB` color string into a Slint color.
///
/// Accepts an optional leading `#` followed by exactly six hex
/// digits. The entry draft validation only bounds the stored
/// string's length, not its shape, so anything unparsable falls
/// back to white — a swatch that stays visible on the dark theme
/// instead of silently disappearing.
pub fn parse_hex_color(value: &str) -> slint::Color {
    let trimmed = value.trim();
    let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        if let Ok(rgb) = u32::from_str_radix(digits, 16) {
            return slint::Color::from_rgb_u8(
                ((rgb >> 16) & 0xFF) as u8,
                ((rgb >> 8) & 0xFF) as u8,
                (rgb & 0xFF) as u8,
            );
        }
    }
    slint::Color::from_rgb_u8(0xFF, 0xFF, 0xFF)
}

// ── Catalog projection (game-data rows + wanted state) ─────────────────────

/// Resolve the asset-repository root carrying the localized game data
/// (`assets/original/data`): CWD-first (installed bundle layout — packaging
/// copies the whole `assets/` tree into the bundle and starts the binary
/// with the bundle root as CWD), then the compile-time dev-checkout root.
/// `None` when neither location carries the data; the projections then
/// render the wanted entries only instead of failing.
///
/// The same seam `actions::build_import_dialogs` and `actions::settings`
/// use for their vendored asset lookups, and the seam the live runtime
/// resolves its game-data root through (`runtime_prepare`).
pub(crate) fn catalog_repo_root() -> Option<std::path::PathBuf> {
    let mut root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // d4lc-app
    root.pop(); // crates
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    candidates.push(root);
    candidates
        .into_iter()
        .find(|root| root.join(d4lc_assets::DATA_DIR).is_dir())
}

/// Ensure the cached catalog matches the selected data language.
///
/// The catalog is loaded once per language and reused by every later
/// projection, so an entry mutation or a toggle change re-projects from
/// memory instead of re-reading the index files. A load failure is reported
/// through the logging surface (once per language) and leaves an empty
/// catalog: the screen then lists the preset's own entries.
fn ensure_affix_catalog(state: &mut AppState) {
    let language = state.shell.settings.language.clone();
    if state.affix_catalog_language.as_deref() == Some(language.as_str()) {
        return;
    }
    state.affix_catalog = None;
    state.affix_catalog_language = Some(language.clone());
    let Some(root) = catalog_repo_root() else {
        state.append_log_message(
            format!("affix catalog: localized data for '{language}' not found"),
            crate::clock::hh_mm_now(),
        );
        return;
    };
    let repo = d4lc_assets::AssetRepository::new(root);
    match d4lc_rules::AffixCatalog::load(&repo, &language) {
        Ok(catalog) => state.affix_catalog = Some(catalog),
        Err(error) => {
            state.append_log_message(format!("affix catalog: {error}"), crate::clock::hh_mm_now())
        }
    }
}

/// Resolve a preset entry id against one kind's localized index rows.
///
/// Upstream resolves every wanted row's text through the same index the
/// catalog browses (`AffixIdToDescriptionConverter` →
/// `AffixManager.GetAffixDescription` → `AffixInfo.Description`), so an id
/// the index knows must never fall back to the preset file's own text. The
/// index joins the ids a single data row covers into its `IdName`
/// (`AttackSpeed;AttackSpeed_Greater;…`, 698 of the 894 enUS affix rows),
/// while the shipped preset files address one member at a time
/// (`Config/AffixPresets-v2.json` stores `Life`, `AttackSpeed`,
/// `X2_DamageType_Fire`), so the lookup accepts the row that *lists* the id
/// as well as the row whose id *is* the id.
fn resolve_catalog_index(entries: &[d4lc_rules::CatalogEntry], id: &str) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.id == id || entry.id.split(';').any(|member| member.trim() == id))
}

/// One projected catalog row for a preset entry the localized index does not
/// carry.
///
/// A preset can reference ids from another data version or another language
/// (an import, or a language switch); those entries keep their rows and
/// cards — with the preset's own label and no game-data markers — so no
/// wanted entry ever disappears from the screen. Upstream resolves the row
/// text through the index and keeps the entry in the preset either way
/// (`AffixManager.GetAffixDescription` returns an empty string for an
/// unknown id).
fn orphan_row(entry: &d4lc_gui::PresetEntry) -> d4lc_gui::affix::catalog::CatalogRow {
    d4lc_gui::affix::catalog::CatalogRow {
        entry: d4lc_rules::CatalogEntry::without_data(
            &entry.id,
            entry.label.as_deref().unwrap_or(&entry.id),
        ),
        wanted: Some(entry.clone()),
    }
}

/// The projected rows of one catalog kind: the localized index rows with the
/// selected preset's wanted state merged in, preceded by the preset's own
/// rows for ids the index does not carry.
///
/// The index rows are the browsable catalog (filtered, sorted and windowed
/// like upstream's list views); the preset's own rows are never windowed, so
/// a wanted entry stays on the rail and in the grid whether the data knows
/// its id or not.
fn catalog_rows(
    kind: d4lc_rules::CatalogKind,
    catalog: &d4lc_rules::AffixCatalog,
    preset_entries: &[d4lc_gui::PresetEntry],
    filter: &d4lc_gui::affix::catalog::CatalogFilter,
    window: usize,
) -> Vec<d4lc_gui::affix::catalog::CatalogRow> {
    let entries = catalog.entries(kind);
    let kind_entries: Vec<&d4lc_gui::PresetEntry> = preset_entries
        .iter()
        .filter(|entry| catalog_kind(entry.kind) == kind)
        .collect();
    let resolved: Vec<Option<usize>> = kind_entries
        .iter()
        .map(|entry| resolve_catalog_index(entries, &entry.id))
        .collect();
    // The index row each wanted entry resolves to; the first entry wins when
    // several preset entries land on one row (one affix wanted for several
    // slots carries one card, upstream's per-id index lookup).
    let mut wanted_by_row: HashMap<usize, &d4lc_gui::PresetEntry> = HashMap::new();
    for (entry, position) in kind_entries.iter().zip(&resolved) {
        if let Some(position) = *position {
            wanted_by_row.entry(position).or_insert(entry);
        }
    }
    let index_by_id: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.as_str(), index))
        .collect();

    let mut rows: Vec<d4lc_gui::affix::catalog::CatalogRow> = kind_entries
        .iter()
        .zip(&resolved)
        .filter(|(_, position)| position.is_none())
        .map(|(entry, _)| orphan_row(entry))
        .collect();
    // The wanted rows the render window does not carry lead the grid, before
    // the windowed rows: the window is the port's stand-in for upstream's
    // virtualizing panel, and a wanted entry has to stay reachable (and in
    // the accessibility tree) without scrolling to the end of a bounded
    // window. A row the window carries keeps its sorted position — the
    // beyond-window rows leave the lead block as the window grows, so no row
    // ever appears twice.
    let mut windowed: Vec<d4lc_gui::affix::catalog::CatalogRow> = Vec::new();
    let mut beyond_window: Vec<d4lc_gui::affix::catalog::CatalogRow> = Vec::new();
    for (index, mut row) in d4lc_gui::affix::catalog::project(kind, catalog, preset_entries, filter)
        .into_iter()
        .enumerate()
    {
        if row.wanted.is_none() {
            row.wanted = index_by_id
                .get(row.entry.id.as_str())
                .and_then(|position| wanted_by_row.get(position))
                .map(|entry| (*entry).clone());
        }
        if index < window {
            windowed.push(row);
        } else if row.wanted.is_some() {
            beyond_window.push(row);
        }
    }
    rows.append(&mut beyond_window);
    rows.append(&mut windowed);
    rows
}

/// The catalog kind a preset-entry kind browses under.
fn catalog_kind(kind: d4lc_gui::PresetEntryKind) -> d4lc_rules::CatalogKind {
    match kind {
        d4lc_gui::PresetEntryKind::Affix => d4lc_rules::CatalogKind::Affix,
        d4lc_gui::PresetEntryKind::Aspect => d4lc_rules::CatalogKind::Aspect,
        d4lc_gui::PresetEntryKind::Sigil => d4lc_rules::CatalogKind::Sigil,
        d4lc_gui::PresetEntryKind::Unique => d4lc_rules::CatalogKind::Unique,
        d4lc_gui::PresetEntryKind::Rune => d4lc_rules::CatalogKind::Rune,
    }
}

/// Map one projected affix catalog row onto the screen's `AffixEntry`.
fn affix_catalog_view(row: &d4lc_gui::affix::catalog::CatalogRow) -> AffixEntry {
    let classes = row.entry.classes;
    AffixEntry {
        id: row.entry.id.as_str().into(),
        // The callback key follows the preset's own entry id while the
        // row is wanted (see `key-id` on the screen structs).
        key_id: row
            .wanted
            .as_ref()
            .map(|entry| entry.id.as_str())
            .unwrap_or(row.entry.id.as_str())
            .into(),
        label: row.entry.label.as_str().into(),
        wanted_label: row.wanted_display_name().unwrap_or_default().into(),
        item_type: row.wanted_item_type().into(),
        color: parse_hex_color(row.wanted_color().unwrap_or_default()),
        is_wanted: row.wanted.is_some(),
        is_tempered: row.entry.is_tempered,
        wanted_is_tempered: row.wanted.as_ref().is_some_and(|entry| entry.is_tempered),
        // The wanted row's upstream fg/bg colors are preset-entry properties
        // (`AffixTypeToFgSolidBrushConverter` / `AffixTypeToBgSolidBrushConverter`
        // read `ItemAffix.IsGreater` / `IsImplicit` / `IsTempered`).
        wanted_is_greater: row.wanted.as_ref().is_some_and(|entry| entry.is_greater),
        wanted_is_implicit: row.wanted.as_ref().is_some_and(|entry| entry.is_implicit),
        class_sorc: classes.sorcerer,
        class_druid: classes.druid,
        class_barb: classes.barbarian,
        class_rogue: classes.rogue,
        class_necro: classes.necromancer,
        class_spirit: classes.spiritborn,
        class_pala: classes.paladin,
        class_warlk: classes.warlock,
    }
}

/// Map one projected aspect catalog row onto the screen's `AspectEntry`.
fn aspect_catalog_view(row: &d4lc_gui::affix::catalog::CatalogRow) -> AspectEntry {
    let classes = row.entry.classes;
    AspectEntry {
        id: row.entry.id.as_str().into(),
        // The callback key follows the preset's own entry id while the
        // row is wanted (see `key-id` on the screen structs).
        key_id: row
            .wanted
            .as_ref()
            .map(|entry| entry.id.as_str())
            .unwrap_or(row.entry.id.as_str())
            .into(),
        label: row.entry.label.as_str().into(),
        wanted_label: row.wanted_display_name().unwrap_or_default().into(),
        description: row.entry.description.as_str().into(),
        color: parse_hex_color(row.wanted_color().unwrap_or_default()),
        is_wanted: row.wanted.is_some(),
        is_codex: row.entry.is_codex,
        is_seasonal: row.entry.is_seasonal,
        is_drop_only: row.entry.is_drop_only,
        dungeon: row.entry.dungeon.as_str().into(),
        class_sorc: classes.sorcerer,
        class_druid: classes.druid,
        class_barb: classes.barbarian,
        class_rogue: classes.rogue,
        class_necro: classes.necromancer,
        class_spirit: classes.spiritborn,
        class_pala: classes.paladin,
        class_warlk: classes.warlock,
    }
}

/// Map one projected sigil catalog row onto the screen's `SigilEntry`.
fn sigil_catalog_view(state: &AppState, row: &d4lc_gui::affix::catalog::CatalogRow) -> SigilEntry {
    SigilEntry {
        id: row.entry.id.as_str().into(),
        key_id: row
            .wanted
            .as_ref()
            .map(|entry| entry.id.as_str())
            .unwrap_or(row.entry.id.as_str())
            .into(),
        label: row.entry.label.as_str().into(),
        wanted_label: row.wanted_display_name().unwrap_or_default().into(),
        description: row.entry.description.as_str().into(),
        color: parse_hex_color(row.wanted_color().unwrap_or_default()),
        is_wanted: row.wanted.is_some(),
        tier_label: sigil_tier_label(state, &row.entry),
        is_seasonal: row.entry.is_seasonal,
    }
}

/// Map one projected unique catalog row onto the screen's `UniqueEntry`.
fn unique_catalog_view(row: &d4lc_gui::affix::catalog::CatalogRow) -> UniqueEntry {
    let classes = row.entry.classes;
    UniqueEntry {
        id: row.entry.id.as_str().into(),
        // The callback key follows the preset's own entry id while the
        // row is wanted (see `key-id` on the screen structs).
        key_id: row
            .wanted
            .as_ref()
            .map(|entry| entry.id.as_str())
            .unwrap_or(row.entry.id.as_str())
            .into(),
        label: row.entry.label.as_str().into(),
        wanted_label: row.wanted_display_name().unwrap_or_default().into(),
        description: row.entry.description.as_str().into(),
        color: parse_hex_color(row.wanted_color().unwrap_or_default()),
        is_wanted: row.wanted.is_some(),
        class_sorc: classes.sorcerer,
        class_druid: classes.druid,
        class_barb: classes.barbarian,
        class_rogue: classes.rogue,
        class_necro: classes.necromancer,
        class_spirit: classes.spiritborn,
        class_pala: classes.paladin,
        class_warlk: classes.warlock,
    }
}

/// Map one projected rune catalog row onto the screen's `RuneEntry`.
fn rune_catalog_view(row: &d4lc_gui::affix::catalog::CatalogRow) -> RuneEntry {
    RuneEntry {
        id: row.entry.id.as_str().into(),
        // The callback key follows the preset's own entry id while the
        // row is wanted (see `key-id` on the screen structs).
        key_id: row
            .wanted
            .as_ref()
            .map(|entry| entry.id.as_str())
            .unwrap_or(row.entry.id.as_str())
            .into(),
        label: row.entry.label.as_str().into(),
        wanted_label: row.wanted_display_name().unwrap_or_default().into(),
        description: row.entry.description.as_str().into(),
        color: parse_hex_color(row.wanted_color().unwrap_or_default()),
        is_wanted: row.wanted.is_some(),
        is_condition: row.entry.is_condition,
        is_effect: row.entry.is_effect,
    }
}

/// The sigil tier badge text: the configured dungeon tier for the sigil id
/// while the tier display is enabled, and only for dungeon sigils
/// (upstream `SigilInfoWanted.IsTierInfoEnabled`, SigilInfoVM.cs:108-112,
/// and `AffixManager.GetSigilDungeonTier`, AffixManager.cs:744-747).
///
/// The value is the same projection the Sigil config dialog's tier rows
/// use (`d4lc_gui::affix::sigil_tier_rows`): the stored
/// `Config/DungeonTiers.json` tier verbatim, or the upstream `F` default
/// for a dungeon sigil without one — upstream's tier button shows
/// `Tier` = `GetSigilDungeonTier(id)`, which is never empty.
fn sigil_tier_label(state: &AppState, entry: &d4lc_rules::CatalogEntry) -> SharedString {
    if !state.shell.settings.dungeon_tiers_enabled || entry.sigil_type.as_deref() != Some("Dungeon")
    {
        return SharedString::default();
    }
    SharedString::from(
        state
            .dungeon_tiers
            .get(&entry.id)
            .map(String::as_str)
            .unwrap_or(d4lc_gui::affix::SIGIL_DUNGEON_TIER_DEFAULT),
    )
}

/// The ten upstream gear-slot sections of the wanted-affix rail in upstream
/// order (`ItemTypeConstants`, `AffixView.xaml:160-865`): the preset entry's
/// slot token and its localized caption.
const AFFIX_RAIL_SLOTS: [(&str, &str); 10] = [
    ("helm", "Helm"),
    ("chest", "Chest"),
    ("gloves", "Gloves"),
    ("pants", "Pants"),
    ("boots", "Boots"),
    ("amulet", "Amulet"),
    ("ring", "Ring"),
    ("weapon", "Weapon"),
    ("ranged", "Ranged"),
    ("offhand", "Offhand"),
];

/// A section-header row of the wanted-entry rail.
fn rail_header_row(tab: i32, token: &str, caption: &str) -> AffixRailRow {
    AffixRailRow {
        kind: SharedString::default(),
        label: caption.into(),
        token: token.into(),
        key: SharedString::default(),
        color: slint::Color::from_rgb_u8(0xFF, 0xFF, 0xFF),
        is_tempered_or_implicit: false,
        is_greater: false,
        tab,
        is_header: true,
    }
}

/// One wanted chip of the left rail: the selected preset's entry plus the
/// text the chip shows for it.
///
/// The chip is addressed by the preset entry's own kind-tagged key
/// (`affix:<id>:<item-type>`, the callback contract), so the entry callbacks
/// keep resolving the preset's entry even when the display text comes from
/// the localized index.
struct RailChip {
    kind: d4lc_gui::PresetEntryKind,
    id: SharedString,
    label: SharedString,
    color: slint::Color,
    item_type: SharedString,
    is_tempered_or_implicit: bool,
    is_greater: bool,
}

/// The rail's chips: one per selected-preset entry, in preset order.
///
/// The chip text is the localized index row's text when the data carries the
/// id and the preset's own label otherwise. Upstream renders the wanted row
/// through `AffixIdToDescriptionConverter` (`AffixView.xaml:201`) and never
/// through the preset file, whose `Label` field the port alone carries.
fn rail_chips(
    catalog: &d4lc_rules::AffixCatalog,
    preset_entries: &[d4lc_gui::PresetEntry],
) -> Vec<RailChip> {
    preset_entries
        .iter()
        .map(|entry| {
            let kind = catalog_kind(entry.kind);
            let label = resolve_catalog_index(catalog.entries(kind), &entry.id)
                .map(|position| SharedString::from(catalog.entries(kind)[position].label.as_str()))
                .unwrap_or_else(|| {
                    entry
                        .label
                        .as_deref()
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from(entry.id.as_str()))
                });
            RailChip {
                kind: entry.kind,
                id: entry.id.as_str().into(),
                label,
                color: parse_hex_color(&entry.color),
                item_type: entry.item_type.as_deref().unwrap_or_default().into(),
                is_tempered_or_implicit: entry.is_tempered || entry.is_implicit,
                is_greater: entry.is_greater,
            }
        })
        .collect()
}

/// One wanted-affix chip row: the upstream chip shows the localized
/// description (`AffixIdToDescriptionConverter`), the preset color swatch and
/// the type-based colors, and addresses the preset entry through its
/// kind-tagged key.
fn rail_affix_row(chip: &RailChip) -> AffixRailRow {
    AffixRailRow {
        kind: "affix".into(),
        label: chip.label.clone(),
        token: SharedString::default(),
        key: format!("affix:{}:{}", chip.id, chip.item_type).into(),
        color: chip.color,
        is_tempered_or_implicit: chip.is_tempered_or_implicit,
        is_greater: chip.is_greater,
        tab: 0,
        is_header: false,
    }
}

/// One wanted family chip row (aspect / sigil / unique / rune).
fn rail_family_row(kind: &str, tab: i32, chip: &RailChip) -> AffixRailRow {
    AffixRailRow {
        kind: kind.into(),
        label: chip.label.clone(),
        token: SharedString::default(),
        key: format!("{kind}:{}:", chip.id).into(),
        color: chip.color,
        is_tempered_or_implicit: false,
        is_greater: false,
        tab,
        is_header: false,
    }
}

/// Project the wanted-entry rail rows (upstream's slot-grouped left column,
/// `AffixView.xaml:150-1118`).
///
/// One flat row list instead of nested per-section repetitions: the rail is a
/// single `for` over this model, which keeps the grouping data in Rust and
/// the Slint element tree to the rows that actually render. Only wanted
/// entries produce chip rows; an affix entry whose slot token matches none of
/// the ten upstream sections has no section to appear under (upstream filters
/// `SelectedAffixesFiltered*` by `ItemAffix.Type` the same way). The row set
/// depends only on the preset, never on the catalog render window.
fn rail_rows(chips: &[RailChip]) -> Vec<AffixRailRow> {
    let mut rows = Vec::new();
    for (token, caption) in AFFIX_RAIL_SLOTS {
        rows.push(rail_header_row(0, token, caption));
        rows.extend(
            chips
                .iter()
                .filter(|chip| {
                    chip.kind == d4lc_gui::PresetEntryKind::Affix
                        && chip.item_type.eq_ignore_ascii_case(token)
                })
                .map(rail_affix_row),
        );
    }
    for (tab, token, caption, kind) in [
        (1, "aspect", "Aspects", d4lc_gui::PresetEntryKind::Aspect),
        (2, "sigil", "Sigils", d4lc_gui::PresetEntryKind::Sigil),
        (3, "unique", "Uniques", d4lc_gui::PresetEntryKind::Unique),
        (4, "rune", "Runes", d4lc_gui::PresetEntryKind::Rune),
    ] {
        rows.push(rail_header_row(tab, token, caption));
        rows.extend(
            chips
                .iter()
                .filter(|chip| chip.kind == kind)
                .map(|chip| rail_family_row(token, tab, chip)),
        );
    }
    rows
}

/// Project the localized catalog plus the selected preset's wanted state
/// onto the AffixScreen's five entry models.
///
/// Upstream parity: the right-column grids iterate the loaded indexes for
/// the tab's kind (`AffixView.xaml:1499-1996`), filtered by the keyword
/// text (`;`-separated terms AND-combine, `AffixViewModel.cs:1103-1112`)
/// and the class, category and rune toggles, and ordered by the kind's
/// `CustomSort` (`AffixViewModel.cs:1082-1387`). The left rail renders the
/// wanted rows out of the same models, so one projection feeds both.
///
/// The screen-local entry properties are unreachable from Rust, so the
/// models live on the root and this function is the single population path.
/// Every preset-entry mutation success path calls it (see the module design
/// notes); the keyword filter re-runs it per keystroke
/// (`handle_affix_filter_text_changed`) and a toggle change re-runs it too
/// (`handle_affix_filters_changed`), matching upstream's
/// `RefreshAffixViewFilter` (`AffixViewModel.cs:434-456`).
pub fn project_affix_entries(app: &AppWindow, state: &mut AppState) {
    project_affix_screen(app, state, true);
}

/// Re-project the catalog models after the render window grew.
///
/// The wanted-entry rail is a function of the preset alone (its rows carry no
/// catalog row state), so a window growth rebuilds the five windowed models
/// and leaves the rail untouched. Growth is the per-scroll event: the screen
/// asks for a larger window whenever the user reaches the grid's bottom edge,
/// and rebuilding the rail on every one of those events is the port's
/// equivalent of upstream's `VirtualizingWrapPanel` re-materializing — the
/// card window is bounded, the rest of the screen stays still.
fn project_affix_window(app: &AppWindow, state: &mut AppState) {
    project_affix_screen(app, state, false);
}

/// The screen's single projection path: the five per-tab catalog models, and
/// (when `refresh_rail`) the wanted-entry rail rows.
fn project_affix_screen(app: &AppWindow, state: &mut AppState, refresh_rail: bool) {
    #[cfg(test)]
    {
        AFFIX_PROJECTION_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        THREAD_PROJECTION_COUNT.set(THREAD_PROJECTION_COUNT.get() + 1);
    }
    ensure_affix_catalog(state);
    // The render window lives on the root (the screen's bottom-edge trigger
    // drives it); push the state value so a screen that was re-instantiated
    // with the tab shows the current window.
    app.set_affix_catalog_window(state.affix_catalog_window as i32);
    let filter = d4lc_gui::affix::catalog::CatalogFilter::from_toggles(
        &state.affix_catalog_filter_text,
        &state.shell.settings.toggles,
    );
    let catalog = state
        .affix_catalog
        .clone()
        .unwrap_or_else(|| d4lc_rules::AffixCatalog::empty(&state.shell.settings.language));
    let preset_entries: Vec<d4lc_gui::PresetEntry> = state
        .affix_preset_editor
        .selected_preset_entries()
        .unwrap_or(&[])
        .to_vec();
    let window = state.affix_catalog_window;

    let affix: Vec<AffixEntry> = catalog_rows(
        d4lc_rules::CatalogKind::Affix,
        &catalog,
        &preset_entries,
        &filter,
        window,
    )
    .iter()
    .map(affix_catalog_view)
    .collect();
    let aspect: Vec<AspectEntry> = catalog_rows(
        d4lc_rules::CatalogKind::Aspect,
        &catalog,
        &preset_entries,
        &filter,
        window,
    )
    .iter()
    .map(aspect_catalog_view)
    .collect();
    let sigil: Vec<SigilEntry> = catalog_rows(
        d4lc_rules::CatalogKind::Sigil,
        &catalog,
        &preset_entries,
        &filter,
        window,
    )
    .iter()
    .map(|row| sigil_catalog_view(state, row))
    .collect();
    let unique: Vec<UniqueEntry> = catalog_rows(
        d4lc_rules::CatalogKind::Unique,
        &catalog,
        &preset_entries,
        &filter,
        window,
    )
    .iter()
    .map(unique_catalog_view)
    .collect();
    let rune: Vec<RuneEntry> = catalog_rows(
        d4lc_rules::CatalogKind::Rune,
        &catalog,
        &preset_entries,
        &filter,
        window,
    )
    .iter()
    .map(rune_catalog_view)
    .collect();

    app.set_affix_entries(Rc::new(VecModel::from(affix)).into());
    app.set_aspect_entries(Rc::new(VecModel::from(aspect)).into());
    app.set_sigil_entries(Rc::new(VecModel::from(sigil)).into());
    app.set_unique_entries(Rc::new(VecModel::from(unique)).into());
    app.set_rune_entries(Rc::new(VecModel::from(rune)).into());

    if !refresh_rail {
        return;
    }

    // The wanted-entry rail: one flat row list grouped by the upstream slot
    // and family sections (the left column's StackPanel of section headers
    // with their wanted chips, `AffixView.xaml:150-1118`), one chip per
    // selected-preset entry.
    #[cfg(test)]
    {
        AFFIX_RAIL_BUILD_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        THREAD_RAIL_BUILD_COUNT.set(THREAD_RAIL_BUILD_COUNT.get() + 1);
    }
    let chips = rail_chips(&catalog, &preset_entries);
    app.set_affix_rail_rows(Rc::new(VecModel::from(rail_rows(&chips))).into());
}

// ── Affix filter toggle helpers ─────────────────────────────────────────────

/// Convert `RuleToggleSettings` to a 9-element class toggle array.
///
/// Order: core, barbarian, druid, necromancer, paladin, rogue, sorcerer, spiritborn, warlock
pub fn settings_to_class_array(toggles: &d4lc_config::RuleToggleSettings) -> Vec<bool> {
    vec![
        toggles.class_toggles.core,
        toggles.class_toggles.barbarian,
        toggles.class_toggles.druid,
        toggles.class_toggles.necromancer,
        toggles.class_toggles.paladin,
        toggles.class_toggles.rogue,
        toggles.class_toggles.sorcerer,
        toggles.class_toggles.spiritborn,
        toggles.class_toggles.warlock,
    ]
}

/// Convert `RuleToggleSettings` to a 4-element category toggle array.
///
/// Order: dungeons, positive, minor, major
pub fn settings_to_category_array(toggles: &d4lc_config::RuleToggleSettings) -> Vec<bool> {
    vec![
        toggles.dungeons,
        toggles.positive,
        toggles.minor,
        toggles.major,
    ]
}

/// Convert `RuleToggleSettings` to a 2-element rune toggle array.
///
/// Order: rune_condition, rune_effect
pub fn settings_to_rune_array(toggles: &d4lc_config::RuleToggleSettings) -> Vec<bool> {
    vec![toggles.rune_condition, toggles.rune_effect]
}

/// Convert three toggle arrays back to `RuleToggleSettings`.
///
/// Validates that arrays have lengths 9, 4, and 2 respectively.
pub fn arrays_to_rule_toggle_settings(
    class: &[bool],
    category: &[bool],
    rune: &[bool],
) -> Result<d4lc_config::RuleToggleSettings, String> {
    if class.len() != 9 {
        return Err(format!("class array length {} != 9", class.len()));
    }
    if category.len() != 4 {
        return Err(format!("category array length {} != 4", category.len()));
    }
    if rune.len() != 2 {
        return Err(format!("rune array length {} != 2", rune.len()));
    }
    Ok(d4lc_config::RuleToggleSettings {
        class_toggles: d4lc_config::ClassToggleSettings {
            core: class[0],
            barbarian: class[1],
            druid: class[2],
            necromancer: class[3],
            paladin: class[4],
            rogue: class[5],
            sorcerer: class[6],
            spiritborn: class[7],
            warlock: class[8],
        },
        dungeons: category[0],
        positive: category[1],
        minor: category[2],
        major: category[3],
        rune_condition: rune[0],
        rune_effect: rune[1],
    })
}

/// Show `toggles` on the affix screen's class, category and rune filter
/// icons.
pub fn set_filter_toggle_models(app: &AppWindow, toggles: &d4lc_config::RuleToggleSettings) {
    let model = |values: Vec<bool>| std::rc::Rc::new(slint::VecModel::from(values)).into();
    app.set_class_toggles(model(settings_to_class_array(toggles)));
    app.set_category_toggles(model(settings_to_category_array(toggles)));
    app.set_rune_toggles(model(settings_to_rune_array(toggles)));
}

/// Handle the aggregate `affix-filters-changed()` callback.
///
/// Reads the three toggle arrays from callback parameters, validates lengths,
/// maps to `RuleToggleSettings`, updates `AppState`, persists through config,
/// pushes the arrays back to AppWindow for tab-switch consistency, and
/// re-projects the catalog grids so the new toggle arms filter immediately
/// (upstream refreshes the active catalog view from every toggle setter,
/// `RefreshAffixViewFilter`, AffixViewModel.cs:434-456).
pub fn handle_affix_filters_changed(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    class: &slint::ModelRc<bool>,
    category: &slint::ModelRc<bool>,
    rune: &slint::ModelRc<bool>,
) {
    let collect = |model: &slint::ModelRc<bool>| model.iter().collect::<Vec<bool>>();
    let toggles =
        match arrays_to_rule_toggle_settings(&collect(class), &collect(category), &collect(rune)) {
            Ok(toggles) => toggles,
            Err(error) => {
                eprintln!("[d4lc] affix-filters-changed: {error}");
                return;
            }
        };

    state.shell.settings.toggles = toggles;
    state.bump_rules_input_revision();

    // Persist through config
    let mut settings = d4lc_config::load_app_settings(config_root).unwrap_or_default();
    settings.rule_settings.toggles = toggles;
    if let Err(e) = d4lc_config::save_app_settings(config_root, &settings) {
        eprintln!("[d4lc] failed to save affix filter toggles: {e}");
    }

    // Push arrays back to AppWindow for tab-switch consistency
    app.set_class_toggles(class.clone());
    app.set_category_toggles(category.clone());
    app.set_rune_toggles(rune.clone());

    // Re-filter: the projected grids hold the rows the old predicate
    // accepted, so the new toggle state only becomes visible through a
    // fresh projection (upstream `RefreshAffixViewFilter`).
    project_affix_entries(app, state);
}

/// Handle the `affix-filter-text-changed()` callback: the live keyword
/// filter over the projected catalog grids.
///
/// Upstream's `AffixTextFilter` setter re-filters the active catalog view on
/// every edit (`AffixViewModel.cs:248-256` → `RefreshAffixViewFilter`,
/// `:434-456`), with `;`-separated terms AND-combined (`:1096-1418`). The
/// text is stored on `AppState` and the projection re-runs, so the grids
/// narrow while typing, clearing the field restores the full catalog, and
/// tab switches keep the same predicate (all five models are projected from
/// the one filter). The wanted-entry rail is not filtered, matching upstream
/// (the filter refreshes the catalog views only).
pub fn handle_affix_filter_text_changed(app: &AppWindow, state: &mut AppState, text: &str) {
    // The accessibility `SetValue` path and rapid re-typing can emit an
    // unchanged value; a projection is only needed when the predicate
    // actually changes.
    if state.affix_catalog_filter_text == text {
        return;
    }
    state.affix_catalog_filter_text.clear();
    state.affix_catalog_filter_text.push_str(text);
    project_affix_entries(app, state);
}

/// Handle the `affix-catalog-window-changed()` callback: the catalog render
/// window in rows per tab.
///
/// Upstream materializes only the visible viewport's cards
/// (`VirtualizingWrapPanel`, `AffixView.xaml:1499-1996`); the port projects
/// at most `window` catalog rows per kind so a grid instantiates a bounded
/// number of cards (a full 894-row affix grid instantiates thousands of
/// cards and makes the whole element tree unmanageable). The screen's
/// bottom-edge trigger asks for the next page while the user keeps scrolling;
/// a request that does not grow the window is a no-op, one growth re-projects
/// the windowed models only (the rail is window-independent), and a request
/// is capped at one page beyond the current window so a burst of scroll
/// events can never telescope the window in a single gesture step.
pub fn handle_affix_catalog_window_changed(app: &AppWindow, state: &mut AppState, size: i32) {
    let requested = usize::try_from(size)
        .unwrap_or(DEFAULT_CATALOG_WINDOW)
        .max(DEFAULT_CATALOG_WINDOW);
    let size = requested.min(state.affix_catalog_window + CATALOG_WINDOW_PAGE);
    if state.affix_catalog_window == size {
        return;
    }
    state.affix_catalog_window = size;
    project_affix_window(app, state);
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Refresh every AffixScreen surface that mirrors the preset list after a
/// preset mutation.
///
/// Upstream parity: `AffixViewModel` exposes `AffixPresets` and
/// `SelectedAffixPreset` as observable state, so `AddAffixPreset`,
/// `ImportAffixPreset`, `RemoveAffixPreset` and `RenamePreset`
/// (`AffixViewModel.cs:919-931`, `:1746-1772`, `:372-373`) update the
/// AffixView preset ComboBox and its bound wanted-entry lists in the same
/// session — no restart. The Slint root properties are static, so every
/// preset-list mutation funnels through this single refresh: the
/// `preset-names` ComboBox model, `selected-preset` (the editor's
/// selection; a stale name that no longer exists is dropped exactly like
/// the hotkey preset switch does), the `AppState` shell summary, and the
/// selected preset's entry rows/cards through [`project_affix_entries`].
///
/// The build-import add-as-preset / merge / delete confirms, the local-tab
/// import, the inline create/remove flows, the dialog confirm bodies and
/// the startup hydration seam all call this; none of them seeds the screen
/// models on its own.
pub fn refresh_affix_screen_preset_models(app: &AppWindow, state: &mut AppState) {
    let selected_preset = state
        .affix_preset_editor
        .selected_preset_name
        .clone()
        .filter(|selected_name| {
            state
                .affix_preset_editor
                .presets
                .iter()
                .any(|preset| preset.name == *selected_name)
        });
    state.affix_preset_editor.selected_preset_name = selected_preset.clone();
    state.bump_rules_input_revision();
    update_preset_names(app, state);
    state.shell.affix_presets.preset_count = state.affix_preset_editor.presets.len();
    state.shell.affix_presets.selected_preset = selected_preset.clone();
    app.set_selected_preset(SharedString::from(selected_preset.unwrap_or_default()));
    project_affix_entries(app, state);
}

/// Refresh the `preset-names` ComboBox model from the editor.
///
/// Kept separate from [`refresh_affix_screen_preset_models`] for the one
/// pre-hydration startup seed (`main()` runs before the config root is
/// available); every post-mutation path uses the shared refresh.
pub fn update_preset_names(app: &AppWindow, state: &AppState) {
    let names: Vec<SharedString> = state
        .affix_preset_editor
        .presets
        .iter()
        .map(|preset| SharedString::from(preset.name.as_str()))
        .collect();
    app.set_preset_names(Rc::new(VecModel::from(names)).into());
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Unique, per-test config root under the system temp dir (the same
    /// hermetic-root pattern the other app test modules use).
    fn temp_config_root(test_name: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|_| "time_error".to_string());
        let root = std::env::temp_dir().join(format!("d4lc-affix-{test_name}-{suffix}"));
        std::fs::create_dir_all(&root).expect("create temp config root");
        root
    }

    /// The Logging-screen projection rows, newest last.
    fn log_rows(app: &AppWindow) -> Vec<String> {
        use slint::Model as _;
        let model = app.get_log_messages();
        (0..model.row_count())
            .map(|row| {
                model
                    .row_data(row)
                    .map(|entry| entry.message.to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn preset(app: &AppWindow, name: &str) -> AppState {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset(name).unwrap();
        app.set_selected_preset(SharedString::from(name));
        state
    }

    /// The remove-preset click asks first, and only Yes deletes.
    ///
    /// Upstream `RemoveAffixPresetNameExecute` (`AffixViewModel.cs:1762-1772`)
    /// shows the MahApps prompt with `"Delete"` /
    /// `Are you sure you want to delete preset "{name}"` before
    /// `RemoveAffixPreset`; No leaves the preset alone.
    #[test]
    fn remove_preset_asks_through_the_confirm_prompt() {
        let app = crate::test_window::new_app_window();
        let root = temp_config_root("remove-preset-prompt");
        let mut state = preset(&app, "Beta");
        state.affix_preset_editor.create_preset("Alpha").unwrap();
        handle_remove_preset(&app, &mut state, &root);
        assert!(
            app.get_dialog_visible(),
            "the remove-preset click must ask before deleting"
        );
        assert_eq!(app.get_dialog_id().to_string(), "confirm");
        assert_eq!(app.get_dialog_title().to_string(), "Delete");
        assert_eq!(
            app.get_dialog_message().to_string(),
            "Are you sure you want to delete preset \"Beta\""
        );
        assert_eq!(
            state.dialog.confirm,
            Some(ConfirmAction::DeletePreset {
                name: "Beta".to_string()
            })
        );
        assert_eq!(
            state.affix_preset_editor.presets.len(),
            2,
            "an unanswered prompt must not delete anything"
        );

        // No cancels: the preset survives and the pending action is dropped.
        crate::actions::dialogs::handle_cancel(&app, &mut state);
        assert!(!app.get_dialog_visible());
        assert!(state.dialog.confirm.is_none());
        assert!(
            state
                .affix_preset_editor
                .presets
                .iter()
                .any(|preset| preset.name == "Beta"),
            "No must keep the preset"
        );

        // Yes deletes, re-projects the fallback selection, and logs the
        // removal the way upstream logs it (`AffixViewModel.cs:1768`).
        handle_remove_preset(&app, &mut state, &root);
        crate::actions::dialogs::handle_confirm(&app, &mut state, &root)
            .expect("the confirmed delete must succeed");
        assert!(!app.get_dialog_visible());
        assert!(!state
            .affix_preset_editor
            .presets
            .iter()
            .any(|preset| preset.name == "Beta"));
        assert_eq!(app.get_selected_preset().to_string(), "Alpha");
        assert_eq!(
            log_rows(&app).last().cloned(),
            Some("Deleted preset \"Beta\"".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A failing delete reaches the Logging screen and the prompt's error
    /// row — never stderr only (the upstream command pre-gates its
    /// `CanExecute`; a stale selection is the case that can still fail).
    #[test]
    fn remove_preset_failure_reaches_the_logging_screen() {
        let app = crate::test_window::new_app_window();
        let root = temp_config_root("remove-preset-failure");
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Alpha").unwrap();
        // A stale screen selection: the name the screen shows is gone from
        // the editor, which the upstream `CanExecute` pre-gate cannot catch.
        app.set_selected_preset(SharedString::from("Ghost"));
        assert!(!state
            .affix_preset_editor
            .presets
            .iter()
            .any(|entry| entry.name == "Ghost"));

        handle_remove_preset(&app, &mut state, &root);
        let error = crate::actions::dialogs::handle_confirm(&app, &mut state, &root)
            .expect_err("a missing preset must fail");
        assert_eq!(error, "remove preset failed: preset 'Ghost' not found");
        assert!(
            app.get_dialog_visible() && app.get_dialog_error_visible(),
            "a failed delete must keep the prompt open with the error row"
        );
        assert_eq!(
            app.get_dialog_error().to_string(),
            "remove preset failed: preset 'Ghost' not found"
        );
        assert_eq!(
            log_rows(&app).last().cloned(),
            Some("remove preset failed: preset 'Ghost' not found".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_preset_updates_editor_state() {
        let mut state = AppState::default_startup();
        assert!(state.affix_preset_editor.presets.is_empty());

        // We can't unit-test AppWindow interaction (requires Slint runtime),
        // but we verify the AffixPresetEditorState logic used by the handler:
        state.affix_preset_editor.create_preset("Speed").unwrap();
        assert_eq!(state.affix_preset_editor.presets.len(), 1);
        assert_eq!(state.affix_preset_editor.presets[0].name, "Speed");
        assert_eq!(
            state.affix_preset_editor.selected_preset_name.as_deref(),
            Some("Speed")
        );
    }

    #[test]
    fn remove_preset_updates_editor_state() {
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        state.affix_preset_editor.create_preset("Boss").unwrap();
        assert_eq!(state.affix_preset_editor.presets.len(), 2);

        state.affix_preset_editor.remove_preset("Speed").unwrap();
        assert_eq!(state.affix_preset_editor.presets.len(), 1);
        assert_eq!(state.affix_preset_editor.presets[0].name, "Boss");

        // Removing the only remaining preset
        state.affix_preset_editor.remove_preset("Boss").unwrap();
        assert!(state.affix_preset_editor.presets.is_empty());
        assert!(state.affix_preset_editor.selected_preset_name.is_none());
    }

    #[test]
    fn rename_preset_opens_dialog_via_dispatch() {
        // The `handle_rename_preset`
        // handler now delegates to
        // `affix_cmds::handle_open_rename_preset` which opens
        // the `RenamePresetDialog` and seeds the root in-out
        // props. We cannot exercise the Slint setter side here
        // (it requires a Slint runtime), so we verify the
        // underlying state mutation the confirm body performs.
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        state
            .affix_preset_editor
            .rename_preset("Speed", "Boss")
            .unwrap();
        assert_eq!(state.affix_preset_editor.presets[0].name, "Boss");
    }

    #[test]
    fn import_preset_editor_selects_imported_preset() {
        // Structural: verify AffixPresetEditorState::import_preset works.
        let mut state = AppState::default_startup();
        state.affix_preset_editor.import_preset(
            d4lc_gui::AffixPresetEditorPreset {
                name: "Imported".to_string(),
                entries: Vec::new(),
            },
            Vec::new(),
        );
        assert_eq!(state.affix_preset_editor.presets.len(), 1);
        assert_eq!(
            state.affix_preset_editor.selected_preset_name.as_deref(),
            Some("Imported")
        );
    }

    // ── Affix filter toggle tests ─────────────────────────────────────

    #[test]
    fn settings_to_class_array_produces_9_elements() {
        let toggles = d4lc_config::RuleToggleSettings {
            class_toggles: d4lc_config::ClassToggleSettings {
                core: true,
                barbarian: false,
                druid: true,
                necromancer: false,
                paladin: true,
                rogue: false,
                sorcerer: true,
                spiritborn: false,
                warlock: true,
            },
            ..d4lc_config::RuleToggleSettings::default()
        };
        let class = settings_to_class_array(&toggles);
        assert_eq!(class.len(), 9);
        assert!(class[0]); // core
        assert!(!class[1]); // barbarian
        assert!(class[2]); // druid
        assert!(!class[3]); // necromancer
        assert!(class[4]); // paladin
        assert!(!class[5]); // rogue
        assert!(class[6]); // sorcerer
        assert!(!class[7]); // spiritborn
        assert!(class[8]); // warlock
    }

    #[test]
    fn settings_to_category_array_produces_4_elements() {
        let toggles = d4lc_config::RuleToggleSettings {
            dungeons: true,
            positive: false,
            minor: true,
            major: false,
            ..d4lc_config::RuleToggleSettings::default()
        };
        let category = settings_to_category_array(&toggles);
        assert_eq!(category.len(), 4);
        assert!(category[0]); // dungeons
        assert!(!category[1]); // positive
        assert!(category[2]); // minor
        assert!(!category[3]); // major
    }

    #[test]
    fn settings_to_rune_array_produces_2_elements() {
        let toggles = d4lc_config::RuleToggleSettings {
            rune_condition: true,
            rune_effect: false,
            ..d4lc_config::RuleToggleSettings::default()
        };
        let rune = settings_to_rune_array(&toggles);
        assert_eq!(rune.len(), 2);
        assert!(rune[0]); // rune_condition
        assert!(!rune[1]); // rune_effect
    }

    #[test]
    fn toggle_arrays_roundtrip_through_rule_toggle_settings() {
        let class = vec![true, false, true, false, true, false, true, false, true];
        let category = vec![true, false, true, false];
        let rune = vec![true, false];

        let toggles = arrays_to_rule_toggle_settings(&class, &category, &rune).unwrap();
        assert!(toggles.class_toggles.core);
        assert!(!toggles.class_toggles.barbarian);
        assert!(toggles.class_toggles.warlock);
        assert!(toggles.dungeons);
        assert!(!toggles.positive);
        assert!(toggles.minor);
        assert!(!toggles.major);
        assert!(toggles.rune_condition);
        assert!(!toggles.rune_effect);

        // Roundtrip back
        let class2 = settings_to_class_array(&toggles);
        assert_eq!(class, class2);
        let category2 = settings_to_category_array(&toggles);
        assert_eq!(category, category2);
        let rune2 = settings_to_rune_array(&toggles);
        assert_eq!(rune, rune2);
    }

    #[test]
    fn invalid_class_array_length_is_rejected() {
        let class = vec![true; 8]; // wrong length
        let category = vec![true; 4];
        let rune = vec![true; 2];
        assert!(arrays_to_rule_toggle_settings(&class, &category, &rune).is_err());
    }

    #[test]
    fn invalid_category_array_length_is_rejected() {
        let class = vec![true; 9];
        let category = vec![true; 5]; // wrong length
        let rune = vec![true; 2];
        assert!(arrays_to_rule_toggle_settings(&class, &category, &rune).is_err());
    }

    #[test]
    fn invalid_rune_array_length_is_rejected() {
        let class = vec![true; 9];
        let category = vec![true; 4];
        let rune = vec![true; 3]; // wrong length
        assert!(arrays_to_rule_toggle_settings(&class, &category, &rune).is_err());
    }

    #[test]
    fn arrays_to_settings_maps_all_15_fields_correctly() {
        // All true
        let class = vec![true; 9];
        let category = vec![true; 4];
        let rune = vec![true; 2];
        let toggles = arrays_to_rule_toggle_settings(&class, &category, &rune).unwrap();
        assert!(toggles.class_toggles.core);
        assert!(toggles.class_toggles.barbarian);
        assert!(toggles.class_toggles.druid);
        assert!(toggles.class_toggles.necromancer);
        assert!(toggles.class_toggles.paladin);
        assert!(toggles.class_toggles.rogue);
        assert!(toggles.class_toggles.sorcerer);
        assert!(toggles.class_toggles.spiritborn);
        assert!(toggles.class_toggles.warlock);
        assert!(toggles.dungeons);
        assert!(toggles.positive);
        assert!(toggles.minor);
        assert!(toggles.major);
        assert!(toggles.rune_condition);
        assert!(toggles.rune_effect);

        // All false
        let class = vec![false; 9];
        let category = vec![false; 4];
        let rune = vec![false; 2];
        let toggles = arrays_to_rule_toggle_settings(&class, &category, &rune).unwrap();
        assert!(!toggles.class_toggles.core);
        assert!(!toggles.class_toggles.warlock);
        assert!(!toggles.dungeons);
        assert!(!toggles.rune_condition);
        assert!(!toggles.rune_effect);
    }

    // ── Entry projection tests ────────────────────────────────────────
    //
    // The generated view structs are plain Rust values, so the
    // AppState -> AffixScreen mapping is unit-testable without a
    // Slint runtime; the live projection is exercised by the headless
    // GUI harness scenario (testing_harness.rs).

    fn preset_entry(
        kind: d4lc_gui::PresetEntryKind,
        id: &str,
        label: Option<&str>,
        item_type: Option<&str>,
        color: &str,
    ) -> d4lc_gui::PresetEntry {
        d4lc_gui::PresetEntry {
            kind,
            id: id.to_string(),
            label: label.map(str::to_string),
            item_type: item_type.map(str::to_string),
            color: color.to_string(),
            is_any_type: item_type.is_none(),
            is_greater: true,
            is_implicit: false,
            is_tempered: true,
            tuning_prisms: Vec::new(),
        }
    }

    #[test]
    fn parse_hex_color_parses_six_digit_hex_with_hash() {
        let color = parse_hex_color("#22C55E");
        assert_eq!(
            (color.red(), color.green(), color.blue()),
            (0x22, 0xC5, 0x5E)
        );
    }

    #[test]
    fn parse_hex_color_accepts_missing_hash_and_whitespace() {
        let color = parse_hex_color(" ff8800 ");
        assert_eq!(
            (color.red(), color.green(), color.blue()),
            (0xFF, 0x88, 0x00)
        );
    }

    #[test]
    fn parse_hex_color_falls_back_to_white_on_unparsable_input() {
        // The entry draft validation bounds the stored string's
        // length, not its shape, so stored values can be non-hex.
        for garbage in ["", "red", "#FFF", "#12345", "1234567", "######"] {
            let color = parse_hex_color(garbage);
            assert_eq!(
                (color.red(), color.green(), color.blue()),
                (0xFF, 0xFF, 0xFF),
                "unparsable color {garbage:?} must fall back to white"
            );
        }
    }

    // ── Catalog projection tests ──────────────────────────────────────
    //
    // The generated view structs are plain Rust values, so the
    // catalog-row -> AffixScreen mapping is unit-testable without a Slint
    // runtime; the live projection is exercised by the headless GUI
    // harness catalog scenarios (testing_harness.rs).

    fn catalog_row(
        entry: d4lc_rules::CatalogEntry,
        wanted: Option<d4lc_gui::PresetEntry>,
    ) -> d4lc_gui::affix::catalog::CatalogRow {
        d4lc_gui::affix::catalog::CatalogRow { entry, wanted }
    }

    /// The upstream `Description` the index row of a fixture id carries, read
    /// out of the vendored enUS affix index: the text
    /// `AffixIdToDescriptionConverter` resolves a wanted row to
    /// (`AffixManager.GetAffixDescription`).
    fn upstream_affix_description(id: &str) -> String {
        let root = catalog_repo_root().expect("the vendored assets tree must resolve");
        let text = std::fs::read_to_string(root.join("assets/original/data/Affixes.enUS.json"))
            .expect("the vendored enUS affix index must be readable");
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&text).expect("the vendored enUS affix index must parse");
        rows.iter()
            .find(|row| {
                let id_name = row["IdName"].as_str().unwrap_or_default();
                id_name == id || id_name.split(';').any(|member| member.trim() == id)
            })
            .and_then(|row| row["Description"].as_str().map(str::to_string))
            .unwrap_or_else(|| panic!("the vendored enUS affix index must carry {id:?}"))
    }

    /// The projected card/chip text of a wanted entry is the localized index
    /// text, even when the preset stores the id as one `IdName` list member
    /// and carries its own label.
    ///
    /// Upstream renders every wanted row through
    /// `AffixIdToDescriptionConverter` → `AffixInfo.Description`
    /// (`AffixView.xaml:201`), never through the preset file: the shipped
    /// `AffixPresets-v2.json` stores `Life`, `X2_DamageType_Fire`, … one list
    /// member at a time while the index row carries the `;`-joined `IdName`,
    /// and an exact-id match lets those entries fall back to the preset's own
    /// `Label` ("Maximum Life", "x Fire Damage Multiplier"). This pins the
    /// projection to the converter output read from the vendored data
    /// itself.
    #[test]
    fn affix_projection_renders_the_localized_index_text_for_preset_ids() {
        use slint::Model as _;
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let entries = vec![
            // The live preset's shape: a list-member id whose row text the
            // preset's own label contradicts, plus a second slot for the
            // same id (one chip per preset entry).
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "Life",
                Some("Maximum Life"),
                Some("chest"),
                "#008000",
            ),
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "Life",
                Some("Maximum Life"),
                Some("weapon"),
                "#008000",
            ),
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "X2_DamageType_Fire",
                Some("x Fire Damage Multiplier"),
                Some("weapon"),
                "#008000",
            ),
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "Lucky_Hit_Resource",
                Some("Lucky Hit: Up to a 15 Chance to Restore Primary Resource"),
                Some("ring"),
                "#008000",
            ),
            // An id the vendored index does not carry keeps the preset's text.
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "NotInTheLocalizedData",
                Some("Unknown Affix"),
                Some("boots"),
                "#008000",
            ),
        ];
        state.affix_preset_editor.create_preset("Parity").unwrap();
        for entry in entries {
            state
                .affix_preset_editor
                .edit_entry("Parity", entry)
                .expect("the fixture entry must persist");
        }
        project_affix_entries(&app, &mut state);

        let expected = |id: &str| upstream_affix_description(id);
        let labels: Vec<String> = app
            .get_affix_entries()
            .iter()
            .map(|entry| entry.label.to_string())
            .collect();
        for id in ["Life", "X2_DamageType_Fire", "Lucky_Hit_Resource"] {
            let text = expected(id);
            assert!(
                labels.contains(&text),
                "the {id:?} row must render the index text {text:?}; projected labels: {labels:?}"
            );
        }
        assert!(
            !labels.iter().any(|label| label == "Maximum Life"
                || label == "x Fire Damage Multiplier"
                || label == "Lucky Hit: Up to a 15 Chance to Restore Primary Resource"),
            "no row may render the preset file's own label for an id the data carries: {labels:?}"
        );
        assert!(labels.iter().any(|label| label == "Unknown Affix"));

        // Every preset entry keeps its own rail chip, labelled with the index
        // text — two entries wanting `Life` for two slots included.
        let rail: Vec<(String, String)> = app
            .get_affix_rail_rows()
            .iter()
            .filter(|row| !row.is_header)
            .map(|row| (row.key.to_string(), row.label.to_string()))
            .collect();
        let life_text = expected("Life");
        assert_eq!(
            rail.iter().filter(|(_, label)| *label == life_text).count(),
            2,
            "one chip per wanted slot must render {life_text:?}: {rail:?}"
        );
        assert!(rail
            .iter()
            .any(|(key, label)| key == "affix:X2_DamageType_Fire:weapon"
                && *label == expected("X2_DamageType_Fire")));
        assert!(rail
            .iter()
            .any(|(key, label)| key == "affix:NotInTheLocalizedData:boots"
                && label == "Unknown Affix"));
    }

    /// A window growth re-projects the windowed models once, and never
    /// rebuilds the wanted-entry rail.
    #[test]
    fn catalog_window_growth_is_capped_and_leaves_the_rail_alone() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Scroll").unwrap();
        state
            .affix_preset_editor
            .edit_entry(
                "Scroll",
                preset_entry(
                    d4lc_gui::PresetEntryKind::Affix,
                    "Life",
                    None,
                    Some("helm"),
                    "#008000",
                ),
            )
            .unwrap();
        project_affix_entries(&app, &mut state);

        let rows_for = |app: &AppWindow| app.get_affix_entries().row_count();
        let row_count = rows_for(&app);
        let rail_rows = app.get_affix_rail_rows().row_count();

        // One request grows the window by exactly one page.
        let projections = THREAD_PROJECTION_COUNT.get();
        let rail_builds = THREAD_RAIL_BUILD_COUNT.get();
        handle_affix_catalog_window_changed(
            &app,
            &mut state,
            (DEFAULT_CATALOG_WINDOW + CATALOG_WINDOW_PAGE) as i32,
        );
        assert_eq!(
            state.affix_catalog_window,
            DEFAULT_CATALOG_WINDOW + CATALOG_WINDOW_PAGE
        );
        assert!(
            rows_for(&app) > row_count,
            "the window must carry more rows"
        );
        assert_eq!(
            THREAD_PROJECTION_COUNT.get() - projections,
            1,
            "one growth request must project the windowed models once"
        );
        assert_eq!(
            THREAD_RAIL_BUILD_COUNT.get(),
            rail_builds,
            "a window growth must not rebuild the wanted-entry rail"
        );
        assert_eq!(app.get_affix_rail_rows().row_count(), rail_rows);

        // A burst of requests for the window the screen already has (the
        // shape a bottom-edge handler repeats while the user keeps scrolling)
        // is idempotent: no re-projection, no rail rebuild.
        let projections = THREAD_PROJECTION_COUNT.get();
        let rail_builds = THREAD_RAIL_BUILD_COUNT.get();
        let current = state.affix_catalog_window as i32;
        for _ in 0..8 {
            handle_affix_catalog_window_changed(&app, &mut state, current);
        }
        assert_eq!(
            THREAD_PROJECTION_COUNT.get() - projections,
            0,
            "repeated requests for the current window must not re-project"
        );
        assert_eq!(THREAD_RAIL_BUILD_COUNT.get(), rail_builds);

        // A single request can never telescope the window: it caps at one page.
        handle_affix_catalog_window_changed(&app, &mut state, 5000);
        assert_eq!(
            state.affix_catalog_window,
            DEFAULT_CATALOG_WINDOW + 2 * CATALOG_WINDOW_PAGE,
            "one request may grow the window by at most one page"
        );

        // The screen's own reset (a tab switch or a filter edit asks for one
        // page) still narrows the window back to a single page — plus the
        // seeded preset's wanted row, whose sorted catalog row sits beyond the
        // window and therefore leads the grid: the projection is the windowed
        // page plus the wanted rows the window does not carry (`catalog_rows`).
        handle_affix_catalog_window_changed(&app, &mut state, CATALOG_WINDOW_PAGE as i32);
        assert_eq!(state.affix_catalog_window, CATALOG_WINDOW_PAGE);
        assert_eq!(
            rows_for(&app),
            CATALOG_WINDOW_PAGE + 1,
            "the window is one page plus the wanted row it does not carry"
        );
        let lead = app
            .get_affix_entries()
            .row_data(0)
            .expect("the one-page projection must carry the wanted lead row");
        assert!(
            lead.is_wanted,
            "the lead row must be the seeded wanted entry"
        );
        assert_eq!(
            lead.key_id, "Life",
            "the wanted `Life` row beyond the window must lead the grid"
        );
    }

    #[test]
    fn affix_catalog_view_maps_data_markers_and_wanted_state() {
        let mut entry = d4lc_rules::CatalogEntry::without_data("AttackSpeed", "+# Attack Speed");
        entry.is_tempered = true;
        entry.classes = d4lc_rules::ClassFlags {
            sorcerer: true,
            barbarian: true,
            ..d4lc_rules::ClassFlags::default()
        };
        let wanted = preset_entry(
            d4lc_gui::PresetEntryKind::Affix,
            "AttackSpeed",
            Some("Attack Speed"),
            Some("Amulet"),
            "#22C55E",
        );
        let view = affix_catalog_view(&catalog_row(entry, Some(wanted)));
        assert_eq!(view.id, "AttackSpeed");
        assert_eq!(view.label, "+# Attack Speed");
        assert_eq!(
            view.wanted_label, "Attack Speed",
            "the rail row shows the preset's own label"
        );
        assert_eq!(view.item_type, "Amulet");
        assert!(view.is_wanted);
        assert!(
            view.is_tempered,
            "the tempering marker is the data attribute"
        );
        assert!(
            view.wanted_is_tempered,
            "the preset's tempered rule flag travels too"
        );
        assert!(
            view.wanted_is_greater,
            "the chip fg color follows the preset entry's IsGreater \
             (`AffixTypeToFgSolidBrushConverter`)"
        );
        assert!(
            !view.wanted_is_implicit,
            "the chip bg color follows the preset entry's IsImplicit/IsTempered \
             (`AffixTypeToBgSolidBrushConverter`)"
        );
        assert!(view.class_sorc && view.class_barb);
        assert!(!view.class_druid && !view.class_warlk);
        assert_eq!(
            (view.color.red(), view.color.green(), view.color.blue()),
            (0x22, 0xC5, 0x5E)
        );
    }

    #[test]
    fn affix_catalog_view_maps_the_wanted_implicit_flag_for_the_chip_background() {
        // A plain (non-greater, implicit) wanted affix: white text on the
        // lighter chip background, the opposite of the tempered-greater case.
        let entry = d4lc_rules::CatalogEntry::without_data("Life", "+# Life");
        let mut wanted = preset_entry(
            d4lc_gui::PresetEntryKind::Affix,
            "Life",
            Some("Life"),
            Some("helm"),
            "#EF4444",
        );
        wanted.is_greater = false;
        wanted.is_implicit = true;
        wanted.is_tempered = false;
        let view = affix_catalog_view(&catalog_row(entry, Some(wanted)));
        assert!(!view.wanted_is_greater);
        assert!(view.wanted_is_implicit);
        assert!(!view.wanted_is_tempered);
        assert_eq!(view.item_type, "helm");
    }

    #[test]
    fn catalog_view_keeps_unwanted_rows_marker_only() {
        let entry = d4lc_rules::CatalogEntry::without_data("PickupRadius", "+# Pickup Radius");
        let view = affix_catalog_view(&catalog_row(entry, None));
        assert!(!view.is_wanted);
        assert_eq!(view.wanted_label, "");
        assert_eq!(view.item_type, "");
        assert!(!view.is_tempered);
    }

    #[test]
    fn family_catalog_views_carry_the_data_attributes() {
        let mut aspect = d4lc_rules::CatalogEntry::without_data("legendary_druid_108", "Ballistic");
        aspect.description = "Cast a Core skill".to_string();
        aspect.dungeon = "Whispering Pines".to_string();
        aspect.is_codex = true;
        aspect.classes = d4lc_rules::ClassFlags {
            druid: true,
            ..d4lc_rules::ClassFlags::default()
        };
        let view = aspect_catalog_view(&catalog_row(
            aspect,
            Some(preset_entry(
                d4lc_gui::PresetEntryKind::Aspect,
                "legendary_druid_108",
                None,
                Some("Amulet"),
                "#3B82F6",
            )),
        ));
        assert_eq!(view.label, "Ballistic");
        assert_eq!(
            view.wanted_label, "Ballistic",
            "a label-less wanted entry falls back to the catalog text"
        );
        assert_eq!(view.description, "Cast a Core skill");
        assert_eq!(view.dungeon, "Whispering Pines");
        assert!(view.is_codex && !view.is_seasonal && !view.is_drop_only);
        assert!(view.class_druid && !view.class_sorc);

        let mut sigil =
            d4lc_rules::CatalogEntry::without_data("World_DGN_X", "Abandoned Mineworks");
        sigil.description = "Dungeon sigil".to_string();
        sigil.is_seasonal = true;
        sigil.sigil_type = Some("Dungeon".to_string());
        let state = AppState::default_startup();
        let view = sigil_catalog_view(&state, &catalog_row(sigil, None));
        assert_eq!(view.label, "Abandoned Mineworks");
        assert!(view.is_seasonal);
        assert_eq!(
            view.tier_label,
            d4lc_gui::affix::SIGIL_DUNGEON_TIER_DEFAULT,
            "a dungeon sigil without a configured tier projects the upstream F default \
             (`AffixManager.GetSigilDungeonTier`, AffixManager.cs:744-747)"
        );

        let mut unique = d4lc_rules::CatalogEntry::without_data("RingOfVayar", "Ring of Vayar");
        unique.description = "Unique ring".to_string();
        let view = unique_catalog_view(&catalog_row(unique, None));
        assert_eq!(view.label, "Ring of Vayar");
        assert_eq!(view.description, "Unique ring");

        let mut rune = d4lc_rules::CatalogEntry::without_data("Item_Rune_Condition_OnMove", "Bac");
        rune.is_condition = true;
        let view = rune_catalog_view(&catalog_row(rune, None));
        assert_eq!(view.label, "Bac");
        assert!(view.is_condition && !view.is_effect);
    }

    #[test]
    fn orphan_rows_keep_preset_entries_the_data_does_not_know() {
        // A preset that references ids from another data version or
        // language keeps its entries: they project as data-less rows with
        // the preset's own label, before the sorted catalog.
        let catalog = d4lc_rules::AffixCatalog::empty("enUS");
        let filter = d4lc_gui::affix::catalog::CatalogFilter::default();
        let entries = vec![
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "LegacyAffix",
                Some("Legacy Affix"),
                Some("Helm"),
                "#EF4444",
            ),
            preset_entry(
                d4lc_gui::PresetEntryKind::Aspect,
                "LegacyAspect",
                None,
                None,
                "#008000",
            ),
        ];
        let affixes = catalog_rows(
            d4lc_rules::CatalogKind::Affix,
            &catalog,
            &entries,
            &filter,
            DEFAULT_CATALOG_WINDOW,
        );
        assert_eq!(affixes.len(), 1);
        assert_eq!(affixes[0].entry.id, "LegacyAffix");
        assert_eq!(affixes[0].entry.label, "Legacy Affix");
        assert_eq!(affixes[0].wanted_display_name(), Some("Legacy Affix"));
        assert!(affixes[0].wanted.is_some());

        let aspects = catalog_rows(
            d4lc_rules::CatalogKind::Aspect,
            &catalog,
            &entries,
            &filter,
            DEFAULT_CATALOG_WINDOW,
        );
        assert_eq!(aspects.len(), 1);
        assert_eq!(
            aspects[0].entry.label, "LegacyAspect",
            "a label-less entry falls back to its id"
        );

        // The other kinds carry no orphans.
        assert!(catalog_rows(
            d4lc_rules::CatalogKind::Sigil,
            &catalog,
            &entries,
            &filter,
            DEFAULT_CATALOG_WINDOW
        )
        .is_empty());
    }

    #[test]
    fn sigil_tier_label_follows_the_tier_setting_and_the_sigil_type() {
        let mut state = AppState::default_startup();
        let mut sigil = d4lc_rules::CatalogEntry::without_data("World_DGN_X", "Sigil");
        sigil.sigil_type = Some("Dungeon".to_string());
        state
            .dungeon_tiers
            .insert("World_DGN_X".to_string(), "S".to_string());
        assert_eq!(sigil_tier_label(&state, &sigil), "S");

        state.shell.settings.dungeon_tiers_enabled = false;
        assert_eq!(
            sigil_tier_label(&state, &sigil),
            "",
            "the tier badge hides while the tier display is off"
        );

        state.shell.settings.dungeon_tiers_enabled = true;
        let mut positive = d4lc_rules::CatalogEntry::without_data("World_DGN_X", "Sigil");
        positive.sigil_type = Some("Positive".to_string());
        assert_eq!(
            sigil_tier_label(&state, &positive),
            "",
            "only dungeon sigils carry a tier badge"
        );

        // A dungeon sigil without a stored tier shows the upstream `F`
        // default, exactly like the Sigil config dialog's tier rows
        // (`AffixManager.GetSigilDungeonTier`, AffixManager.cs:744-747).
        let mut unstored = d4lc_rules::CatalogEntry::without_data("World_DGN_Y", "Sigil");
        unstored.sigil_type = Some("Dungeon".to_string());
        assert_eq!(
            sigil_tier_label(&state, &unstored),
            d4lc_gui::affix::SIGIL_DUNGEON_TIER_DEFAULT
        );
    }

    // ── Kind-tagged entry key contract tests ──────────────────────────

    #[test]
    fn parse_entry_key_reads_all_kind_prefixes_and_item_types() {
        let key = parse_entry_key("affix:AttackSpeed:Amulet").unwrap();
        assert_eq!(key.kind, d4lc_gui::PresetEntryKind::Affix);
        assert_eq!(key.id, "AttackSpeed");
        assert_eq!(key.item_type.as_deref(), Some("Amulet"));

        let bare = parse_entry_key("affix:Life:").unwrap();
        assert_eq!(bare.id, "Life");
        assert_eq!(bare.item_type, None, "empty item-type segment means None");

        for (text, kind) in [
            ("aspect:x:", d4lc_gui::PresetEntryKind::Aspect),
            ("sigil:x:", d4lc_gui::PresetEntryKind::Sigil),
            ("unique:x:", d4lc_gui::PresetEntryKind::Unique),
            ("rune:x:", d4lc_gui::PresetEntryKind::Rune),
        ] {
            let key = parse_entry_key(text).unwrap();
            assert_eq!(key.kind, kind);
            assert_eq!(key.id, "x");
        }
    }

    #[test]
    fn parse_entry_key_rejects_malformed_keys() {
        for garbage in ["", "Affix:Life:", "affix:", "affix", "affix::Amulet"] {
            assert!(
                parse_entry_key(garbage).is_none(),
                "malformed key {garbage:?} must be rejected"
            );
        }
    }

    #[test]
    fn find_entry_index_matches_kind_and_never_a_foreign_kind_index() {
        // The screen's per-kind view indices are not indices into the
        // mixed-kind entry list (affix index 1 is entry 3 here). Keys must
        // resolve to the entry's real mixed-list index.
        let entries = vec![
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "Life",
                None,
                None,
                "#EF4444",
            ),
            preset_entry(
                d4lc_gui::PresetEntryKind::Aspect,
                "BloodForBlood",
                Some("Blood For Blood"),
                None,
                "#3B82F6",
            ),
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "AttackSpeed",
                Some("Attack Speed"),
                Some("Amulet"),
                "#22C55E",
            ),
        ];
        let aspect_key = parse_entry_key("aspect:BloodForBlood:").unwrap();
        assert_eq!(find_entry_index(&entries, &aspect_key), Some(1));
        let amulet_key = parse_entry_key("affix:AttackSpeed:Amulet").unwrap();
        assert_eq!(find_entry_index(&entries, &amulet_key), Some(2));
        let bare_key = parse_entry_key("affix:Life:").unwrap();
        assert_eq!(find_entry_index(&entries, &bare_key), Some(0));
    }

    #[test]
    fn find_entry_index_distinguishes_affix_item_types() {
        let entries = vec![
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "AttackSpeed",
                None,
                None,
                "#111111",
            ),
            preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "AttackSpeed",
                None,
                Some("Amulet"),
                "#222222",
            ),
        ];
        let bare = parse_entry_key("affix:AttackSpeed:").unwrap();
        assert_eq!(find_entry_index(&entries, &bare), Some(0));
        let amulet = parse_entry_key("affix:AttackSpeed:Amulet").unwrap();
        assert_eq!(find_entry_index(&entries, &amulet), Some(1));
    }

    #[test]
    fn find_entry_index_matches_non_affix_kind_on_kind_and_id_only() {
        // Non-affix kinds project no item type, so the key's empty
        // segment must not require the entry's item_type to be None.
        let entries = vec![d4lc_gui::PresetEntry::non_affix(
            d4lc_gui::PresetEntryKind::Aspect,
            "BloodForBlood",
            Some("Amulet"),
            "#3B82F6",
        )];
        let key = parse_entry_key("aspect:BloodForBlood:").unwrap();
        assert_eq!(find_entry_index(&entries, &key), Some(0));
    }
}
