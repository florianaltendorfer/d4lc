//! Headless confirm helpers for the settings-family config dialogs.

use std::path::Path;

use d4lc_config::{save_controller_config, PresetColor};
#[cfg(test)]
use d4lc_gui::SettingsPanelState;

use super::panel::persist_settings;
use crate::app_state::AppState;

/// Numeric range for the overlay font size control
/// (`SettingsD4.OverlayFontSize`).
pub(super) const OVERLAY_FONT_SIZE_MIN: u32 = 10;
pub(super) const OVERLAY_FONT_SIZE_MAX: u32 = 50;

/// Numeric range for the type-area offset left control
/// (`SettingsD4.TypeAreaOffsetLeft`, `DebugView.xaml:108-109`
/// `Minimum="0" Maximum="25"`).
pub(super) const TYPE_AREA_OFFSET_LEFT_MIN: u32 = 0;
pub(super) const TYPE_AREA_OFFSET_LEFT_MAX: u32 = 25;

/// Numeric range for the overlay icon placement
/// (`OverlayConfigView.xaml:66-77` slider bounds).
pub(super) const OVERLAY_ICON_POS_MIN: u32 = 0;
pub(super) const OVERLAY_ICON_POS_MAX: u32 = d4lc_config::OVERLAY_ICON_POS_MAX;

/// Numeric range for the item-power limit
/// (`OverlayConfigView.xaml:110-116` NumericUpDown bounds).
pub(super) const ITEM_POWER_LIMIT_MIN: u32 = d4lc_config::ITEM_POWER_LIMIT_MIN;
pub(super) const ITEM_POWER_LIMIT_MAX: u32 = d4lc_config::ITEM_POWER_LIMIT_MAX;

/// Numeric ranges of the overlay performance rows
/// (`OverlayConfigView.xaml:192-217` NumericUpDown bounds).
pub(super) const SCREEN_CAPTURE_DELAY_MIN: u32 = 5;
pub(super) const SCREEN_CAPTURE_DELAY_MAX: u32 = 500;
pub(super) const OVERLAY_UPDATE_DELAY_MIN: u32 = 5;
pub(super) const OVERLAY_UPDATE_DELAY_MAX: u32 = 500;
pub(super) const SCAN_HEIGHT_MIN: u32 = 10;
pub(super) const SCAN_HEIGHT_MAX: u32 = 100;

/// Paragon layout ranges (upstream `ParagonConfigView.xaml:39-104`).
pub(super) const PARAGON_BORDER_SIZE_MIN: u32 = d4lc_config::PARAGON_BORDER_SIZE_MIN;
pub(super) const PARAGON_BORDER_SIZE_MAX: u32 = d4lc_config::PARAGON_BORDER_SIZE_MAX;
pub(super) const PARAGON_NODE_SIZE_MIN: u32 = d4lc_config::PARAGON_NODE_SIZE_MIN;
pub(super) const PARAGON_NODE_SIZE_MAX: u32 = d4lc_config::PARAGON_NODE_SIZE_MAX;
pub(super) const PARAGON_OFFSET_MIN: i32 = d4lc_config::PARAGON_OFFSET_MIN;
pub(super) const PARAGON_OFFSET_MAX: i32 = d4lc_config::PARAGON_OFFSET_MAX;

// ── Headless helpers (testable without AppWindow) ────────────────────────
//
// The AppWindow-touching handlers above read form values off the
// root `AppWindow` and call these headless helpers. The helpers
// own the validation + state-mutation + persistence side of
// each confirm path, and are covered by `cargo test -p d4lc-app`
// without needing a Slint runtime.

/// Headless overlay-config draft used by [`confirm_overlay_config`].
///
/// Carries every row the dialog edits: overlay enabled / topmost / font
/// size / menu lock / type-area offset, overlay icon visibility and
/// placement, the item-power rule, the overlay marker mode, the trade
/// overlay cross-entry, the socket / tempered / aspect detection gates and
/// the performance rows (capture delay, update delay, scan height).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayConfigDraft {
    pub overlay_enabled: bool,
    pub overlay_top_most: bool,
    pub overlay_font_size: i32,
    pub overlay_menu_locked: bool,
    pub type_area_offset_left: i32,
    pub show_overlay_icon: bool,
    pub overlay_icon_pos_x: i32,
    pub overlay_icon_pos_y: i32,
    pub item_power_limit_enabled: bool,
    pub item_power_limit: i32,
    pub overlay_marker_mode: String,
    pub trade_overlay_enabled: bool,
    pub socket_detection_enabled: bool,
    pub tempered_detection_enabled: bool,
    pub aspect_detection_enabled: bool,
    pub screen_capture_delay: i32,
    pub overlay_update_delay: i32,
    /// Whole percent (`scan_height_percent` is stored as a float).
    pub scan_height: i32,
}

impl OverlayConfigDraft {
    /// Seed every row from the current settings, so a draft never resets
    /// the rows it does not edit. The production toggles use the
    /// single-field setters instead; this seeds the test drafts.
    #[cfg(test)]
    pub fn from_settings(settings: &SettingsPanelState) -> Self {
        Self {
            overlay_enabled: settings.overlay_enabled,
            overlay_top_most: settings.overlay_top_most,
            overlay_font_size: settings.overlay_font_size as i32,
            overlay_menu_locked: settings.overlay_menu_locked,
            type_area_offset_left: settings.type_area_offset_left as i32,
            show_overlay_icon: settings.show_overlay_icon,
            overlay_icon_pos_x: settings.overlay_icon_pos_x as i32,
            overlay_icon_pos_y: settings.overlay_icon_pos_y as i32,
            item_power_limit_enabled: settings.item_power_limit_enabled,
            item_power_limit: settings.item_power_limit as i32,
            overlay_marker_mode: settings.overlay_marker_mode.as_str().to_string(),
            trade_overlay_enabled: settings.trade_overlay_enabled,
            socket_detection_enabled: settings.socket_detection_enabled,
            tempered_detection_enabled: settings.tempered_detection_enabled,
            aspect_detection_enabled: settings.aspect_detection_enabled,
            screen_capture_delay: settings.screen_capture_delay as i32,
            overlay_update_delay: settings.overlay_update_delay as i32,
            scan_height: settings.scan_height_percent.round() as i32,
        }
    }
}

/// Validate an [`OverlayConfigDraft`] and apply it to
/// `state.shell.settings`.
///
/// Returns `Err(message)` when a numeric range or the marker-mode token is
/// out of bounds so the shared dialog router keeps the dialog open with
/// the validation error. State is not mutated on validation failure.
pub fn confirm_overlay_config(
    draft: &OverlayConfigDraft,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    if draft.overlay_font_size < 0
        || !(OVERLAY_FONT_SIZE_MIN..=OVERLAY_FONT_SIZE_MAX)
            .contains(&(draft.overlay_font_size as u32))
    {
        return Err(format!(
            "Overlay font size must be {OVERLAY_FONT_SIZE_MIN}..={OVERLAY_FONT_SIZE_MAX} (got {})",
            draft.overlay_font_size
        ));
    }
    if draft.type_area_offset_left < 0
        || !(TYPE_AREA_OFFSET_LEFT_MIN..=TYPE_AREA_OFFSET_LEFT_MAX)
            .contains(&(draft.type_area_offset_left as u32))
    {
        return Err(format!(
            "Type-area offset left must be {TYPE_AREA_OFFSET_LEFT_MIN}..={TYPE_AREA_OFFSET_LEFT_MAX} (got {})",
            draft.type_area_offset_left
        ));
    }
    for (label, value) in [
        ("Overlay icon X", draft.overlay_icon_pos_x),
        ("Overlay icon Y", draft.overlay_icon_pos_y),
    ] {
        if value < 0 || !(OVERLAY_ICON_POS_MIN..=OVERLAY_ICON_POS_MAX).contains(&(value as u32)) {
            return Err(format!(
                "{label} must be {OVERLAY_ICON_POS_MIN}..={OVERLAY_ICON_POS_MAX} (got {value})"
            ));
        }
    }
    if draft.item_power_limit < 0
        || !(ITEM_POWER_LIMIT_MIN..=ITEM_POWER_LIMIT_MAX).contains(&(draft.item_power_limit as u32))
    {
        return Err(format!(
            "Item power limit must be {ITEM_POWER_LIMIT_MIN}..={ITEM_POWER_LIMIT_MAX} (got {})",
            draft.item_power_limit
        ));
    }
    for (label, value, min, max) in [
        (
            "Screencapture delay",
            draft.screen_capture_delay,
            SCREEN_CAPTURE_DELAY_MIN,
            SCREEN_CAPTURE_DELAY_MAX,
        ),
        (
            "Overlay update delay",
            draft.overlay_update_delay,
            OVERLAY_UPDATE_DELAY_MIN,
            OVERLAY_UPDATE_DELAY_MAX,
        ),
        (
            "Scan height",
            draft.scan_height,
            SCAN_HEIGHT_MIN,
            SCAN_HEIGHT_MAX,
        ),
    ] {
        if value < 0 || !(min..=max).contains(&(value as u32)) {
            return Err(format!("{label} must be {min}..={max} (got {value})"));
        }
    }
    let marker_mode = match draft.overlay_marker_mode.trim() {
        "Show All" => d4lc_config::OverlayMarkerModeSetting::ShowAll,
        "Hide Unwanted" => d4lc_config::OverlayMarkerModeSetting::HideUnwanted,
        other => {
            return Err(format!(
                "Overlay marker mode must be \"Show All\" or \"Hide Unwanted\" (got '{other}')"
            ))
        }
    };

    state.shell.settings.overlay_enabled = draft.overlay_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.overlay_top_most = draft.overlay_top_most;
    state.bump_rules_input_revision();
    state.shell.settings.overlay_font_size = draft.overlay_font_size as u32;
    state.bump_rules_input_revision();
    state.shell.settings.overlay_menu_locked = draft.overlay_menu_locked;
    state.bump_rules_input_revision();
    state.shell.settings.type_area_offset_left = draft.type_area_offset_left as u32;
    state.bump_rules_input_revision();
    state.shell.settings.show_overlay_icon = draft.show_overlay_icon;
    state.bump_rules_input_revision();
    state.shell.settings.overlay_icon_pos_x = draft.overlay_icon_pos_x as u32;
    state.bump_rules_input_revision();
    state.shell.settings.overlay_icon_pos_y = draft.overlay_icon_pos_y as u32;
    state.bump_rules_input_revision();
    state.shell.settings.item_power_limit_enabled = draft.item_power_limit_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.item_power_limit = draft.item_power_limit as u32;
    state.bump_rules_input_revision();
    state.shell.settings.overlay_marker_mode = marker_mode;
    state.bump_rules_input_revision();
    state.shell.settings.trade_overlay_enabled = draft.trade_overlay_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.socket_detection_enabled = draft.socket_detection_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.tempered_detection_enabled = draft.tempered_detection_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.aspect_detection_enabled = draft.aspect_detection_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.screen_capture_delay = draft.screen_capture_delay as u32;
    state.bump_rules_input_revision();
    state.shell.settings.overlay_update_delay = draft.overlay_update_delay as u32;
    state.bump_rules_input_revision();
    // The dialog edits whole percent: a hand-edited fractional value is
    // only replaced when the row was actually changed.
    if draft.scan_height != state.shell.settings.scan_height_percent.round() as i32 {
        state.shell.settings.scan_height_percent = f64::from(draft.scan_height);
        state.bump_rules_input_revision();
    }
    persist_settings(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    Ok(())
}

/// Flip the overlay on/off setting alone and persist.
///
/// The toggles (Ctrl+F12, the power button, the overlay menu lock) change
/// one value, as upstream's `ToggleOverlayMessage` handlers do
/// (`SettingsViewModel.cs:376-392`). Unlike [`confirm_overlay_config`] this
/// does not re-validate the other rows, so a persisted value outside the
/// dialog's ranges (the codec does not clamp on load) cannot block the toggle.
pub fn set_overlay_enabled(
    enabled: bool,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    state.shell.settings.overlay_enabled = enabled;
    state.bump_rules_input_revision();
    persist_settings(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    Ok(())
}

/// Flip `IsParagonModeActive` alone and persist (Ctrl+F6; upstream flips
/// only this property, `SettingsViewModel.cs:326-332`). See
/// [`set_overlay_enabled`] for why this is not a full dialog confirm.
pub fn set_paragon_mode_active(
    active: bool,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    state.shell.settings.paragon_mode_active = active;
    state.bump_rules_input_revision();
    persist_settings(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    Ok(())
}

/// Headless paragon-config draft used by [`confirm_paragon_config`].
///
/// Carries the two paragon toggles plus the six upstream layout values
/// (collapsed-board switch and the five numerics, `SettingsD4.cs:28,71-75`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParagonConfigDraft {
    pub paragon_mode_active: bool,
    pub is_show_browser_mobalytics_enabled: bool,
    pub collapsed_paragonboard_enabled: bool,
    pub paragon_border_size: i32,
    pub paragon_node_size: i32,
    pub paragon_node_size_collapsed: i32,
    pub paragon_left_offset_collapsed: i32,
    pub paragon_top_offset_collapsed: i32,
}

impl ParagonConfigDraft {
    /// Seed every row from the current settings; see
    /// [`OverlayConfigDraft::from_settings`].
    #[cfg(test)]
    pub fn from_settings(settings: &SettingsPanelState) -> Self {
        Self {
            paragon_mode_active: settings.paragon_mode_active,
            is_show_browser_mobalytics_enabled: settings.mobalytics_show_browser,
            collapsed_paragonboard_enabled: settings.collapsed_paragonboard_enabled,
            paragon_border_size: settings.paragon_border_size as i32,
            paragon_node_size: settings.paragon_node_size as i32,
            paragon_node_size_collapsed: settings.paragon_node_size_collapsed as i32,
            paragon_left_offset_collapsed: settings.paragon_left_offset_collapsed,
            paragon_top_offset_collapsed: settings.paragon_top_offset_collapsed,
        }
    }
}

/// Apply a [`ParagonConfigDraft`] to `state.shell.settings` and persist.
///
/// The layout numerics are validated against the upstream dialog ranges
/// (`ParagonConfigView.xaml:39-104`); an out-of-range value returns `Err`
/// without mutating state, so the router keeps the dialog open with the
/// validation error.
///
/// Upstream's `ParagonConfigView.xaml` has no D2Core row; the
/// `IsImportParagonD2CoreEnabled` provider toggle belongs to the
/// build-import workflow and is not touched here.
pub fn confirm_paragon_config(
    draft: &ParagonConfigDraft,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    for (label, value, min, max) in [
        (
            "Paragon border size",
            draft.paragon_border_size,
            PARAGON_BORDER_SIZE_MIN,
            PARAGON_BORDER_SIZE_MAX,
        ),
        (
            "Paragon node size",
            draft.paragon_node_size,
            PARAGON_NODE_SIZE_MIN,
            PARAGON_NODE_SIZE_MAX,
        ),
        (
            "Paragon node size collapsed",
            draft.paragon_node_size_collapsed,
            PARAGON_NODE_SIZE_MIN,
            PARAGON_NODE_SIZE_MAX,
        ),
    ] {
        if value < i32::try_from(min).unwrap_or(0) || value > i32::try_from(max).unwrap_or(i32::MAX)
        {
            return Err(format!("{label} must be {min}..={max} (got {value})"));
        }
    }
    for (label, value) in [
        (
            "Paragon left offset collapsed",
            draft.paragon_left_offset_collapsed,
        ),
        (
            "Paragon top offset collapsed",
            draft.paragon_top_offset_collapsed,
        ),
    ] {
        if !(PARAGON_OFFSET_MIN..=PARAGON_OFFSET_MAX).contains(&value) {
            return Err(format!(
                "{label} must be {PARAGON_OFFSET_MIN}..={PARAGON_OFFSET_MAX} (got {value})"
            ));
        }
    }

    state.shell.settings.paragon_mode_active = draft.paragon_mode_active;
    state.bump_rules_input_revision();
    state.shell.settings.mobalytics_show_browser = draft.is_show_browser_mobalytics_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.collapsed_paragonboard_enabled = draft.collapsed_paragonboard_enabled;
    state.bump_rules_input_revision();
    state.shell.settings.paragon_border_size = draft.paragon_border_size as u32;
    state.bump_rules_input_revision();
    state.shell.settings.paragon_node_size = draft.paragon_node_size as u32;
    state.bump_rules_input_revision();
    state.shell.settings.paragon_node_size_collapsed = draft.paragon_node_size_collapsed as u32;
    state.bump_rules_input_revision();
    state.shell.settings.paragon_left_offset_collapsed = draft.paragon_left_offset_collapsed;
    state.bump_rules_input_revision();
    state.shell.settings.paragon_top_offset_collapsed = draft.paragon_top_offset_collapsed;
    state.bump_rules_input_revision();
    persist_settings(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    Ok(())
}

/// Headless controller-config draft used by
/// [`confirm_controller_config_draft`]: the `IsControllerModeActive`
/// toggle plus the staged `Controllers.json` selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControllerConfigDraft {
    pub controller_mode: bool,
    /// Active controller image filenames. Written verbatim (order
    /// preserved, case-insensitive duplicates dropped) to
    /// `Config/Controllers.json`.
    pub active_images: Vec<String>,
}

impl ControllerConfigDraft {
    pub fn new(controller_mode: bool, active_images: Vec<String>) -> Self {
        Self {
            controller_mode,
            active_images,
        }
    }
}

/// Apply a controller-mode toggle to `state.shell.settings` and
/// persist. No validation.
///
/// Kept for callers that edit the mode alone (the Ctrl+F9 hotkey arm in
/// `crate::hotkeys`); the dialog uses
/// [`confirm_controller_config_draft`] so the staged image selection is
/// written in the same confirm.
pub fn confirm_controller_config(
    controller_mode: bool,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let images = state.shell.settings.active_controller_images.clone();
    confirm_controller_config_draft(
        &ControllerConfigDraft::new(controller_mode, images),
        state,
        config_root,
    )
}

/// Validate + apply a [`ControllerConfigDraft`], write
/// `Config/Controllers.json`, and persist the mode.
///
/// Mirrors the upstream `SystemPresetManager` write-back
/// (`SystemPresetManager.cs:89-153`): the persisted file is the active
/// controller image list, and the loaded list is what the controller-mode
/// runtime preparation consumes. Empty names and control characters are
/// rejected so a malformed row cannot reach the file; duplicate names are
/// collapsed case-insensitively by `save_controller_config`.
pub fn confirm_controller_config_draft(
    draft: &ControllerConfigDraft,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    for name in &draft.active_images {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("controller image names must not be empty".to_string());
        }
        if trimmed.chars().any(|ch| ch.is_control()) {
            return Err(format!(
                "controller image name '{name}' contains control characters"
            ));
        }
    }

    state.shell.settings.controller_mode = draft.controller_mode;
    state.bump_rules_input_revision();
    save_controller_config(config_root, &draft.active_images)?;
    state.shell.settings.active_controller_images =
        d4lc_config::load_controller_config(config_root)
            .map_err(|error| format!("failed to reload Config/Controllers.json: {error}"))?;
    persist_settings(state, config_root)?;
    Ok(())
}

/// Headless colors-config draft used by [`confirm_colors_config`].
///
/// The seven upstream `DefaultColor*` values as the dialog submits them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorsConfigDraft {
    pub implicit: String,
    pub normal: String,
    pub greater: String,
    pub tempered: String,
    pub aspects: String,
    pub uniques: String,
    pub runes: String,
}

impl ColorsConfigDraft {
    pub fn new(
        implicit: String,
        normal: String,
        greater: String,
        tempered: String,
        aspects: String,
        uniques: String,
        runes: String,
    ) -> Self {
        Self {
            implicit,
            normal,
            greater,
            tempered,
            aspects,
            uniques,
            runes,
        }
    }
}

/// Validate + apply the seven default colors and persist them.
///
/// Every value is parsed strictly with [`PresetColor::try_parse`]: a typo
/// must surface as a validation error instead of silently becoming green
/// (the lenient parser is the stored-preset path). The stored form is the
/// canonical `#RRGGBB` / `#AARRGGBB` string, so the dialog re-seeds and
/// the palette swatch previews exactly what was saved.
pub fn confirm_colors_config(
    draft: &ColorsConfigDraft,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let mut current = state.shell.settings.default_colors.clone();
    for (key, label, value) in [
        ("implicit", "Implicit", &draft.implicit),
        ("normal", "Normal", &draft.normal),
        ("greater", "Greater", &draft.greater),
        ("tempered", "Tempered", &draft.tempered),
        ("aspects", "Aspects", &draft.aspects),
        ("uniques", "Uniques", &draft.uniques),
        ("runes", "Runes", &draft.runes),
    ] {
        let parsed = PresetColor::try_parse(value)
            .map_err(|error| format!("{label} default color: {error}"))?;
        if !current.set_field(key, &parsed.to_string_form()) {
            return Err(format!("unknown default color field '{key}'"));
        }
    }

    state.shell.settings.default_colors = current;
    state.bump_rules_input_revision();
    persist_settings(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    Ok(())
}

/// Apply a tuning-prism hints toggle (`IsTuningPrismHintsEnabled`,
/// Ctrl+F7 by default) to `state.shell.settings` and persist through
/// the same path as the other settings toggles. Rebuilds the
/// app-owned overlay descriptor projection because the toggle gates
/// the `d4lc-rules` tuning-prism hint descriptors.
///
/// Upstream: `SettingsViewModel.HandleToggleTuningPrismHintsKeyBindingMessage`
/// flips `IsTuningPrismHintsEnabled`
/// (`D4Companion/ViewModels/SettingsViewModel.cs:366-373`); the
/// upstream `SettingsManager` persists it with the rest of
/// `Config/Settings.json`.
pub fn confirm_tuning_prism_hints_config(
    is_tuning_prism_hints_enabled: bool,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    state.shell.settings.is_tuning_prism_hints_enabled = is_tuning_prism_hints_enabled;
    state.bump_rules_input_revision();
    persist_settings(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::actions::settings::panel::persist_settings;
    use crate::actions::settings::test_support::{load_persisted, with_temp_config_root};
    use d4lc_config::save_app_settings;

    fn overlay_draft(
        overlay_enabled: bool,
        overlay_top_most: bool,
        overlay_font_size: i32,
        overlay_menu_locked: bool,
        type_area_offset_left: i32,
    ) -> OverlayConfigDraft {
        OverlayConfigDraft {
            overlay_enabled,
            overlay_top_most,
            overlay_font_size,
            overlay_menu_locked,
            type_area_offset_left,
            ..OverlayConfigDraft::from_settings(
                &crate::app_state::AppState::default_startup().shell.settings,
            )
        }
    }

    fn paragon_draft(
        paragon_mode_active: bool,
        is_show_browser_mobalytics_enabled: bool,
    ) -> ParagonConfigDraft {
        ParagonConfigDraft {
            paragon_mode_active,
            is_show_browser_mobalytics_enabled,
            ..ParagonConfigDraft::from_settings(
                &crate::app_state::AppState::default_startup().shell.settings,
            )
        }
    }

    // ── Settings-family confirm helper tests ────────────────────────

    #[test]
    fn toggles_ignore_persisted_values_outside_the_dialog_ranges() {
        // The codec does not clamp on load, so a hand-edited file can hold
        // values the dialog would reject. A toggle flips one value and must
        // not be blocked by them.
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            {
                let settings = &mut state.shell.settings;
                settings.type_area_offset_left = 30;
                settings.overlay_update_delay = 0;
                settings.item_power_limit = 925;
                settings.paragon_node_size = 500;
                settings.paragon_left_offset_collapsed = 5000;
            }
            assert!(confirm_overlay_config(
                &OverlayConfigDraft::from_settings(&state.shell.settings),
                &mut state,
                config_root
            )
            .is_err());

            set_overlay_enabled(true, &mut state, config_root)
                .expect("the overlay toggle must not re-validate other rows");
            set_paragon_mode_active(true, &mut state, config_root)
                .expect("the paragon toggle must not re-validate other rows");

            assert!(state.shell.settings.overlay_enabled);
            assert!(state.shell.settings.paragon_mode_active);
            let persisted = load_persisted(config_root);
            assert!(persisted.overlay_enabled);
            assert!(persisted.paragon_mode_active);
            assert_eq!(persisted.type_area_offset_left, 30);
            assert_eq!(persisted.paragon_node_size, 500);
        });
    }

    #[test]
    fn overlay_draft_from_settings_confirms_without_changing_any_setting() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            {
                let settings = &mut state.shell.settings;
                settings.overlay_enabled = true;
                settings.overlay_top_most = true;
                settings.overlay_font_size = 31;
                settings.overlay_menu_locked = true;
                settings.type_area_offset_left = 17;
                settings.show_overlay_icon = true;
                settings.overlay_icon_pos_x = 120;
                settings.overlay_icon_pos_y = 340;
                settings.item_power_limit_enabled = true;
                settings.item_power_limit = 750;
                settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
                settings.trade_overlay_enabled = false;
                settings.socket_detection_enabled = false;
                settings.tempered_detection_enabled = false;
                settings.aspect_detection_enabled = false;
                settings.screen_capture_delay = 120;
                settings.overlay_update_delay = 15;
                // A hand-edited fractional height is not rounded away by an
                // unrelated confirm.
                settings.scan_height_percent = 45.5;
            }
            let before = state.shell.settings.clone();
            let draft = OverlayConfigDraft::from_settings(&state.shell.settings);
            confirm_overlay_config(&draft, &mut state, config_root)
                .expect("a draft seeded from the settings must confirm");
            assert_eq!(state.shell.settings, before);
        });
    }

    #[test]
    fn paragon_draft_from_settings_confirms_without_changing_any_setting() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            {
                let settings = &mut state.shell.settings;
                settings.paragon_mode_active = true;
                settings.mobalytics_show_browser = true;
                settings.collapsed_paragonboard_enabled = true;
                settings.paragon_border_size = 4;
                settings.paragon_node_size = 55;
                settings.paragon_node_size_collapsed = 30;
                settings.paragon_left_offset_collapsed = -120;
                settings.paragon_top_offset_collapsed = 75;
            }
            let before = state.shell.settings.clone();
            let draft = ParagonConfigDraft::from_settings(&state.shell.settings);
            confirm_paragon_config(&draft, &mut state, config_root)
                .expect("a draft seeded from the settings must confirm");
            assert_eq!(state.shell.settings, before);
        });
    }

    #[test]
    fn confirm_overlay_config_validates_font_size_range() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            // Out-of-range font size (9 is below MIN=10).
            let draft = overlay_draft(true, false, 9, false, 5);
            let result = confirm_overlay_config(&draft, &mut state, config_root);
            assert!(result.is_err(), "out-of-range font size must fail");
            // State must not be mutated on validation failure.
            assert_eq!(state.shell.settings.overlay_enabled, false);
            assert_eq!(state.shell.settings.overlay_font_size, 18);

            // Out-of-range font size (51 is above MAX=50).
            let draft = overlay_draft(true, false, 51, false, 5);
            let result = confirm_overlay_config(&draft, &mut state, config_root);
            assert!(result.is_err(), "out-of-range font size must fail");
        });
    }

    #[test]
    fn confirm_overlay_config_validates_type_area_offset_range() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            // Out-of-range offset (26 is above MAX=25, DebugView.xaml:109).
            let draft = overlay_draft(true, false, 18, false, 26);
            let result = confirm_overlay_config(&draft, &mut state, config_root);
            assert!(result.is_err(), "out-of-range offset must fail");
            // State must not be mutated on validation failure.
            assert_eq!(state.shell.settings.type_area_offset_left, 5);

            // Negative offset is also rejected.
            let draft = overlay_draft(true, false, 18, false, -1);
            let result = confirm_overlay_config(&draft, &mut state, config_root);
            assert!(result.is_err(), "negative offset must fail");
        });
    }

    #[test]
    fn confirm_overlay_config_accepts_in_range_draft_and_persists() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            let draft = overlay_draft(true, true, 25, true, 17);
            confirm_overlay_config(&draft, &mut state, config_root)
                .expect("in-range draft should succeed");

            // State is mutated.
            assert_eq!(state.shell.settings.overlay_enabled, true);
            assert_eq!(state.shell.settings.overlay_top_most, true);
            assert_eq!(state.shell.settings.overlay_font_size, 25);
            assert_eq!(state.shell.settings.overlay_menu_locked, true);
            assert_eq!(state.shell.settings.type_area_offset_left, 17);

            // Persisted to disk via the d4lc_config path.
            let persisted = load_persisted(config_root);
            assert_eq!(persisted.overlay_enabled, true);
            assert_eq!(persisted.is_top_most, true);
            assert_eq!(persisted.overlay_font_size, 25);
            assert_eq!(persisted.overlay_menu_locked, true);
            assert_eq!(persisted.type_area_offset_left, 17);
        });
    }

    #[test]
    fn confirm_overlay_config_accepts_boundary_values() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            // Lower boundary: font_size=10, offset=0.
            let draft = overlay_draft(false, false, 10, false, 0);
            confirm_overlay_config(&draft, &mut state, config_root)
                .expect("boundary values should succeed");
            assert_eq!(state.shell.settings.overlay_font_size, 10);
            assert_eq!(state.shell.settings.type_area_offset_left, 0);

            // Upper boundary: font_size=50, offset=25.
            let draft = overlay_draft(true, true, 50, true, 25);
            confirm_overlay_config(&draft, &mut state, config_root)
                .expect("boundary values should succeed");
            assert_eq!(state.shell.settings.overlay_font_size, 50);
            assert_eq!(state.shell.settings.type_area_offset_left, 25);
        });
    }

    #[test]
    fn confirm_overlay_config_applies_the_upstream_rows_and_persists() {
        // The rows whose config fields already exist and are
        // consumed — overlay icon visibility + placement, the item-power
        // rule, the overlay marker mode, the trade-overlay cross-entry and
        // the socket / tempered detection gates.
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            let draft = OverlayConfigDraft {
                show_overlay_icon: true,
                overlay_icon_pos_x: 120,
                overlay_icon_pos_y: 340,
                item_power_limit_enabled: true,
                item_power_limit: 750,
                overlay_marker_mode: "Hide Unwanted".to_string(),
                trade_overlay_enabled: false,
                socket_detection_enabled: false,
                tempered_detection_enabled: false,
                aspect_detection_enabled: false,
                screen_capture_delay: 120,
                overlay_update_delay: 15,
                scan_height: 70,
                ..overlay_draft(true, false, 22, true, 7)
            };
            confirm_overlay_config(&draft, &mut state, config_root)
                .expect("the upstream overlay rows should confirm");

            // OverlayConfigView.xaml:192-217 rows reach the settings the
            // live loop and the detector read.
            assert_eq!(state.shell.settings.screen_capture_delay, 120);
            assert_eq!(state.shell.settings.overlay_update_delay, 15);
            assert_eq!(state.shell.settings.scan_height_percent, 70.0);
            let persisted = load_persisted(config_root);
            assert_eq!(persisted.screen_capture_delay, 120);
            assert_eq!(persisted.overlay_update_delay, 15);
            assert_eq!(persisted.scan_height_percent, 70.0);

            assert!(state.shell.settings.show_overlay_icon);
            assert_eq!(state.shell.settings.overlay_icon_pos_x, 120);
            assert_eq!(state.shell.settings.overlay_icon_pos_y, 340);
            assert!(state.shell.settings.item_power_limit_enabled);
            assert_eq!(state.shell.settings.item_power_limit, 750);
            assert_eq!(
                state.shell.settings.overlay_marker_mode.as_str(),
                "Hide Unwanted"
            );
            assert!(!state.shell.settings.trade_overlay_enabled);
            assert!(!state.shell.settings.socket_detection_enabled);
            assert!(!state.shell.settings.tempered_detection_enabled);
            assert!(!state.shell.settings.aspect_detection_enabled);

            // Every value survives a restart through the real file, and
            // the marker mode / item-power rule reach the rule runtime
            // projection.
            let persisted = load_persisted(config_root);
            assert!(persisted.show_overlay_icon);
            assert_eq!(persisted.overlay_icon_pos_x, 120);
            assert_eq!(persisted.overlay_icon_pos_y, 340);
            assert!(persisted.rule_settings.item_power.enabled);
            assert_eq!(persisted.rule_settings.item_power.limit, 750);
            assert_eq!(
                persisted.rule_settings.overlay_marker_mode,
                d4lc_config::OverlayMarkerModeSetting::HideUnwanted
            );
            assert!(!persisted.rule_settings.trade_overlay_enabled);
            assert!(!persisted.is_socket_detection_enabled);
            assert!(!persisted.is_tempered_affix_detection_enabled);
            assert!(!persisted.rule_settings.aspect_detection_enabled);
            assert_eq!(
                persisted
                    .rule_settings
                    .to_rule_runtime_settings()
                    .overlay_marker_mode,
                d4lc_config::OverlayMarkerModeSetting::HideUnwanted
            );

            // Out-of-range / unknown values are rejected without mutating
            // state.
            for bad in [
                OverlayConfigDraft {
                    overlay_icon_pos_x: 1001,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    overlay_icon_pos_y: -1,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    item_power_limit: 99,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    item_power_limit: 801,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    overlay_marker_mode: "Sometimes".to_string(),
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    screen_capture_delay: 4,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    screen_capture_delay: 501,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    overlay_update_delay: 4,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    overlay_update_delay: 501,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    scan_height: 9,
                    ..overlay_draft(true, false, 22, true, 7)
                },
                OverlayConfigDraft {
                    scan_height: 101,
                    ..overlay_draft(true, false, 22, true, 7)
                },
            ] {
                assert!(
                    confirm_overlay_config(&bad, &mut state, config_root).is_err(),
                    "invalid overlay row must be rejected: {bad:?}"
                );
            }
            let unchanged = load_persisted(config_root);
            assert_eq!(unchanged.overlay_icon_pos_x, 120);
            assert_eq!(unchanged.rule_settings.item_power.limit, 750);
            assert_eq!(
                unchanged.rule_settings.overlay_marker_mode,
                d4lc_config::OverlayMarkerModeSetting::HideUnwanted
            );
        });
    }

    #[test]
    fn confirm_paragon_config_applies_both_toggles_and_persists() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            let draft = paragon_draft(true, true);
            confirm_paragon_config(&draft, &mut state, config_root)
                .expect("paragon confirm should succeed");

            assert_eq!(state.shell.settings.paragon_mode_active, true);
            assert_eq!(state.shell.settings.mobalytics_show_browser, true);

            let persisted = load_persisted(config_root);
            assert!(persisted.paragon_mode_active);
            assert!(persisted.is_show_browser_mobalytics_enabled);
            // The D2Core provider toggle is owned by the build import.
            assert!(
                persisted.is_import_paragon_d2core_enabled,
                "the paragon confirm must not touch the D2Core toggle"
            );
        });
    }

    #[test]
    fn confirm_controller_config_applies_toggle_and_persists() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            // Default is false; flip to true.
            confirm_controller_config(true, &mut state, config_root)
                .expect("controller confirm should succeed");
            assert_eq!(state.shell.settings.controller_mode, true);

            let persisted = load_persisted(config_root);
            assert_eq!(persisted.controller_mode, true);
        });
    }

    #[test]
    fn confirm_controller_config_draft_writes_controllers_json_and_reloads() {
        // The dialog's selection is
        // written to `Config/Controllers.json` in the upstream shape
        // (SystemPresetManager.cs:89-153), and the loaded list is what the
        // panel state — and therefore the GUI runtime configuration —
        // carries afterwards.
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            let draft = ControllerConfigDraft::new(
                true,
                vec![
                    "tooltip_gc_enus_amulet_0.png".to_string(),
                    "tooltip_gc_enus_boots_1.png".to_string(),
                    "TOOLTIP_GC_ENUS_AMULET_0.PNG".to_string(),
                ],
            );
            confirm_controller_config_draft(&draft, &mut state, config_root)
                .expect("controller draft should confirm");

            assert!(state.shell.settings.controller_mode);
            assert_eq!(
                state.shell.settings.active_controller_images,
                vec![
                    "tooltip_gc_enus_amulet_0.png".to_string(),
                    "tooltip_gc_enus_boots_1.png".to_string()
                ],
                "duplicates collapse case-insensitively on the file round-trip"
            );

            let raw = std::fs::read_to_string(config_root.join(d4lc_config::CONTROLLERS_PATH))
                .expect("Controllers.json must be written");
            assert_eq!(
                raw,
                "[\n  \"tooltip_gc_enus_amulet_0.png\",\n  \"tooltip_gc_enus_boots_1.png\"\n]\n"
            );

            // The runtime preparation path reads the same file, so the
            // selection reaches the GUI runtime config. Gated on the
            // feature that compiles the runtime-preparation module.
            #[cfg(feature = "real-runtime")]
            {
                let resolved =
                    crate::runtime_prepare::resolve_runtime_config(config_root, None, None, None)
                        .expect("runtime config resolution should succeed");
                assert_eq!(
                    resolved.controller_config,
                    state.shell.settings.active_controller_images
                );
            }

            // A restart restores the selection from disk.
            let reloaded = d4lc_gui::SettingsPanelState::from_config(
                &load_persisted(config_root),
                d4lc_config::load_controller_config(config_root).unwrap_or_default(),
            );
            assert_eq!(
                reloaded.active_controller_images,
                state.shell.settings.active_controller_images
            );

            // Clearing the selection writes the empty array.
            confirm_controller_config_draft(
                &ControllerConfigDraft::new(true, Vec::new()),
                &mut state,
                config_root,
            )
            .expect("clearing the selection should confirm");
            assert!(state.shell.settings.active_controller_images.is_empty());
            assert_eq!(
                std::fs::read_to_string(config_root.join(d4lc_config::CONTROLLERS_PATH))
                    .expect("Controllers.json must still exist"),
                "[]\n"
            );
        });
    }

    #[test]
    fn confirm_controller_config_draft_rejects_empty_and_control_chars() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            assert!(confirm_controller_config_draft(
                &ControllerConfigDraft::new(false, vec!["   ".to_string()]),
                &mut state,
                config_root,
            )
            .is_err());
            assert!(confirm_controller_config_draft(
                &ControllerConfigDraft::new(false, vec!["bad\nname.png".to_string()]),
                &mut state,
                config_root,
            )
            .is_err());
            assert!(
                !config_root.join(d4lc_config::CONTROLLERS_PATH).is_file(),
                "a rejected draft must not write the file"
            );
        });
    }

    #[test]
    fn confirm_paragon_config_applies_layout_fields_ranges_and_persists() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            // Upstream defaults (SettingsD4.cs:71-75).
            assert_eq!(state.shell.settings.paragon_border_size, 2);
            assert_eq!(state.shell.settings.paragon_node_size, 40);
            assert_eq!(state.shell.settings.paragon_node_size_collapsed, 23);
            assert_eq!(state.shell.settings.paragon_left_offset_collapsed, -200);
            assert_eq!(state.shell.settings.paragon_top_offset_collapsed, -50);
            assert!(!state.shell.settings.collapsed_paragonboard_enabled);

            let draft = ParagonConfigDraft {
                collapsed_paragonboard_enabled: true,
                paragon_border_size: 4,
                paragon_node_size: 55,
                paragon_node_size_collapsed: 30,
                paragon_left_offset_collapsed: -120,
                paragon_top_offset_collapsed: 75,
                ..paragon_draft(true, true)
            };
            confirm_paragon_config(&draft, &mut state, config_root)
                .expect("paragon layout should confirm");

            assert!(state.shell.settings.paragon_mode_active);
            assert!(state.shell.settings.collapsed_paragonboard_enabled);
            assert_eq!(state.shell.settings.paragon_border_size, 4);
            assert_eq!(state.shell.settings.paragon_node_size, 55);
            assert_eq!(state.shell.settings.paragon_node_size_collapsed, 30);
            assert_eq!(state.shell.settings.paragon_left_offset_collapsed, -120);
            assert_eq!(state.shell.settings.paragon_top_offset_collapsed, 75);

            // Survives a restart through the real settings file.
            let persisted = load_persisted(config_root);
            assert_eq!(persisted.paragon_border_size, 4);
            assert_eq!(persisted.paragon_node_size, 55);
            assert_eq!(persisted.paragon_node_size_collapsed, 30);
            assert_eq!(persisted.paragon_left_offset_collapsed, -120);
            assert_eq!(persisted.paragon_top_offset_collapsed, 75);
            assert!(persisted.rule_settings.is_collapsed_paragonboard_enabled);

            // Out-of-range values are rejected without mutating state.
            for bad in [
                ParagonConfigDraft {
                    paragon_border_size: 11,
                    ..paragon_draft(true, true)
                },
                ParagonConfigDraft {
                    paragon_node_size: 9,
                    ..paragon_draft(true, true)
                },
                ParagonConfigDraft {
                    paragon_node_size_collapsed: 101,
                    ..paragon_draft(true, true)
                },
                ParagonConfigDraft {
                    paragon_left_offset_collapsed: -1001,
                    ..paragon_draft(true, true)
                },
                ParagonConfigDraft {
                    paragon_top_offset_collapsed: 1001,
                    ..paragon_draft(true, true)
                },
            ] {
                assert!(
                    confirm_paragon_config(&bad, &mut state, config_root).is_err(),
                    "out-of-range layout value must be rejected: {bad:?}"
                );
            }
            let unchanged = load_persisted(config_root);
            assert_eq!(unchanged.paragon_border_size, 4);
            assert_eq!(unchanged.paragon_left_offset_collapsed, -120);
        });
    }

    #[test]
    fn confirm_tuning_prism_hints_config_applies_toggle_and_persists() {
        // The Ctrl+F7 hotkey and any future settings-screen
        // toggle share this path — flip the headless
        // `SettingsPanelState` bit, persist to `Config/Settings.json`,
        // and rebuild the overlay descriptor projection (the gate for
        // the `d4lc-rules` prism descriptors).
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            assert!(!state.shell.settings.is_tuning_prism_hints_enabled);

            confirm_tuning_prism_hints_config(true, &mut state, config_root)
                .expect("tuning-prism confirm should succeed");
            assert!(state.shell.settings.is_tuning_prism_hints_enabled);

            let persisted = load_persisted(config_root);
            assert!(persisted.is_tuning_prism_hints_enabled);

            // Toggling back off persists the default value again.
            confirm_tuning_prism_hints_config(false, &mut state, config_root)
                .expect("tuning-prism confirm should succeed");
            let persisted = load_persisted(config_root);
            assert!(!persisted.is_tuning_prism_hints_enabled);
        });
    }

    #[test]
    fn new_v5_4_settings_fields_project_from_panel_state_and_persist() {
        // The app projection path. Mutating the
        // headless `SettingsPanelState` (the struct every settings
        // screen / dialog confirm writes through) must reach
        // `d4lc_config::AppSettings` and the on-disk
        // `Config/Settings.json` through the regular persist path.
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();

            // Upstream defaults survive the projection untouched.
            assert!(!state.shell.settings.is_tuning_prism_hints_enabled);
            assert!(
                state
                    .shell
                    .settings
                    .is_import_paragon_infinity_builds_enabled
            );
            assert_eq!(
                state.shell.settings.selected_build_import_website,
                "Maxroll.gg"
            );
            assert_eq!(state.shell.settings.type_area_offset_right, 5);
            assert_eq!(state.shell.settings.aspect_area_max_height, 300);

            state.shell.settings.is_tuning_prism_hints_enabled = true;
            state
                .shell
                .settings
                .is_import_paragon_infinity_builds_enabled = false;
            state.shell.settings.selected_build_import_website = "D4Builds.gg".to_string();
            state.shell.settings.type_area_offset_right = 12;
            state.shell.settings.aspect_area_max_height = 260;

            persist_settings(&state, config_root).expect("persist should succeed");

            let persisted = load_persisted(config_root);
            assert!(persisted.is_tuning_prism_hints_enabled);
            assert!(!persisted.is_import_paragon_infinity_builds_enabled);
            assert_eq!(persisted.selected_build_import_website, "D4Builds.gg");
            assert_eq!(persisted.type_area_offset_right, 12);
            assert_eq!(persisted.aspect_area_max_height, 260);
        });
    }

    #[test]
    fn persisted_paragon_toggles_survive_the_startup_hydration_path() {
        // The startup projection in `main.rs`
        // loads `AppSettings` from disk, rebuilds the panel state, and
        // calls `hydrate_build_import_from_settings`. A persisted `false`
        // paragon toggle must survive that exact path — the tabbed import
        // dialog may not silently reset the toggles to the all-true
        // defaults after a restart.
        with_temp_config_root(|config_root| {
            // Persist a config with two toggles off through the regular
            // save path.
            let mut config = load_persisted(config_root);
            config.is_import_paragon_maxroll_enabled = false;
            config.is_import_paragon_mobalytics_enabled = false;
            save_app_settings(config_root, &config).expect("save should succeed");

            // The startup projection (main.rs) with the same loaded
            // settings.
            let loaded = load_persisted(config_root);
            let mut state = crate::app_state::AppState::default_startup();
            state.shell.settings = d4lc_gui::SettingsPanelState::from_config(&loaded, Vec::new());
            state.hydrate_build_import_from_settings(&loaded);

            assert!(
                !state.build_import.paragon_toggles.get("Maxroll"),
                "a persisted false Maxroll toggle must survive startup hydration"
            );
            assert!(
                !state.build_import.paragon_toggles.get("Mobalytics"),
                "a persisted false Mobalytics toggle must survive startup hydration"
            );
            assert!(
                state.build_import.paragon_toggles.get("D2Core"),
                "an untouched toggle keeps its persisted true default"
            );
            // The shell panel projection stays on the same loaded config.
            assert_eq!(
                state.shell.settings.selected_build_import_website,
                loaded.selected_build_import_website
            );
        });
    }

    #[test]
    fn confirm_colors_config_validates_persists_and_normalizes_default_colors() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            // Upstream defaults: all seven are Colors.Green (#008000).
            assert_eq!(state.shell.settings.default_colors.normal, "#008000");

            let draft = ColorsConfigDraft::new(
                "#FF8800".to_string(),
                "Green".to_string(),
                "  #00FF00  ".to_string(),
                "Tempered".to_string(),
                "#FF0000".to_string(),
                "#00FFFF".to_string(),
                "#FF00FF".to_string(),
            );
            // The unparseable value must fail the whole confirm.
            assert!(confirm_colors_config(&draft, &mut state, config_root).is_err());
            assert!(
                load_persisted(config_root).default_colors.normal == "#008000",
                "a rejected draft must not change the panel state"
            );
            assert_eq!(
                state.shell.settings.default_colors.normal, "#008000",
                "a rejected draft must not mutate the panel state"
            );

            let draft = ColorsConfigDraft::new(
                "#FF8800".to_string(),
                "Green".to_string(),
                "  #00ff00  ".to_string(),
                "Teal".to_string(),
                "#FF0000".to_string(),
                "#00FFFF".to_string(),
                "#FF00FF".to_string(),
            );
            confirm_colors_config(&draft, &mut state, config_root)
                .expect("valid color strings should confirm");

            // Every value is stored in the canonical form the dialog
            // re-seeds from (names and mixed case normalize to hex).
            assert_eq!(state.shell.settings.default_colors.implicit, "#FF8800");
            assert_eq!(state.shell.settings.default_colors.normal, "#008000");
            assert_eq!(state.shell.settings.default_colors.greater, "#00FF00");
            assert_eq!(
                state.shell.settings.default_colors.tempered, "#008080",
                "'Teal' normalizes to its canonical hex form"
            );
            assert_eq!(state.shell.settings.default_colors.aspects, "#FF0000");
            assert_eq!(state.shell.settings.default_colors.uniques, "#00FFFF");
            assert_eq!(state.shell.settings.default_colors.runes, "#FF00FF");

            // Survives a restart through the real load path.
            let persisted = load_persisted(config_root);
            assert_eq!(persisted.default_colors.implicit, "#FF8800");
            assert_eq!(persisted.default_colors.greater, "#00FF00");
            assert_eq!(persisted.default_colors.normal, "#008000");
            let reloaded = d4lc_gui::SettingsPanelState::from_config(&persisted, Vec::new());
            assert_eq!(reloaded.default_colors, persisted.default_colors);

            // Other settings ride along through the same persist path.
            state.shell.settings.overlay_enabled = true;
            confirm_colors_config(
                &ColorsConfigDraft::new(
                    "#FF8800".to_string(),
                    "#008000".to_string(),
                    "#00FF00".to_string(),
                    "#008080".to_string(),
                    "#FF0000".to_string(),
                    "#00FFFF".to_string(),
                    "#FF00FF".to_string(),
                ),
                &mut state,
                config_root,
            )
            .expect("second colors confirm should persist");
            assert!(load_persisted(config_root).overlay_enabled);
        });
    }

    #[test]
    fn confirm_colors_config_rejects_unparseable_and_unknown_input() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            let base = |normal: &str| {
                ColorsConfigDraft::new(
                    "#FF8800".to_string(),
                    normal.to_string(),
                    "#00FF00".to_string(),
                    "#008080".to_string(),
                    "#FF0000".to_string(),
                    "#00FFFF".to_string(),
                    "#FF00FF".to_string(),
                )
            };
            for invalid in ["", "   ", "#12345", "ultramarine", "#GGGGGG"] {
                let error = confirm_colors_config(&base(invalid), &mut state, config_root)
                    .expect_err("invalid color must be rejected");
                assert!(
                    error.contains("Normal") || error.contains("color"),
                    "the error must name the offending field/value: {error}"
                );
            }
        });
    }

    #[test]
    fn overlay_font_size_range_constants_match_trade_config() {
        // The overlay-config dialog shares the
        // `10..=50` range with the trade-config dialog
        // (which uses `confirm_trade_config_draft`). The two
        // surfaces must stay in sync; this test pins the
        // contract.
        assert_eq!(OVERLAY_FONT_SIZE_MIN, 10);
        assert_eq!(OVERLAY_FONT_SIZE_MAX, 50);
    }
}
