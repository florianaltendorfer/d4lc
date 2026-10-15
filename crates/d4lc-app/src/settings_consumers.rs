//! Settings consumer matrix — every stored setting, the runtime part that
//! consumes it, and how a change reaches that consumer.
//!
//! The matrix is code, not only documentation: the settings hot-reload
//! monitor (`runtime_bridge::settings_reload`) decides from it whether a
//! changed `Config/Settings.json` needs a new runtime preparation, a runtime
//! restart, or nothing, and [`setting_entries`] destructures `AppSettings`
//! exhaustively, so a new settings field does not compile until it is
//! classified here.
//!
//! Besides `Settings.json`, the runtime preparation reads three asset sets
//! whose change must replace the running detector or OCR preparation just
//! the same: the controller image list (`Config/Controllers.json`),
//! the selected preset's template files, and the language data behind the
//! fuzzy catalogs. [`AssetStamps`] observes them.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::UNIX_EPOCH;

use d4lc_config::AppSettings;

/// How a changed setting reaches its consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateKind {
    /// Drawn or shown with the next presentation; no new analysis.
    Presentation,
    /// A new rules snapshot revision re-evaluates the accepted analysis
    /// without new OCR (`runtime_bridge::live_state::RulesSnapshot`).
    Reevaluation,
    /// A new OCR preparation: the live loop re-prepares its runtime (new
    /// OCR worker), bumps the applied configuration revision, and discards
    /// every OCR result of the old configuration.
    OcrPreparation,
    /// A new detector: the same runtime re-preparation, with new templates
    /// and thresholds.
    Detector,
    /// Read once when the runtime starts; applies after a runtime restart.
    SessionRestart,
    /// Not a runtime setting: companion shell, imports, persistence.
    ///
    /// Every stored value has a live consumer, except the documented
    /// [`Self::StoredOnly`] class, and the register asserts it.
    NotRuntime,
    /// Persisted for `Settings.json` compatibility only: upstream has no
    /// consumer for the value either, so the port does not invent one. The
    /// entry's consumer text names the upstream evidence.
    StoredOnly,
}

impl UpdateKind {
    #[cfg(test)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Presentation => "presentation",
            Self::Reevaluation => "re-evaluation",
            Self::OcrPreparation => "new OCR preparation",
            Self::Detector => "new detector",
            Self::SessionRestart => "runtime restart",
            Self::NotRuntime => "not a runtime setting",
            Self::StoredOnly => "stored only (no upstream consumer)",
        }
    }

    /// Whether a change needs a runtime re-preparation (hot reload).
    pub const fn requires_reprepare(self) -> bool {
        matches!(self, Self::OcrPreparation | Self::Detector)
    }

    /// Whether the live runtime reads the value from the app state (so an
    /// in-app change is live at once, an external file edit is not).
    pub const fn follows_app_state(self) -> bool {
        matches!(self, Self::Presentation | Self::Reevaluation)
    }
}

/// One classified setting with its current value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingEntry {
    /// The `AppSettings` field name, or `field.member` when one member of a
    /// grouped field has a different consumer than the rest of the group.
    pub key: &'static str,
    pub kind: UpdateKind,
    /// Who reads it.
    pub consumer: &'static str,
    /// Comparable rendering of the value.
    pub value: String,
}

fn entry(
    key: &'static str,
    kind: UpdateKind,
    consumer: &'static str,
    value: &impl std::fmt::Debug,
) -> SettingEntry {
    SettingEntry {
        key,
        kind,
        consumer,
        value: format!("{value:?}"),
    }
}

/// Names every `AppSettings` field exactly once. The destructuring stops
/// compiling when a field is added or removed, and the name list backs the
/// test that every field has a classified entry.
macro_rules! name_every_setting_field {
    ($($field:ident),* $(,)?) => {
        /// The `AppSettings` field names, as written in the guard.
        #[cfg(test)]
        const SETTING_FIELDS: &[&str] = &[$(stringify!($field)),*];

        fn name_every_field(settings: &AppSettings) {
            let AppSettings { $($field: _),* } = settings;
        }
    };
}

name_every_setting_field!(
    selected_system_preset,
    selected_affix_language,
    selected_app_language,
    selected_affix_preset,
    capture_backend,
    display_backend,
    wayland_restore_token,
    controller_mode,
    debug_mode,
    dev_mode,
    is_debug_info_enabled,
    check_for_updates,
    launch_minimized,
    minimize_to_tray,
    window_geometry,
    paragon_mode_active,
    scan_height_percent,
    screen_capture_delay,
    overlay_update_delay,
    minimal_ocr_match_type,
    threshold_min,
    threshold_max,
    threshold_similarity_tooltip,
    threshold_similarity_affix_location,
    threshold_similarity_aspect_location,
    threshold_similarity_socket_location,
    threshold_similarity_splitter_location,
    tooltip_width,
    tooltip_max_height,
    affix_area_height_offset_top,
    affix_area_height_offset_bottom,
    affix_aspect_area_width_offset,
    aspect_area_height_offset_top,
    is_socket_detection_enabled,
    is_tempered_affix_detection_enabled,
    overlay_enabled,
    is_top_most,
    overlay_font_size,
    show_current_item,
    show_overlay_icon,
    overlay_icon_pos_x,
    overlay_icon_pos_y,
    type_area_offset_left,
    type_area_offset_right,
    aspect_area_max_height,
    aspect_icon_offset,
    overlay_menu_locked,
    is_import_paragon_d2core_enabled,
    is_import_paragon_d4builds_enabled,
    is_import_paragon_infinity_builds_enabled,
    is_import_paragon_maxroll_enabled,
    is_import_paragon_mobalytics_enabled,
    selected_build_import_website,
    builds_dir,
    is_tuning_prism_hints_enabled,
    is_show_browser_mobalytics_enabled,
    default_colors,
    paragon_border_size,
    paragon_node_size,
    paragon_node_size_collapsed,
    paragon_left_offset_collapsed,
    paragon_top_offset_collapsed,
    rule_settings,
    hotkey_config,
    is_diagnostics_export_enabled,
    update_check_enabled,
);

/// The consumer matrix over one settings value, grouped by kind.
pub fn setting_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    name_every_field(settings);
    [
        detector_entries(settings),
        ocr_preparation_entries(settings),
        session_restart_entries(settings),
        presentation_entries(settings),
        reevaluation_entries(settings),
        stored_only_entries(settings),
        not_runtime_entries(settings),
    ]
    .concat()
}

/// Settings whose change rebuilds the detector.
fn detector_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    use UpdateKind::*;
    let AppSettings {
        selected_system_preset,
        capture_backend,
        controller_mode,
        scan_height_percent,
        threshold_min,
        threshold_max,
        threshold_similarity_tooltip,
        tooltip_width,
        ..
    } = settings;
    vec![
        entry(
            "selected_system_preset",
            Detector,
            "tooltip templates (runtime_prepare::build_prepared_detector) and OCR marker templates",
            selected_system_preset,
        ),
        entry(
            "controller_mode",
            Detector,
            "template selection: keyboard or the configured controller images",
            controller_mode,
        ),
        entry(
            "threshold_min",
            Detector,
            "detector brightness filter and OCR marker templates",
            threshold_min,
        ),
        entry(
            "threshold_max",
            Detector,
            "detector brightness filter and OCR marker templates",
            threshold_max,
        ),
        entry(
            "threshold_similarity_tooltip",
            Detector,
            "detector tooltip match and OCR crop preparation",
            threshold_similarity_tooltip,
        ),
        entry(
            "capture_backend",
            Detector,
            "X11 capture backend of the prepared runtime (re-prepared with the detector)",
            capture_backend,
        ),
        entry(
            "scan_height_percent",
            Detector,
            "mouse-mode search band: the cursor-centered TooltipWidth * 2.5 column over \
             the bottom ScanHeight percent of the frame (ScreenProcessHandler.cs:563-566)",
            scan_height_percent,
        ),
        entry(
            "tooltip_width",
            Detector,
            "detector tooltip search/crop width and the OCR crop preparation \
             (ScreenProcessHandler.cs:564,670), seeded from the system preset",
            tooltip_width,
        ),
    ]
}

/// Settings whose change re-prepares the OCR.
fn ocr_preparation_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    use UpdateKind::*;
    let AppSettings {
        selected_affix_language,
        minimal_ocr_match_type,
        threshold_similarity_affix_location,
        threshold_similarity_aspect_location,
        threshold_similarity_socket_location,
        threshold_similarity_splitter_location,
        tooltip_max_height,
        affix_area_height_offset_top,
        affix_area_height_offset_bottom,
        affix_aspect_area_width_offset,
        aspect_area_height_offset_top,
        is_socket_detection_enabled,
        is_tempered_affix_detection_enabled,
        type_area_offset_left,
        type_area_offset_right,
        aspect_area_max_height,
        rule_settings,
        ..
    } = settings;
    vec![
        entry(
            "selected_affix_language",
            OcrPreparation,
            "Tesseract language and the fuzzy catalogs of the OCR preparation",
            selected_affix_language,
        ),
        entry(
            "minimal_ocr_match_type",
            OcrPreparation,
            "fuzzy match thresholds (FuzzyMatchThresholds::from_settings)",
            minimal_ocr_match_type,
        ),
        entry(
            "threshold_similarity_affix_location",
            OcrPreparation,
            "OCR affix marker search",
            threshold_similarity_affix_location,
        ),
        entry(
            "threshold_similarity_aspect_location",
            OcrPreparation,
            "OCR aspect marker search",
            threshold_similarity_aspect_location,
        ),
        entry(
            "threshold_similarity_socket_location",
            OcrPreparation,
            "OCR socket marker search",
            threshold_similarity_socket_location,
        ),
        entry(
            "threshold_similarity_splitter_location",
            OcrPreparation,
            "OCR splitter search",
            threshold_similarity_splitter_location,
        ),
        entry(
            "is_socket_detection_enabled",
            OcrPreparation,
            "OCR socket detection and segment routing",
            is_socket_detection_enabled,
        ),
        entry(
            "is_tempered_affix_detection_enabled",
            OcrPreparation,
            "OCR tempered affix detection",
            is_tempered_affix_detection_enabled,
        ),
        // The detection toggles live inside `rule_settings` (re-evaluation) but
        // gate the OCR segment plan, so a change also re-prepares the OCR.
        entry(
            "rule_settings.aspect_detection_enabled",
            OcrPreparation,
            "OCR segment plan: no aspect OCR for non-unique items \
             (ScreenProcessHandler.cs:471)",
            &rule_settings.aspect_detection_enabled,
        ),
        entry(
            "rule_settings.unique_detection_enabled",
            OcrPreparation,
            "OCR segment plan: no aspect OCR for unique and mythic items \
             (ScreenProcessHandler.cs:476)",
            &rule_settings.unique_detection_enabled,
        ),
        entry(
            "rule_settings.rune_detection_enabled",
            OcrPreparation,
            "OCR segment plan: no affix OCR for rune tooltips \
             (ScreenProcessHandler.cs:419)",
            &rule_settings.rune_detection_enabled,
        ),
        entry(
            "type_area_offset_left",
            OcrPreparation,
            "OCR item-type region, and the item card layout of the descriptors",
            type_area_offset_left,
        ),
        entry(
            "type_area_offset_right",
            OcrPreparation,
            "OCR item-type region",
            type_area_offset_right,
        ),
        entry(
            "aspect_area_max_height",
            OcrPreparation,
            "OCR aspect region",
            aspect_area_max_height,
        ),
        entry(
            "affix_area_height_offset_top",
            OcrPreparation,
            "OCR affix area top offset (ScreenProcessHandler.cs:951-959), seeded from the \
             system preset's config",
            affix_area_height_offset_top,
        ),
        entry(
            "affix_area_height_offset_bottom",
            OcrPreparation,
            "OCR affix area bottom offset (ScreenProcessHandler.cs:951-959), seeded from the \
             system preset's config",
            affix_area_height_offset_bottom,
        ),
        entry(
            "affix_aspect_area_width_offset",
            OcrPreparation,
            "OCR affix/aspect area width offset (ScreenProcessHandler.cs:951-959), seeded \
             from the system preset's config",
            affix_aspect_area_width_offset,
        ),
        entry(
            "aspect_area_height_offset_top",
            OcrPreparation,
            "OCR aspect area top offset (ScreenProcessHandler.cs:1372-1377), seeded from the \
             system preset's config",
            aspect_area_height_offset_top,
        ),
        entry(
            "tooltip_max_height",
            OcrPreparation,
            "OCR item-type region height (ScreenProcessHandler.cs:695-696), seeded from \
             the system preset's TooltipHeightType",
            tooltip_max_height,
        ),
    ]
}

/// Settings the runtime reads once at start; a change needs a restart.
fn session_restart_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    use UpdateKind::*;
    let AppSettings {
        display_backend,
        show_overlay_icon,
        overlay_icon_pos_x,
        overlay_icon_pos_y,
        overlay_menu_locked,
        ..
    } = settings;
    vec![
        entry(
            "display_backend",
            SessionRestart,
            "overlay backend and capture path; the app restarts the live loop through the \
             start/stop paths when it changes",
            display_backend,
        ),
        entry(
            "show_overlay_icon",
            SessionRestart,
            "overlay menu, created with the overlay surface",
            show_overlay_icon,
        ),
        entry(
            "overlay_icon_pos_x",
            SessionRestart,
            "overlay menu position, created with the overlay surface",
            overlay_icon_pos_x,
        ),
        entry(
            "overlay_icon_pos_y",
            SessionRestart,
            "overlay menu position, created with the overlay surface",
            overlay_icon_pos_y,
        ),
        entry(
            "overlay_menu_locked",
            SessionRestart,
            "initial overlay menu lock; the running overlay owns the lock afterwards",
            overlay_menu_locked,
        ),
    ]
}

/// Settings applied with the next presentation or tick.
fn presentation_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    use UpdateKind::*;
    let AppSettings {
        is_debug_info_enabled,
        screen_capture_delay,
        overlay_update_delay,
        overlay_enabled,
        show_current_item,
        show_overlay_icon,
        overlay_icon_pos_x,
        overlay_icon_pos_y,
        overlay_menu_locked,
        ..
    } = settings;
    vec![
        entry(
            "overlay_enabled",
            Presentation,
            "coordinator overlay gate through the rules snapshot (hides at once)",
            overlay_enabled,
        ),
        entry(
            "show_current_item",
            Presentation,
            "current-item card of the trade screen",
            show_current_item,
        ),
        entry(
            "show_overlay_icon",
            Presentation,
            "overlay menu icon visibility, applied to the running overlay surface",
            show_overlay_icon,
        ),
        entry(
            "overlay_icon_pos_x",
            Presentation,
            "overlay menu icon position, applied to the running overlay surface",
            overlay_icon_pos_x,
        ),
        entry(
            "overlay_icon_pos_y",
            Presentation,
            "overlay menu icon position, applied to the running overlay surface",
            overlay_icon_pos_y,
        ),
        entry(
            "overlay_menu_locked",
            Presentation,
            "overlay menu lock applied to the running surface's state machine; the running \
             overlay owns the lock afterwards",
            overlay_menu_locked,
        ),
        entry(
            "screen_capture_delay",
            Presentation,
            "live-loop tick pacing floor: the pause between captures, read per tick \
             (ScreenCaptureHandler.cs:266)",
            screen_capture_delay,
        ),
        entry(
            "overlay_update_delay",
            Presentation,
            "live-loop tick pacing floor: the pause after each processing pass, read per \
             tick (ScreenProcessHandler.cs:148)",
            overlay_update_delay,
        ),
        entry(
            "is_debug_info_enabled",
            Presentation,
            "Debug screen DebugInfo toggle; the live pipeline does not draw the debug image \
             on the accepted snapshot yet (upstream gates its debug drawing on it, e.g. \
             ScreenProcessHandler.cs:561,605,614)",
            is_debug_info_enabled,
        ),
    ]
}

/// Settings the rules snapshot re-evaluates.
fn reevaluation_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    use UpdateKind::*;
    let AppSettings {
        selected_affix_preset,
        paragon_mode_active,
        overlay_font_size,
        is_tuning_prism_hints_enabled,
        paragon_border_size,
        paragon_node_size,
        paragon_node_size_collapsed,
        paragon_left_offset_collapsed,
        paragon_top_offset_collapsed,
        rule_settings,
        ..
    } = settings;
    vec![
        entry(
            "selected_affix_preset",
            Reevaluation,
            "rules snapshot (active affix preset)",
            selected_affix_preset,
        ),
        entry(
            "rule_settings",
            Reevaluation,
            "rules snapshot (evaluation switches, multi-build, marker mode)",
            rule_settings,
        ),
        entry(
            "overlay_font_size",
            Reevaluation,
            "descriptor layout of the rules snapshot",
            overlay_font_size,
        ),
        entry(
            "is_tuning_prism_hints_enabled",
            Reevaluation,
            "rules snapshot (tuning prism hints)",
            is_tuning_prism_hints_enabled,
        ),
        entry(
            "paragon_mode_active",
            Reevaluation,
            "paragon descriptors of the rules snapshot",
            paragon_mode_active,
        ),
        entry(
            "paragon_border_size",
            Reevaluation,
            "paragon descriptor layout",
            paragon_border_size,
        ),
        entry(
            "paragon_node_size",
            Reevaluation,
            "paragon descriptor layout",
            paragon_node_size,
        ),
        entry(
            "paragon_node_size_collapsed",
            Reevaluation,
            "paragon descriptor layout",
            paragon_node_size_collapsed,
        ),
        entry(
            "paragon_left_offset_collapsed",
            Reevaluation,
            "paragon descriptor layout",
            paragon_left_offset_collapsed,
        ),
        entry(
            "paragon_top_offset_collapsed",
            Reevaluation,
            "paragon descriptor layout",
            paragon_top_offset_collapsed,
        ),
    ]
}

/// Settings upstream itself never reads.
fn stored_only_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    use UpdateKind::*;
    let AppSettings {
        aspect_icon_offset, ..
    } = settings;
    vec![
        entry(
            "aspect_icon_offset",
            StoredOnly,
            "upstream has no consumer: SettingsD4.cs:14 declares it (default 50) and only \
             AspectConfigViewModel.cs:51-57 reads it for a control that AspectConfigView.xaml:48-61 \
             collapses; the port keeps the value in Settings.json and shows no control",
            aspect_icon_offset,
        ),
    ]
}

/// Settings without a runtime consumer.
fn not_runtime_entries(settings: &AppSettings) -> Vec<SettingEntry> {
    use UpdateKind::*;
    let AppSettings {
        selected_app_language,
        wayland_restore_token,
        debug_mode,
        dev_mode,
        check_for_updates,
        launch_minimized,
        minimize_to_tray,
        window_geometry,
        is_top_most,
        is_import_paragon_d2core_enabled,
        is_import_paragon_d4builds_enabled,
        is_import_paragon_infinity_builds_enabled,
        is_import_paragon_maxroll_enabled,
        is_import_paragon_mobalytics_enabled,
        selected_build_import_website,
        builds_dir,
        is_show_browser_mobalytics_enabled,
        default_colors,
        hotkey_config,
        is_diagnostics_export_enabled,
        update_check_enabled,
        ..
    } = settings;
    vec![
        entry(
            "dev_mode",
            NotRuntime,
            "settings screen: gates the Debug Mode toggle's visibility \
             (SettingsView.xaml:88,97); seeded from AppSettings.dev_mode, default off",
            dev_mode,
        ),
        entry(
            "debug_mode",
            NotRuntime,
            "not ported: upstream DebugMode is the settings toggle (SettingsView.xaml:90) \
             that selects the Firefox \"Screenshot\" window as the capture source \
             (ScreenCaptureHandler.cs:170-180); the port has no such capture source. It \
             gates no tab: the Debug tab is always shown",
            debug_mode,
        ),
        entry(
            "wayland_restore_token",
            NotRuntime,
            "written by the Wayland capture preparation, read at the next start",
            wayland_restore_token,
        ),
        entry(
            "selected_app_language",
            NotRuntime,
            "UI language: applied at startup and on a pick of the settings selector \
             (i18n::select); no runtime consumer",
            selected_app_language,
        ),
        entry("is_top_most", NotRuntime, "companion window", is_top_most),
        entry(
            "check_for_updates",
            NotRuntime,
            "update banner",
            check_for_updates,
        ),
        entry(
            "update_check_enabled",
            NotRuntime,
            "update banner",
            update_check_enabled,
        ),
        entry(
            "launch_minimized",
            NotRuntime,
            "companion window",
            launch_minimized,
        ),
        entry(
            "minimize_to_tray",
            NotRuntime,
            "companion window",
            minimize_to_tray,
        ),
        entry(
            "window_geometry",
            NotRuntime,
            "companion window",
            window_geometry,
        ),
        entry(
            "is_import_paragon_d2core_enabled",
            NotRuntime,
            "build import",
            is_import_paragon_d2core_enabled,
        ),
        entry(
            "is_import_paragon_d4builds_enabled",
            NotRuntime,
            "build import",
            is_import_paragon_d4builds_enabled,
        ),
        entry(
            "is_import_paragon_infinity_builds_enabled",
            NotRuntime,
            "build import",
            is_import_paragon_infinity_builds_enabled,
        ),
        entry(
            "is_import_paragon_maxroll_enabled",
            NotRuntime,
            "build import",
            is_import_paragon_maxroll_enabled,
        ),
        entry(
            "is_import_paragon_mobalytics_enabled",
            NotRuntime,
            "build import",
            is_import_paragon_mobalytics_enabled,
        ),
        entry(
            "selected_build_import_website",
            NotRuntime,
            "build import",
            selected_build_import_website,
        ),
        entry("builds_dir", NotRuntime, "build import", builds_dir),
        entry(
            "is_show_browser_mobalytics_enabled",
            NotRuntime,
            "build import",
            is_show_browser_mobalytics_enabled,
        ),
        entry(
            "default_colors",
            NotRuntime,
            "colors of newly added preset entries",
            default_colors,
        ),
        entry(
            "hotkey_config",
            NotRuntime,
            "hotkey registration",
            hotkey_config,
        ),
        entry(
            "is_diagnostics_export_enabled",
            NotRuntime,
            "crash-log export",
            is_diagnostics_export_enabled,
        ),
    ]
}

/// The settings whose value differs between `before` and `after`.
pub fn changed_settings(before: &AppSettings, after: &AppSettings) -> Vec<SettingEntry> {
    let before = setting_entries(before);
    setting_entries(after)
        .into_iter()
        .zip(before)
        .filter(|(after, before)| after.value != before.value)
        .map(|(after, _)| after)
        .collect()
}

/// An asset set the runtime preparation reads besides `Settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeAsset {
    /// `Config/Controllers.json`: the active controller images.
    ControllerList,
    /// The selected preset's template files.
    TemplateSet,
    /// The language data behind the fuzzy catalogs.
    LanguageData,
}

impl RuntimeAsset {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ControllerList => "controller list",
            Self::TemplateSet => "template set",
            Self::LanguageData => "language data",
        }
    }
}

/// Revision stamps of the runtime assets (file names, sizes, mtimes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AssetStamps {
    pub controller_list: u64,
    pub template_set: u64,
    pub language_data: u64,
}

impl AssetStamps {
    /// Stamp the assets the preparation for `settings` reads.
    pub fn observe(config_root: &Path, game_data_root: &Path, settings: &AppSettings) -> Self {
        let preset = &settings.selected_system_preset;
        let mut templates = DefaultHasher::new();
        for dir in [
            config_root
                .join(d4lc_assets::EXTRACTED_PRESETS_DIR)
                .join(preset),
            config_root
                .join(d4lc_assets::SYSTEM_PRESETS_DIR)
                .join(preset),
            game_data_root
                .join(d4lc_assets::SYSTEM_PRESETS_DIR)
                .join(preset),
        ] {
            stamp_tree(&dir, &dir, 3, &|_| true, &mut templates);
        }
        let language = format!(".{}.", settings.selected_affix_language);
        let data_dir = game_data_root.join(d4lc_assets::DATA_DIR);
        let mut language_data = DefaultHasher::new();
        stamp_tree(
            &data_dir,
            &data_dir,
            1,
            &|name: &str| name.contains(&language),
            &mut language_data,
        );
        let mut controllers = DefaultHasher::new();
        stamp_file(
            &config_root.join(d4lc_config::CONTROLLERS_PATH),
            "",
            &mut controllers,
        );
        Self {
            controller_list: controllers.finish(),
            template_set: templates.finish(),
            language_data: language_data.finish(),
        }
    }

    /// The asset sets whose stamp differs.
    pub fn changed(&self, other: &Self) -> Vec<RuntimeAsset> {
        let mut changed = Vec::new();
        if self.controller_list != other.controller_list {
            changed.push(RuntimeAsset::ControllerList);
        }
        if self.template_set != other.template_set {
            changed.push(RuntimeAsset::TemplateSet);
        }
        if self.language_data != other.language_data {
            changed.push(RuntimeAsset::LanguageData);
        }
        changed
    }
}

fn stamp_file(path: &Path, relative: &str, hasher: &mut DefaultHasher) {
    relative.hash(hasher);
    match fs::metadata(path) {
        Ok(metadata) => {
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_nanos())
                .hash(hasher);
        }
        Err(_) => "absent".hash(hasher),
    }
}

fn stamp_tree(
    root: &Path,
    dir: &Path,
    depth: usize,
    accept: &dyn Fn(&str) -> bool,
    hasher: &mut DefaultHasher,
) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if depth > 1 {
                stamp_tree(root, &path, depth - 1, accept, hasher);
            }
        } else if accept(&name) {
            let relative = path
                .strip_prefix(root)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or(name);
            stamp_file(&path, &relative, hasher);
        }
    }
}

/// Desired versus applied settings, as the Debug screen shows them.
///
/// "Desired" is what `Config/Settings.json` and the runtime assets say now;
/// "applied" is what the running runtime was prepared with. A failed switch
/// stays visible until a later preparation succeeds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsApplication {
    /// Applied configuration revision of the running runtime; `None` while
    /// no runtime runs.
    pub applied_revision: Option<u64>,
    /// Changes that requested a runtime re-preparation not yet applied.
    pub pending: Vec<String>,
    /// Changes that apply only after a runtime restart.
    pub restart_required: Vec<String>,
    /// Stored values the running app does not use (edited outside the app;
    /// the app state keeps its own values until the next app start).
    pub not_live: Vec<String>,
    /// The last failed re-preparation; the previous configuration stays
    /// applied.
    pub failed: Option<String>,
}

impl SettingsApplication {
    fn add(list: &mut Vec<String>, keys: &[String]) {
        for key in keys {
            if !list.contains(key) {
                list.push(key.clone());
            }
        }
    }

    pub fn request_reprepare(&mut self, keys: &[String]) {
        Self::add(&mut self.pending, keys);
    }

    pub fn require_restart(&mut self, keys: &[String]) {
        Self::add(&mut self.restart_required, keys);
    }

    pub fn set_not_live(&mut self, keys: Vec<String>) {
        self.not_live = keys;
    }

    /// A runtime started: it was prepared from the current settings.
    pub fn started(&mut self, revision: u64) {
        self.applied_revision = Some(revision);
        self.pending.clear();
        self.restart_required.clear();
        self.failed = None;
    }

    /// A re-preparation was applied.
    pub fn applied(&mut self, revision: u64) {
        self.applied_revision = Some(revision);
        self.pending.clear();
        self.failed = None;
    }

    /// A re-preparation failed; the pending changes stay unapplied.
    pub fn failed(&mut self, message: &str) {
        self.failed = Some(message.to_string());
    }

    /// The runtime stopped: nothing is applied.
    pub fn stopped(&mut self) {
        self.applied_revision = None;
        self.pending.clear();
        self.restart_required.clear();
    }

    /// Text for the Debug screen.
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        match self.applied_revision {
            Some(revision) => parts.push(format!("applied revision {revision}")),
            None => parts.push("not applied (runtime stopped)".to_string()),
        }
        if let Some(message) = &self.failed {
            parts.push(format!(
                "FAILED to apply {}: {message}",
                if self.pending.is_empty() {
                    "change".to_string()
                } else {
                    self.pending.join(", ")
                }
            ));
        } else if !self.pending.is_empty() {
            parts.push(format!("applying: {}", self.pending.join(", ")));
        }
        if !self.restart_required.is_empty() {
            parts.push(format!(
                "restart runtime to apply: {}",
                self.restart_required.join(", ")
            ));
        }
        if !self.not_live.is_empty() {
            parts.push(format!(
                "stored but not live until app restart: {}",
                self.not_live.join(", ")
            ));
        }
        parts.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_is_classified_once() {
        let entries = setting_entries(&AppSettings::default());
        // A key may carry two kinds when it applies through two paths — the
        // overlay menu values are created with the surface (runtime restart)
        // and applied to the running surface (presentation) — but the same
        // (key, kind) pair must not repeat.
        let mut pairs: Vec<_> = entries
            .iter()
            .map(|entry| (entry.key, entry.kind.as_str()))
            .collect();
        let count = pairs.len();
        pairs.sort_unstable();
        pairs.dedup();
        assert_eq!(pairs.len(), count, "a (key, kind) pair is listed twice");
        assert!(entries.iter().all(|entry| !entry.consumer.is_empty()));
    }

    #[test]
    fn every_setting_field_has_an_entry() {
        let entries = setting_entries(&AppSettings::default());
        for field in SETTING_FIELDS {
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.key.split('.').next() == Some(*field)),
                "AppSettings::{field} has no entry in the consumer matrix"
            );
        }
    }

    #[test]
    fn no_stored_value_is_inactive_any_more() {
        // Every stored setting has a live consumer, except the documented
        // stored-only class: values upstream itself never reads.
        let entries = setting_entries(&AppSettings::default());
        assert!(
            entries.iter().all(|entry| !entry.consumer.is_empty()),
            "every setting must name a consumer"
        );
        let stored_only: Vec<_> = entries
            .iter()
            .filter(|entry| entry.kind == UpdateKind::StoredOnly)
            .collect();
        assert_eq!(
            stored_only
                .iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>(),
            vec!["aspect_icon_offset"],
            "a stored-only setting needs upstream evidence of its missing consumer"
        );
        assert!(stored_only
            .iter()
            .all(|entry| entry.consumer.starts_with("upstream has no consumer")));
        assert!(!UpdateKind::StoredOnly.requires_reprepare());
        assert!(!UpdateKind::StoredOnly.follows_app_state());
        let kind_of = |key: &str| {
            entries
                .iter()
                .find(|entry| entry.key == key)
                .unwrap_or_else(|| panic!("{key} must be classified"))
                .kind
        };
        assert_eq!(kind_of("scan_height_percent"), UpdateKind::Detector);
        assert_eq!(kind_of("tooltip_width"), UpdateKind::Detector);
        assert_eq!(kind_of("tooltip_max_height"), UpdateKind::OcrPreparation);
        assert_eq!(
            kind_of("affix_area_height_offset_top"),
            UpdateKind::OcrPreparation
        );
        assert_eq!(
            kind_of("aspect_area_height_offset_top"),
            UpdateKind::OcrPreparation
        );
        assert_eq!(kind_of("screen_capture_delay"), UpdateKind::Presentation);
        assert_eq!(kind_of("overlay_update_delay"), UpdateKind::Presentation);
        assert_eq!(kind_of("is_debug_info_enabled"), UpdateKind::Presentation);
        assert_eq!(kind_of("dev_mode"), UpdateKind::NotRuntime);
        for flag in [
            "rule_settings.aspect_detection_enabled",
            "rule_settings.unique_detection_enabled",
            "rule_settings.rune_detection_enabled",
        ] {
            assert_eq!(kind_of(flag), UpdateKind::OcrPreparation);
        }
    }

    #[test]
    fn changed_settings_names_the_kind_of_each_change() {
        let before = AppSettings::default();
        let mut after = before.clone();
        after.threshold_similarity_socket_location += 0.01;
        after.display_backend = "wayland".to_string();
        after.debug_mode = !before.debug_mode;
        let changed: Vec<_> = changed_settings(&before, &after)
            .into_iter()
            .map(|entry| (entry.key, entry.kind))
            .collect();
        assert_eq!(
            changed,
            vec![
                (
                    "threshold_similarity_socket_location",
                    UpdateKind::OcrPreparation
                ),
                ("display_backend", UpdateKind::SessionRestart),
                ("debug_mode", UpdateKind::NotRuntime),
            ]
        );
    }

    #[test]
    fn detection_toggles_reprepare_the_ocr_and_reevaluate() {
        let before = AppSettings::default();
        let mut after = before.clone();
        after.rule_settings.aspect_detection_enabled = false;
        let changed: Vec<_> = changed_settings(&before, &after)
            .into_iter()
            .map(|entry| (entry.key, entry.kind))
            .collect();
        assert_eq!(
            changed,
            vec![
                (
                    "rule_settings.aspect_detection_enabled",
                    UpdateKind::OcrPreparation
                ),
                ("rule_settings", UpdateKind::Reevaluation),
            ]
        );
        assert!(changed.iter().any(|(_, kind)| kind.requires_reprepare()));
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("d4lc-consumers-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("temp root: {error}"));
        dir
    }

    #[test]
    fn asset_stamps_see_controller_template_and_language_changes() {
        let root = temp_root("assets");
        let settings = AppSettings::default();
        let base = AssetStamps::observe(&root, &root, &settings);

        d4lc_config::save_controller_config(&root, &["tooltip_gc_x.png".to_string()])
            .unwrap_or_else(|error| panic!("controllers: {error}"));
        let controllers = AssetStamps::observe(&root, &root, &settings);
        assert_eq!(
            controllers.changed(&base),
            vec![RuntimeAsset::ControllerList]
        );

        let tooltips = root
            .join(d4lc_assets::EXTRACTED_PRESETS_DIR)
            .join(&settings.selected_system_preset)
            .join("Tooltips");
        fs::create_dir_all(&tooltips).unwrap_or_else(|error| panic!("{error}"));
        fs::write(tooltips.join("tooltip_gc_x.png"), b"png")
            .unwrap_or_else(|error| panic!("{error}"));
        let templates = AssetStamps::observe(&root, &root, &settings);
        assert_eq!(
            templates.changed(&controllers),
            vec![RuntimeAsset::TemplateSet]
        );

        let data = root.join(d4lc_assets::DATA_DIR);
        fs::create_dir_all(&data).unwrap_or_else(|error| panic!("{error}"));
        fs::write(data.join("Affixes.deDE.json"), b"[]").unwrap_or_else(|error| panic!("{error}"));
        let other_language = AssetStamps::observe(&root, &root, &settings);
        assert!(
            other_language.changed(&templates).is_empty(),
            "another language's data is not this preparation's input"
        );
        fs::write(
            data.join(format!("Affixes.{}.json", settings.selected_affix_language)),
            b"[]",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let language = AssetStamps::observe(&root, &root, &settings);
        assert_eq!(
            language.changed(&templates),
            vec![RuntimeAsset::LanguageData]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_switch_stays_visibly_failed() {
        let mut status = SettingsApplication::default();
        status.started(1);
        status.request_reprepare(&["selected_system_preset".to_string()]);
        assert!(status.text().contains("applying: selected_system_preset"));
        status.failed("template load failed: no tooltip templates");
        let text = status.text();
        assert!(text.contains("applied revision 1"), "{text}");
        assert!(
            text.contains("FAILED to apply selected_system_preset: template load failed"),
            "{text}"
        );
        // Still failed on the next observation until something applies.
        assert!(status.text().contains("FAILED"));
        status.applied(2);
        assert_eq!(status.text(), "applied revision 2");
    }
}
