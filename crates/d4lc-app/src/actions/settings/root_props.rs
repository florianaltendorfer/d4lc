//! Projection of `SettingsPanelState` onto the mirrored root props.
//!
//! The settings values exist three times: the persisted panel state, the
//! `settings-*` form props the config dialogs edit, and the plain root
//! props other screens bind (`controller-mode`, `paragon-mode`,
//! `affix-overlay-enabled`). Every open, confirm and cancel path goes
//! through [`sync_root_props_from_settings`], so a prop can never keep a
//! value the settings no longer hold. A stale `controller-mode` or
//! `paragon-mode` would otherwise be read back by the next
//! `settings-changed` and revert the confirmed value.

use d4lc_config::PresetColor;
use d4lc_gui::SettingsPanelState;

use super::confirm::{
    ITEM_POWER_LIMIT_MAX, ITEM_POWER_LIMIT_MIN, OVERLAY_FONT_SIZE_MAX, OVERLAY_FONT_SIZE_MIN,
    OVERLAY_ICON_POS_MAX, OVERLAY_ICON_POS_MIN, OVERLAY_UPDATE_DELAY_MAX, OVERLAY_UPDATE_DELAY_MIN,
    PARAGON_BORDER_SIZE_MAX, PARAGON_BORDER_SIZE_MIN, PARAGON_NODE_SIZE_MAX, PARAGON_NODE_SIZE_MIN,
    PARAGON_OFFSET_MAX, PARAGON_OFFSET_MIN, SCAN_HEIGHT_MAX, SCAN_HEIGHT_MIN,
    SCREEN_CAPTURE_DELAY_MAX, SCREEN_CAPTURE_DELAY_MIN, TYPE_AREA_OFFSET_LEFT_MAX,
    TYPE_AREA_OFFSET_LEFT_MIN,
};
use crate::slint_generated::AppWindow;

/// Seed every mirrored root prop from `settings`.
pub fn sync_root_props_from_settings(app: &AppWindow, settings: &SettingsPanelState) {
    sync_controller_props(app, settings);
    sync_overlay_props(app, settings);
    sync_paragon_props(app, settings);
    sync_colors_props(app, settings);
}

/// Upstream `SystemPresetChangeAllowed` (`SettingsViewModel.cs:283-289`):
/// the system preset ComboBox, the Debug Mode toggle and the community
/// preset download are only usable while the overlay is off. Upstream sets
/// it from `!IsEnabled` of both overlay toggle messages (`:379`, `:389`);
/// every overlay toggle of the port lands in `overlay_enabled`, so the gate
/// is derived from it.
pub fn system_preset_change_allowed(settings: &SettingsPanelState) -> bool {
    !settings.overlay_enabled
}

/// Project the overlay gate: the affix screen's power indicator and the
/// settings screen's preset gate.
pub fn project_overlay_gate(app: &AppWindow, settings: &SettingsPanelState) {
    app.set_affix_overlay_enabled(settings.overlay_enabled);
    app.set_system_preset_change_allowed(system_preset_change_allowed(settings));
}

/// The controller dialog toggle, the settings-screen `controller-mode`
/// toggle and the selected-image count.
pub(super) fn sync_controller_props(app: &AppWindow, settings: &SettingsPanelState) {
    app.set_settings_controller_mode(settings.controller_mode);
    app.set_controller_mode(settings.controller_mode);
    app.set_settings_controller_images_count(
        i32::try_from(settings.active_controller_images.len()).unwrap_or(i32::MAX),
    );
}

/// The `settings-overlay-*` rows plus the affix screen's
/// overlay power indicator.
pub(super) fn sync_overlay_props(app: &AppWindow, settings: &SettingsPanelState) {
    app.set_settings_overlay_enabled(settings.overlay_enabled);
    project_overlay_gate(app, settings);
    app.set_settings_overlay_top_most(settings.overlay_top_most);
    let font_size = i32::try_from(settings.overlay_font_size)
        .unwrap_or(18)
        .clamp(
            i32::try_from(OVERLAY_FONT_SIZE_MIN).unwrap_or(10),
            i32::try_from(OVERLAY_FONT_SIZE_MAX).unwrap_or(50),
        );
    app.set_settings_overlay_font_size(font_size);
    app.set_settings_overlay_menu_locked(settings.overlay_menu_locked);
    let type_area_offset = i32::try_from(settings.type_area_offset_left)
        .unwrap_or(0)
        .clamp(
            i32::try_from(TYPE_AREA_OFFSET_LEFT_MIN).unwrap_or(0),
            i32::try_from(TYPE_AREA_OFFSET_LEFT_MAX).unwrap_or(25),
        );
    app.set_settings_overlay_type_area_offset_left(type_area_offset);
    app.set_settings_overlay_show_icon(settings.show_overlay_icon);
    app.set_settings_overlay_icon_pos_x(
        i32::try_from(settings.overlay_icon_pos_x)
            .unwrap_or(0)
            .clamp(
                i32::try_from(OVERLAY_ICON_POS_MIN).unwrap_or(0),
                i32::try_from(OVERLAY_ICON_POS_MAX).unwrap_or(1000),
            ),
    );
    app.set_settings_overlay_icon_pos_y(
        i32::try_from(settings.overlay_icon_pos_y)
            .unwrap_or(0)
            .clamp(
                i32::try_from(OVERLAY_ICON_POS_MIN).unwrap_or(0),
                i32::try_from(OVERLAY_ICON_POS_MAX).unwrap_or(1000),
            ),
    );
    app.set_settings_overlay_item_power_enabled(settings.item_power_limit_enabled);
    app.set_settings_overlay_item_power_limit(
        i32::try_from(settings.item_power_limit)
            .unwrap_or(800)
            .clamp(
                i32::try_from(ITEM_POWER_LIMIT_MIN).unwrap_or(100),
                i32::try_from(ITEM_POWER_LIMIT_MAX).unwrap_or(800),
            ),
    );
    app.set_settings_overlay_marker_mode(settings.overlay_marker_mode.as_str().into());
    app.set_settings_overlay_trade_overlay(settings.trade_overlay_enabled);
    app.set_settings_overlay_socket_detection(settings.socket_detection_enabled);
    app.set_settings_overlay_tempered_detection(settings.tempered_detection_enabled);
    app.set_settings_overlay_aspect_detection(settings.aspect_detection_enabled);
    app.set_settings_overlay_screen_capture_delay(
        settings
            .screen_capture_delay
            .clamp(SCREEN_CAPTURE_DELAY_MIN, SCREEN_CAPTURE_DELAY_MAX) as f32,
    );
    app.set_settings_overlay_update_delay(
        settings
            .overlay_update_delay
            .clamp(OVERLAY_UPDATE_DELAY_MIN, OVERLAY_UPDATE_DELAY_MAX) as f32,
    );
    app.set_settings_overlay_scan_height(
        settings
            .scan_height_percent
            .round()
            .clamp(f64::from(SCAN_HEIGHT_MIN), f64::from(SCAN_HEIGHT_MAX)) as f32,
    );
}

/// The paragon dialog rows plus the settings-screen `paragon-mode`
/// toggle.
pub(super) fn sync_paragon_props(app: &AppWindow, settings: &SettingsPanelState) {
    app.set_settings_paragon_mode_active(settings.paragon_mode_active);
    app.set_paragon_mode(settings.paragon_mode_active);
    app.set_settings_paragon_show_browser_mobalytics_enabled(settings.mobalytics_show_browser);
    app.set_settings_paragon_collapsed_board(settings.collapsed_paragonboard_enabled);
    app.set_settings_paragon_border_size(
        i32::try_from(settings.paragon_border_size)
            .unwrap_or(2)
            .clamp(
                i32::try_from(PARAGON_BORDER_SIZE_MIN).unwrap_or(1),
                i32::try_from(PARAGON_BORDER_SIZE_MAX).unwrap_or(10),
            ),
    );
    app.set_settings_paragon_node_size(
        i32::try_from(settings.paragon_node_size)
            .unwrap_or(40)
            .clamp(
                i32::try_from(PARAGON_NODE_SIZE_MIN).unwrap_or(10),
                i32::try_from(PARAGON_NODE_SIZE_MAX).unwrap_or(100),
            ),
    );
    app.set_settings_paragon_node_size_collapsed(
        i32::try_from(settings.paragon_node_size_collapsed)
            .unwrap_or(23)
            .clamp(
                i32::try_from(PARAGON_NODE_SIZE_MIN).unwrap_or(10),
                i32::try_from(PARAGON_NODE_SIZE_MAX).unwrap_or(100),
            ),
    );
    app.set_settings_paragon_left_offset_collapsed(
        settings
            .paragon_left_offset_collapsed
            .clamp(PARAGON_OFFSET_MIN, PARAGON_OFFSET_MAX),
    );
    app.set_settings_paragon_top_offset_collapsed(
        settings
            .paragon_top_offset_collapsed
            .clamp(PARAGON_OFFSET_MIN, PARAGON_OFFSET_MAX),
    );
}

/// The seven `settings-colors-*` strings, normalized through
/// [`PresetColor::to_string_form`] so the swatch palette matches.
pub(super) fn sync_colors_props(app: &AppWindow, settings: &SettingsPanelState) {
    let colors = &settings.default_colors;
    app.set_settings_colors_implicit(normalized_color(&colors.implicit).into());
    app.set_settings_colors_normal(normalized_color(&colors.normal).into());
    app.set_settings_colors_greater(normalized_color(&colors.greater).into());
    app.set_settings_colors_tempered(normalized_color(&colors.tempered).into());
    app.set_settings_colors_aspects(normalized_color(&colors.aspects).into());
    app.set_settings_colors_uniques(normalized_color(&colors.uniques).into());
    app.set_settings_colors_runes(normalized_color(&colors.runes).into());
}

/// Normalize a stored color string to the canonical `#RRGGBB` form the
/// colors dialog edits (a stored name like `"Green"` renders as
/// `"#008000"`, so the palette swatch previews it and a confirm writes
/// the canonical form back).
fn normalized_color(value: &str) -> String {
    PresetColor::parse(value).to_string_form()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn non_default_settings() -> SettingsPanelState {
        let mut settings = crate::app_state::AppState::default_startup().shell.settings;
        settings.controller_mode = true;
        settings.paragon_mode_active = true;
        settings.overlay_enabled = true;
        settings.collapsed_paragonboard_enabled = true;
        settings.paragon_border_size = 4;
        settings.paragon_node_size = 55;
        settings.item_power_limit = 750;
        settings.screen_capture_delay = 120;
        settings.overlay_update_delay = 15;
        settings.scan_height_percent = 70.0;
        settings.default_colors.implicit = "#FF8800".to_string();
        settings
    }

    #[test]
    fn sync_sets_every_mirrored_root_prop() {
        let app = crate::test_window::new_app_window();
        let settings = non_default_settings();

        sync_root_props_from_settings(&app, &settings);

        assert!(app.get_controller_mode());
        assert!(app.get_settings_controller_mode());
        assert!(app.get_paragon_mode());
        assert!(app.get_settings_paragon_mode_active());
        assert!(app.get_affix_overlay_enabled());
        assert!(app.get_settings_overlay_enabled());
        assert!(app.get_settings_paragon_collapsed_board());
        assert_eq!(app.get_settings_paragon_border_size(), 4);
        assert_eq!(app.get_settings_paragon_node_size(), 55);
        assert_eq!(app.get_settings_overlay_item_power_limit(), 750);
        assert_eq!(app.get_settings_overlay_screen_capture_delay(), 120.0);
        assert_eq!(app.get_settings_overlay_update_delay(), 15.0);
        assert_eq!(app.get_settings_overlay_scan_height(), 70.0);
        assert_eq!(app.get_settings_colors_implicit().as_str(), "#FF8800");
    }

    #[test]
    fn preset_changes_are_allowed_only_while_the_overlay_is_off() {
        // SettingsViewModel.cs:379,389: SystemPresetChangeAllowed = !IsEnabled.
        let mut settings = non_default_settings();
        settings.overlay_enabled = true;
        assert!(!system_preset_change_allowed(&settings));
        settings.overlay_enabled = false;
        assert!(system_preset_change_allowed(&settings));

        let app = crate::test_window::new_app_window();
        assert!(app.get_system_preset_change_allowed(), "upstream default");
        settings.overlay_enabled = true;
        project_overlay_gate(&app, &settings);
        assert!(app.get_affix_overlay_enabled());
        assert!(!app.get_system_preset_change_allowed());
        settings.overlay_enabled = false;
        project_overlay_gate(&app, &settings);
        assert!(app.get_system_preset_change_allowed());
    }

    #[test]
    fn sync_overwrites_stale_props_instead_of_resetting_to_defaults() {
        let app = crate::test_window::new_app_window();
        // Props left over from an earlier dialog.
        app.set_controller_mode(false);
        app.set_paragon_mode(false);
        app.set_settings_overlay_item_power_limit(123);

        sync_root_props_from_settings(&app, &non_default_settings());

        assert!(app.get_controller_mode());
        assert!(app.get_paragon_mode());
        assert_eq!(app.get_settings_overlay_item_power_limit(), 750);
    }
}
