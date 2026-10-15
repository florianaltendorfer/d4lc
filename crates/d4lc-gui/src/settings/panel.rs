//! [`SettingsPanelState`] operations.

use d4lc_config::AppSettings;

use crate::{SettingsChangeEffects, SettingsPanelState};

impl SettingsPanelState {
    /// The one effective update-check switch for the live session.
    ///
    /// Both persisted spellings feed this predicate: the upstream
    /// `CheckForUpdates` row (`SettingsD4.CheckForUpdates`, the single
    /// gate `ReleaseManager.cs:72` reads) and the port-specific
    /// release-banner row. The release check consults this method
    /// instead of the port-specific field alone, so turning off either
    /// row stops the check — before this, the upstream row was dead.
    /// Mirrors
    /// `d4lc_config::AppSettings::update_check_effective`.
    pub const fn update_check_effective(&self) -> bool {
        self.check_for_updates && self.update_check_enabled
    }

    pub fn from_config(settings: &AppSettings, active_controller_images: Vec<String>) -> Self {
        Self {
            selected_system_preset: settings.selected_system_preset.clone(),
            language: settings.selected_affix_language.clone(),
            app_language: settings.selected_app_language.clone(),
            capture_backend: settings.capture_backend.clone(),
            controller_mode: settings.controller_mode,
            debug_mode: settings.debug_mode,
            dev_mode: settings.dev_mode,
            check_for_updates: settings.check_for_updates,
            launch_minimized: settings.launch_minimized,
            minimize_to_tray: settings.minimize_to_tray,
            paragon_mode_active: settings.paragon_mode_active,
            active_controller_images,
            scan_height_percent: settings.scan_height_percent,
            tooltip_width: settings.tooltip_width,
            tooltip_max_height: settings.tooltip_max_height,
            affix_area_height_offset_top: settings.affix_area_height_offset_top,
            affix_area_height_offset_bottom: settings.affix_area_height_offset_bottom,
            affix_aspect_area_width_offset: settings.affix_aspect_area_width_offset,
            aspect_area_height_offset_top: settings.aspect_area_height_offset_top,
            screen_capture_delay: settings.screen_capture_delay,
            overlay_update_delay: settings.overlay_update_delay,
            threshold_min: settings.threshold_min,
            threshold_max: settings.threshold_max,
            threshold_similarity_tooltip: settings.threshold_similarity_tooltip,
            threshold_similarity_affix_location: settings.threshold_similarity_affix_location,
            threshold_similarity_aspect_location: settings.threshold_similarity_aspect_location,
            threshold_similarity_socket_location: settings.threshold_similarity_socket_location,
            threshold_similarity_splitter_location: settings.threshold_similarity_splitter_location,
            minimal_ocr_match_type: settings.minimal_ocr_match_type,
            item_power_limit_enabled: settings.rule_settings.item_power.enabled,
            item_power_limit: settings.rule_settings.item_power.limit,
            minimal_affix_value_filter_enabled: settings.rule_settings.minimal_affix_value_enabled,
            multi_build_mode_enabled: settings.rule_settings.multi_build_enabled,
            trade_overlay_enabled: settings.rule_settings.trade_overlay_enabled,
            overlay_marker_mode: settings.rule_settings.overlay_marker_mode,
            aspect_detection_enabled: settings.rule_settings.aspect_detection_enabled,
            unique_detection_enabled: settings.rule_settings.unique_detection_enabled,
            rune_detection_enabled: settings.rule_settings.rune_detection_enabled,
            sigil_detection_enabled: settings.rule_settings.sigil_detection_enabled,
            sigil_display_mode: settings.rule_settings.sigil_display_mode,
            dungeon_tiers_enabled: settings.rule_settings.dungeon_tiers_enabled,
            toggles: settings.rule_settings.toggles,
            socket_detection_enabled: settings.is_socket_detection_enabled,
            tempered_detection_enabled: settings.is_tempered_affix_detection_enabled,
            overlay_enabled: settings.overlay_enabled,
            overlay_top_most: settings.is_top_most,
            overlay_font_size: settings.overlay_font_size,
            overlay_menu_locked: settings.overlay_menu_locked,
            mobalytics_show_browser: settings.is_show_browser_mobalytics_enabled,
            type_area_offset_left: settings.type_area_offset_left,
            type_area_offset_right: settings.type_area_offset_right,
            aspect_area_max_height: settings.aspect_area_max_height,
            is_tuning_prism_hints_enabled: settings.is_tuning_prism_hints_enabled,
            is_import_paragon_infinity_builds_enabled: settings
                .is_import_paragon_infinity_builds_enabled,
            selected_build_import_website: settings.selected_build_import_website.clone(),
            default_colors: settings.default_colors.clone(),
            collapsed_paragonboard_enabled: settings
                .rule_settings
                .is_collapsed_paragonboard_enabled,
            paragon_border_size: settings.paragon_border_size,
            paragon_node_size: settings.paragon_node_size,
            paragon_node_size_collapsed: settings.paragon_node_size_collapsed,
            paragon_left_offset_collapsed: settings.paragon_left_offset_collapsed,
            paragon_top_offset_collapsed: settings.paragon_top_offset_collapsed,
            show_overlay_icon: settings.show_overlay_icon,
            overlay_icon_pos_x: settings.overlay_icon_pos_x,
            overlay_icon_pos_y: settings.overlay_icon_pos_y,
            diagnostics_export_enabled: settings.is_diagnostics_export_enabled,
            update_check_enabled: settings.update_check_enabled,
        }
    }

    pub fn set_language(&mut self, language: &str) -> SettingsChangeEffects {
        if self.language == language {
            return SettingsChangeEffects {
                selected_language: None,
                active_language: self.language.clone(),
                ocr_profile_rebuild_required: false,
                fuzzy_index_rebuild_required: false,
                localized_parser_reload_required: false,
                localized_import_reload_required: false,
                localized_paragon_reload_required: false,
                reload_error: None,
            };
        }

        self.language = language.to_string();
        SettingsChangeEffects {
            selected_language: Some(self.language.clone()),
            active_language: self.language.clone(),
            ocr_profile_rebuild_required: true,
            fuzzy_index_rebuild_required: true,
            localized_parser_reload_required: true,
            localized_import_reload_required: true,
            localized_paragon_reload_required: true,
            reload_error: None,
        }
    }

    /// Select the UI language (`SettingsViewModel.cs:222-235`). Returns
    /// whether the selection changed, so an unchanged pick does not
    /// rewrite the settings file.
    pub fn set_app_language(&mut self, culture: &str) -> bool {
        if self.app_language == culture {
            return false;
        }
        self.app_language = culture.to_string();
        true
    }

    pub fn apply_to_config(&self, settings: &mut AppSettings) {
        settings.selected_system_preset = self.selected_system_preset.clone();
        settings.selected_affix_language = self.language.clone();
        settings.selected_app_language = self.app_language.clone();
        settings.capture_backend = self.capture_backend.clone();
        settings.controller_mode = self.controller_mode;
        settings.debug_mode = self.debug_mode;
        settings.dev_mode = self.dev_mode;
        settings.check_for_updates = self.check_for_updates;
        settings.launch_minimized = self.launch_minimized;
        settings.minimize_to_tray = self.minimize_to_tray;
        settings.paragon_mode_active = self.paragon_mode_active;
        settings.scan_height_percent = self.scan_height_percent;
        settings.tooltip_width = self.tooltip_width;
        settings.tooltip_max_height = self.tooltip_max_height;
        settings.affix_area_height_offset_top = self.affix_area_height_offset_top;
        settings.affix_area_height_offset_bottom = self.affix_area_height_offset_bottom;
        settings.affix_aspect_area_width_offset = self.affix_aspect_area_width_offset;
        settings.aspect_area_height_offset_top = self.aspect_area_height_offset_top;
        settings.screen_capture_delay = self.screen_capture_delay;
        settings.overlay_update_delay = self.overlay_update_delay;
        settings.threshold_min = self.threshold_min;
        settings.threshold_max = self.threshold_max;
        settings.threshold_similarity_tooltip = self.threshold_similarity_tooltip;
        settings.threshold_similarity_affix_location = self.threshold_similarity_affix_location;
        settings.threshold_similarity_aspect_location = self.threshold_similarity_aspect_location;
        settings.threshold_similarity_socket_location = self.threshold_similarity_socket_location;
        settings.threshold_similarity_splitter_location =
            self.threshold_similarity_splitter_location;
        settings.minimal_ocr_match_type = self.minimal_ocr_match_type;
        settings.rule_settings.item_power.enabled = self.item_power_limit_enabled;
        settings.rule_settings.item_power.limit = self.item_power_limit;
        settings.rule_settings.minimal_affix_value_enabled =
            self.minimal_affix_value_filter_enabled;
        settings.rule_settings.multi_build_enabled = self.multi_build_mode_enabled;
        settings.rule_settings.trade_overlay_enabled = self.trade_overlay_enabled;
        settings.rule_settings.overlay_marker_mode = self.overlay_marker_mode;
        settings.rule_settings.aspect_detection_enabled = self.aspect_detection_enabled;
        settings.rule_settings.unique_detection_enabled = self.unique_detection_enabled;
        settings.rule_settings.rune_detection_enabled = self.rune_detection_enabled;
        settings.rule_settings.sigil_detection_enabled = self.sigil_detection_enabled;
        settings.rule_settings.sigil_display_mode = self.sigil_display_mode;
        settings.rule_settings.dungeon_tiers_enabled = self.dungeon_tiers_enabled;
        settings.rule_settings.toggles = self.toggles;
        settings.is_socket_detection_enabled = self.socket_detection_enabled;
        settings.is_tempered_affix_detection_enabled = self.tempered_detection_enabled;
        settings.overlay_enabled = self.overlay_enabled;
        settings.is_top_most = self.overlay_top_most;
        settings.overlay_font_size = self.overlay_font_size;
        settings.overlay_menu_locked = self.overlay_menu_locked;
        settings.is_show_browser_mobalytics_enabled = self.mobalytics_show_browser;
        settings.type_area_offset_left = self.type_area_offset_left;
        settings.type_area_offset_right = self.type_area_offset_right;
        settings.aspect_area_max_height = self.aspect_area_max_height;
        settings.is_tuning_prism_hints_enabled = self.is_tuning_prism_hints_enabled;
        settings.is_import_paragon_infinity_builds_enabled =
            self.is_import_paragon_infinity_builds_enabled;
        settings.selected_build_import_website = self.selected_build_import_website.clone();
        settings.default_colors = self.default_colors.clone();
        settings.rule_settings.is_collapsed_paragonboard_enabled =
            self.collapsed_paragonboard_enabled;
        settings.paragon_border_size = self.paragon_border_size;
        settings.paragon_node_size = self.paragon_node_size;
        settings.paragon_node_size_collapsed = self.paragon_node_size_collapsed;
        settings.paragon_left_offset_collapsed = self.paragon_left_offset_collapsed;
        settings.paragon_top_offset_collapsed = self.paragon_top_offset_collapsed;
        settings.show_overlay_icon = self.show_overlay_icon;
        settings.overlay_icon_pos_x = self.overlay_icon_pos_x;
        settings.overlay_icon_pos_y = self.overlay_icon_pos_y;
        settings.is_diagnostics_export_enabled = self.diagnostics_export_enabled;
        settings.update_check_enabled = self.update_check_enabled;
    }
}
