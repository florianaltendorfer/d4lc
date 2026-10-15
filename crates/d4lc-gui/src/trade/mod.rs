//! Trade ViewModel — pure state + command methods returning typed effects.
//!
//! ## Architecture
//!
//! `TradeViewModel` wraps [`TradeImportsPanelState`] and provides command
//! methods that return `Vec<TradeEffect>`. The `d4lc-app` crate interprets
//! these effects.
//!
//! ## Item editing surface (upstream `AddTradeItemViewModel.cs`)
//!
//! - [`TRADE_ITEM_TYPES`] is the fixed 11-entry trade item-type
//!   enumeration upstream builds in `InitItemTypes`
//!   (AddTradeItemViewModel.cs:314-324); [`is_known_item_type`]
//!   recognizes entries case-insensitively (upstream `Type.Type`
//!   constants are lowercase, `ItemTypeConstants.cs:5-17`).
//! - Rune items allow at most one affix — upstream
//!   `CanAddAffixRuneExecute` gates the add on
//!   `TradeItem.Affixes.Count == 0` (AddTradeItemViewModel.cs:177-180).
//! - Switching the item type across the rune boundary clears the
//!   affixes (upstream `SelectedItemType` setter,
//!   AddTradeItemViewModel.cs:108-119).
//! - Affix adds append with `IsGreater = false` and re-sort the list
//!   by id (upstream `AddAffixExecute` / `AddAffixRuneExecute` +
//!   `OrderBy(a => a.Id)`, AddTradeItemViewModel.cs:158-201). Upstream
//!   hardcodes the added affix's `Type` to the Amulet constant
//!   (AddTradeItemViewModel.cs:167); the d4lc `TradeItemAffixView`
//!   row has no type field, so that quirk has no carrier here.
//! - The affix candidate filter splits on `;` and requires every
//!   non-blank keyword to be contained case-insensitively in the
//!   description (name for runes) — upstream `FilterAffixes` /
//!   `FilterAffixesRunes` (AddTradeItemViewModel.cs:248-297).
//!
//! ## Boundary rules
//!
//! - No Slint handles, no file I/O, no platform state, no runtime handles.
//! - All side effects are returned as typed [`TradeEffect`] values.

use std::cmp::Ordering;

use crate::{TradeImportsPanelState, TradeItemAffixView, TradeItemRow};

/// The fixed trade item-type enumeration upstream offers in the
/// Add-Trade-Item dialog (`InitItemTypes`, AddTradeItemViewModel.cs:314-324,
/// entries in upstream order; localized names rendered as English tokens).
pub const TRADE_ITEM_TYPES: [&str; 11] = [
    "Amulet", "Boots", "Chest", "Gloves", "Helm", "Offhand", "Pants", "Ranged", "Ring", "Rune",
    "Weapon",
];

/// Case-insensitive membership in the fixed trade item-type enumeration.
///
/// Upstream stores the lowercase `Type.Type` constants
/// (`ItemTypeConstants.cs:5-17`) while the dialog displays localized
/// names, so both casings must resolve.
pub fn is_known_item_type(item_type: &str) -> bool {
    TRADE_ITEM_TYPES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(item_type.trim()))
}

/// Whether the item type is the rune type (`IsItemTypeRune`,
/// TradeItemVM.cs:88-91, matched against `ItemTypeConstants.Rune`).
pub fn is_rune_item_type(item_type: &str) -> bool {
    item_type.trim().eq_ignore_ascii_case("Rune")
}

/// Whether switching from `previous` to `next` crosses the rune
/// boundary (upstream `SelectedItemType` setter,
/// AddTradeItemViewModel.cs:113-119): a non-blank previous type that
/// is rune/non-rune while the next is the opposite.
pub fn crosses_rune_boundary(previous: &str, next: &str) -> bool {
    if previous.trim().is_empty() {
        return false;
    }
    is_rune_item_type(previous) != is_rune_item_type(next)
}

/// Coerce the value text the way upstream's `TradeItemWanted.Value`
/// setter does (TradeItemVM.cs:95-103): whitespace-only input becomes
/// `"0"`, other input is trimmed.
pub fn normalize_trade_value(value_text: &str) -> String {
    let trimmed = value_text.trim();
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// One affix candidate row offered by the trade item editing surface
/// (upstream `AffixInfoWanted` over `_affixManager.Affixes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeAffixCandidate {
    pub id: String,
    pub description: String,
}

/// One rune candidate row (upstream `RuneInfoWanted` over
/// `_affixManager.Runes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeRuneCandidate {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// Filter affix candidates the way upstream `FilterAffixes` does
/// (AddTradeItemViewModel.cs:248-266): the filter text splits on `;`,
/// blank keywords are skipped, and every remaining keyword must be
/// contained case-insensitively in the candidate description. An empty
/// filter keeps all candidates.
pub fn filter_affix_candidates<'a>(
    candidates: &'a [TradeAffixCandidate],
    filter: &str,
) -> Vec<&'a TradeAffixCandidate> {
    candidates
        .iter()
        .filter(|candidate| matches_keyword_filter(&candidate.description, filter))
        .collect()
}

/// Filter rune candidates over the rune name (upstream
/// `FilterAffixesRunes`, AddTradeItemViewModel.cs:279-297).
pub fn filter_rune_candidates<'a>(
    candidates: &'a [TradeRuneCandidate],
    filter: &str,
) -> Vec<&'a TradeRuneCandidate> {
    candidates
        .iter()
        .filter(|candidate| matches_keyword_filter(&candidate.name, filter))
        .collect()
}

fn matches_keyword_filter(text: &str, filter: &str) -> bool {
    filter.split(';').all(|keyword| {
        let keyword = keyword.trim();
        keyword.is_empty() || text.to_lowercase().contains(&keyword.to_lowercase())
    })
}

/// Order two wanted rows for display the way upstream
/// `TradeItemCustomSort` does (TradeItemVM.cs:138-159): by item type,
/// then by the value texts zero-padded to equal length and compared
/// ordinally (so `"9M"` sorts before `"10M"`).
pub fn compare_wanted_display_rows(left: &TradeItemRow, right: &TradeItemRow) -> Ordering {
    left.item_type
        .cmp(&right.item_type)
        .then_with(|| compare_zero_padded_values(&left.value_text, &right.value_text))
}

fn compare_zero_padded_values(left: &str, right: &str) -> Ordering {
    let mut padded_left = left.to_string();
    let mut padded_right = right.to_string();
    let target = padded_left
        .chars()
        .count()
        .max(padded_right.chars().count());
    while padded_left.chars().count() < target {
        padded_left.insert(0, '0');
    }
    while padded_right.chars().count() < target {
        padded_right.insert(0, '0');
    }
    padded_left.cmp(&padded_right)
}

/// Effect emitted by a [`TradeViewModel`] command.
#[derive(Debug, Clone, PartialEq)]
pub enum TradeEffect {
    /// Open the add trade item dialog.
    OpenAddDialog,
    /// Open the trade config dialog.
    OpenConfigDialog,
    /// Persist trade state to config storage.
    PersistConfig,
}

/// ViewModel for the Trade screen.
///
/// Pure state + command methods. Every command returns `Vec<TradeEffect>`
/// that the app layer interprets.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeViewModel {
    pub state: TradeImportsPanelState,
}

impl TradeViewModel {
    pub fn new(state: TradeImportsPanelState) -> Self {
        Self { state }
    }

    /// Add a new trade item.
    /// Returns OpenAddDialog effect — the dialog handles the actual item creation.
    pub fn add_trade_item(&mut self) -> Vec<TradeEffect> {
        vec![TradeEffect::OpenAddDialog]
    }

    /// Edit an existing trade item by ID.
    pub fn edit_trade_item(&mut self, item_id: &str) -> Vec<TradeEffect> {
        // In the future, this will set draft state and open the dialog.
        // For now, it returns the dialog effect.
        self.state.selected_trade_item_id = Some(item_id.to_string());
        vec![TradeEffect::OpenAddDialog]
    }

    /// Remove a trade item by ID.
    pub fn remove_trade_item(&mut self, item_id: &str) -> Vec<TradeEffect> {
        self.state
            .trade_items
            .retain(|item| item.trade_item_id != item_id);
        if self.state.selected_trade_item_id.as_deref() == Some(item_id) {
            self.state.selected_trade_item_id = None;
        }
        vec![TradeEffect::PersistConfig]
    }

    /// Open the trade config dialog.
    pub fn open_trade_config(&mut self) -> Vec<TradeEffect> {
        vec![TradeEffect::OpenConfigDialog]
    }

    /// Confirm the add-trade-item form with the user-entered values.
    ///
    /// Validates the inputs, then creates a new [`TradeItemRow`] and
    /// upserts it into the panel state. Returns the canonical
    /// generated `trade_item_id` and a [`TradeEffect::PersistConfig`]
    /// effect so the caller can persist the updated state.
    ///
    /// ## Validation
    ///
    /// - `item_type` must be non-empty and at most 64 characters.
    /// - `value_text` is whitespace-trimmed; an empty value is coerced
    ///   to `"0"` exactly like upstream's `TradeItemWanted.Value`
    ///   setter (TradeItemVM.cs:95-103) and capped at 32 characters
    ///   (d4lc addition without an upstream equivalent).
    ///
    /// On validation failure, returns `Err(message)` and does NOT
    /// mutate state. The caller is expected to surface the error
    /// through the shared dialog error contract.
    pub fn confirm_add_trade_item(
        &mut self,
        item_type: &str,
        value_text: &str,
    ) -> Result<(String, Vec<TradeEffect>), String> {
        let (item_type, value_text) = validate_trade_form(item_type, value_text)?;

        let trade_item_id = next_trade_item_id(&self.state.trade_items, &item_type);
        let row = TradeItemRow {
            trade_item_id: trade_item_id.clone(),
            item_type,
            value_text,
            // A fresh add starts without affixes; affixes are attached
            // through `add_trade_affix` / `add_trade_rune_affix` (the
            // upstream dialog's candidate picker).
            affixes: Vec::new(),
        };
        self.state.upsert_trade_item(row);
        Ok((trade_item_id, vec![TradeEffect::PersistConfig]))
    }

    /// Confirm the edit-trade-item form for the row with
    /// `trade_item_id` (upstream `EditTradeItemExecute` reuses the add
    /// dialog in edit mode and saves afterwards,
    /// TradeViewModel.cs:113-130).
    ///
    /// The row id is preserved, the type and value are replaced with
    /// the validated form values, and switching the type across the
    /// rune boundary clears the affixes (upstream `SelectedItemType`
    /// setter, AddTradeItemViewModel.cs:108-119). Surviving affixes
    /// are re-normalized into id order (`OrderBy(a => a.Id)`,
    /// AddTradeItemViewModel.cs:171).
    ///
    /// On validation failure or an unknown `trade_item_id` returns
    /// `Err(message)` without mutating state.
    pub fn confirm_edit_trade_item(
        &mut self,
        trade_item_id: &str,
        item_type: &str,
        value_text: &str,
    ) -> Result<Vec<TradeEffect>, String> {
        let (item_type, value_text) = validate_trade_form(item_type, value_text)?;
        let row = self
            .state
            .trade_items
            .iter_mut()
            .find(|row| row.trade_item_id == trade_item_id)
            .ok_or_else(|| format!("trade item '{trade_item_id}' not found"))?;

        let previous_type = row.item_type.clone();
        if crosses_rune_boundary(&previous_type, &item_type) {
            row.affixes.clear();
        } else {
            row.affixes
                .sort_by(|left, right| left.affix_id.cmp(&right.affix_id));
        }
        row.item_type = item_type;
        row.value_text = value_text;
        Ok(vec![TradeEffect::PersistConfig])
    }

    /// Attach an affix candidate to a wanted row (upstream
    /// `AddAffixExecute`, AddTradeItemViewModel.cs:158-175): the row is
    /// appended with `IsGreater = false` and the list is re-sorted by
    /// affix id.
    pub fn add_trade_affix(
        &mut self,
        trade_item_id: &str,
        affix_id: &str,
    ) -> Result<Vec<TradeEffect>, String> {
        self.push_trade_affix(trade_item_id, affix_id, false)
    }

    /// Attach a rune candidate to a wanted row (upstream
    /// `AddAffixRuneExecute`, AddTradeItemViewModel.cs:182-201). The
    /// add is gated on the row having no affix yet — upstream
    /// `CanAddAffixRuneExecute` requires `TradeItem.Affixes.Count == 0`
    /// (AddTradeItemViewModel.cs:177-180).
    pub fn add_trade_rune_affix(
        &mut self,
        trade_item_id: &str,
        rune_id: &str,
    ) -> Result<Vec<TradeEffect>, String> {
        if let Some(row) = self
            .state
            .trade_items
            .iter()
            .find(|row| row.trade_item_id == trade_item_id)
        {
            if !row.affixes.is_empty() {
                return Err("A rune item can hold at most one affix.".to_string());
            }
        }
        self.push_trade_affix(trade_item_id, rune_id, true)
    }

    fn push_trade_affix(
        &mut self,
        trade_item_id: &str,
        affix_id: &str,
        is_greater: bool,
    ) -> Result<Vec<TradeEffect>, String> {
        let row = self
            .state
            .trade_items
            .iter_mut()
            .find(|row| row.trade_item_id == trade_item_id)
            .ok_or_else(|| format!("trade item '{trade_item_id}' not found"))?;
        row.affixes.push(TradeItemAffixView {
            affix_id: affix_id.to_string(),
            is_greater,
        });
        row.affixes
            .sort_by(|left, right| left.affix_id.cmp(&right.affix_id));
        Ok(vec![TradeEffect::PersistConfig])
    }

    /// Remove an affix from a wanted row (upstream `RemoveAffixExecute`,
    /// AddTradeItemViewModel.cs:211-218).
    pub fn remove_trade_affix(
        &mut self,
        trade_item_id: &str,
        affix_id: &str,
    ) -> Result<Vec<TradeEffect>, String> {
        let row = self
            .state
            .trade_items
            .iter_mut()
            .find(|row| row.trade_item_id == trade_item_id)
            .ok_or_else(|| format!("trade item '{trade_item_id}' not found"))?;
        let initial_len = row.affixes.len();
        row.affixes.retain(|affix| affix.affix_id != affix_id);
        if row.affixes.len() == initial_len {
            return Err(format!("affix '{affix_id}' not found on '{trade_item_id}'"));
        }
        Ok(vec![TradeEffect::PersistConfig])
    }

    /// Set the IsGreater flag of one affix row (upstream binds the
    /// flag through a per-row checkbox, AddTradeItemView.xaml:87-88).
    pub fn set_trade_affix_is_greater(
        &mut self,
        trade_item_id: &str,
        affix_id: &str,
        is_greater: bool,
    ) -> Result<Vec<TradeEffect>, String> {
        let row = self
            .state
            .trade_items
            .iter_mut()
            .find(|row| row.trade_item_id == trade_item_id)
            .ok_or_else(|| format!("trade item '{trade_item_id}' not found"))?;
        let affix = row
            .affixes
            .iter_mut()
            .find(|affix| affix.affix_id == affix_id)
            .ok_or_else(|| format!("affix '{affix_id}' not found on '{trade_item_id}'"))?;
        affix.is_greater = is_greater;
        Ok(vec![TradeEffect::PersistConfig])
    }
}

/// Shared add/edit form validation: the trimmed type must be non-empty
/// and at most 64 characters; the value is whitespace-trimmed, coerced
/// to `"0"` when empty (TradeItemVM.cs:95-103) and capped at 32
/// characters (d4lc addition without an upstream equivalent).
fn validate_trade_form(item_type: &str, value_text: &str) -> Result<(String, String), String> {
    let trimmed_type = item_type.trim();
    if trimmed_type.is_empty() {
        return Err("Item type is required.".to_string());
    }
    if trimmed_type.chars().count() > 64 {
        return Err("Item type must be 64 characters or fewer.".to_string());
    }

    let value = normalize_trade_value(value_text);
    if value.chars().count() > 32 {
        return Err("Value must be 32 characters or fewer.".to_string());
    }

    Ok((trimmed_type.to_string(), value))
}

// ── TradeConfig dialog ──────────────────────────────

/// Draft of the `TradeConfigDialog` form values, sourced from the
/// root in-out props on the `AppWindow` host.
///
/// Upstream parity (`TradeConfigViewModel.cs:45-81`,
/// `TradeConfigView.xaml:27-57`):
///
/// - `is_trade_overlay_enabled` — `IsTradeOverlayEnabled` toggle.
/// - `show_current_item` — `ShowCurrentItem` toggle; the `d4lc-app`
///   confirm path persists it through the app-settings save path.
/// - `overlay_font_size` — `OverlayFontSize` numeric input, range
///   `10..=50` matching the upstream `NumericUpDown` bounds in
///   `TradeConfigView.xaml:51-57`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeConfigDraft {
    pub is_trade_overlay_enabled: bool,
    pub show_current_item: bool,
    pub overlay_font_size: u32,
}

impl TradeConfigDraft {
    /// Construct a draft from the three primitive values the Slint
    /// form posts to Rust. `font_size` is cast from `i32` at the
    /// boundary; the validator rejects negative or out-of-range
    /// values before they reach persistence.
    pub fn new(
        is_trade_overlay_enabled: bool,
        show_current_item: bool,
        overlay_font_size: i32,
    ) -> Self {
        Self {
            is_trade_overlay_enabled,
            show_current_item,
            overlay_font_size: overlay_font_size.max(0) as u32,
        }
    }
}

/// Validate the trade-config form draft and return the canonical
/// effects the caller should dispatch.
///
/// On success returns `Ok(vec![TradeEffect::PersistConfig])` so the
/// `d4lc-app` adapter can run the existing app-settings save path.
/// On validation failure returns `Err(message)` without producing
/// any effects; the caller surfaces the error through the shared
/// `dialog-error` contract and keeps the dialog open.
///
/// The `PersistConfig` effect is intentionally the same variant the
/// `AddTradeItem` confirm emits: it signals "some trade-related
/// config changed, please persist". The `d4lc-app` confirm handler
/// for `TradeConfig` interprets it as a save to the app settings
/// JSON; the `AddTradeItem` and remove handlers interpret it as a
/// save of the wanted rows to `Config/TradeItems.json`.
pub fn confirm_trade_config_draft(draft: &TradeConfigDraft) -> Result<Vec<TradeEffect>, String> {
    if !(d4lc_config::OVERLAY_FONT_SIZE_MIN..=d4lc_config::OVERLAY_FONT_SIZE_MAX)
        .contains(&draft.overlay_font_size)
    {
        return Err(format!(
            "Overlay font size must be between 10 and 50 (got {}).",
            draft.overlay_font_size
        ));
    }
    Ok(vec![TradeEffect::PersistConfig])
}

/// Generate a stable, unique trade item id from the existing items
/// and the user-entered item type. We avoid pulling in a uuid crate
/// to keep the headless crate dependency-free; the
/// `Trade_<sanitized-type>_<n>` form is human-readable and stable
/// across re-opens of the dialog for the same item type.
fn next_trade_item_id(existing: &[TradeItemRow], item_type: &str) -> String {
    let mut sanitized: String = item_type
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        sanitized.push_str("Item");
    }
    let base = format!("Trade_{sanitized}");
    if !existing.iter().any(|item| item.trade_item_id == base) {
        return base;
    }
    for n in 2..u32::MAX {
        let candidate = format!("{base}_{n}");
        if !existing.iter().any(|item| item.trade_item_id == candidate) {
            return candidate;
        }
    }
    base
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{TradeImportsPanelState, TradeItemAffixView, TradeItemRow};

    fn sample_state() -> TradeImportsPanelState {
        TradeImportsPanelState::with_providers(Vec::new())
    }

    fn sample_trade_item() -> TradeItemRow {
        TradeItemRow {
            trade_item_id: "item1".to_string(),
            item_type: "Amulet".to_string(),
            value_text: "10M".to_string(),
            affixes: vec![TradeItemAffixView {
                affix_id: "Affix_AttackSpeed".to_string(),
                is_greater: false,
            }],
        }
    }

    #[test]
    fn new_wraps_panel_state() {
        let state = sample_state();
        let vm = TradeViewModel::new(state.clone());
        assert_eq!(vm.state, state);
    }

    #[test]
    fn add_trade_item_returns_open_dialog() {
        let mut vm = TradeViewModel::new(sample_state());
        let effects = vm.add_trade_item();
        assert_eq!(effects, vec![TradeEffect::OpenAddDialog]);
    }

    #[test]
    fn remove_trade_item_removes_item_and_returns_persist() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());
        assert_eq!(vm.state.trade_items.len(), 1);

        let effects = vm.remove_trade_item("item1");
        assert!(vm.state.trade_items.is_empty());
        assert_eq!(effects, vec![TradeEffect::PersistConfig]);
    }

    #[test]
    fn remove_trade_item_clears_selected_id_when_matches() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());
        vm.state.selected_trade_item_id = Some("item1".to_string());

        let _effects = vm.remove_trade_item("item1");
        assert!(vm.state.trade_items.is_empty());
        assert_eq!(vm.state.selected_trade_item_id, None);
    }

    #[test]
    fn remove_trade_item_preserves_selected_id_when_different() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());
        vm.state.selected_trade_item_id = Some("other-item".to_string());

        let _effects = vm.remove_trade_item("item1");
        assert_eq!(
            vm.state.selected_trade_item_id.as_deref(),
            Some("other-item")
        );
    }

    #[test]
    fn edit_trade_item_sets_selected_id() {
        let mut vm = TradeViewModel::new(sample_state());
        let effects = vm.edit_trade_item("item-abc");
        assert_eq!(vm.state.selected_trade_item_id.as_deref(), Some("item-abc"));
        assert_eq!(effects, vec![TradeEffect::OpenAddDialog]);
    }

    #[test]
    fn open_trade_config_returns_open_config_dialog() {
        let mut vm = TradeViewModel::new(sample_state());
        let effects = vm.open_trade_config();
        assert_eq!(effects, vec![TradeEffect::OpenConfigDialog]);
    }

    #[test]
    fn remove_trade_item_noop_when_item_not_found() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());

        let effects = vm.remove_trade_item("nonexistent");
        assert_eq!(vm.state.trade_items.len(), 1);
        assert_eq!(effects, vec![TradeEffect::PersistConfig]);
    }

    // ── confirm_add_trade_item tests ───────────────────────────────────

    #[test]
    fn confirm_add_trade_item_inserts_row_and_returns_persist() {
        let mut vm = TradeViewModel::new(sample_state());
        assert!(vm.state.trade_items.is_empty());

        let (id, effects) = vm
            .confirm_add_trade_item("Amulet", "10M")
            .expect("valid input should succeed");

        assert_eq!(id, "Trade_Amulet");
        assert_eq!(vm.state.trade_items.len(), 1);
        assert_eq!(vm.state.trade_items[0].trade_item_id, "Trade_Amulet");
        assert_eq!(vm.state.trade_items[0].item_type, "Amulet");
        assert_eq!(vm.state.trade_items[0].value_text, "10M");
        // A fresh add carries no affixes yet; affixes attach through
        // `add_trade_affix` / `add_trade_rune_affix`.
        assert!(vm.state.trade_items[0].affixes.is_empty());
        // selected_trade_item_id is updated by upsert_trade_item.
        assert_eq!(
            vm.state.selected_trade_item_id.as_deref(),
            Some("Trade_Amulet")
        );
        assert_eq!(effects, vec![TradeEffect::PersistConfig]);
    }

    #[test]
    fn confirm_add_trade_item_trims_surrounding_whitespace() {
        let mut vm = TradeViewModel::new(sample_state());
        let (id, _) = vm
            .confirm_add_trade_item("  Sword  ", "\t5b\n")
            .expect("valid input should succeed after trimming");

        assert_eq!(id, "Trade_Sword");
        assert_eq!(vm.state.trade_items[0].item_type, "Sword");
        assert_eq!(vm.state.trade_items[0].value_text, "5b");
    }

    #[test]
    fn confirm_add_trade_item_rejects_empty_item_type() {
        let mut vm = TradeViewModel::new(sample_state());
        let result = vm.confirm_add_trade_item("   ", "10M");
        assert!(result.is_err());
        assert!(vm.state.trade_items.is_empty());
    }

    #[test]
    fn confirm_add_trade_item_coerces_empty_value_to_zero() {
        // Upstream TradeItemWanted.Value setter (TradeItemVM.cs:95-103):
        // whitespace-only input becomes "0" instead of an error.
        let mut vm = TradeViewModel::new(sample_state());
        let (_, _) = vm
            .confirm_add_trade_item("Amulet", "")
            .expect("empty value must coerce to 0");
        assert_eq!(vm.state.trade_items[0].value_text, "0");

        let mut vm = TradeViewModel::new(sample_state());
        vm.confirm_add_trade_item("Amulet", "   ")
            .expect("whitespace-only value must coerce to 0");
        assert_eq!(vm.state.trade_items[0].value_text, "0");
    }

    // ── item editing (fixed types, rune boundary, affix rows) ─────────

    #[test]
    fn trade_item_types_match_the_upstream_enumeration() {
        assert_eq!(TRADE_ITEM_TYPES.len(), 11);
        assert_eq!(TRADE_ITEM_TYPES[0], "Amulet");
        assert_eq!(TRADE_ITEM_TYPES[9], "Rune");
        assert_eq!(TRADE_ITEM_TYPES[10], "Weapon");
        for known in TRADE_ITEM_TYPES {
            assert!(is_known_item_type(known));
            assert!(is_known_item_type(&known.to_lowercase()));
        }
        assert!(!is_known_item_type("Two-Handed Sword"));
        assert!(!is_known_item_type("Belt"));
    }

    #[test]
    fn rune_detection_and_boundary_match_upstream_setter() {
        assert!(is_rune_item_type("Rune"));
        assert!(is_rune_item_type("rune"));
        assert!(!is_rune_item_type("Ring"));

        // Upstream requires a non-blank previous type and an exact
        // rune/non-rune flip (AddTradeItemViewModel.cs:113-119).
        assert!(crosses_rune_boundary("Ring", "Rune"));
        assert!(crosses_rune_boundary("Rune", "Ring"));
        assert!(!crosses_rune_boundary("Amulet", "Helm"));
        assert!(!crosses_rune_boundary("Rune", "Rune"));
        assert!(!crosses_rune_boundary("", "Rune"));
    }

    #[test]
    fn confirm_edit_trade_item_updates_type_and_value_keeps_id() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());

        let effects = vm
            .confirm_edit_trade_item("item1", "Ring", "20M")
            .expect("edit confirm should succeed");
        assert_eq!(effects, vec![TradeEffect::PersistConfig]);
        let row = &vm.state.trade_items[0];
        assert_eq!(row.trade_item_id, "item1", "the row id must be kept");
        assert_eq!(row.item_type, "Ring");
        assert_eq!(row.value_text, "20M");
    }

    #[test]
    fn confirm_edit_trade_item_coerces_empty_value() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());
        vm.confirm_edit_trade_item("item1", "Ring", "")
            .expect("empty value coerces to 0 on edit too");
        assert_eq!(vm.state.trade_items[0].value_text, "0");
    }

    #[test]
    fn confirm_edit_trade_item_rejects_unknown_id_and_bad_input() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());
        assert!(vm.confirm_edit_trade_item("missing", "Ring", "1b").is_err());
        assert!(vm.confirm_edit_trade_item("item1", "  ", "1b").is_err());
        // Neither failure mutates the row.
        assert_eq!(vm.state.trade_items[0].item_type, "Amulet");
        assert_eq!(vm.state.trade_items[0].value_text, "10M");
    }

    #[test]
    fn confirm_edit_across_rune_boundary_clears_affixes() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());

        vm.confirm_edit_trade_item("item1", "Rune", "1b")
            .expect("switching to rune clears the affixes");
        assert!(vm.state.trade_items[0].affixes.is_empty());
    }

    #[test]
    fn confirm_edit_within_rune_types_keeps_affixes_sorted() {
        let mut vm = TradeViewModel::new(sample_state());
        let mut row = sample_trade_item();
        row.affixes.push(TradeItemAffixView {
            affix_id: "Affix_Armor".to_string(),
            is_greater: false,
        });
        // Deliberately unsorted seed: AttackSpeed before Armor.
        vm.state.trade_items.push(row);

        vm.confirm_edit_trade_item("item1", "Helm", "800k")
            .expect("a non-rune type switch keeps the affixes");
        let ids: Vec<&str> = vm.state.trade_items[0]
            .affixes
            .iter()
            .map(|affix| affix.affix_id.as_str())
            .collect();
        assert_eq!(ids, vec!["Affix_Armor", "Affix_AttackSpeed"]);
    }

    #[test]
    fn add_trade_affix_appends_false_is_greater_and_sorts_by_id() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());

        vm.add_trade_affix("item1", "Affix_MaxLife")
            .expect("first add succeeds");
        vm.add_trade_affix("item1", "Affix_Armor")
            .expect("second add succeeds");

        let affixes = &vm.state.trade_items[0].affixes;
        let rows: Vec<(&str, bool)> = affixes
            .iter()
            .map(|affix| (affix.affix_id.as_str(), affix.is_greater))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Affix_Armor", false),
                ("Affix_AttackSpeed", false),
                ("Affix_MaxLife", false)
            ],
            "affixes are kept in id order with IsGreater starting false"
        );
    }

    #[test]
    fn add_trade_rune_affix_allows_one_affix_only() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());
        let _ = vm.remove_trade_item("item1");
        vm.confirm_add_trade_item("Rune", "1b")
            .expect("rune row added");

        vm.add_trade_rune_affix("Trade_Rune", "Item_Rune_Condition_OnMove")
            .expect("an empty rune item accepts one affix");
        let error = vm
            .add_trade_rune_affix("Trade_Rune", "Item_Rune_Effect_SummonSpiritWolf")
            .expect_err("a second rune affix must be rejected");
        assert!(error.contains("at most one affix"), "unexpected: {error}");
        assert_eq!(vm.state.trade_items[0].affixes.len(), 1);
    }

    #[test]
    fn remove_trade_affix_removes_only_the_matching_row() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());

        vm.remove_trade_affix("item1", "Affix_AttackSpeed")
            .expect("existing affix removes");
        assert!(vm.state.trade_items[0].affixes.is_empty());
        assert!(vm.remove_trade_affix("item1", "Affix_AttackSpeed").is_err());
        assert!(vm
            .remove_trade_affix("missing", "Affix_AttackSpeed")
            .is_err());
    }

    #[test]
    fn set_trade_affix_is_greater_toggles_the_flag() {
        let mut vm = TradeViewModel::new(sample_state());
        vm.state.trade_items.push(sample_trade_item());

        vm.set_trade_affix_is_greater("item1", "Affix_AttackSpeed", true)
            .expect("flag set");
        assert!(vm.state.trade_items[0].affixes[0].is_greater);
        vm.set_trade_affix_is_greater("item1", "Affix_AttackSpeed", false)
            .expect("flag cleared");
        assert!(!vm.state.trade_items[0].affixes[0].is_greater);
        assert!(vm
            .set_trade_affix_is_greater("item1", "Affix_Missing", true)
            .is_err());
    }

    // ── candidate filters (upstream `;`-keyword AND semantics) ────────

    fn sample_affix_candidates() -> Vec<TradeAffixCandidate> {
        vec![
            TradeAffixCandidate {
                id: "AttackSpeed".to_string(),
                description: "+#% Attack Speed".to_string(),
            },
            TradeAffixCandidate {
                id: "MovementSpeed".to_string(),
                description: "+#% Movement Speed".to_string(),
            },
            TradeAffixCandidate {
                id: "MaxLife".to_string(),
                description: "+# Maximum Life".to_string(),
            },
        ]
    }

    #[test]
    fn filter_affix_candidates_requires_every_keyword() {
        let candidates = sample_affix_candidates();
        let ids: Vec<&str> = filter_affix_candidates(&candidates, "")
            .into_iter()
            .map(|candidate| candidate.id.as_str())
            .collect();
        assert_eq!(ids.len(), 3, "an empty filter keeps all candidates");

        let ids: Vec<&str> = filter_affix_candidates(&candidates, "speed")
            .into_iter()
            .map(|candidate| candidate.id.as_str())
            .collect();
        assert_eq!(ids, vec!["AttackSpeed", "MovementSpeed"]);

        // `;`-separated keywords AND together; blanks are skipped.
        let ids: Vec<&str> = filter_affix_candidates(&candidates, "speed; movement ;")
            .into_iter()
            .map(|candidate| candidate.id.as_str())
            .collect();
        assert_eq!(ids, vec!["MovementSpeed"]);

        assert!(filter_affix_candidates(&candidates, "speed;life").is_empty());
    }

    #[test]
    fn filter_rune_candidates_matches_the_name() {
        let runes = vec![TradeRuneCandidate {
            id: "Item_Rune_Condition_OnMove".to_string(),
            name: "Bac".to_string(),
            description: "Travel # meters.".to_string(),
        }];
        assert_eq!(filter_rune_candidates(&runes, "ba").len(), 1);
        assert!(filter_rune_candidates(&runes, "meters").is_empty());
    }

    // ── wanted display ordering (upstream TradeItemCustomSort) ─────────

    fn display_row(id: &str, item_type: &str, value: &str) -> TradeItemRow {
        TradeItemRow {
            trade_item_id: id.to_string(),
            item_type: item_type.to_string(),
            value_text: value.to_string(),
            affixes: Vec::new(),
        }
    }

    #[test]
    fn compare_wanted_display_rows_sorts_by_type_then_padded_value() {
        let mut rows = [
            display_row("b", "Ring", "10M"),
            display_row("a", "Amulet", "5b"),
            display_row("c", "Ring", "9M"),
            display_row("d", "Ring", "10M"),
        ];
        rows.sort_by(compare_wanted_display_rows);
        let ids: Vec<&str> = rows.iter().map(|row| row.trade_item_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "c", "b", "d"]);
    }

    #[test]
    fn confirm_add_trade_item_rejects_oversized_item_type() {
        let mut vm = TradeViewModel::new(sample_state());
        let oversized = "A".repeat(65);
        let result = vm.confirm_add_trade_item(&oversized, "10M");
        assert!(result.is_err());
        assert!(vm.state.trade_items.is_empty());
    }

    #[test]
    fn confirm_add_trade_item_rejects_oversized_value_text() {
        let mut vm = TradeViewModel::new(sample_state());
        let oversized = "1".repeat(33);
        let result = vm.confirm_add_trade_item("Amulet", &oversized);
        assert!(result.is_err());
        assert!(vm.state.trade_items.is_empty());
    }

    #[test]
    fn confirm_add_trade_item_generates_unique_id_on_collision() {
        let mut vm = TradeViewModel::new(sample_state());
        let (id1, _) = vm
            .confirm_add_trade_item("Amulet", "10M")
            .expect("first insert succeeds");
        assert_eq!(id1, "Trade_Amulet");

        let (id2, _) = vm
            .confirm_add_trade_item("Amulet", "11M")
            .expect("second insert succeeds with unique id");
        assert_eq!(id2, "Trade_Amulet_2");
        assert_eq!(vm.state.trade_items.len(), 2);
    }

    #[test]
    fn confirm_add_trade_item_sanitizes_non_alphanumeric_chars_in_id() {
        let mut vm = TradeViewModel::new(sample_state());
        let (id, _) = vm
            .confirm_add_trade_item("Two-Handed Sword", "20M")
            .expect("valid input with spaces and dashes should succeed");
        assert_eq!(id, "Trade_Two_Handed_Sword");
    }

    // ── confirm_trade_config_draft tests ──────────────────────────────

    fn sample_draft() -> TradeConfigDraft {
        TradeConfigDraft {
            is_trade_overlay_enabled: true,
            show_current_item: false,
            overlay_font_size: 20,
        }
    }

    #[test]
    fn confirm_trade_config_draft_accepts_valid_font_size_in_range() {
        // 10..=50 inclusive matches the upstream NumericUpDown bounds.
        for size in [10, 11, 20, 49, 50] {
            let mut draft = sample_draft();
            draft.overlay_font_size = size;
            let effects =
                confirm_trade_config_draft(&draft).expect("in-range font size should be accepted");
            assert_eq!(effects, vec![TradeEffect::PersistConfig]);
        }
    }

    #[test]
    fn confirm_trade_config_draft_rejects_font_size_below_min() {
        let mut draft = sample_draft();
        draft.overlay_font_size = 9;
        let result = confirm_trade_config_draft(&draft);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("between 10 and 50"), "unexpected error: {err}");
        assert!(
            err.contains("9"),
            "error must include the offending value: {err}"
        );
    }

    #[test]
    fn confirm_trade_config_draft_rejects_font_size_above_max() {
        let mut draft = sample_draft();
        draft.overlay_font_size = 51;
        let result = confirm_trade_config_draft(&draft);
        assert!(result.is_err());
    }

    #[test]
    fn confirm_trade_config_draft_preserves_toggle_values() {
        // The validator only checks the numeric field; both toggles
        // pass through to the caller.
        let draft = TradeConfigDraft {
            is_trade_overlay_enabled: true,
            show_current_item: true,
            overlay_font_size: 18,
        };
        let _effects = confirm_trade_config_draft(&draft).expect("valid draft should succeed");
        // The struct itself is unchanged; the caller reads the
        // values off it after validation passes.
        assert!(draft.is_trade_overlay_enabled);
        assert!(draft.show_current_item);
        assert_eq!(draft.overlay_font_size, 18);
    }

    #[test]
    fn trade_config_draft_new_clamps_negative_font_size_to_zero() {
        // Slint posts font size as i32; the constructor must not
        // panic on negative input. The validator then rejects 0 as
        // out-of-range, which is the correct error path.
        let draft = TradeConfigDraft::new(false, false, -5);
        assert_eq!(draft.overlay_font_size, 0);
    }

    #[test]
    fn trade_config_draft_new_accepts_in_range_font_size() {
        let draft = TradeConfigDraft::new(true, true, 25);
        assert!(draft.is_trade_overlay_enabled);
        assert!(draft.show_current_item);
        assert_eq!(draft.overlay_font_size, 25);
    }
}
