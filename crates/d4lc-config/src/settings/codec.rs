//! The JSON shape of `Config/Settings.json`: the lenient parser and the
//! upstream-ordered writer.

use super::store::SETTINGS_SCHEMA_VERSION;
use super::wpf_key;
use super::{
    AppSettings, HotkeyBinding, HotkeyConfig, MultiBuildRuleConfig, OverlayMarkerModeSetting,
    RuleToggleSettings, SigilDisplayModeSetting,
};
use crate::presets::PresetColor;
use crate::wpf_color;
use crate::{json, SETTINGS_PATH};
use d4lc_core::Rect;
use serde_json::Value;

pub fn parse_manual_region(value: &str) -> Result<Rect, String> {
    d4lc_capture_free_parse_manual_region(value)
}

pub(super) fn parse_app_settings(text: &str) -> Result<AppSettings, String> {
    let document = json::parse_object(text, SETTINGS_PATH)?;
    let fields = json::Fields::new(&document, SETTINGS_PATH);
    let mut settings = AppSettings::default();

    if let Some(value) = fields.non_blank("SelectedSystemPreset") {
        settings.selected_system_preset = value;
    }
    if let Some(value) = fields.non_blank("SelectedAffixLanguage") {
        settings.selected_affix_language = value;
    }
    if let Some(value) = fields.non_blank("SelectedAppLanguage") {
        settings.selected_app_language = value;
    }
    // Upstream: SettingsD4.cs:80 SelectedAffixPreset, default "".
    // An empty stored value and the default mean the same thing ("no
    // persisted selection"), so a blank field is left as the default.
    if let Some(value) = fields.non_blank("SelectedAffixPreset") {
        settings.selected_affix_preset = value;
    }
    if let Some(value) = fields.non_blank("CaptureBackend") {
        settings.capture_backend = value;
    }
    if let Some(value) = fields.non_blank("DisplayBackend") {
        settings.display_backend = value;
    }
    if let Some(value) = fields.string("WaylandRestoreToken") {
        // An empty stored token is treated as "no stored session" so a
        // hand-edited file can never persist an unusable empty token.
        if !value.trim().is_empty() {
            settings.wayland_restore_token = Some(value);
        }
    }
    // Migration: `OcrBackend`/`OcrProfile` were
    // port-own keys without an upstream counterpart and without a second
    // real backend behind them — the live OCR worker is the in-process
    // Tesseract engine regardless of the stored value. The keys are
    // deliberately not read or written; a file that still carries
    // them loads with the keys ignored (the lenient parser skips unknown
    // fields), and the next save drops them.
    if let Some(value) = fields.bool("ControllerMode") {
        settings.controller_mode = value;
    }
    if let Some(value) = fields.bool("DebugMode") {
        settings.debug_mode = value;
    }
    // Upstream: SettingsD4.cs:25 DevMode, default false. Upstream has no
    // UI control for it (hand-edit only); the port keeps the key alive
    // across saves so a hand-edited file is not silently stripped when
    // d4lc rewrites the whole settings file.
    if let Some(value) = fields.bool("DevMode") {
        settings.dev_mode = value;
    }
    // Upstream: SettingsD4.cs:29 IsDebugInfoEnabled, default false.
    if let Some(value) = fields.bool("IsDebugInfoEnabled") {
        settings.is_debug_info_enabled = value;
    }
    if let Some(value) = fields.bool("CheckForUpdates") {
        settings.check_for_updates = value;
    }
    if let Some(value) = fields.bool("LaunchMinimized") {
        settings.launch_minimized = value;
    }
    if let Some(value) = fields.bool("MinimizeToTray") {
        settings.minimize_to_tray = value;
    }
    if let Some(geometry) = fields
        .value("WindowGeometry")
        .and_then(|value| json::Fields::of(value, SETTINGS_PATH))
    {
        settings.window_geometry = Some(parse_window_geometry(geometry)?);
    }
    if let Some(value) = fields.bool("IsParagonModeActive") {
        settings.paragon_mode_active = value;
    }
    if let Some(value) = fields.float("ScanHeight") {
        if value > 0.0 {
            settings.scan_height_percent = value;
        }
    }
    // Upstream: SettingsD4.cs:83 ScreenCaptureDelay, default 50.
    if let Some(value) = fields.u32("ScreenCaptureDelay")? {
        settings.screen_capture_delay = value;
    }
    // Upstream: SettingsD4.cs:70 OverlayUpdateDelay, default 5.
    if let Some(value) = fields.u32("OverlayUpdateDelay")? {
        settings.overlay_update_delay = value;
    }
    if let Some(value) = fields.u8("ThresholdMin")? {
        settings.threshold_min = value;
    }
    if let Some(value) = fields.u8("MinimalOcrMatchType")? {
        settings.minimal_ocr_match_type = value;
    }
    if let Some(value) = fields.u8("ThresholdMax")? {
        settings.threshold_max = value;
    }
    if let Some(value) = fields.float("ThresholdSimilarityTooltip") {
        settings.threshold_similarity_tooltip = value;
    }
    if let Some(value) = fields.float("ThresholdSimilarityAffixLocation") {
        settings.threshold_similarity_affix_location = value;
    }
    if let Some(value) = fields.float("ThresholdSimilarityAspectLocation") {
        settings.threshold_similarity_aspect_location = value;
    }
    if let Some(value) = fields.float("ThresholdSimilaritySocketLocation") {
        settings.threshold_similarity_socket_location = value;
    }
    if let Some(value) = fields.float("ThresholdSimilaritySplitterLocation") {
        settings.threshold_similarity_splitter_location = value;
    }
    if let Some(value) = fields.u32("TooltipWidth")? {
        settings.tooltip_width = value;
    }
    if let Some(value) = fields.u32("TooltipMaxHeight")? {
        settings.tooltip_max_height = value;
    }
    if let Some(value) = fields.i32("AffixAreaHeightOffsetTop")? {
        settings.affix_area_height_offset_top = value;
    }
    if let Some(value) = fields.i32("AffixAreaHeightOffsetBottom")? {
        settings.affix_area_height_offset_bottom = value;
    }
    if let Some(value) = fields.i32("AffixAspectAreaWidthOffset")? {
        settings.affix_aspect_area_width_offset = value;
    }
    if let Some(value) = fields.i32("AspectAreaHeightOffsetTop")? {
        settings.aspect_area_height_offset_top = value;
    }
    if let Some(value) = fields.bool("IsSocketDetectionEnabled") {
        settings.is_socket_detection_enabled = value;
    }
    if let Some(value) = fields.bool("IsTemperedAffixDetectionEnabled") {
        settings.is_tempered_affix_detection_enabled = value;
    }
    if let Some(value) = fields.bool("IsOverlayEnabled") {
        settings.overlay_enabled = value;
    }
    if let Some(value) = fields.bool("IsTopMost") {
        settings.is_top_most = value;
    }
    if let Some(value) = fields.u32("OverlayFontSize")? {
        settings.overlay_font_size = value;
    }
    if let Some(value) = fields.bool("ShowCurrentItem") {
        settings.show_current_item = value;
    }
    if let Some(value) = fields.bool("ShowOverlayIcon") {
        settings.show_overlay_icon = value;
    }
    if let Some(value) = fields.u32("OverlayIconPosX")? {
        settings.overlay_icon_pos_x = value;
    }
    if let Some(value) = fields.u32("OverlayIconPosY")? {
        settings.overlay_icon_pos_y = value;
    }
    if let Some(value) = fields.u32("TypeAreaOffsetLeft")? {
        settings.type_area_offset_left = value;
    }
    if let Some(value) = fields.u32("TypeAreaOffsetRight")? {
        settings.type_area_offset_right = value;
    }
    if let Some(value) = fields.u32("AspectAreaMaxHeight")? {
        settings.aspect_area_max_height = value;
    }
    if let Some(value) = fields.i32("AspectIconOffset")? {
        settings.aspect_icon_offset = value;
    }
    if let Some(value) = fields.bool("IsOverlayMenuLocked") {
        settings.overlay_menu_locked = value;
    }
    if let Some(value) = fields.bool("IsImportParagonD2CoreEnabled") {
        settings.is_import_paragon_d2core_enabled = value;
    }
    if let Some(value) = fields.bool("IsImportParagonD4BuildsEnabled") {
        settings.is_import_paragon_d4builds_enabled = value;
    }
    if let Some(value) = fields.bool("IsImportParagonInfinityBuildsEnabled") {
        settings.is_import_paragon_infinity_builds_enabled = value;
    }
    if let Some(value) = fields.bool("IsImportParagonMaxrollEnabled") {
        settings.is_import_paragon_maxroll_enabled = value;
    }
    if let Some(value) = fields.bool("IsImportParagonMobalyticsEnabled") {
        settings.is_import_paragon_mobalytics_enabled = value;
    }
    if let Some(value) = fields.string("SelectedBuildImportWebsite") {
        settings.selected_build_import_website = value;
    }
    // d4lc-native builds directory (no upstream counterpart). Parsed only
    // when present so every settings file written before this field
    // existed loads with the `None` default — the state that keeps the
    // resolved `{config_root}/Builds/{provider}` home. A blank stored
    // value is treated as "not configured" so a hand-edited file can
    // never persist an unusable empty path.
    if let Some(value) = fields.non_blank("BuildsDir") {
        settings.builds_dir = Some(value);
    }
    if let Some(value) = fields.bool("IsTuningPrismHintsEnabled") {
        settings.is_tuning_prism_hints_enabled = value;
    }
    if let Some(value) = fields.bool("IsShowBrowserMobalyticsEnabled") {
        settings.is_show_browser_mobalytics_enabled = value;
    }
    for (key, slot) in [
        ("DefaultColorGreater", &mut settings.default_colors.greater),
        (
            "DefaultColorImplicit",
            &mut settings.default_colors.implicit,
        ),
        ("DefaultColorNormal", &mut settings.default_colors.normal),
        (
            "DefaultColorTempered",
            &mut settings.default_colors.tempered,
        ),
        ("DefaultColorAspects", &mut settings.default_colors.aspects),
        ("DefaultColorUniques", &mut settings.default_colors.uniques),
        ("DefaultColorRunes", &mut settings.default_colors.runes),
    ] {
        // Upstream writes a `Color` object (SettingsD4.cs:18-24); the older
        // string form is still read.
        if let Some(color) = fields.value(key).and_then(wpf_color::color_from_json) {
            *slot = color.to_string_form();
        }
    }
    if let Some(value) = fields.u32("ParagonBorderSize")? {
        settings.paragon_border_size = value;
    }
    if let Some(value) = fields.u32("ParagonNodeSize")? {
        settings.paragon_node_size = value;
    }
    if let Some(value) = fields.u32("ParagonNodeSizeCollapsed")? {
        settings.paragon_node_size_collapsed = value;
    }
    if let Some(value) = fields.i32("ParagonLeftOffsetCollapsed")? {
        settings.paragon_left_offset_collapsed = value;
    }
    if let Some(value) = fields.i32("ParagonTopOffsetCollapsed")? {
        settings.paragon_top_offset_collapsed = value;
    }
    if let Some(value) = fields.bool("IsDiagnosticsExportEnabled") {
        settings.is_diagnostics_export_enabled = value;
    }
    if let Some(value) = fields.bool("UpdateCheckEnabled") {
        settings.update_check_enabled = value;
    }
    parse_rule_settings(fields, &mut settings)?;
    parse_hotkey_settings(fields, &mut settings.hotkey_config);

    Ok(settings)
}

fn parse_rule_settings(fields: json::Fields<'_>, settings: &mut AppSettings) -> Result<(), String> {
    if let Some(value) = fields.bool("IsItemPowerLimitEnabled") {
        settings.rule_settings.item_power.enabled = value;
    }
    if let Some(value) = fields.u32("ItemPowerLimit")? {
        settings.rule_settings.item_power.limit = value;
    }
    if let Some(value) = fields.bool("IsMinimalAffixValueFilterEnabled") {
        settings.rule_settings.minimal_affix_value_enabled = value;
    }
    if let Some(value) = fields.bool("IsMultiBuildModeEnabled") {
        settings.rule_settings.multi_build_enabled = value;
    }
    if let Some(value) = fields.bool("IsTradeOverlayEnabled") {
        settings.rule_settings.trade_overlay_enabled = value;
    }
    if let Some(value) = fields.string("SelectedOverlayMarkerMode") {
        settings.rule_settings.overlay_marker_mode =
            OverlayMarkerModeSetting::from_upstream(&value);
    }
    if let Some(value) = fields.bool("IsAspectDetectionEnabled") {
        settings.rule_settings.aspect_detection_enabled = value;
    }
    if let Some(value) = fields.bool("IsUniqueDetectionEnabled") {
        settings.rule_settings.unique_detection_enabled = value;
    }
    if let Some(value) = fields.bool("IsRuneDetectionEnabled") {
        settings.rule_settings.rune_detection_enabled = value;
    }
    if let Some(value) = fields.bool("IsSigilDetectionEnabled") {
        settings.rule_settings.sigil_detection_enabled = value;
    }
    if let Some(value) = fields.string("SelectedSigilDisplayMode") {
        settings.rule_settings.sigil_display_mode = SigilDisplayModeSetting::from_upstream(&value);
    }
    if let Some(value) = fields.bool("DungeonTiers") {
        settings.rule_settings.dungeon_tiers_enabled = value;
    }
    if let Some(value) = fields.bool("IsCollapsedParagonboardEnabled") {
        settings.rule_settings.is_collapsed_paragonboard_enabled = value;
    }
    parse_rule_toggles(fields, &mut settings.rule_settings.toggles);
    if let Some(value) = fields.value("MultiBuildList") {
        settings.rule_settings.multi_builds = parse_multi_build_list(value)?;
    }
    Ok(())
}

fn parse_window_geometry(fields: json::Fields<'_>) -> Result<Rect, String> {
    let field = |key: &str| {
        fields
            .i32(key)?
            .ok_or_else(|| format!("Config/Settings.json WindowGeometry is missing {key}"))
    };
    let x = field("X")?;
    let y = field("Y")?;
    let width_i32 = field("Width")?;
    let height_i32 = field("Height")?;

    let width = u32::try_from(width_i32).map_err(|_| {
        format!("Config/Settings.json WindowGeometry.Width value {width_i32} is outside 0..=u32")
    })?;
    let height = u32::try_from(height_i32).map_err(|_| {
        format!("Config/Settings.json WindowGeometry.Height value {height_i32} is outside 0..=u32")
    })?;

    if width == 0 || height == 0 {
        return Err(
            "Config/Settings.json WindowGeometry must have non-zero Width and Height".to_string(),
        );
    }

    Ok(Rect {
        x,
        y,
        width,
        height,
    })
}

fn parse_rule_toggles(fields: json::Fields<'_>, toggles: &mut RuleToggleSettings) {
    if let Some(value) = fields.bool("IsToggleCoreActive") {
        toggles.class_toggles.core = value;
    }
    if let Some(value) = fields.bool("IsToggleBarbarianActive") {
        toggles.class_toggles.barbarian = value;
    }
    if let Some(value) = fields.bool("IsToggleDruidActive") {
        toggles.class_toggles.druid = value;
    }
    if let Some(value) = fields.bool("IsToggleNecromancerActive") {
        toggles.class_toggles.necromancer = value;
    }
    if let Some(value) = fields.bool("IsTogglePaladinActive") {
        toggles.class_toggles.paladin = value;
    }
    if let Some(value) = fields.bool("IsToggleRogueActive") {
        toggles.class_toggles.rogue = value;
    }
    if let Some(value) = fields.bool("IsToggleSorcererActive") {
        toggles.class_toggles.sorcerer = value;
    }
    if let Some(value) = fields.bool("IsToggleSpiritbornActive") {
        toggles.class_toggles.spiritborn = value;
    }
    if let Some(value) = fields.bool("IsToggleWarlockActive") {
        toggles.class_toggles.warlock = value;
    }
    if let Some(value) = fields.bool("IsToggleDungeonsActive") {
        toggles.dungeons = value;
    }
    if let Some(value) = fields.bool("IsTogglePositiveActive") {
        toggles.positive = value;
    }
    if let Some(value) = fields.bool("IsToggleMinorActive") {
        toggles.minor = value;
    }
    if let Some(value) = fields.bool("IsToggleMajorActive") {
        toggles.major = value;
    }
    if let Some(value) = fields.bool("IsToggleRuneConditionActive") {
        toggles.rune_condition = value;
    }
    if let Some(value) = fields.bool("IsToggleRuneEffectActive") {
        toggles.rune_effect = value;
    }
}

fn parse_hotkey_settings(fields: json::Fields<'_>, hotkey_config: &mut HotkeyConfig) {
    // Always reset to the upstream default first — missing or
    // partial JSON keeps the 7 known targets present so the UI
    // and OS-registration layers can iterate a stable list.
    *hotkey_config = HotkeyConfig::default_bindings();
    // Files written before the upstream shape was adopted carry a
    // `HotkeyBindings` array. Merge its entries over the default: an unknown
    // name is kept and the known targets are replaced by their parsed
    // counterpart.
    for binding in fields
        .objects("HotkeyBindings")
        .unwrap_or_default()
        .into_iter()
        .filter_map(parse_hotkey_binding)
    {
        hotkey_config.upsert(binding);
    }
    // Upstream's shape (SettingsD4.cs:99-153): one `KeyBindingConfig*` object
    // per target. It wins over the array, which upstream never writes.
    for (target, property, _) in wpf_key::UPSTREAM_HOTKEYS {
        let Some(object) = fields
            .value(property)
            .and_then(|value| json::Fields::of(value, SETTINGS_PATH))
        else {
            continue;
        };
        let Some(binding) = hotkey_config.find_by_name_mut(target) else {
            continue;
        };
        if let Some(enabled) = object.bool("IsEnabled") {
            binding.enabled = enabled;
        }
        // A present object deserializes into a fresh `KeyBindingConfig`
        // (KeyBindingConfig.cs:10-13), so a missing gesture field is 0.
        let key = object.int("KeyGestureKey").unwrap_or(0);
        let modifiers = object.int("KeyGestureModifier").unwrap_or(0);
        binding.binding = wpf_key::wpf_to_binding(
            i32::try_from(key).unwrap_or(0),
            i32::try_from(modifiers).unwrap_or(0),
        );
    }
}

/// Parse one `{ "Name": ..., "Label": ..., "Binding": ..., "Enabled": ... }`
/// object of the older `HotkeyBindings` array; an entry without a name is
/// skipped.
fn parse_hotkey_binding(fields: json::Fields<'_>) -> Option<HotkeyBinding> {
    let name = fields.non_blank("Name")?;
    Some(HotkeyBinding {
        label: fields.string("Label").unwrap_or_else(|| name.clone()),
        binding: fields.string("Binding").unwrap_or_default(),
        enabled: fields.bool("Enabled").unwrap_or(false),
        name,
    })
}

fn d4lc_capture_free_parse_manual_region(value: &str) -> Result<Rect, String> {
    let normalized = value.replace(['x', 'X', ':'], ",");
    let parts = normalized
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(format!(
            "manual region must have four values x,y,width,height; got '{value}'"
        ));
    }
    let x = parts[0]
        .parse::<i32>()
        .map_err(|error| format!("invalid manual region x '{}': {error}", parts[0]))?;
    let y = parts[1]
        .parse::<i32>()
        .map_err(|error| format!("invalid manual region y '{}': {error}", parts[1]))?;
    let width = parts[2]
        .parse::<u32>()
        .map_err(|error| format!("invalid manual region width '{}': {error}", parts[2]))?;
    let height = parts[3]
        .parse::<u32>()
        .map_err(|error| format!("invalid manual region height '{}': {error}", parts[3]))?;
    if width == 0 || height == 0 {
        return Err(format!(
            "manual region width and height must be non-zero; got {x},{y},{width},{height}"
        ));
    }
    Ok(Rect {
        x,
        y,
        width,
        height,
    })
}

fn parse_multi_build_list(value: &Value) -> Result<Vec<MultiBuildRuleConfig>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| "Config/Settings.json MultiBuildList must be a JSON array".to_string())?;
    Ok(items
        .iter()
        .filter_map(|item| json::Fields::of(item, SETTINGS_PATH))
        .map(|fields| MultiBuildRuleConfig {
            name: fields.string("Name").unwrap_or_default(),
            index: fields
                .int("Index")
                .and_then(|index| usize::try_from(index).ok())
                .unwrap_or(0),
            // MultiBuild.cs:7: a `Color` object; the older string form is
            // still read.
            color: fields
                .value("Color")
                .and_then(wpf_color::color_from_json)
                .map_or_else(|| "Green".to_string(), PresetColor::to_string_form),
        })
        .collect())
}

pub(crate) fn app_settings_json(settings: &AppSettings) -> String {
    let object = json::Object::new()
        .field("schema_version", SETTINGS_SCHEMA_VERSION)
        .field("SelectedSystemPreset", &settings.selected_system_preset)
        .field("SelectedAffixLanguage", &settings.selected_affix_language)
        .field("SelectedAppLanguage", &settings.selected_app_language)
        .field("SelectedAffixPreset", &settings.selected_affix_preset)
        .field("CaptureBackend", &settings.capture_backend)
        .field("DisplayBackend", &settings.display_backend)
        .field("WaylandRestoreToken", &settings.wayland_restore_token)
        .field("ControllerMode", settings.controller_mode)
        .field("ScanHeight", json::number(settings.scan_height_percent))
        .field("ScreenCaptureDelay", settings.screen_capture_delay)
        .field("OverlayUpdateDelay", settings.overlay_update_delay)
        .field("MinimalOcrMatchType", settings.minimal_ocr_match_type)
        .field("ThresholdMin", settings.threshold_min)
        .field("ThresholdMax", settings.threshold_max)
        .field(
            "ThresholdSimilarityTooltip",
            json::number(settings.threshold_similarity_tooltip),
        )
        .field(
            "ThresholdSimilarityAffixLocation",
            json::number(settings.threshold_similarity_affix_location),
        )
        .field(
            "ThresholdSimilarityAspectLocation",
            json::number(settings.threshold_similarity_aspect_location),
        )
        .field(
            "ThresholdSimilaritySocketLocation",
            json::number(settings.threshold_similarity_socket_location),
        )
        .field(
            "ThresholdSimilaritySplitterLocation",
            json::number(settings.threshold_similarity_splitter_location),
        )
        .field("TooltipWidth", settings.tooltip_width)
        .field("TooltipMaxHeight", settings.tooltip_max_height)
        .field(
            "AffixAreaHeightOffsetTop",
            settings.affix_area_height_offset_top,
        )
        .field(
            "AffixAreaHeightOffsetBottom",
            settings.affix_area_height_offset_bottom,
        )
        .field(
            "AffixAspectAreaWidthOffset",
            settings.affix_aspect_area_width_offset,
        )
        .field(
            "AspectAreaHeightOffsetTop",
            settings.aspect_area_height_offset_top,
        )
        .field(
            "IsSocketDetectionEnabled",
            settings.is_socket_detection_enabled,
        )
        .field(
            "IsTemperedAffixDetectionEnabled",
            settings.is_tempered_affix_detection_enabled,
        )
        .field("IsOverlayEnabled", settings.overlay_enabled)
        .field("IsTopMost", settings.is_top_most)
        .field("OverlayFontSize", settings.overlay_font_size)
        .field("ShowCurrentItem", settings.show_current_item)
        .field("ShowOverlayIcon", settings.show_overlay_icon)
        .field("OverlayIconPosX", settings.overlay_icon_pos_x)
        .field("OverlayIconPosY", settings.overlay_icon_pos_y)
        .field("TypeAreaOffsetLeft", settings.type_area_offset_left)
        .field("TypeAreaOffsetRight", settings.type_area_offset_right)
        .field("AspectAreaMaxHeight", settings.aspect_area_max_height)
        .field("AspectIconOffset", settings.aspect_icon_offset)
        .field("IsOverlayMenuLocked", settings.overlay_menu_locked)
        .field(
            "IsImportParagonD2CoreEnabled",
            settings.is_import_paragon_d2core_enabled,
        )
        .field(
            "IsImportParagonD4BuildsEnabled",
            settings.is_import_paragon_d4builds_enabled,
        )
        .field(
            "IsImportParagonInfinityBuildsEnabled",
            settings.is_import_paragon_infinity_builds_enabled,
        )
        .field(
            "IsImportParagonMaxrollEnabled",
            settings.is_import_paragon_maxroll_enabled,
        )
        .field(
            "IsImportParagonMobalyticsEnabled",
            settings.is_import_paragon_mobalytics_enabled,
        )
        .field(
            "SelectedBuildImportWebsite",
            &settings.selected_build_import_website,
        )
        .field("BuildsDir", &settings.builds_dir)
        .field(
            "IsTuningPrismHintsEnabled",
            settings.is_tuning_prism_hints_enabled,
        )
        .field(
            "IsShowBrowserMobalyticsEnabled",
            settings.is_show_browser_mobalytics_enabled,
        )
        .field(
            "DefaultColorGreater",
            color_object(&settings.default_colors.greater),
        )
        .field(
            "DefaultColorImplicit",
            color_object(&settings.default_colors.implicit),
        )
        .field(
            "DefaultColorNormal",
            color_object(&settings.default_colors.normal),
        )
        .field(
            "DefaultColorTempered",
            color_object(&settings.default_colors.tempered),
        )
        .field(
            "DefaultColorAspects",
            color_object(&settings.default_colors.aspects),
        )
        .field(
            "DefaultColorUniques",
            color_object(&settings.default_colors.uniques),
        )
        .field(
            "DefaultColorRunes",
            color_object(&settings.default_colors.runes),
        )
        .field("ParagonBorderSize", settings.paragon_border_size)
        .field("ParagonNodeSize", settings.paragon_node_size)
        .field(
            "ParagonNodeSizeCollapsed",
            settings.paragon_node_size_collapsed,
        )
        .field(
            "ParagonLeftOffsetCollapsed",
            settings.paragon_left_offset_collapsed,
        )
        .field(
            "ParagonTopOffsetCollapsed",
            settings.paragon_top_offset_collapsed,
        )
        .field("DebugMode", settings.debug_mode)
        .field("DevMode", settings.dev_mode)
        .field("IsDebugInfoEnabled", settings.is_debug_info_enabled)
        .field("CheckForUpdates", settings.check_for_updates)
        .field("LaunchMinimized", settings.launch_minimized)
        .field("MinimizeToTray", settings.minimize_to_tray)
        .field(
            "WindowGeometry",
            settings.window_geometry.map(window_geometry_object),
        )
        .field("IsParagonModeActive", settings.paragon_mode_active)
        .field(
            "IsItemPowerLimitEnabled",
            settings.rule_settings.item_power.enabled,
        )
        .field("ItemPowerLimit", settings.rule_settings.item_power.limit)
        .field(
            "IsMinimalAffixValueFilterEnabled",
            settings.rule_settings.minimal_affix_value_enabled,
        )
        .field(
            "IsMultiBuildModeEnabled",
            settings.rule_settings.multi_build_enabled,
        )
        .field(
            "IsTradeOverlayEnabled",
            settings.rule_settings.trade_overlay_enabled,
        )
        .field(
            "SelectedOverlayMarkerMode",
            settings.rule_settings.overlay_marker_mode.as_str(),
        )
        .field(
            "IsAspectDetectionEnabled",
            settings.rule_settings.aspect_detection_enabled,
        )
        .field(
            "IsUniqueDetectionEnabled",
            settings.rule_settings.unique_detection_enabled,
        )
        .field(
            "IsRuneDetectionEnabled",
            settings.rule_settings.rune_detection_enabled,
        )
        .field(
            "IsSigilDetectionEnabled",
            settings.rule_settings.sigil_detection_enabled,
        )
        .field(
            "SelectedSigilDisplayMode",
            settings.rule_settings.sigil_display_mode.as_str(),
        )
        .field("DungeonTiers", settings.rule_settings.dungeon_tiers_enabled)
        .field(
            "IsCollapsedParagonboardEnabled",
            settings.rule_settings.is_collapsed_paragonboard_enabled,
        )
        .field(
            "IsToggleCoreActive",
            settings.rule_settings.toggles.class_toggles.core,
        )
        .field(
            "IsToggleBarbarianActive",
            settings.rule_settings.toggles.class_toggles.barbarian,
        )
        .field(
            "IsToggleDruidActive",
            settings.rule_settings.toggles.class_toggles.druid,
        )
        .field(
            "IsToggleNecromancerActive",
            settings.rule_settings.toggles.class_toggles.necromancer,
        )
        .field(
            "IsTogglePaladinActive",
            settings.rule_settings.toggles.class_toggles.paladin,
        )
        .field(
            "IsToggleRogueActive",
            settings.rule_settings.toggles.class_toggles.rogue,
        )
        .field(
            "IsToggleSorcererActive",
            settings.rule_settings.toggles.class_toggles.sorcerer,
        )
        .field(
            "IsToggleSpiritbornActive",
            settings.rule_settings.toggles.class_toggles.spiritborn,
        )
        .field(
            "IsToggleWarlockActive",
            settings.rule_settings.toggles.class_toggles.warlock,
        )
        .field(
            "IsToggleDungeonsActive",
            settings.rule_settings.toggles.dungeons,
        )
        .field(
            "IsTogglePositiveActive",
            settings.rule_settings.toggles.positive,
        )
        .field("IsToggleMinorActive", settings.rule_settings.toggles.minor)
        .field("IsToggleMajorActive", settings.rule_settings.toggles.major)
        .field(
            "IsToggleRuneConditionActive",
            settings.rule_settings.toggles.rune_condition,
        )
        .field(
            "IsToggleRuneEffectActive",
            settings.rule_settings.toggles.rune_effect,
        )
        .field(
            "MultiBuildList",
            settings
                .rule_settings
                .multi_builds
                .iter()
                .map(multi_build_object)
                .collect::<Vec<_>>(),
        )
        .field(
            "IsDiagnosticsExportEnabled",
            settings.is_diagnostics_export_enabled,
        )
        .field("UpdateCheckEnabled", settings.update_check_enabled);
    json::to_file_text(&hotkey_objects(object, &settings.hotkey_config))
}

fn window_geometry_object(rect: Rect) -> json::Object {
    json::Object::new()
        .field("X", rect.x)
        .field("Y", rect.y)
        .field("Width", rect.width)
        .field("Height", rect.height)
}

/// The upstream `Color` object of a stored color string; a string that is not
/// a color reads as the upstream default, `Colors.Green`.
fn color_object(color: &str) -> json::Object {
    wpf_color::color_object(PresetColor::parse(color))
}

fn multi_build_object(build: &MultiBuildRuleConfig) -> json::Object {
    // MultiBuild.cs:7-9 declaration order.
    json::Object::new()
        .field("Color", color_object(&build.color))
        .field("Index", build.index)
        .field("Name", &build.name)
}

/// Append the seven `KeyBindingConfig*` objects upstream persists
/// (`SettingsD4.cs:99-153`, `KeyBindingConfig.cs:8-14`). Targets missing from
/// the config keep the upstream default; names upstream has no property for
/// are not persisted.
fn hotkey_objects(object: json::Object, hotkey_config: &HotkeyConfig) -> json::Object {
    let defaults = HotkeyConfig::default_bindings();
    wpf_key::UPSTREAM_HOTKEYS
        .iter()
        .fold(object, |object, (target, property, name)| {
            let Some(binding) = hotkey_config
                .find_by_name(target)
                .or_else(|| defaults.find_by_name(target))
            else {
                return object;
            };
            let (key, modifiers) = wpf_key::binding_to_wpf(&binding.binding);
            object.field(
                property,
                json::Object::new()
                    .field("IsEnabled", binding.enabled)
                    .field("Name", name)
                    .field("KeyGestureKey", key)
                    .field("KeyGestureModifier", modifiers),
            )
        })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::{create_file, create_temp_root, remove_dir};
    use crate::*;
    use std::env;
    use std::fs;

    #[test]
    fn parses_manual_capture_region() {
        assert_eq!(
            parse_manual_region("5,10,1920,1080"),
            Ok(Rect {
                x: 5,
                y: 10,
                width: 1920,
                height: 1080,
            })
        );
        assert!(parse_manual_region("5,10,0,1080").is_err());
    }

    #[test]
    fn show_current_item_defaults_true_and_survives_a_round_trip() {
        // Fresh install: no Settings.json exists yet, so the loader must
        // produce the upstream default (SettingsD4.cs:83, `true`).
        let root = create_temp_root("show_current_item_default_round_trip");
        let fresh = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load fresh settings: {error}"));
        assert!(
            fresh.show_current_item,
            "a fresh config must default ShowCurrentItem to true"
        );

        // Round trip: saving the fresh defaults and reloading keeps the
        // default on (the persisted file carries the value explicitly).
        save_app_settings(&root, &fresh)
            .unwrap_or_else(|error| panic!("failed to save settings: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to reload settings: {error}"));
        assert!(reloaded.show_current_item);
        let text = fs::read_to_string(root.join(SETTINGS_PATH)).expect("read persisted settings");
        assert!(
            text.contains("\"ShowCurrentItem\": true"),
            "the persisted file must carry the default explicitly: {text:.400}"
        );

        remove_dir(&root);
    }

    #[test]
    fn builds_dir_round_trips_and_defaults_to_unset() {
        // d4lc-native builds directory. A fresh install has no configured directory:
        // `None` is the state that keeps the resolved
        // `{config_root}/Builds/{provider}` home, and it is still written
        // explicitly so the persisted file stays self-describing.
        let root = create_temp_root("builds_dir_round_trip");
        let fresh = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load fresh settings: {error}"));
        assert_eq!(fresh.builds_dir, None);
        save_app_settings(&root, &fresh)
            .unwrap_or_else(|error| panic!("failed to save settings: {error}"));
        let text = fs::read_to_string(root.join(SETTINGS_PATH)).expect("read persisted settings");
        assert!(
            text.contains("\"BuildsDir\": null"),
            "an unset builds directory persists as null: {text:.400}"
        );

        // A configured directory survives save -> load verbatim.
        let configured = env::temp_dir().join("d4lc-config-builds-dir");
        let mut settings = fresh;
        settings.builds_dir = Some(configured.display().to_string());
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("failed to save configured settings: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to reload settings: {error}"));
        assert_eq!(reloaded.builds_dir, settings.builds_dir);
        let text = fs::read_to_string(root.join(SETTINGS_PATH)).expect("read persisted settings");
        assert!(
            text.contains(&format!("\"BuildsDir\": \"{}\"", configured.display())),
            "the configured directory must be written verbatim: {text:.400}"
        );

        remove_dir(&root);
    }

    #[test]
    fn builds_dir_absent_or_blank_stays_unset() {
        // Back-compat: files written before the field existed carry no
        // `BuildsDir` key, and a hand-edited blank value means the same
        // thing ("not configured"). Both keep the state that resolves the
        // `{config_root}/Builds/{provider}` home instead of an unusable
        // empty path.
        let root = create_temp_root("builds_dir_back_compat");
        create_file(
            &root.join(SETTINGS_PATH),
            r##"{
  "SelectedBuildImportWebsite": "Maxroll.gg",
  "ThresholdMin": 70
}"##,
        );
        let legacy = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load legacy settings: {error}"));
        assert_eq!(
            legacy.builds_dir, None,
            "a file without the key keeps the resolved default"
        );

        create_file(
            &root.join(SETTINGS_PATH),
            r##"{
  "BuildsDir": "   "
}"##,
        );
        let blank = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load blank settings: {error}"));
        assert_eq!(
            blank.builds_dir, None,
            "a blank stored value stays unset instead of an unusable empty path"
        );

        remove_dir(&root);
    }

    #[test]
    fn loads_upstream_shaped_settings_json() {
        let root = create_temp_root("loads_upstream_shaped_settings_json");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "SelectedSystemPreset": "1080p_SMF",
  "SelectedAffixLanguage": "deDE",
  "SelectedAppLanguage": "es-ES",
  "ControllerMode": true,
  "ScanHeight": 45,
  "ScreenCaptureDelay": 80,
  "OverlayUpdateDelay": 0,
  "OcrBackend": "leptess",
  "OcrProfile": "affix",
  "MinimalOcrMatchType": 82,
  "ThresholdMin": 71,
  "ThresholdMax": 250,
  "ThresholdSimilarityTooltip": 0.025,
  "TypeAreaOffsetLeft": 10,
  "TypeAreaOffsetRight": 8,
  "AspectAreaMaxHeight": 250,
  "AspectIconOffset": 75,
  "ShowCurrentItem": true,
  "IsImportParagonInfinityBuildsEnabled": false,
  "SelectedBuildImportWebsite": "D4Builds.gg",
  "IsTuningPrismHintsEnabled": true,
  "IsItemPowerLimitEnabled": true,
  "ItemPowerLimit": 925,
  "IsMinimalAffixValueFilterEnabled": true,
  "IsMultiBuildModeEnabled": true,
  "IsTradeOverlayEnabled": false,
  "SelectedOverlayMarkerMode": "Hide Unwanted",
  "IsAspectDetectionEnabled": false,
  "IsUniqueDetectionEnabled": false,
  "IsRuneDetectionEnabled": false,
  "SelectedSigilDisplayMode": "Blacklisting",
  "DungeonTiers": false,
  "IsToggleBarbarianActive": false,
  "IsToggleDungeonsActive": false,
  "IsToggleRuneEffectActive": false,
  "MultiBuildList": [
    { "Name": "Speed", "Index": 0, "Color": "Blue" },
    { "Name": "Boss", "Index": 1, "Color": "#FFAA00" }
  ]
}"##,
        );

        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.selected_system_preset, "1080p_SMF");
        assert_eq!(settings.selected_affix_language, "deDE");
        assert_eq!(settings.selected_app_language, "es-ES");
        assert!(settings.controller_mode);
        assert_eq!(settings.scan_height_percent, 45.0);
        assert_eq!(settings.screen_capture_delay, 80);
        assert_eq!(settings.overlay_update_delay, 0);
        // The retired port-own OCR keys load as ignored:
        // no field behind them, no error, and the next save drops them.
        assert_eq!(settings.minimal_ocr_match_type, 82);
        assert_eq!(settings.threshold_min, 71);
        assert_eq!(settings.threshold_max, 250);
        assert_eq!(settings.threshold_similarity_tooltip, 0.025);
        assert_eq!(settings.type_area_offset_left, 10);
        assert_eq!(settings.type_area_offset_right, 8);
        assert_eq!(settings.aspect_area_max_height, 250);
        assert_eq!(settings.aspect_icon_offset, 75);
        assert!(settings.show_current_item);
        assert!(!settings.is_import_paragon_infinity_builds_enabled);
        assert_eq!(settings.selected_build_import_website, "D4Builds.gg");
        assert!(settings.is_tuning_prism_hints_enabled);
        assert_eq!(
            settings.rule_settings.item_power,
            ItemPowerRuleSettings {
                enabled: true,
                limit: 925,
            }
        );
        assert!(settings.rule_settings.minimal_affix_value_enabled);
        assert!(settings.rule_settings.multi_build_enabled);
        assert!(!settings.rule_settings.trade_overlay_enabled);
        assert_eq!(
            settings.rule_settings.overlay_marker_mode,
            OverlayMarkerModeSetting::HideUnwanted
        );
        assert!(!settings.rule_settings.aspect_detection_enabled);
        assert!(!settings.rule_settings.unique_detection_enabled);
        assert!(!settings.rule_settings.rune_detection_enabled);
        assert_eq!(
            settings.rule_settings.sigil_display_mode,
            SigilDisplayModeSetting::Blacklisting
        );
        assert!(!settings.rule_settings.dungeon_tiers_enabled);
        assert!(!settings.rule_settings.toggles.class_toggles.barbarian);
        assert!(!settings.rule_settings.toggles.dungeons);
        assert!(!settings.rule_settings.toggles.rune_effect);
        assert_eq!(
            settings.rule_settings.multi_builds,
            vec![
                MultiBuildRuleConfig {
                    name: "Speed".to_string(),
                    index: 0,
                    color: "#0000FF".to_string(),
                },
                MultiBuildRuleConfig {
                    name: "Boss".to_string(),
                    index: 1,
                    color: "#FFAA00".to_string(),
                },
            ]
        );

        remove_dir(&root);
    }

    #[test]
    fn new_v5_4_settings_fields_default_and_round_trip_through_save_and_load() {
        // Upstream SettingsD4.cs defaults:
        // IsTuningPrismHintsEnabled = false (line 60),
        // IsImportParagonInfinityBuildsEnabled = true (line 32),
        // SelectedBuildImportWebsite = "Maxroll.gg" (line 81).
        let defaults = AppSettings::default();
        assert!(!defaults.is_tuning_prism_hints_enabled);
        assert!(defaults.is_import_paragon_infinity_builds_enabled);
        assert_eq!(defaults.selected_build_import_website, "Maxroll.gg");

        let root = create_temp_root(
            "new_v5_4_settings_fields_default_and_round_trip_through_save_and_load",
        );
        let settings = AppSettings {
            is_tuning_prism_hints_enabled: true,
            is_import_paragon_infinity_builds_enabled: false,
            selected_build_import_website: "D4Builds.gg".to_string(),
            ..AppSettings::default()
        };

        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));

        assert!(reloaded.is_tuning_prism_hints_enabled);
        assert!(!reloaded.is_import_paragon_infinity_builds_enabled);
        assert_eq!(reloaded.selected_build_import_website, "D4Builds.gg");

        remove_dir(&root);
    }

    #[test]
    fn collapsed_paragonboard_setting_defaults_and_round_trips() {
        // Upstream SettingsD4.cs line 28: IsCollapsedParagonboardEnabled
        // defaults to false and persists as a plain settings boolean.
        let defaults = AppSettings::default();
        assert!(!defaults.rule_settings.is_collapsed_paragonboard_enabled);

        let root = create_temp_root("collapsed_paragonboard_setting_defaults_and_round_trips");
        let mut settings = AppSettings::default();
        settings.rule_settings.is_collapsed_paragonboard_enabled = true;
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert!(reloaded.rule_settings.is_collapsed_paragonboard_enabled);

        // A pre-collapse settings file keeps loading with the upstream
        // default (missing field -> default, never an error).
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r#"{
  "schema_version": 1,
  "SelectedSystemPreset": "1440p_SMF"
}"#,
        );
        let legacy = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("legacy settings should load: {error}"));
        assert!(!legacy.rule_settings.is_collapsed_paragonboard_enabled);

        remove_dir(&root);
    }

    #[test]
    fn default_colors_default_to_upstream_green_and_round_trip() {
        // Upstream SettingsD4.cs:18-24: all seven DefaultColor* values
        // default to Colors.Green (WPF #008000); d4lc stores the string
        // form and persists the upstream field names.
        let defaults = AppSettings::default();
        assert_eq!(defaults.default_colors, DefaultColors::default());
        assert!(defaults
            .default_colors
            .fields()
            .iter()
            .all(|(_, value)| *value == DEFAULT_COLOR));

        let root = create_temp_root("default_colors_default_to_upstream_green_and_round_trip");
        let mut settings = AppSettings::default();
        assert!(settings.default_colors.set_field("greater", "#FFAA00"));
        assert!(!settings.default_colors.set_field("not-a-color", "#FFAA00"));
        settings.default_colors.set_field("runes", "Blue");
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));

        let persisted: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(SETTINGS_PATH))
                .unwrap_or_else(|error| panic!("read settings: {error}")),
        )
        .expect("written settings are JSON");
        assert_eq!(
            persisted["DefaultColorGreater"],
            serde_json::json!({
                "ColorContext": null,
                "A": 255,
                "R": 255,
                "G": 170,
                "B": 0,
                "ScA": 1,
                "ScR": 1,
                "ScG": 0.4019778,
                "ScB": 0
            }),
            "the upstream PascalCase key holds the Media.Color object"
        );

        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert_eq!(reloaded.default_colors.greater, "#FFAA00");
        assert_eq!(reloaded.default_colors.runes, "#0000FF");
        // The untouched classes keep the upstream default.
        assert_eq!(reloaded.default_colors.normal, DEFAULT_COLOR);

        remove_dir(&root);
    }

    #[test]
    fn paragon_layout_fields_default_and_round_trip() {
        // Upstream SettingsD4.cs:71-75 defaults.
        let defaults = AppSettings::default();
        assert_eq!(defaults.paragon_border_size, 2);
        assert_eq!(defaults.paragon_node_size, 40);
        assert_eq!(defaults.paragon_node_size_collapsed, 23);
        assert_eq!(defaults.paragon_left_offset_collapsed, -200);
        assert_eq!(defaults.paragon_top_offset_collapsed, -50);

        let root = create_temp_root("paragon_layout_fields_default_and_round_trip");
        let settings = AppSettings {
            paragon_border_size: 4,
            paragon_node_size: 55,
            paragon_node_size_collapsed: 30,
            paragon_left_offset_collapsed: -120,
            paragon_top_offset_collapsed: 75,
            ..AppSettings::default()
        };
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert_eq!(reloaded.paragon_border_size, 4);
        assert_eq!(reloaded.paragon_node_size, 55);
        assert_eq!(reloaded.paragon_node_size_collapsed, 30);
        assert_eq!(reloaded.paragon_left_offset_collapsed, -120);
        assert_eq!(reloaded.paragon_top_offset_collapsed, 75);

        remove_dir(&root);
    }

    #[test]
    fn absent_new_fields_keep_pre_v5_4_settings_json_valid() {
        // A settings file written before the v5.4.0 fields existed
        // must keep loading with the upstream defaults (migration
        // is "missing field -> default", never an error).
        let root = create_temp_root("absent_new_fields_keep_pre_v5_4_settings_json_valid");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "SelectedSystemPreset": "1440p_SMF",
  "SelectedAffixLanguage": "enUS",
  "TypeAreaOffsetLeft": 5,
  "IsImportParagonD2CoreEnabled": true
}"##,
        );
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert!(!settings.is_tuning_prism_hints_enabled);
        assert!(settings.is_import_paragon_infinity_builds_enabled);
        assert_eq!(settings.selected_build_import_website, "Maxroll.gg");
        // The remaining fields stay on their upstream defaults too.
        assert_eq!(settings.type_area_offset_right, 5);
        assert_eq!(settings.aspect_area_max_height, 300);
        assert_eq!(settings.aspect_icon_offset, 50);
        // DebugInfo (SettingsD4.cs:29) defaults to false when absent.
        assert!(!settings.is_debug_info_enabled);
        // And the hotkey list still carries all seven upstream
        // targets with the new binding default.
        assert_eq!(settings.hotkey_config.bindings.len(), 7);
        let prism = settings
            .hotkey_config
            .find_by_name("ToggleTuningPrismHints")
            .expect("default hotkey config should include ToggleTuningPrismHints");
        assert_eq!(prism.binding, "Ctrl+F7");
        assert!(!prism.enabled);

        remove_dir(&root);
    }

    #[test]
    fn window_geometry_loads_as_none_when_field_is_missing() {
        let root = create_temp_root("window_geometry_loads_as_none_when_field_is_missing");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "SelectedSystemPreset": "1440p_SMF",
  "MinimizeToTray": true
}"##,
        );

        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.window_geometry, None);

        remove_dir(&root);
    }

    #[test]
    fn window_geometry_round_trip_through_save_and_load() {
        let root = create_temp_root("window_geometry_round_trip_through_save_and_load");
        let settings = AppSettings {
            window_geometry: Some(Rect {
                x: 120,
                y: 240,
                width: 1150,
                height: 625,
            }),
            ..AppSettings::default()
        };

        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));

        assert_eq!(reloaded.window_geometry, settings.window_geometry);

        remove_dir(&root);
    }

    #[test]
    fn settings_ui_fields_load_with_upstream_defaults_and_persist() {
        let root = create_temp_root("settings_ui_fields_load_with_upstream_defaults_and_persist");
        let default_settings = AppSettings::default();

        assert_eq!(default_settings.capture_backend, "auto");
        assert_eq!(
            default_settings.selected_app_language, "en-US",
            "upstream default (SettingsD4.cs:79)"
        );
        assert_eq!(default_settings.display_backend, DEFAULT_DISPLAY_BACKEND);
        assert_eq!(default_settings.tooltip_width, 500);
        assert_eq!(default_settings.tooltip_max_height, 200);
        assert_eq!(default_settings.screen_capture_delay, 50);
        assert_eq!(default_settings.overlay_update_delay, 5);
        assert_eq!(default_settings.minimal_ocr_match_type, 75);
        assert_eq!(default_settings.type_area_offset_left, 5);
        assert!(
            default_settings.show_current_item,
            "upstream default true (SettingsD4.cs:83)"
        );
        assert!(!default_settings.show_overlay_icon);
        assert_eq!(default_settings.overlay_icon_pos_x, 0);
        assert_eq!(default_settings.overlay_icon_pos_y, 0);
        assert!(default_settings.is_import_paragon_d2core_enabled);
        assert!(default_settings.is_import_paragon_infinity_builds_enabled);
        assert_eq!(
            default_settings.selected_build_import_website,
            DEFAULT_SELECTED_BUILD_IMPORT_WEBSITE
        );
        assert!(!default_settings.is_tuning_prism_hints_enabled);
        assert!(!default_settings.is_show_browser_mobalytics_enabled);
        assert_eq!(default_settings.window_geometry, None);

        let settings = AppSettings {
            selected_system_preset: "1080p_SMF".to_string(),
            selected_affix_language: "frFR".to_string(),
            selected_app_language: "de-DE".to_string(),
            capture_backend: "x11-native".to_string(),
            display_backend: "wayland".to_string(),
            controller_mode: true,
            window_geometry: Some(Rect {
                x: 64,
                y: 96,
                width: 1280,
                height: 720,
            }),
            tooltip_width: 640,
            tooltip_max_height: 260,
            screen_capture_delay: 120,
            overlay_update_delay: 15,
            threshold_min: 80,
            threshold_max: 240,
            threshold_similarity_tooltip: 0.03,
            type_area_offset_left: 10,
            type_area_offset_right: 12,
            aspect_area_max_height: 260,
            aspect_icon_offset: 80,
            is_debug_info_enabled: true,
            is_socket_detection_enabled: false,
            is_tempered_affix_detection_enabled: false,
            overlay_enabled: true,
            is_top_most: true,
            overlay_font_size: 22,
            show_current_item: true,
            show_overlay_icon: true,
            overlay_icon_pos_x: 125,
            overlay_icon_pos_y: 875,
            overlay_menu_locked: true,
            is_import_paragon_d2core_enabled: false,
            is_import_paragon_infinity_builds_enabled: false,
            selected_build_import_website: "Mobalytics.gg".to_string(),
            is_tuning_prism_hints_enabled: true,
            is_show_browser_mobalytics_enabled: true,
            rule_settings: RuleSettingsConfig {
                item_power: ItemPowerRuleSettings {
                    enabled: true,
                    limit: 925,
                },
                minimal_affix_value_enabled: true,
                multi_build_enabled: true,
                aspect_detection_enabled: false,
                unique_detection_enabled: false,
                rune_detection_enabled: false,
                sigil_detection_enabled: false,
                toggles: RuleToggleSettings {
                    class_toggles: ClassToggleSettings {
                        barbarian: false,
                        ..ClassToggleSettings::default()
                    },
                    rune_effect: false,
                    ..RuleToggleSettings::default()
                },
                multi_builds: vec![MultiBuildRuleConfig {
                    name: "Speed".to_string(),
                    index: 1,
                    color: "#00FF00".to_string(),
                }],
                ..RuleSettingsConfig::default()
            },
            ..AppSettings::default()
        };

        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));

        assert_eq!(reloaded.selected_system_preset, "1080p_SMF");
        assert_eq!(reloaded.selected_affix_language, "frFR");
        assert_eq!(reloaded.selected_app_language, "de-DE");
        assert_eq!(reloaded.capture_backend, "x11-native");
        assert_eq!(reloaded.display_backend, "wayland");
        assert!(reloaded.controller_mode);
        assert_eq!(reloaded.tooltip_width, 640);
        assert_eq!(reloaded.tooltip_max_height, 260);
        assert_eq!(reloaded.screen_capture_delay, 120);
        assert_eq!(reloaded.overlay_update_delay, 15);
        assert_eq!(reloaded.threshold_min, 80);
        assert_eq!(reloaded.threshold_max, 240);
        assert_eq!(reloaded.threshold_similarity_tooltip, 0.03);
        assert!(reloaded.rule_settings.item_power.enabled);
        assert_eq!(reloaded.rule_settings.item_power.limit, 925);
        assert!(reloaded.rule_settings.minimal_affix_value_enabled);
        assert!(reloaded.rule_settings.multi_build_enabled);
        assert!(!reloaded.rule_settings.aspect_detection_enabled);
        assert!(!reloaded.rule_settings.unique_detection_enabled);
        assert!(!reloaded.rule_settings.rune_detection_enabled);
        assert!(!reloaded.rule_settings.sigil_detection_enabled);
        assert!(!reloaded.rule_settings.toggles.class_toggles.barbarian);
        assert!(!reloaded.rule_settings.toggles.rune_effect);
        assert_eq!(
            reloaded.rule_settings.multi_builds,
            vec![MultiBuildRuleConfig {
                name: "Speed".to_string(),
                index: 1,
                color: "#00FF00".to_string(),
            }]
        );
        assert!(!reloaded.is_socket_detection_enabled);
        assert!(!reloaded.is_tempered_affix_detection_enabled);
        assert!(reloaded.overlay_enabled);
        assert!(reloaded.is_top_most);
        assert_eq!(reloaded.overlay_font_size, 22);
        assert!(reloaded.show_current_item);
        assert!(reloaded.show_overlay_icon);
        assert_eq!(reloaded.overlay_icon_pos_x, 125);
        assert_eq!(reloaded.overlay_icon_pos_y, 875);
        assert_eq!(reloaded.type_area_offset_left, 10);
        assert_eq!(reloaded.type_area_offset_right, 12);
        assert_eq!(reloaded.aspect_area_max_height, 260);
        assert_eq!(reloaded.aspect_icon_offset, 80);
        assert!(reloaded.is_debug_info_enabled);
        assert!(reloaded.overlay_menu_locked);
        assert!(!reloaded.is_import_paragon_d2core_enabled);
        assert!(!reloaded.is_import_paragon_infinity_builds_enabled);
        assert_eq!(reloaded.selected_build_import_website, "Mobalytics.gg");
        assert!(reloaded.is_tuning_prism_hints_enabled);
        assert!(reloaded.is_show_browser_mobalytics_enabled);
        assert_eq!(
            reloaded.window_geometry,
            Some(Rect {
                x: 64,
                y: 96,
                width: 1280,
                height: 720,
            })
        );

        remove_dir(&root);
    }

    // -----------------------------------------------------------------------
    // Hotkey binding config
    // -----------------------------------------------------------------------

    #[test]
    fn app_language_defaults_when_missing_or_blank() {
        // `SelectedAppLanguage` is independent of the game-data language and
        // falls back to the upstream default (SettingsD4.cs:79).
        let root = create_temp_root("app_language_defaults_when_missing_or_blank");
        let path = root.join(SETTINGS_PATH);
        create_file(&path, r##"{ "SelectedAffixLanguage": "deDE" }"##);
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.selected_app_language, "en-US");
        assert_eq!(settings.selected_affix_language, "deDE");
        create_file(&path, r##"{ "SelectedAppLanguage": "  " }"##);
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.selected_app_language, "en-US");
        remove_dir(&root);
    }

    #[test]
    fn hotkey_settings_load_when_field_is_missing() {
        // Backward compat: legacy settings files without a
        // HotkeyBindings field must produce the upstream default
        // (seven disabled entries).
        let root = create_temp_root("hotkey_settings_load_when_field_is_missing");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "SelectedSystemPreset": "1440p_SMF",
  "SelectedAffixLanguage": "enUS"
}"##,
        );
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.hotkey_config.bindings.len(), 7);
        assert!(settings.hotkey_config.bindings.iter().all(|b| !b.enabled));
        assert_eq!(
            settings
                .hotkey_config
                .find_by_name("SwitchPreset")
                .expect("loaded hotkey config should include SwitchPreset")
                .binding,
            "Ctrl+F5"
        );
        remove_dir(&root);
    }

    #[test]
    fn hotkey_settings_load_merges_parsed_over_default() {
        // When the file overrides SwitchPreset, the parsed
        // value must replace the default entry. The other six
        // entries keep their upstream defaults so the list is
        // always 7 entries long.
        let root = create_temp_root("hotkey_settings_load_merges_parsed_over_default");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "HotkeyBindings": [
    { "Name": "SwitchPreset", "Label": "Switch preset", "Binding": "Ctrl+Shift+F5", "Enabled": true }
  ]
}"##,
        );
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.hotkey_config.bindings.len(), 7);
        let switch_preset = settings
            .hotkey_config
            .find_by_name("SwitchPreset")
            .expect("merged hotkey config should include SwitchPreset");
        assert_eq!(switch_preset.binding, "Ctrl+Shift+F5");
        assert!(switch_preset.enabled);
        // Other targets keep their upstream defaults.
        assert_eq!(
            settings
                .hotkey_config
                .find_by_name("SwitchOverlay")
                .expect("merged hotkey config should include SwitchOverlay")
                .binding,
            "Ctrl+F6"
        );
        assert!(
            !settings
                .hotkey_config
                .find_by_name("SwitchOverlay")
                .expect("merged hotkey config should include SwitchOverlay")
                .enabled
        );
        remove_dir(&root);
    }

    const UPSTREAM_SETTINGS: &str = include_str!("../../tests/fixtures/upstream-Settings.json");

    fn upstream_fixture_value() -> Value {
        serde_json::from_str(UPSTREAM_SETTINGS).expect("fixture is valid JSON")
    }

    #[test]
    fn hotkeys_load_from_the_upstream_settings_shape() {
        // Fixture: a Settings.json as upstream's `SettingsManager.SaveSettings`
        // writes it (see tests/fixtures/upstream-Settings.json).
        let root = create_temp_root("hotkeys_load_from_the_upstream_settings_shape");
        create_file(&root.join(SETTINGS_PATH), UPSTREAM_SETTINGS);
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        let loaded: Vec<(&str, &str, bool)> = settings
            .hotkey_config
            .bindings
            .iter()
            .map(|b| (b.name.as_str(), b.binding.as_str(), b.enabled))
            .collect();
        assert_eq!(
            loaded,
            vec![
                ("SwitchPreset", "Ctrl+Shift+F5", true),
                ("SwitchOverlay", "Ctrl+F6", false),
                ("TakeScreenshot", "Ctrl+F10", true),
                ("ToggleController", "Ctrl+F9", false),
                ("ToggleOverlay", "Alt+A", true),
                ("ToggleDebugLockScreencapture", "Ctrl+F11", false),
                ("ToggleTuningPrismHints", "Ctrl+F7", false),
            ]
        );
        remove_dir(&root);
    }

    #[test]
    fn hotkeys_are_written_in_the_upstream_settings_shape() {
        let root = create_temp_root("hotkeys_are_written_in_the_upstream_settings_shape");
        let mut settings = AppSettings::default();
        settings
            .hotkey_config
            .find_by_name_mut("SwitchPreset")
            .expect("default hotkey config should include SwitchPreset")
            .enabled = true;
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let written: Value =
            serde_json::from_str(&fs::read_to_string(root.join(SETTINGS_PATH)).expect("read"))
                .expect("written settings are JSON");
        assert!(written.get("HotkeyBindings").is_none());
        assert_eq!(
            written["KeyBindingConfigSwitchPreset"],
            serde_json::json!({
                "IsEnabled": true,
                "Name": "Switch Preset",
                "KeyGestureKey": 94,
                "KeyGestureModifier": 2
            })
        );
        // SettingsD4.cs:99-153: defaults of the other six targets.
        for (property, name, key) in [
            ("KeyBindingConfigSwitchOverlay", "Switch Overlay", 95),
            ("KeyBindingConfigTakeScreenshot", "Take Screenshot", 99),
            ("KeyBindingConfigToggleController", "Toggle Controller", 98),
            (
                "KeyBindingConfigToggleTuningPrismHints",
                "Toggle Tuning Prism Hints",
                96,
            ),
            ("KeyBindingConfigToggleOverlay", "Toggle Overlay", 101),
            (
                "KeyBindingConfigToggleDebugLockScreencapture",
                "Debug: Toggle Lock Screencapture",
                100,
            ),
        ] {
            assert_eq!(
                written[property],
                serde_json::json!({
                    "IsEnabled": false,
                    "Name": name,
                    "KeyGestureKey": key,
                    "KeyGestureModifier": 2
                }),
                "{property}"
            );
        }
        remove_dir(&root);
    }

    #[test]
    fn upstream_hotkey_objects_survive_a_load_and_save() {
        let root = create_temp_root("upstream_hotkey_objects_survive_a_load_and_save");
        create_file(&root.join(SETTINGS_PATH), UPSTREAM_SETTINGS);
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let written: Value =
            serde_json::from_str(&fs::read_to_string(root.join(SETTINGS_PATH)).expect("read"))
                .expect("written settings are JSON");
        let fixture = upstream_fixture_value();
        for (_, property, _) in wpf_key::UPSTREAM_HOTKEYS {
            assert_eq!(written[*property], fixture[*property], "{property}");
        }
        remove_dir(&root);
    }

    #[test]
    fn older_hotkey_array_migrates_to_the_upstream_shape_on_save() {
        let root = create_temp_root("older_hotkey_array_migrates_to_the_upstream_shape_on_save");
        create_file(
            &root.join(SETTINGS_PATH),
            r##"{
  "HotkeyBindings": [
    { "Name": "ToggleOverlay", "Label": "Toggle overlay on/off", "Binding": "Ctrl+Alt+F8", "Enabled": true }
  ]
}"##,
        );
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let written: Value =
            serde_json::from_str(&fs::read_to_string(root.join(SETTINGS_PATH)).expect("read"))
                .expect("written settings are JSON");
        assert!(written.get("HotkeyBindings").is_none());
        assert_eq!(
            written["KeyBindingConfigToggleOverlay"],
            serde_json::json!({
                "IsEnabled": true,
                "Name": "Toggle Overlay",
                "KeyGestureKey": 97,
                "KeyGestureModifier": 3
            })
        );
        remove_dir(&root);
    }

    #[test]
    fn upstream_hotkey_object_wins_over_the_older_array() {
        let root = create_temp_root("upstream_hotkey_object_wins_over_the_older_array");
        create_file(
            &root.join(SETTINGS_PATH),
            r##"{
  "HotkeyBindings": [
    { "Name": "SwitchPreset", "Label": "Switch preset", "Binding": "Ctrl+F1", "Enabled": true }
  ],
  "KeyBindingConfigSwitchPreset": { "IsEnabled": false, "KeyGestureKey": 95, "KeyGestureModifier": 4 }
}"##,
        );
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        let binding = settings
            .hotkey_config
            .find_by_name("SwitchPreset")
            .expect("default hotkey config should include SwitchPreset");
        assert_eq!(binding.binding, "Shift+F6");
        assert!(!binding.enabled);
        remove_dir(&root);
    }

    #[test]
    fn colors_load_from_the_upstream_settings_shape() {
        let root = create_temp_root("colors_load_from_the_upstream_settings_shape");
        create_file(&root.join(SETTINGS_PATH), UPSTREAM_SETTINGS);
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.default_colors.greater, "#FFAA00");
        assert_eq!(settings.default_colors.normal, "#0000FF");
        assert_eq!(settings.default_colors.implicit, DEFAULT_COLOR);
        assert_eq!(
            settings.rule_settings.multi_builds,
            vec![
                MultiBuildRuleConfig {
                    name: "Whirlwind".to_string(),
                    index: 0,
                    color: "#FF0000".to_string(),
                },
                MultiBuildRuleConfig {
                    name: "Bleed".to_string(),
                    index: 1,
                    color: "#008000".to_string(),
                },
            ]
        );
        remove_dir(&root);
    }

    #[test]
    fn colors_survive_a_load_and_save_of_the_upstream_shape() {
        let root = create_temp_root("colors_survive_a_load_and_save_of_the_upstream_shape");
        create_file(&root.join(SETTINGS_PATH), UPSTREAM_SETTINGS);
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let written: Value =
            serde_json::from_str(&fs::read_to_string(root.join(SETTINGS_PATH)).expect("read"))
                .expect("written settings are JSON");
        let fixture = upstream_fixture_value();
        for key in [
            "DefaultColorGreater",
            "DefaultColorImplicit",
            "DefaultColorNormal",
            "DefaultColorTempered",
            "DefaultColorAspects",
            "DefaultColorUniques",
            "DefaultColorRunes",
            "MultiBuildList",
        ] {
            assert_eq!(written[key], fixture[key], "{key}");
        }
        remove_dir(&root);
    }

    #[test]
    fn older_string_colors_migrate_to_the_upstream_shape_on_save() {
        let root = create_temp_root("older_string_colors_migrate_to_the_upstream_shape_on_save");
        create_file(
            &root.join(SETTINGS_PATH),
            r##"{
  "DefaultColorRunes": "#FFAA00",
  "MultiBuildList": [ { "Name": "Speed", "Index": 2, "Color": "Blue" } ]
}"##,
        );
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(settings.default_colors.runes, "#FFAA00");
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let written: Value =
            serde_json::from_str(&fs::read_to_string(root.join(SETTINGS_PATH)).expect("read"))
                .expect("written settings are JSON");
        assert_eq!(written["DefaultColorRunes"]["G"], 170);
        assert_eq!(written["MultiBuildList"][0]["Color"]["B"], 255);
        assert_eq!(written["MultiBuildList"][0]["Name"], "Speed");
        remove_dir(&root);
    }

    #[test]
    fn hotkey_settings_round_trip_through_save_and_load() {
        let root = create_temp_root("hotkey_settings_round_trip_through_save_and_load");
        let mut settings = AppSettings::default();
        // User toggles two bindings.
        settings
            .hotkey_config
            .find_by_name_mut("SwitchPreset")
            .expect("default hotkey config should include mutable SwitchPreset")
            .enabled = true;
        settings
            .hotkey_config
            .find_by_name_mut("TakeScreenshot")
            .expect("default hotkey config should include mutable TakeScreenshot")
            .binding = "Ctrl+Shift+F10".to_string();
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert_eq!(reloaded.hotkey_config.bindings.len(), 7);
        assert!(
            reloaded
                .hotkey_config
                .find_by_name("SwitchPreset")
                .expect("reloaded hotkey config should include SwitchPreset")
                .enabled
        );
        assert_eq!(
            reloaded
                .hotkey_config
                .find_by_name("TakeScreenshot")
                .expect("reloaded hotkey config should include TakeScreenshot")
                .binding,
            "Ctrl+Shift+F10"
        );
        // The other 5 entries keep their upstream defaults.
        assert!(
            !reloaded
                .hotkey_config
                .find_by_name("SwitchOverlay")
                .expect("reloaded hotkey config should include SwitchOverlay")
                .enabled
        );
        assert_eq!(
            reloaded
                .hotkey_config
                .find_by_name("SwitchOverlay")
                .expect("reloaded hotkey config should include SwitchOverlay")
                .binding,
            "Ctrl+F6"
        );
        remove_dir(&root);
    }

    #[test]
    fn hotkey_settings_legacy_settings_json_unchanged_when_default_loaded() {
        // Loading the default AppSettings from a non-existent
        // settings file produces the same shape as a fresh
        // default: 7 disabled hotkey bindings, so settings files
        // without hotkeys need no migration.
        let root =
            create_temp_root("hotkey_settings_legacy_settings_json_unchanged_when_default_loaded");
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load default settings: {error}"));
        assert_eq!(settings.hotkey_config.bindings.len(), 7);
        assert!(settings.hotkey_config.bindings.iter().all(|b| !b.enabled));
        remove_dir(&root);
    }

    #[test]
    fn diagnostics_export_toggle_defaults_off_and_round_trips() {
        // The diagnostics export toggle is opt-in: defaults, legacy
        // files without the field, and explicit `false` all load
        // with the export path disabled; an explicit `true` survives
        // a save/load round trip without a schema_version bump.
        assert!(!AppSettings::default().is_diagnostics_export_enabled);

        let root = create_temp_root("diagnostics_export_toggle_defaults_off_and_round_trips");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "SelectedSystemPreset": "1440p_SMF"
}"##,
        );
        let legacy = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert!(
            !legacy.is_diagnostics_export_enabled,
            "legacy files without the field must load with export disabled"
        );

        let mut enabled = legacy.clone();
        enabled.is_diagnostics_export_enabled = true;
        save_app_settings(&root, &enabled)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert!(reloaded.is_diagnostics_export_enabled);
        remove_dir(&root);
    }

    #[test]
    fn update_check_toggle_defaults_on_and_round_trips() {
        // The release check is on by default: defaults, legacy files
        // without the field, and an explicit `true` all load enabled;
        // an explicit `false` survives a save/load round trip without
        // a schema_version bump.
        assert!(AppSettings::default().update_check_enabled);

        let root = create_temp_root("update_check_toggle_defaults_on_and_round_trips");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "SelectedSystemPreset": "1440p_SMF"
}"##,
        );
        let legacy = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert!(
            legacy.update_check_enabled,
            "legacy files without the field must load with the check enabled"
        );

        let mut disabled = legacy.clone();
        disabled.update_check_enabled = false;
        save_app_settings(&root, &disabled)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert!(!reloaded.update_check_enabled);
        remove_dir(&root);
    }

    #[test]
    fn upstream_check_for_updates_key_is_honored_and_survives_a_round_trip() {
        // An upstream-authored Settings.json carries `CheckForUpdates`
        // and no `UpdateCheckEnabled`; the port must honor the upstream
        // value instead of falling back to its own default.
        let root = create_temp_root("upstream_check_for_updates_is_honored");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "CheckForUpdates": false
}"##,
        );
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert!(
            !reloaded.check_for_updates,
            "the upstream toggle value must be parsed"
        );
        assert!(
            !reloaded.update_check_effective(),
            "an upstream file with the check off must stay off"
        );

        save_app_settings(&root, &reloaded)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let rewritten = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert!(!rewritten.check_for_updates);
        assert!(!rewritten.update_check_effective());
        remove_dir(&root);
    }

    #[test]
    fn dev_mode_parses_the_upstream_key_and_survives_a_round_trip() {
        // Upstream `SettingsD4.DevMode` (SettingsD4.cs:25, default
        // false) is hand-edited only; the port keeps the key across a
        // save so the settings screen's dev-mode-gated rows stay
        // reachable the way upstream's do.
        assert!(!AppSettings::default().dev_mode);

        let root = create_temp_root("dev_mode_round_trips");
        let path = root.join(SETTINGS_PATH);
        create_file(
            &path,
            r##"{
  "DevMode": true,
  "DebugMode": true
}"##,
        );
        let settings = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert!(settings.dev_mode, "the upstream DevMode key must be parsed");
        assert!(settings.debug_mode);

        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read persisted settings: {error}"));
        assert!(
            text.contains("\"DevMode\": true"),
            "a hand-edited DevMode must survive the rewrite: {text}"
        );
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("settings should reload: {error}"));
        assert!(reloaded.dev_mode);
        remove_dir(&root);
    }

    // -----------------------------------------------------------------------
    // Settings schema_version migration
    // -----------------------------------------------------------------------

    #[test]
    fn selected_affix_preset_round_trips_through_save_and_load() {
        let root = create_temp_root("selected_affix_preset_round_trips");
        let settings = AppSettings {
            selected_affix_preset: "Speed".to_string(),
            ..AppSettings::default()
        };
        save_app_settings(&root, &settings).unwrap_or_else(|error| panic!("save failed: {error}"));

        // Round-trip through the full settings file.
        let reloaded =
            load_app_settings(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(reloaded.selected_affix_preset, "Speed");

        // The persisted key is the upstream SettingsD4.cs:80 name.
        let text = fs::read_to_string(root.join(SETTINGS_PATH))
            .unwrap_or_else(|error| panic!("failed to read settings: {error}"));
        assert!(text.contains("\"SelectedAffixPreset\": \"Speed\""));

        // A file without the field backfills the upstream default.
        create_file(
            &root.join(SETTINGS_PATH),
            "{\n  \"SelectedSystemPreset\": \"1080p_SMF\"\n}",
        );
        let legacy =
            load_app_settings(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(legacy.selected_affix_preset, "");

        remove_dir(&root);
    }
}
