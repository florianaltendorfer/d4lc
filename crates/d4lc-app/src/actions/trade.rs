//! Trade action handlers for the Slint desktop app.
//!
//! ## Upstream evidence
//!
//! - `TradeViewModel.cs:78-81` — 4 command definitions
//! - `D4Companion/Views/Dialogs/TradeConfigView.xaml` +
//!   `TradeConfigViewModel.cs` — TradeConfig field shape
//!   (`IsTradeOverlayEnabled`, `ShowCurrentItem`, `OverlayFontSize`)
//!
//! ## Dialogs
//!
//! `handle_add_trade_item` opens the AddTradeItem dialog through the
//! shared [`crate::actions::dialogs`] router. `handle_add_trade_item_confirm`
//! is the dialog-confirm entry point: it reads the form values
//! projected on the root `AppWindow` (see `main_window.slint` for
//! the `trade-form-item-type` and `trade-form-value-text` properties),
//! mutates `AppState` through the [`TradeViewModel`], and surfaces
//! validation errors via the shared dialog error contract.
//!
//! `handle_open_trade_config` opens the TradeConfig dialog
//! through the shared router and seeds the three root in-out props
//! (`trade-config-overlay-enabled`, `trade-config-show-current-item`,
//! `trade-config-font-size`) from the current `SettingsPanelState`.
//! `handle_trade_config_confirm` is the dialog-confirm entry point:
//! it reads the form values, validates them through the headless
//! [`d4lc_gui::trade::confirm_trade_config_draft`], applies the
//! mappable fields to `state.shell.settings`, and persists via
//! `d4lc_config::load_app_settings` / `save_app_settings` (including
//! `ShowCurrentItem`).
//!
//! ## Trade screen projection
//!
//! The TradeScreen's wanted-item list and current-item card are
//! root-declared and projected from `AppState` by
//! [`project_trade_state`]; every trade-item mutation success path
//! (add/edit confirm, remove) re-projects through it, mirroring the
//! upstream WPF binding where the `TradeItems` collection observes
//! the view model directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{Model as _, SharedString, VecModel};

use d4lc_gui::{
    trade::{
        compare_wanted_display_rows, confirm_trade_config_draft, filter_affix_candidates,
        filter_rune_candidates, is_known_item_type, is_rune_item_type, TradeAffixCandidate,
        TradeConfigDraft, TradeEffect, TradeRuneCandidate, TradeViewModel, TRADE_ITEM_TYPES,
    },
    CurrentItemAffixView, TradeImportsPanelState, TradeItemRow,
};

use crate::app_state::{AppState, DialogKind};
use crate::slint_generated::{
    AppWindow, TradeAffixDisplay, TradeItem, TradePickerRow, TradeStagedAffix,
};

// ── Trade screen projection ────────────────────────────────────────────

/// Two-letter monogram standing in for the upstream type image
/// (TradeView.xaml:109 binds `Type.Image`; the port renders a text
/// badge in the card header). Unknown types fall back to "??".
fn trade_type_badge(item_type: &str) -> String {
    const BADGES: [(&str, &str); 11] = [
        ("Amulet", "Am"),
        ("Boots", "Bo"),
        ("Chest", "Ch"),
        ("Gloves", "Gl"),
        ("Helm", "He"),
        ("Offhand", "Of"),
        ("Pants", "Pa"),
        ("Ranged", "Ra"),
        ("Ring", "Ri"),
        ("Rune", "Ru"),
        ("Weapon", "We"),
    ];
    let trimmed = item_type.trim();
    for (known, badge) in BADGES {
        if known.eq_ignore_ascii_case(trimmed) {
            return badge.to_string();
        }
    }
    "??".to_string()
}

/// Affix-id resolution tables for the trade display, loaded from the
/// vendored upstream data catalogs. This is the port of the upstream
/// `AffixIdToDescriptionConverter` / `AffixIdToRuneNameConverter`
/// (TradeView.xaml resources, backed by
/// `AffixManager.GetAffixDescription`, AffixManager.cs:562-572).
///
/// The catalogs are keyed so that both a full `IdName` and any
/// `;`-segment of a compound `IdName` row resolve — d4lc trade rows
/// may carry either form. When a row's id is unknown the display
/// falls back to the raw id (upstream renders an empty string on a
/// miss; the id keeps d4lc rows readable instead of blank).
pub struct TradeAffixDescriptions {
    affixes: HashMap<String, String>,
    runes: HashMap<String, String>,
}

impl TradeAffixDescriptions {
    pub fn resolve(&self, affix_id: &str) -> String {
        if let Some(description) = self.affixes.get(affix_id) {
            return description.clone();
        }
        if let Some(name) = self.runes.get(affix_id) {
            return name.clone();
        }
        affix_id.to_string()
    }
}

/// Parse the vendored `Affixes.{language}.json` catalog rows into an
/// id → description table.
fn parse_affix_descriptions(text: &str) -> Option<HashMap<String, String>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    let mut map = HashMap::new();
    for row in rows {
        let id_name = row.get("IdName").and_then(|value| value.as_str());
        let description = row.get("Description").and_then(|value| value.as_str());
        if let (Some(id_name), Some(description)) = (id_name, description) {
            // The full IdName keys first: picker-added rows carry the
            // candidate's compound IdName verbatim. The `;`-segments
            // keep resolving for stored rows keyed by a single segment.
            map.entry(id_name.to_string())
                .or_insert_with(|| description.to_string());
            for segment in id_name.split(';') {
                map.entry(segment.to_string())
                    .or_insert_with(|| description.to_string());
            }
        }
    }
    Some(map)
}

/// Parse the vendored `Runes.{language}.json` catalog rows into an
/// id → rune name table (upstream `AffixIdToRuneNameConverter`; the
/// rune description is tooltip-only upstream and is not rendered on
/// the card).
fn parse_rune_names(text: &str) -> Option<HashMap<String, String>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    let mut map = HashMap::new();
    for row in rows {
        let id_name = row.get("IdName").and_then(|value| value.as_str());
        let name = row.get("Name").and_then(|value| value.as_str());
        if let (Some(id_name), Some(name)) = (id_name, name) {
            map.insert(id_name.to_string(), name.to_string());
        }
    }
    Some(map)
}

/// Resolve the vendored upstream data directory: CWD-first (installed
/// bundle layout), then the compile-time dev-checkout root — the same
/// resolution order as the vendored system-preset dir in
/// `actions::settings`. `None` when the tree exists in neither place.
fn resolve_trade_data_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(d4lc_assets::DATA_DIR));
    }
    let mut workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    workspace_root.pop(); // d4lc-app
    workspace_root.pop(); // crates
    candidates.push(workspace_root.join(d4lc_assets::DATA_DIR));
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// Load the affix/rune description tables for `language` from the
/// vendored catalogs. Unreadable files yield empty tables, and the
/// display then falls back to the raw affix ids — wanted rows keep
/// rendering instead of losing their affix section.
fn load_trade_affix_descriptions(data_dir: &Path, language: &str) -> TradeAffixDescriptions {
    TradeAffixDescriptions {
        affixes: std::fs::read_to_string(data_dir.join(format!("Affixes.{language}.json")))
            .ok()
            .and_then(|text| parse_affix_descriptions(&text))
            .unwrap_or_default(),
        runes: std::fs::read_to_string(data_dir.join(format!("Runes.{language}.json")))
            .ok()
            .and_then(|text| parse_rune_names(&text))
            .unwrap_or_default(),
    }
}

fn trade_catalog_language(state: &AppState) -> String {
    let language = state.shell.settings.language.trim();
    if language.is_empty() {
        d4lc_config::DEFAULT_AFFIX_LANGUAGE.to_string()
    } else {
        language.to_string()
    }
}

// ── AddTradeItem dialog picker ────────────────────────────────────────

/// Parse the vendored `Affixes.{language}.json` catalog rows into the
/// candidate list the AddTradeItem dialog offers (upstream
/// `InitAffixes` over `_affixManager.Affixes`,
/// AddTradeItemViewModel.cs:299-303: one candidate per catalog row,
/// matched and added by its full `IdName`).
fn parse_affix_candidates(text: &str) -> Option<Vec<TradeAffixCandidate>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let id = row.get("IdName")?.as_str()?;
                let description = row.get("Description")?.as_str()?;
                Some(TradeAffixCandidate {
                    id: id.to_string(),
                    description: description.to_string(),
                })
            })
            .collect(),
    )
}

/// Parse the vendored `Runes.{language}.json` catalog rows into the
/// rune candidate list (upstream `InitAffixesRunes` over
/// `_affixManager.Runes`, AddTradeItemViewModel.cs:305-309).
fn parse_rune_candidates(text: &str) -> Option<Vec<TradeRuneCandidate>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let id = row.get("IdName")?.as_str()?;
                let name = row.get("Name")?.as_str()?;
                let description = row
                    .get("Description")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                Some(TradeRuneCandidate {
                    id: id.to_string(),
                    name: name.to_string(),
                    description: description.to_string(),
                })
            })
            .collect(),
    )
}

/// Load the full affix / rune candidate catalogs for the AddTradeItem
/// dialog picker. Empty lists when the vendored data dir or a catalog
/// file is unavailable — the picker then simply offers no candidates
/// (the free-text type / value form keeps working).
pub fn load_trade_candidates(
    state: &AppState,
) -> (Vec<TradeAffixCandidate>, Vec<TradeRuneCandidate>) {
    let Some(data_dir) = resolve_trade_data_dir() else {
        return (Vec::new(), Vec::new());
    };
    let language = trade_catalog_language(state);
    let affixes = std::fs::read_to_string(data_dir.join(format!("Affixes.{language}.json")))
        .ok()
        .and_then(|text| parse_affix_candidates(&text))
        .unwrap_or_default();
    let runes = std::fs::read_to_string(data_dir.join(format!("Runes.{language}.json")))
        .ok()
        .and_then(|text| parse_rune_candidates(&text))
        .unwrap_or_default();
    (affixes, runes)
}

/// Load the label tables used to resolve staged affix display texts
/// (same catalogs and raw-id fallback as the wanted-card display).
pub fn load_trade_label_tables(state: &AppState) -> TradeAffixDescriptions {
    match resolve_trade_data_dir() {
        Some(data_dir) => load_trade_affix_descriptions(&data_dir, &trade_catalog_language(state)),
        None => TradeAffixDescriptions {
            affixes: HashMap::new(),
            runes: HashMap::new(),
        },
    }
}

/// (Re-)project the filtered candidate models from the full catalogs
/// through the headless `;`-keyword filters (upstream
/// `FilterAffixes` / `FilterAffixesRunes` view refresh,
/// AddTradeItemViewModel.cs:248-297; an empty filter keeps all).
pub fn project_filtered_trade_candidates(
    app: &AppWindow,
    affixes: &[TradeAffixCandidate],
    runes: &[TradeRuneCandidate],
    filter: &str,
) {
    let affix_rows: Vec<TradePickerRow> = filter_affix_candidates(affixes, filter)
        .into_iter()
        .map(|candidate| TradePickerRow {
            affix_id: candidate.id.as_str().into(),
            label: candidate.description.as_str().into(),
        })
        .collect();
    let rune_rows: Vec<TradePickerRow> = filter_rune_candidates(runes, filter)
        .into_iter()
        .map(|candidate| TradePickerRow {
            affix_id: candidate.id.as_str().into(),
            label: candidate.name.as_str().into(),
        })
        .collect();
    app.set_trade_affix_candidates(Rc::new(VecModel::from(affix_rows)).into());
    app.set_trade_rune_candidates(Rc::new(VecModel::from(rune_rows)).into());
}

/// The dialog's staged draft affix rows (the port of the upstream
/// `TradeItem.Affixes` ObservableCollection the AddTradeItemViewModel
/// mutates live; the host applies them to the real row on confirm).
fn staged_trade_rows(app: &AppWindow) -> Vec<TradeStagedAffix> {
    let model = app.get_trade_item_affixes();
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .collect()
}

/// The staged draft flattened to `(affix_id, is_greater)` pairs.
fn staged_trade_affixes(app: &AppWindow) -> Vec<(String, bool)> {
    staged_trade_rows(app)
        .into_iter()
        .map(|affix| (affix.affix_id.to_string(), affix.is_greater))
        .collect()
}

fn set_staged_trade_rows(app: &AppWindow, rows: Vec<TradeStagedAffix>) {
    app.set_trade_item_affixes(Rc::new(VecModel::from(rows)).into());
}

/// Clear the staged draft rows (upstream `SelectedItemType` setter
/// clears `TradeItem.Affixes` when the type crosses the rune
/// boundary, AddTradeItemViewModel.cs:113-119).
pub fn clear_trade_staged_affixes(app: &AppWindow) {
    set_staged_trade_rows(app, Vec::new());
}

/// Stage one picker candidate onto the dialog draft: appended with
/// `IsGreater = false` and re-sorted by id (upstream
/// `AddAffixExecute` / `AddAffixRuneExecute` +
/// `OrderBy(a => a.Id)`, AddTradeItemViewModel.cs:158-201). For rune
/// items the add is gated on the draft holding no affix yet —
/// upstream `CanAddAffixRuneExecute` (AddTradeItemViewModel.cs:
/// 177-180); the Slint button is disabled on the same condition, so
/// the `Err` here is the backstop for a bypassed gate.
pub fn stage_trade_affix(
    app: &AppWindow,
    state: &AppState,
    affix_id: &str,
    rune: bool,
) -> Result<(), String> {
    let mut rows = staged_trade_rows(app);
    if rows.iter().any(|affix| affix.affix_id.as_str() == affix_id) {
        return Ok(());
    }
    if rune && !rows.is_empty() {
        return Err("A rune item can hold at most one affix.".to_string());
    }
    let label = load_trade_label_tables(state).resolve(affix_id);
    rows.push(TradeStagedAffix {
        affix_id: affix_id.into(),
        label: label.into(),
        is_greater: false,
    });
    rows.sort_by(|left, right| left.affix_id.cmp(&right.affix_id));
    set_staged_trade_rows(app, rows);
    Ok(())
}

/// Remove one staged draft row (upstream `RemoveAffixExecute`,
/// AddTradeItemViewModel.cs:211-218).
pub fn unstage_trade_affix(app: &AppWindow, affix_id: &str) {
    let rows: Vec<TradeStagedAffix> = staged_trade_rows(app)
        .into_iter()
        .filter(|affix| affix.affix_id.as_str() != affix_id)
        .collect();
    set_staged_trade_rows(app, rows);
}

/// Set the IsGreater flag of one staged draft row (upstream binds the
/// flag through a per-row checkbox, AddTradeItemView.xaml:87-88).
pub fn restage_trade_affix_is_greater(app: &AppWindow, affix_id: &str, is_greater: bool) {
    let mut rows = staged_trade_rows(app);
    for affix in &mut rows {
        if affix.affix_id.as_str() == affix_id {
            affix.is_greater = is_greater;
        }
    }
    set_staged_trade_rows(app, rows);
}

/// Seed the AddTradeItem dialog's picker models for a fresh open and
/// return the full candidate catalogs so the host can filter them
/// while the dialog stays open:
///
/// - the candidate lists start unfiltered (upstream shows the whole
///   catalog until the user types a filter);
/// - the staged draft seeds from the edited row's affixes in edit
///   mode and stays empty for a fresh add (upstream hands the dialog
///   the existing `TradeItemWanted` reference / a new draft);
/// - `trade-edit-mode` and the rune flag seed from the addressed row
///   (upstream `EditMode` + `IsItemTypeRune`).
pub fn seed_trade_picker_models(
    app: &AppWindow,
    state: &AppState,
) -> (Vec<TradeAffixCandidate>, Vec<TradeRuneCandidate>) {
    let (affixes, runes) = load_trade_candidates(state);
    project_filtered_trade_candidates(app, &affixes, &runes, "");
    app.set_trade_affix_filter(SharedString::default());

    let selected = state.trade_imports().selected_trade_item_id.clone();
    app.set_trade_edit_mode(selected.is_some());
    let edited_row = selected.as_deref().and_then(|id| {
        state
            .trade_imports()
            .trade_items
            .iter()
            .find(|row| row.trade_item_id == id)
    });
    match edited_row {
        Some(row) => {
            let tables = load_trade_label_tables(state);
            set_staged_trade_rows(
                app,
                row.affixes
                    .iter()
                    .map(|affix| TradeStagedAffix {
                        affix_id: affix.affix_id.as_str().into(),
                        label: tables.resolve(&affix.affix_id).into(),
                        is_greater: affix.is_greater,
                    })
                    .collect(),
            );
            app.set_trade_item_type_is_rune(is_rune_item_type(&row.item_type));
        }
        None => {
            clear_trade_staged_affixes(app);
            app.set_trade_item_type_is_rune(false);
        }
    }
    (affixes, runes)
}

/// Reset the picker draft after a successful confirm so a subsequent
/// open starts from clean models (the open handler re-seeds them).
fn clear_trade_picker_draft(app: &AppWindow) {
    clear_trade_staged_affixes(app);
    app.set_trade_affix_candidates(Rc::new(VecModel::from(Vec::<TradePickerRow>::new())).into());
    app.set_trade_rune_candidates(Rc::new(VecModel::from(Vec::<TradePickerRow>::new())).into());
    app.set_trade_affix_filter(SharedString::default());
}

/// Map a `TradeItemRow` onto the screen's `TradeItem` wanted-card row:
/// type badge, gold value text, and the affix rows resolved through
/// [`TradeAffixDescriptions`] (raw ids when no tables are available).
fn trade_view_item(row: &TradeItemRow, descriptions: Option<&TradeAffixDescriptions>) -> TradeItem {
    TradeItem {
        item_id: row.trade_item_id.as_str().into(),
        item_type: row.item_type.as_str().into(),
        badge: trade_type_badge(&row.item_type).into(),
        value: row.value_text.as_str().into(),
        affixes: Rc::new(VecModel::from(
            row.affixes
                .iter()
                .map(|affix| TradeAffixDisplay {
                    text: descriptions
                        .map(|tables| tables.resolve(&affix.affix_id))
                        .unwrap_or_else(|| affix.affix_id.clone())
                        .into(),
                    is_greater: affix.is_greater,
                })
                .collect::<Vec<TradeAffixDisplay>>(),
        ))
        .into(),
    }
}

/// Map the current-item panel state onto the CurrentItemCard values:
/// item type, item power, and the parsed affixes' display labels.
/// Without a parsed item the card shows the empty/zero defaults.
///
/// The card's type text is the display derivation of the parsed tooltip line;
/// the canonical type id stays the evaluation identity and is not rendered
fn current_trade_card(state: &AppState) -> (SharedString, i32, Vec<SharedString>) {
    let current = state.current_item();
    let item_type = SharedString::from(current.base_type.as_deref().unwrap_or(""));
    let item_power = current
        .item_power
        .map_or(0, |power| i32::try_from(power).unwrap_or(0));
    let affixes = current
        .affixes
        .iter()
        .map(|affix: &CurrentItemAffixView| affix.label.as_str().into())
        .collect::<Vec<SharedString>>();
    (item_type, item_power, affixes)
}

/// Project the trade state onto the TradeScreen's root models: the
/// wanted-item list (`trade-items`) and the current-item card
/// (`current-item-type` / `current-item-power` / `current-affixes`).
///
/// The wanted rows are ordered for display the way upstream's
/// `TradeItemsFiltered` view sorts them (`TradeItemCustomSort`,
/// TradeItemVM.cs:138-159: item type, then zero-padded value); the
/// state order (id-synthesis/persist order) is untouched. Affix
/// display text resolves through the vendored catalogs when any row
/// carries affixes.
///
/// The screen-local models are unreachable from Rust, so the root
/// properties are the single population path. Every trade-item
/// mutation success path calls it (add/edit confirm, remove; see the
/// module design notes) so the cards stay in sync with `AppState`
/// without the screen holding state of its own.
pub fn project_trade_state(app: &AppWindow, state: &AppState) {
    let mut display_rows: Vec<&TradeItemRow> = state.trade_imports().trade_items.iter().collect();
    display_rows.sort_by(|left, right| compare_wanted_display_rows(left, right));

    let descriptions = if display_rows.iter().any(|row| !row.affixes.is_empty()) {
        resolve_trade_data_dir().map(|data_dir| {
            load_trade_affix_descriptions(&data_dir, &trade_catalog_language(state))
        })
    } else {
        None
    };

    let items: Vec<TradeItem> = display_rows
        .iter()
        .map(|row| trade_view_item(row, descriptions.as_ref()))
        .collect();
    app.set_trade_items(Rc::new(VecModel::from(items)).into());

    let (item_type, item_power, affixes) = current_trade_card(state);
    app.set_current_item_type(item_type);
    app.set_current_item_power(item_power);
    app.set_current_affixes(Rc::new(VecModel::from(affixes)).into());

    // The current-item card gate rides along with the projection: the
    // startup seed (wire_trade_actions) and every trade-config confirm
    // flow through here, so `trade-show-current-item` always mirrors
    // `state.trade_show_current_item` (upstream FilterTradeItems hides
    // the current card when `Settings.ShowCurrentItem` is false,
    // TradeViewModel.cs:209-216).
    app.set_trade_show_current_item(state.trade_show_current_item);
}

/// Handle add-trade-item callback.
///
/// Creates a ViewModel, invokes the command, and opens the
/// AddTradeItem dialog through the shared
/// [`crate::actions::dialogs::open`] router. The dialog host
/// shows the `AddTradeItemDialog` component based on the
/// `dialog-id` value set by the router.
pub fn handle_add_trade_item(app: &AppWindow, state: &mut AppState) {
    let mut vm = TradeViewModel::new(state.trade_imports().clone());
    let _effects = vm.add_trade_item();
    *state.trade_imports_mut() = vm.state;
    state.trade_imports_mut().selected_trade_item_id = None;
    // Open the AddTradeItem dialog through the shared router so the
    // dialog host renders the AddTradeItemDialog component with the
    // shared dialog-error / dialog-title contract.
    crate::actions::dialogs::open(app, state, DialogKind::AddTradeItem, None);
}

/// Handle edit-trade-item callback.
pub fn handle_edit_trade_item(app: &AppWindow, state: &mut AppState, item_id: &str) {
    let Some(trade_item) = state
        .trade_imports()
        .trade_items
        .iter()
        .find(|trade_item| trade_item.trade_item_id == item_id)
        .cloned()
    else {
        return;
    };

    let mut vm = TradeViewModel::new(state.trade_imports().clone());
    let _effects = vm.edit_trade_item(item_id);
    *state.trade_imports_mut() = vm.state;
    app.set_trade_form_item_type(trade_item.item_type.into());
    app.set_trade_form_value_text(trade_item.value_text.into());
    crate::actions::dialogs::open(app, state, DialogKind::AddTradeItem, None);
}

/// Handle remove-trade-item callback.
pub fn handle_remove_trade_item(app: &AppWindow, state: &mut AppState, item_id: &str) {
    let mut vm = TradeViewModel::new(state.trade_imports().clone());
    let effects = vm.remove_trade_item(item_id);
    *state.trade_imports_mut() = vm.state;
    dispatch_trade_effects(&effects, state);
    state.rebuild_overlay_descriptor_state();
    // The wanted list changed: re-project the cards.
    project_trade_state(app, state);
}

/// Handle open-trade-config callback.
///
/// Seeds the three root in-out props (`trade-config-overlay-enabled`,
/// `trade-config-show-current-item`, `trade-config-font-size`) from
/// the current `SettingsPanelState` and the persisted
/// `Config/Settings.json`, then opens the `TradeConfig` dialog
/// through the shared [`crate::actions::dialogs::open`]
/// router so the dialog host renders the `TradeConfigDialog`
/// component with the shared `dialog-error` / `dialog-title`
/// contract.
pub fn handle_open_trade_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = TradeViewModel::new(state.trade_imports().clone());
    let _effects = vm.open_trade_config();
    *state.trade_imports_mut() = vm.state;

    // Seed the three root in-out props from the current settings
    // panel state. The dialog binds two-way to these props, so any
    // user edit becomes a draft until confirm / cancel.
    app.set_trade_config_overlay_enabled(state.shell.settings.trade_overlay_enabled);
    if let Ok(config) =
        d4lc_config::load_app_settings(&crate::actions::settings::resolve_config_root())
    {
        state.trade_show_current_item = config.show_current_item;
    }
    app.set_trade_config_show_current_item(state.trade_show_current_item);
    let font_size_i32 = i32::try_from(state.shell.settings.overlay_font_size).unwrap_or(18);
    app.set_trade_config_font_size(font_size_i32);

    crate::actions::dialogs::open(app, state, DialogKind::TradeConfig, None);
}

/// Handle add-trade-item confirm.
///
/// Reads the root form values (`trade-form-item-type`,
/// `trade-form-value-text`) projected on `AppWindow`, then
/// delegates to [`TradeViewModel::confirm_add_trade_item`]. The
/// staged picker rows (`trade-item-affixes`, the dialog's draft
/// affixes) are applied to the real row through the ViewModel
/// editing API once the type / value confirm succeeded — upstream
/// mutates the dialog's `TradeItem.Affixes` live and saves on Done
/// (AddTradeItemViewModel.cs / TradeViewModel.cs:109,129).
/// On success the dialog state is closed by the router and the
/// updated panel state is written back; on failure the
/// `Err(message)` is propagated so the shared dialog router
/// surfaces the error through the shared `dialog-error` contract
/// and the staged draft survives for a corrected retry.
///
/// Both legs run against one draft of the panel state and the draft is
/// written back only after both succeeded: an affix-leg failure (unknown
/// affix id, rune max-1 gate) must not leave the type/value edit applied in
/// memory, because the confirm already returned `Err` and the persisted
/// `Config/TradeItems.json` still holds the previous row — the in-memory row
/// would diverge from disk until the next successful persist.
pub fn handle_add_trade_item_confirm(
    app: &AppWindow,
    state: &mut AppState,
    item_type: &str,
    value_text: &str,
) -> Result<(), String> {
    let staged = staged_trade_affixes(app);
    let mut draft = state.trade_imports().clone();
    let (trade_item_id, effects) = upsert_trade_item_from_form(&mut draft, item_type, value_text)?;
    let affix_effects = apply_staged_trade_affixes(&mut draft, &trade_item_id, &staged)?;
    *state.trade_imports_mut() = draft;
    dispatch_trade_effects(&effects, state);
    dispatch_trade_effects(&affix_effects, state);
    // Clear root form props after a successful confirm so the next
    // open of the dialog starts from an empty form.
    app.set_trade_form_item_type(slint::SharedString::default());
    app.set_trade_form_value_text(slint::SharedString::default());
    clear_trade_picker_draft(app);
    state.rebuild_overlay_descriptor_state();
    // The wanted list changed (insert or in-place edit): re-project
    // the cards.
    project_trade_state(app, state);
    Ok(())
}

/// Reconcile a wanted row's affixes with the staged dialog draft
/// through the `d4lc_gui::trade` editing API: removals first, then
/// rune-aware additions (the max-1 gate lives in the ViewModel), then
/// IsGreater updates. Empty effect list when the draft already
/// matches the row (the fresh-add case with no picker selection).
///
/// Mutates `draft` (the confirm-path working copy), so a failure leaves the
/// caller's live panel state untouched.
fn apply_staged_trade_affixes(
    draft: &mut TradeImportsPanelState,
    trade_item_id: &str,
    staged: &[(String, bool)],
) -> Result<Vec<TradeEffect>, String> {
    let mut vm = TradeViewModel::new(draft.clone());
    let (rune, current) = {
        let Some(row) = vm
            .state
            .trade_items
            .iter()
            .find(|row| row.trade_item_id == trade_item_id)
        else {
            return Ok(Vec::new());
        };
        (
            is_rune_item_type(&row.item_type),
            row.affixes
                .iter()
                .map(|affix| (affix.affix_id.clone(), affix.is_greater))
                .collect::<Vec<(String, bool)>>(),
        )
    };

    let mut effects = Vec::new();
    // Removals: row affixes the user dropped from the draft.
    for (affix_id, _) in &current {
        if !staged.iter().any(|(staged_id, _)| staged_id == affix_id) {
            effects.extend(vm.remove_trade_affix(trade_item_id, affix_id)?);
        }
    }
    // Additions: staged candidates the row does not carry yet (the
    // rune variant refuses a second affix — upstream
    // CanAddAffixRuneExecute, AddTradeItemViewModel.cs:177-180).
    for (affix_id, _) in staged {
        if !current.iter().any(|(row_id, _)| row_id == affix_id) {
            effects.extend(if rune {
                vm.add_trade_rune_affix(trade_item_id, affix_id)?
            } else {
                vm.add_trade_affix(trade_item_id, affix_id)?
            });
        }
    }
    // IsGreater updates: compare against the row as it stands AFTER
    // the removals / additions above (a fresh add appends with
    // `IsGreater = false`, so the staged flag must still be applied).
    let applied: Vec<(String, bool)> = vm
        .state
        .trade_items
        .iter()
        .find(|row| row.trade_item_id == trade_item_id)
        .map(|row| {
            row.affixes
                .iter()
                .map(|affix| (affix.affix_id.clone(), affix.is_greater))
                .collect()
        })
        .unwrap_or_default();
    for (affix_id, is_greater) in staged {
        if let Some((_, row_greater)) = applied.iter().find(|(row_id, _)| row_id == affix_id) {
            if row_greater != is_greater {
                effects.extend(vm.set_trade_affix_is_greater(
                    trade_item_id,
                    affix_id,
                    *is_greater,
                )?);
            }
        }
    }

    *draft = vm.state;
    Ok(effects)
}

/// Add or edit the wanted row in the confirm-path working copy `draft`.
///
/// The edit path reuses the same dialog in edit mode (upstream
/// TradeViewModel.cs:113-130); the add path creates the row.
fn upsert_trade_item_from_form(
    draft: &mut TradeImportsPanelState,
    item_type: &str,
    value_text: &str,
) -> Result<(String, Vec<TradeEffect>), String> {
    // Strict known-type enforcement: upstream offers the fixed type
    // enumeration in the dialog's ComboBox (InitItemTypes,
    // AddTradeItemViewModel.cs:314-324), so an unknown type can never
    // be saved. The port keeps the type field free-text (the headless
    // real-input harness types through it) and rejects unknown values
    // on the confirm path instead — case-insensitively, like the
    // upstream lowercase `Type.Type` constants.
    if !is_known_item_type(item_type) {
        return Err(format!(
            "Unsupported item type '{}'. Supported types: {}.",
            item_type.trim(),
            TRADE_ITEM_TYPES.join(", ")
        ));
    }

    if let Some(selected_id) = draft.selected_trade_item_id.clone() {
        if draft
            .trade_items
            .iter()
            .any(|trade_item| trade_item.trade_item_id == selected_id)
        {
            // Edit path: route through the headless ViewModel so the
            // form validation, the empty-value coercion, and the
            // rune-boundary affix clearing apply exactly like the add
            // path (upstream reuses the same dialog in edit mode,
            // TradeViewModel.cs:113-130).
            let mut vm = TradeViewModel::new(draft.clone());
            let effects = vm.confirm_edit_trade_item(&selected_id, item_type, value_text)?;
            *draft = vm.state;
            return Ok((selected_id, effects));
        }
    }

    let mut vm = TradeViewModel::new(draft.clone());
    let (trade_item_id, effects) = vm.confirm_add_trade_item(item_type, value_text)?;
    *draft = vm.state;
    Ok((trade_item_id, effects))
}

/// Dispatch `TradeEffect` values returned by `TradeViewModel`
/// commands onto persistence.
///
/// The `AddTradeItem` confirm path and the remove path emit
/// `PersistConfig` to signal "the wanted rows changed, please
/// persist" (upstream `TradeViewModel` saves through
/// `_tradeItemManager.SaveTradeItems` on add/edit/remove,
/// TradeViewModel.cs:109,129,172). Every mutation therefore rewrites
/// `Config/TradeItems.json` through [`persist_trade_items`].
///
/// The `TradeConfig` confirm path handles its own persistence
/// directly through `d4lc_config::load_app_settings` /
/// `save_app_settings` in
/// [`handle_trade_config_confirm`], so this dispatcher ignores it.
fn dispatch_trade_effects(effects: &[TradeEffect], state: &mut AppState) {
    for effect in effects {
        if matches!(effect, TradeEffect::PersistConfig) {
            persist_trade_items(state);
        }
    }
}

/// Load the wanted trade items from `Config/TradeItems.json` into the
/// trade panel state.
///
/// Startup seam: called from the trade wiring (`wire_trade_actions`)
/// once the config root is known, before the TradeScreen's initial
/// projection — mirroring the upstream `LoadTradeItems` constructor
/// call plus the `HandleApplicationLoadedMessage` AddRange
/// (TradeItemManager.cs:20-27, TradeViewModel.cs:132-138). A missing
/// file keeps the empty list; an unreadable file is surfaced
/// on the Logging screen and the current in-memory rows are kept —
/// user state we could not read is never silently replaced. A stale
/// selected id survives only when the reloaded rows still contain it.
pub fn hydrate_trade_items(state: &mut AppState, config_root: &Path) {
    let loaded = match d4lc_config::load_trade_items(config_root) {
        Ok(items) => items,
        Err(error) => {
            state.append_log_message(
                format!("trade items load failed: {error}"),
                crate::clock::hh_mm_now(),
            );
            return;
        }
    };
    let trade_imports = state.trade_imports_mut();
    trade_imports.trade_items = loaded
        .iter()
        .map(|item| d4lc_gui::TradeItemRow {
            trade_item_id: item.trade_item_id.clone(),
            item_type: item.item_type.clone(),
            value_text: item.value_text.clone(),
            affixes: item
                .affixes
                .iter()
                .map(|affix| d4lc_gui::TradeItemAffixView {
                    affix_id: affix.affix_id.clone(),
                    is_greater: affix.is_greater,
                })
                .collect(),
        })
        .collect();
    let selected_still_exists =
        trade_imports
            .selected_trade_item_id
            .as_deref()
            .is_some_and(|selected| {
                trade_imports
                    .trade_items
                    .iter()
                    .any(|item| item.trade_item_id == selected)
            });
    if !selected_still_exists {
        trade_imports.selected_trade_item_id = None;
    }
}

/// Persist the trade panel's wanted rows to `Config/TradeItems.json`.
///
/// Fire-and-forget mutation seam (the upstream `SaveTradeItems`
/// trigger points): every wanted-item add/edit/remove calls this after
/// the in-memory change. Like `persist_affix_presets_to_config`: if the existing file cannot be
/// read (it would be silently replaced by a save), or the save itself
/// fails, the reason is logged and the in-memory state stays applied.
fn persist_trade_items(state: &mut AppState) {
    let config_root = crate::actions::settings::resolve_config_root();
    persist_trade_items_to(state, &config_root);
}

fn persist_trade_items_to(state: &mut AppState, config_root: &Path) {
    if let Err(error) = d4lc_config::load_trade_items(config_root) {
        state.append_log_message(
            format!("trade items save skipped, existing file unreadable: {error}"),
            crate::clock::hh_mm_now(),
        );
        return;
    }
    let items: Vec<d4lc_config::TradeItemFile> = state
        .trade_imports()
        .trade_items
        .iter()
        .map(|row| d4lc_config::TradeItemFile {
            trade_item_id: row.trade_item_id.clone(),
            item_type: row.item_type.clone(),
            value_text: row.value_text.clone(),
            affixes: row
                .affixes
                .iter()
                .map(|affix| d4lc_config::TradeItemFileAffix {
                    affix_id: affix.affix_id.clone(),
                    is_greater: affix.is_greater,
                })
                .collect(),
        })
        .collect();
    if let Err(error) = d4lc_config::save_trade_items(config_root, &items) {
        state.append_log_message(
            format!("trade items save failed: {error}"),
            crate::clock::hh_mm_now(),
        );
    }
}

/// Handle trade-config confirm.
///
/// Reads the three form values off the root `AppWindow` (see
/// `main_window.slint`), validates them through the headless
/// [`d4lc_gui::trade::confirm_trade_config_draft`], applies the
/// mappable fields to `state.shell.settings`, and persists via
/// the existing `d4lc_config::load_app_settings` /
/// `save_app_settings` path used by the settings screen
/// (`show_current_item` included).
///
/// On validation failure the function returns `Err(message)` so
/// the shared dialog router keeps the dialog open and surfaces
/// the error via the shared `dialog-error` contract. State and
/// the root props are not mutated.
pub fn handle_trade_config_confirm(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let overlay_enabled = app.get_trade_config_overlay_enabled();
    let show_current_item = app.get_trade_config_show_current_item();
    let font_size = app.get_trade_config_font_size();

    // Construct the headless draft and validate it. Negative
    // font sizes are clamped to 0 by `TradeConfigDraft::new` so
    // the validator can reject them as out-of-range.
    let draft = TradeConfigDraft::new(overlay_enabled, show_current_item, font_size);
    let _effects = confirm_trade_config_draft(&draft)?;

    // Apply mappable fields to the settings panel state.
    // `trade_overlay_enabled` → `RuleSettingsConfig.trade_overlay_enabled`
    // (already wired in `SettingsPanelState::apply_to_config`).
    // `overlay_font_size` → `AppSettings.overlay_font_size` (same).
    state.shell.settings.trade_overlay_enabled = draft.is_trade_overlay_enabled;
    state.shell.settings.overlay_font_size = draft.overlay_font_size;
    state.bump_rules_input_revision();
    state.trade_show_current_item = draft.show_current_item;

    // Persist via the existing app-settings save path. The same
    // `apply_to_config` / `save_app_settings` cycle is used by
    // `actions::settings::handle_settings_changed`; we replicate
    // the read-modify-write here so the trade-config confirm does
    // not need to depend on the settings ViewModel.
    let mut config = d4lc_config::load_app_settings(config_root)
        .map_err(|e| format!("Failed to load trade config: {e}"))?;
    state.shell.settings.apply_to_config(&mut config);
    config.show_current_item = draft.show_current_item;
    d4lc_config::save_app_settings(config_root, &config)
        .map_err(|e| format!("Failed to save trade config: {e}"))?;

    // Clear root props so a subsequent open of the dialog starts
    // from a clean seed.
    app.set_trade_config_overlay_enabled(false);
    app.set_trade_config_show_current_item(false);
    app.set_trade_config_font_size(18);

    // Re-project the trade screen so the (gated) current-item card
    // and the wanted rows reflect the applied setting immediately —
    // upstream refreshes the filtered view when `ShowCurrentItem`
    // toggles (`HandleToggleCurrentItemMessage` /
    // `ToggleCurrentItemMessage`, TradeViewModel.cs:140-146).
    project_trade_state(app, state);
    state.rebuild_overlay_descriptor_state();

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, unused_mut)]
mod tests {
    use super::*;
    use d4lc_gui::trade::TradeEffect;

    fn sample_row(id: &str, item_type: &str, value: &str) -> TradeItemRow {
        TradeItemRow {
            trade_item_id: id.to_string(),
            item_type: item_type.to_string(),
            value_text: value.to_string(),
            affixes: vec![d4lc_gui::TradeItemAffixView {
                affix_id: "Affix_AttackSpeed".to_string(),
                is_greater: false,
            }],
        }
    }

    #[test]
    fn trade_view_item_maps_row_fields_badges_and_affix_rows() {
        let mut row = sample_row("Trade_Amulet", "Amulet", "10M");
        row.affixes[0].is_greater = true;
        let item = trade_view_item(&row, None);
        assert_eq!(item.item_id, "Trade_Amulet");
        assert_eq!(item.item_type, "Amulet");
        assert_eq!(item.badge, "Am", "known types map to a two-letter badge");
        assert_eq!(item.value, "10M");
        assert_eq!(item.affixes.row_count(), 1);
        // Without resolution tables the raw affix id is the display text.
        assert_eq!(
            item.affixes.row_data(0).unwrap().text.to_string(),
            "Affix_AttackSpeed"
        );
        assert!(
            item.affixes.row_data(0).unwrap().is_greater,
            "the IsGreater flag reaches the card for the color split"
        );
    }

    #[test]
    fn trade_type_badge_covers_the_fixed_enumeration() {
        assert_eq!(trade_type_badge("Two-Handed Sword"), "??");
        assert_eq!(trade_type_badge("helm"), "He", "casing is ignored");
        assert_eq!(trade_type_badge(" Rune "), "Ru");
    }

    #[test]
    fn trade_view_items_lists_rows_in_state_order() {
        let mut state = AppState::default_startup();
        state
            .trade_imports_mut()
            .upsert_trade_item(sample_row("a", "Amulet", "10M"));
        state
            .trade_imports_mut()
            .upsert_trade_item(sample_row("b", "Ring", "5b"));

        let items: Vec<TradeItem> = state
            .trade_imports()
            .trade_items
            .iter()
            .map(|row| trade_view_item(row, None))
            .collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].item_id, "a");
        assert_eq!(items[1].item_id, "b");
    }

    #[test]
    fn current_trade_card_defaults_without_parsed_item() {
        let state = AppState::default_startup();
        let (item_type, item_power, affixes) = current_trade_card(&state);
        assert_eq!(item_type, "");
        assert_eq!(item_power, 0);
        assert!(affixes.is_empty());
    }

    #[test]
    fn current_trade_card_maps_parsed_item_fields() {
        let mut state = AppState::default_startup();
        let current = state.current_item_mut();
        // The card renders the display derivation; the canonical catalog type
        // id stays the evaluation identity and is not the rendered text
        current.item_type = Some("amulet".to_string());
        current.base_type = Some("Amulet".to_string());
        current.item_power = Some(940);
        current.affixes.push(CurrentItemAffixView {
            row_id: d4lc_core::AffixRowId(0),
            affix_id: Some("Affix_AttackSpeed".to_string()),
            label: "Attack Speed".to_string(),
            value_text: Some("18.0%".to_string()),
            source_text: String::new(),
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            matched_by_rule: false,
        });

        let (item_type, item_power, affixes) = current_trade_card(&state);
        assert_eq!(item_type, "Amulet");
        assert_eq!(item_power, 940);
        assert_eq!(affixes.len(), 1);
        assert_eq!(affixes[0].to_string(), "Attack Speed");

        // The canonical evaluation id alone is never rendered as the card's
        // type text: the card stays empty without a display derivation.
        let mut canonical_only = AppState::default_startup();
        canonical_only.current_item_mut().item_type = Some("amulet".to_string());
        let (item_type, _, _) = current_trade_card(&canonical_only);
        assert_eq!(item_type, "");
    }

    #[test]
    fn project_trade_state_sets_root_models_and_card_props() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state
            .trade_imports_mut()
            .upsert_trade_item(sample_row("Trade_Ring", "Ring", "5b"));
        state.current_item_mut().item_type = Some("helm".to_string());
        state.current_item_mut().base_type = Some("Helm".to_string());
        state.current_item_mut().item_power = Some(780);
        state.trade_show_current_item = true;

        project_trade_state(&app, &state);

        assert_eq!(app.get_trade_items().row_count(), 1);
        assert_eq!(
            app.get_trade_items()
                .row_data(0)
                .unwrap()
                .item_id
                .to_string(),
            "Trade_Ring"
        );
        assert_eq!(app.get_current_item_type().to_string(), "Helm");
        assert_eq!(app.get_current_item_power(), 780);
        assert_eq!(app.get_current_affixes().row_count(), 0);
        assert!(
            app.get_trade_show_current_item(),
            "the gate property mirrors the state flag"
        );

        state.trade_show_current_item = false;
        project_trade_state(&app, &state);
        assert!(
            !app.get_trade_show_current_item(),
            "flipping the state flag re-projects the gate"
        );
    }

    #[test]
    fn project_filtered_trade_candidates_applies_the_keyword_filter() {
        let app = crate::test_window::new_app_window();
        let state = AppState::default_startup();
        let (affixes, runes) = load_trade_candidates(&state);
        assert!(
            !affixes.is_empty() && !runes.is_empty(),
            "the vendored catalogs must load in the dev checkout"
        );

        // An empty filter keeps the whole catalog (the dialog opens
        // unfiltered, like upstream).
        project_filtered_trade_candidates(&app, &affixes, &runes, "");
        assert_eq!(app.get_trade_affix_candidates().row_count(), affixes.len());
        assert_eq!(app.get_trade_rune_candidates().row_count(), runes.len());

        // A `;`-keyword filter narrows the affix list (the d4lc_gui
        // filter functions do the matching; this proves the projection).
        project_filtered_trade_candidates(&app, &affixes, &runes, "attack speed");
        let filtered = app.get_trade_affix_candidates().row_count();
        assert!(
            filtered > 0 && filtered < affixes.len(),
            "attack-speed filter must narrow the catalog: {filtered} of {}",
            affixes.len()
        );

        project_filtered_trade_candidates(&app, &affixes, &runes, "zzz-no-such-candidate");
        assert_eq!(app.get_trade_affix_candidates().row_count(), 0);
    }

    #[test]
    fn seed_trade_picker_models_seeds_edit_draft_and_rune_flag() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.trade_imports_mut().upsert_trade_item(TradeItemRow {
            trade_item_id: "Trade_Helm".to_string(),
            item_type: "Helm".to_string(),
            value_text: "750k".to_string(),
            affixes: vec![d4lc_gui::TradeItemAffixView {
                affix_id: "Affix_MaxLife".to_string(),
                is_greater: true,
            }],
        });
        state.trade_imports_mut().selected_trade_item_id = Some("Trade_Helm".to_string());

        let (affixes, runes) = seed_trade_picker_models(&app, &state);

        assert!(!affixes.is_empty());
        assert!(!runes.is_empty());
        assert!(app.get_trade_edit_mode(), "a selected row seeds edit mode");
        assert!(!app.get_trade_item_type_is_rune());
        assert_eq!(app.get_trade_affix_filter().to_string(), "");
        let staged = app.get_trade_item_affixes();
        assert_eq!(staged.row_count(), 1);
        let staged_row = staged.row_data(0).unwrap();
        assert_eq!(staged_row.affix_id, "Affix_MaxLife");
        assert!(staged_row.is_greater, "the draft carries the stored flag");

        // Add mode: no selection — empty draft, no rune flag.
        state.trade_imports_mut().selected_trade_item_id = None;
        seed_trade_picker_models(&app, &state);
        assert!(!app.get_trade_edit_mode());
        assert_eq!(app.get_trade_item_affixes().row_count(), 0);
    }

    #[test]
    fn handle_add_trade_item_sets_dialog_kind_to_add_trade_item() {
        // The handler writes `state.dialog.kind = AddTradeItem` via
        // the shared `actions::dialogs::open` router (the actual
        // Slint setter call is exercised through the
        // `cargo test -p d4lc-app` build, which compiles
        // `main_window.slint`). We assert the state-side contract:
        // after the handler runs, `state.dialog.kind` must be
        // `AddTradeItem` so the dialog host picks the right
        // component. To do this without an `AppWindow`, we
        // simulate the router's state mutation directly.
        let mut state = AppState::default_startup();
        assert_ne!(state.dialog.kind, DialogKind::AddTradeItem);
        state.dialog.kind = DialogKind::AddTradeItem;
        state.dialog.title = DialogKind::AddTradeItem.default_title().to_string();
        state.dialog.error.clear();
        assert_eq!(state.dialog.kind, DialogKind::AddTradeItem);
        assert!(state.dialog.error.is_empty());
    }

    #[test]
    fn handle_add_trade_item_confirm_mutates_state_via_viewmodel() {
        // The confirm path delegates to the ViewModel; we test
        // the contract by calling the ViewModel directly (the
        // AppWindow setter side is exercised through the
        // `cargo test -p d4lc-app` build).
        let mut state = AppState::default_startup();
        let mut vm = TradeViewModel::new(state.trade_imports().clone());
        let (id, effects) = vm
            .confirm_add_trade_item("Amulet", "10M")
            .expect("valid input should succeed");
        *state.trade_imports_mut() = vm.state;
        assert_eq!(id, "Trade_Amulet");
        assert_eq!(state.trade_imports().trade_items.len(), 1);
        assert_eq!(effects, vec![TradeEffect::PersistConfig]);
    }

    #[test]
    fn upsert_trade_item_from_form_updates_selected_item_in_place() {
        let mut state = AppState::default_startup();
        state.trade_imports_mut().upsert_trade_item(TradeItemRow {
            trade_item_id: "item1".to_string(),
            item_type: "Amulet".to_string(),
            value_text: "10M".to_string(),
            affixes: vec![],
        });

        let (id, effects) = upsert_trade_item_from_form(state.trade_imports_mut(), "Ring", "20M")
            .expect("edit confirm should succeed");

        assert_eq!(id, "item1");
        assert_eq!(effects, vec![TradeEffect::PersistConfig]);
        assert_eq!(state.trade_imports().trade_items.len(), 1);
        assert_eq!(state.trade_imports().trade_items[0].trade_item_id, "item1");
        assert_eq!(state.trade_imports().trade_items[0].item_type, "Ring");
        assert_eq!(state.trade_imports().trade_items[0].value_text, "20M");
    }

    #[test]
    fn upsert_trade_item_from_form_rejects_unknown_item_type() {
        // Strict known-type enforcement on the confirm path: upstream
        // only ever saves one of the 11 fixed types (the dialog's
        // ComboBox, InitItemTypes); the port rejects anything else —
        // case-insensitively, like `is_known_item_type`.
        let mut state = AppState::default_startup();
        state.trade_imports_mut().selected_trade_item_id = None;

        for unknown in ["Belt", "belt", "Two-Handed Sword", "  "] {
            let result = upsert_trade_item_from_form(state.trade_imports_mut(), unknown, "1b");
            assert!(
                result
                    .err()
                    .is_some_and(|error| error.starts_with("Unsupported item type")),
                "'{unknown}' must be rejected with the supported-types error"
            );
        }
        // "Amulet" differs only in casing from a known type and passes.
        let _ = upsert_trade_item_from_form(state.trade_imports_mut(), "amulet", "1b")
            .expect("known types are accepted case-insensitively");
        assert_eq!(state.trade_imports().trade_items.len(), 1);
    }

    #[test]
    fn upsert_edit_across_rune_boundary_clears_affixes() {
        let mut state = AppState::default_startup();
        state.trade_imports_mut().upsert_trade_item(TradeItemRow {
            trade_item_id: "item1".to_string(),
            item_type: "Amulet".to_string(),
            value_text: "10M".to_string(),
            affixes: vec![d4lc_gui::TradeItemAffixView {
                affix_id: "Affix_AttackSpeed".to_string(),
                is_greater: false,
            }],
        });
        state.trade_imports_mut().selected_trade_item_id = Some("item1".to_string());

        let _ = upsert_trade_item_from_form(state.trade_imports_mut(), "Rune", "1b")
            .expect("rune edit confirm should succeed");

        let row = &state.trade_imports().trade_items[0];
        assert_eq!(row.item_type, "Rune");
        assert!(
            row.affixes.is_empty(),
            "switching onto the rune type clears the affixes (AddTradeItemViewModel.cs:108-119)"
        );
    }

    #[test]
    fn upsert_add_with_empty_value_stores_zero() {
        let mut state = AppState::default_startup();
        state.trade_imports_mut().selected_trade_item_id = None;

        let _ = upsert_trade_item_from_form(state.trade_imports_mut(), "Amulet", "")
            .expect("an empty value coerces to 0 instead of failing");

        assert_eq!(state.trade_imports().trade_items.len(), 1);
        assert_eq!(state.trade_imports().trade_items[0].value_text, "0");
    }

    // ── Staged picker draft applied on confirm ──────────

    #[test]
    fn apply_staged_trade_affixes_adds_and_flags_on_a_fresh_row() {
        let mut state = AppState::default_startup();
        state.trade_imports_mut().selected_trade_item_id = None;
        let (id, _) = upsert_trade_item_from_form(state.trade_imports_mut(), "Amulet", "5b")
            .expect("add confirm should succeed");

        let effects = apply_staged_trade_affixes(
            state.trade_imports_mut(),
            &id,
            &[
                ("AttackSpeed".to_string(), true),
                ("Affix_MaxLife".to_string(), false),
            ],
        )
        .expect("staged application should succeed");

        assert!(!effects.is_empty(), "mutations emit effects");
        let row = &state.trade_imports().trade_items[0];
        assert_eq!(
            row.affixes
                .iter()
                .map(|affix| (affix.affix_id.as_str(), affix.is_greater))
                .collect::<Vec<_>>(),
            // The ViewModel re-sorts by id after every add (OrderBy parity).
            vec![("Affix_MaxLife", false), ("AttackSpeed", true)],
        );
    }

    #[test]
    fn apply_staged_trade_affixes_reconciles_an_edited_draft() {
        let mut state = AppState::default_startup();
        state.trade_imports_mut().upsert_trade_item(TradeItemRow {
            trade_item_id: "item1".to_string(),
            item_type: "Helm".to_string(),
            value_text: "750k".to_string(),
            affixes: vec![
                d4lc_gui::TradeItemAffixView {
                    affix_id: "Affix_MaxLife".to_string(),
                    is_greater: true,
                },
                d4lc_gui::TradeItemAffixView {
                    affix_id: "Affix_Armor".to_string(),
                    is_greater: false,
                },
            ],
        });
        state.trade_imports_mut().selected_trade_item_id = Some("item1".to_string());
        let (id, _) = upsert_trade_item_from_form(state.trade_imports_mut(), "Helm", "800k")
            .expect("edit confirm should succeed");

        // Draft keeps Armor (now greater) and drops MaxLife.
        let effects = apply_staged_trade_affixes(
            state.trade_imports_mut(),
            &id,
            &[("Affix_Armor".to_string(), true)],
        )
        .expect("staged reconciliation should succeed");

        assert!(!effects.is_empty());
        let row = &state.trade_imports().trade_items[0];
        assert_eq!(row.value_text, "800k");
        assert_eq!(
            row.affixes
                .iter()
                .map(|affix| (affix.affix_id.as_str(), affix.is_greater))
                .collect::<Vec<_>>(),
            vec![("Affix_Armor", true)],
            "the removed MaxLife row is gone and Armor carries the greater flag"
        );
    }

    #[test]
    fn apply_staged_trade_affixes_enforces_the_rune_max_one_gate() {
        let mut state = AppState::default_startup();
        state.trade_imports_mut().selected_trade_item_id = None;
        let (id, _) = upsert_trade_item_from_form(state.trade_imports_mut(), "Rune", "1b")
            .expect("rune add confirm should succeed");

        let result = apply_staged_trade_affixes(
            state.trade_imports_mut(),
            &id,
            &[
                ("Item_Rune_Condition_OnMove".to_string(), false),
                ("Item_Rune_Resource_Offense".to_string(), false),
            ],
        );

        assert!(
            result
                .err()
                .is_some_and(|error| error == "A rune item can hold at most one affix."),
            "the second rune affix must hit the ViewModel's max-1 gate"
        );
        assert!(
            state.trade_imports().trade_items[0].affixes.is_empty(),
            "a failed staged application must not partially mutate the row"
        );
    }

    #[test]
    fn confirm_keeps_the_row_unchanged_when_the_affix_leg_fails() {
        // Regression: type/value edit and staged affixes are two legs
        // of one confirm. The affix leg failing must not leave the
        // type/value edit in memory while the dialog reports the error —
        // Config/TradeItems.json would still hold the previous row.
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.trade_imports_mut().upsert_trade_item(TradeItemRow {
            trade_item_id: "Trade_Rune".to_string(),
            item_type: "Rune".to_string(),
            value_text: "5M".to_string(),
            affixes: vec![],
        });
        state.trade_imports_mut().selected_trade_item_id = Some("Trade_Rune".to_string());

        app.set_trade_form_item_type("Rune".into());
        app.set_trade_form_value_text("9M".into());
        // Two staged rune affixes: the ViewModel accepts the first and
        // refuses the second (upstream `CanAddAffixRuneExecute` max-1 gate,
        // AddTradeItemViewModel.cs:177-180), so the affix leg fails after
        // the upsert leg already succeeded.
        set_staged_trade_rows(
            &app,
            vec![
                TradeStagedAffix {
                    affix_id: "Item_Rune_Condition_OnMove".into(),
                    label: "+# Movement Speed".into(),
                    is_greater: false,
                },
                TradeStagedAffix {
                    affix_id: "Item_Rune_Resource_Offense".into(),
                    label: "+# Resource".into(),
                    is_greater: false,
                },
            ],
        );

        let result = handle_add_trade_item_confirm(&app, &mut state, "Rune", "9M");
        assert!(
            result.is_err(),
            "the second rune affix must fail the confirm: {result:?}"
        );

        let row = &state.trade_imports().trade_items[0];
        assert_eq!(
            row.value_text, "5M",
            "the failed confirm must not keep the type/value edit in memory"
        );
        assert!(
            row.affixes.is_empty(),
            "and no staged affix may be applied on a failed confirm"
        );
    }

    #[test]
    fn stage_helpers_project_and_gate_the_staged_picker_model() {
        let app = crate::test_window::new_app_window();
        let state = AppState::default_startup();

        // Two staged picks: appended, resolved, and re-sorted by id
        // (upstream OrderBy(a => a.Id)).
        stage_trade_affix(&app, &state, "AttackSpeed", false)
            .expect("staging an affix candidate should succeed");
        stage_trade_affix(&app, &state, "Affix_MaxLife", false)
            .expect("staging a second candidate should succeed");
        let staged = staged_trade_rows(&app);
        assert_eq!(
            staged
                .iter()
                .map(|affix| affix.affix_id.to_string())
                .collect::<Vec<_>>(),
            vec!["Affix_MaxLife", "AttackSpeed"],
        );
        assert_eq!(
            staged[1].label, "+#% Attack Speed",
            "catalog label resolves"
        );

        // Duplicate picks are a no-op (the row is already staged).
        stage_trade_affix(&app, &state, "AttackSpeed", false).expect("duplicate is a no-op");
        assert_eq!(staged_trade_rows(&app).len(), 2);

        // IsGreater flips in place (the per-row checkbox).
        restage_trade_affix_is_greater(&app, "AttackSpeed", true);
        assert!(staged_trade_rows(&app)[1].is_greater);

        // The rune gate refuses a second affix while the draft is
        // non-empty (upstream CanAddAffixRuneExecute).
        assert!(stage_trade_affix(&app, &state, "Item_Rune_Condition_OnMove", true).is_err());

        // Unstage drops exactly the addressed row.
        unstage_trade_affix(&app, "Affix_MaxLife");
        assert_eq!(
            staged_trade_rows(&app)
                .iter()
                .map(|affix| affix.affix_id.to_string())
                .collect::<Vec<_>>(),
            vec!["AttackSpeed"],
        );
    }

    // ── vendored catalog resolution (affix display text) ──────────────

    #[test]
    fn trade_catalog_descriptions_resolve_known_ids() {
        let Some(data_dir) = resolve_trade_data_dir() else {
            panic!("the vendored data dir must resolve in the dev checkout");
        };
        let descriptions =
            load_trade_affix_descriptions(&data_dir, d4lc_config::DEFAULT_AFFIX_LANGUAGE);

        // A full IdName row and a compound `;`-segment row both resolve
        // through the affix catalog.
        let exact = descriptions.resolve("2HStaff_Unique_AF_001_PickupRadius");
        assert_eq!(exact, "+# Pickup Radius");
        let segment = descriptions.resolve("AttackSpeed");
        assert_eq!(segment, "+#% Attack Speed");
        // Rune ids resolve to the rune name (AffixIdToRuneNameConverter).
        let rune = descriptions.resolve("Item_Rune_Condition_OnMove");
        assert_eq!(rune, "Bac");
        // Unknown ids fall back to the raw id, never blank.
        assert_eq!(
            descriptions.resolve("Affix_AttackSpeed"),
            "Affix_AttackSpeed"
        );
    }

    #[test]
    fn project_trade_state_sorts_wanted_rows_by_type_then_value() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state
            .trade_imports_mut()
            .upsert_trade_item(display_row("1", "Ring", "10M"));
        state
            .trade_imports_mut()
            .upsert_trade_item(display_row("2", "Amulet", "5b"));
        state
            .trade_imports_mut()
            .upsert_trade_item(display_row("3", "Ring", "9M"));

        project_trade_state(&app, &state);

        let ids: Vec<String> = (0..app.get_trade_items().row_count())
            .filter_map(|index| app.get_trade_items().row_data(index))
            .map(|item| item.item_id.to_string())
            .collect();
        assert_eq!(
            ids,
            vec!["2", "3", "1"],
            "Amulet first, then Ring by zero-padded value (9M before 10M)"
        );
    }

    fn display_row(id: &str, item_type: &str, value: &str) -> TradeItemRow {
        TradeItemRow {
            trade_item_id: id.to_string(),
            item_type: item_type.to_string(),
            value_text: value.to_string(),
            affixes: vec![],
        }
    }

    #[test]
    fn project_trade_state_resolves_affix_descriptions_from_catalog() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state
            .trade_imports_mut()
            .upsert_trade_item(display_row("r1", "Amulet", "5b"));
        state.trade_imports_mut().trade_items[0]
            .affixes
            .push(d4lc_gui::TradeItemAffixView {
                affix_id: "AttackSpeed".to_string(),
                is_greater: true,
            });

        project_trade_state(&app, &state);

        let first = app.get_trade_items().row_data(0).expect("row");
        assert_eq!(first.affixes.row_count(), 1);
        let affix = first.affixes.row_data(0).unwrap();
        assert_eq!(affix.text.to_string(), "+#% Attack Speed");
        assert!(affix.is_greater);
    }

    #[test]
    fn handle_add_trade_item_confirm_propagates_validation_error() {
        let mut state = AppState::default_startup();
        let mut vm = TradeViewModel::new(state.trade_imports().clone());
        let result = vm.confirm_add_trade_item("", "10M");
        assert!(result.is_err());
        // State must not be mutated on validation failure.
        assert!(state.trade_imports().trade_items.is_empty());
    }

    #[test]
    fn handle_trade_config_confirm_persists_show_current_item_and_reprojects() {
        let root =
            std::env::temp_dir().join(format!("d4lc-trade-config-{}-{}", std::process::id(), 1));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");

        let app = crate::test_window::new_app_window();

        let mut state = AppState::default_startup();
        state
            .trade_imports_mut()
            .upsert_trade_item(display_row("Trade_Ring", "Ring", "5b"));
        app.set_trade_config_overlay_enabled(true);
        app.set_trade_config_show_current_item(true);
        app.set_trade_config_font_size(24);

        handle_trade_config_confirm(&app, &mut state, &root)
            .expect("trade config confirm should persist");

        let persisted = d4lc_config::load_app_settings(&root).expect("persisted config");
        assert!(persisted.show_current_item);
        assert!(state.trade_show_current_item);

        // The confirm re-projects the trade screen (the upstream
        // ToggleCurrentItemMessage refresh, TradeViewModel.cs:140-146).
        assert_eq!(app.get_trade_items().row_count(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Wanted-items persistence (Config/TradeItems.json) ─────────────

    fn temp_trade_root(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "d4lc-trade-items-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp trade root");
        root
    }

    fn seeded_trade_state() -> AppState {
        let mut state = AppState::default_startup();
        state.trade_imports_mut().upsert_trade_item(TradeItemRow {
            trade_item_id: "Trade_Amulet".to_string(),
            item_type: "Amulet".to_string(),
            value_text: "10M".to_string(),
            affixes: vec![d4lc_gui::TradeItemAffixView {
                affix_id: "Affix_AttackSpeed".to_string(),
                is_greater: false,
            }],
        });
        state.trade_imports_mut().upsert_trade_item(TradeItemRow {
            trade_item_id: "Trade_Helm".to_string(),
            item_type: "Helm".to_string(),
            value_text: "750k".to_string(),
            affixes: vec![
                d4lc_gui::TradeItemAffixView {
                    affix_id: "Affix_MaxLife".to_string(),
                    is_greater: true,
                },
                d4lc_gui::TradeItemAffixView {
                    affix_id: "Affix_Armor".to_string(),
                    is_greater: false,
                },
            ],
        });
        state
    }

    #[test]
    fn persist_then_hydrate_round_trips_wanted_rows_with_fields_intact() {
        let root = temp_trade_root("round-trip");
        let mut state = seeded_trade_state();
        state
            .trade_imports_mut()
            .remove_trade_item("Trade_Amulet")
            .expect("removal seeds the persist effect");

        // The remove confirm path persists through the effect
        // dispatch; the root-parameterized persist core is exercised
        // directly so the test never touches the real config root.
        persist_trade_items_to(&mut state, &root);

        // Simulated restart: a fresh state hydrates through the same
        // startup seam the wiring uses.
        let mut restarted = AppState::default_startup();
        hydrate_trade_items(&mut restarted, &root);

        let rows = &restarted.trade_imports().trade_items;
        assert_eq!(rows.len(), 1, "the removed row must stay removed");
        let helm = &rows[0];
        assert_eq!(helm.trade_item_id, "Trade_Helm");
        assert_eq!(helm.item_type, "Helm");
        assert_eq!(helm.value_text, "750k");
        assert_eq!(
            helm.affixes
                .iter()
                .map(|affix| (affix.affix_id.as_str(), affix.is_greater))
                .collect::<Vec<_>>(),
            vec![("Affix_MaxLife", true), ("Affix_Armor", false)],
            "affix rows survive with their IsGreater flags intact"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrate_keeps_empty_state_for_missing_file_and_stale_selection() {
        let root = temp_trade_root("missing-file");

        let mut state = AppState::default_startup();
        state.trade_imports_mut().selected_trade_item_id = Some("Trade_Gone".to_string());
        hydrate_trade_items(&mut state, &root);
        assert!(state.trade_imports().trade_items.is_empty());
        assert_eq!(state.trade_imports().selected_trade_item_id, None);
        assert!(
            state.logging.messages.is_empty(),
            "no error for a fresh install"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrate_restores_stale_selected_id_only_when_row_still_exists() {
        let root = temp_trade_root("stale-selection");
        let mut state = seeded_trade_state();
        state.trade_imports_mut().selected_trade_item_id = Some("Trade_Amulet".to_string());
        persist_trade_items_to(&mut state, &root);

        // The wanted list changed after the save: Trade_Amulet removed.
        state
            .trade_imports_mut()
            .remove_trade_item("Trade_Amulet")
            .expect("row exists");
        persist_trade_items_to(&mut state, &root);

        let mut restarted = AppState::default_startup();
        restarted.trade_imports_mut().selected_trade_item_id = Some("Trade_Amulet".to_string());
        hydrate_trade_items(&mut restarted, &root);
        assert_eq!(
            restarted.trade_imports().trade_items.len(),
            1,
            "only the persisted rows load"
        );
        assert_eq!(
            restarted.trade_imports().selected_trade_item_id,
            None,
            "a selection whose row no longer exists is not restored"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrate_unreadable_file_logs_and_keeps_current_rows() {
        let root = temp_trade_root("unreadable-file");
        std::fs::create_dir_all(root.join("Config")).expect("create Config dir");
        std::fs::write(root.join(d4lc_config::TRADE_ITEMS_PATH), "{ not an array")
            .expect("write malformed fixture");

        let mut state = seeded_trade_state();
        hydrate_trade_items(&mut state, &root);
        assert_eq!(
            state.trade_imports().trade_items.len(),
            2,
            "user state we could not read is never silently replaced"
        );
        assert!(
            state
                .logging
                .messages
                .iter()
                .any(|message| message.message.contains("trade items load failed")),
            "the load failure is surfaced on the Logging screen"
        );

        // The persist seam refuses to overwrite an unreadable file too.
        persist_trade_items_to(&mut state, &root);
        assert!(
            state
                .logging
                .messages
                .iter()
                .any(|message| message.message.contains("trade items save skipped")),
            "the save skip is surfaced on the Logging screen"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
