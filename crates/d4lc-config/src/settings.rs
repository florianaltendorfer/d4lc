//! The `Config/Settings.json` model: defaults, the settings types and the
//! rule settings derived from them.

use d4lc_core::Rect;

mod codec;
mod store;
mod wpf_key;

pub(crate) use codec::app_settings_json;
pub use codec::parse_manual_region;
pub(crate) use store::{keep_settings_migration_backup, load_settings_for_write};
pub use store::{
    load_app_settings, load_app_settings_and_save, load_app_settings_with_warnings,
    save_app_settings, save_wayland_restore_token, AppSettingsWarning, SETTINGS_SCHEMA_VERSION,
};

pub const DEFAULT_SYSTEM_PRESET: &str = "1440p_SMF";
pub const DEFAULT_AFFIX_LANGUAGE: &str = "enUS";
/// Upstream `SettingsD4.cs:79` `SelectedAppLanguage` default: the culture of
/// the UI language, not the game-data language above.
pub const DEFAULT_APP_LANGUAGE: &str = "en-US";
pub const DEFAULT_SCAN_HEIGHT_PERCENT: f64 = 50.0;
pub const DEFAULT_THRESHOLD_MIN: u8 = 70;
pub const DEFAULT_THRESHOLD_MAX: u8 = 255;
pub const DEFAULT_THRESHOLD_SIMILARITY_TOOLTIP: f64 = 0.05;
pub const DEFAULT_THRESHOLD_SIMILARITY_AFFIX_LOCATION: f64 = 0.05;
pub const DEFAULT_THRESHOLD_SIMILARITY_ASPECT_LOCATION: f64 = 0.05;
pub const DEFAULT_THRESHOLD_SIMILARITY_SOCKET_LOCATION: f64 = 0.05;
pub const DEFAULT_THRESHOLD_SIMILARITY_SPLITTER_LOCATION: f64 = 0.05;
pub const DEFAULT_MINIMAL_OCR_MATCH_TYPE: u8 = 75;
pub const DEFAULT_CAPTURE_BACKEND: &str = "auto";
// Display backend selection (Auto / X11 / Wayland). New Linux-native
// scope, not upstream parity: X11 stays the default and fallback, and
// the lenient parser backfills `auto` for older settings files.
pub const DEFAULT_DISPLAY_BACKEND: &str = "auto";
pub const DEFAULT_TOOLTIP_WIDTH: u32 = 500;
pub const DEFAULT_TOOLTIP_MAX_HEIGHT: u32 = 200;
// Live-loop pacing floors (upstream `SettingsD4.cs:83` ScreenCaptureDelay,
// default 50 — the pause between captures, read per capture round by
// `ScreenCaptureHandler.cs:266`; upstream `SettingsD4.cs:70`
// OverlayUpdateDelay, default 5 — the pause after each processing pass,
// read per processed screen by `ScreenProcessHandler.cs:148`).
pub const DEFAULT_SCREEN_CAPTURE_DELAY: u32 = 50;
pub const DEFAULT_OVERLAY_UPDATE_DELAY: u32 = 5;
pub const DEFAULT_OVERLAY_FONT_SIZE: u32 = 18;
/// The bounds of the overlay font size setting: the upstream `NumericUpDown`
/// range the Trade config enforces. A hand-edited `Settings.json` can hold
/// anything, so the overlay clamps with [`bounded_overlay_font_size`].
pub const OVERLAY_FONT_SIZE_MIN: u32 = 10;
pub const OVERLAY_FONT_SIZE_MAX: u32 = 50;

/// `size` limited to [`OVERLAY_FONT_SIZE_MIN`]..=[`OVERLAY_FONT_SIZE_MAX`].
pub const fn bounded_overlay_font_size(size: u32) -> u32 {
    if size < OVERLAY_FONT_SIZE_MIN {
        OVERLAY_FONT_SIZE_MIN
    } else if size > OVERLAY_FONT_SIZE_MAX {
        OVERLAY_FONT_SIZE_MAX
    } else {
        size
    }
}
// Upstream: D4Companion.Entities/SettingsD4.cs lines 87-90.
pub const DEFAULT_ITEM_POWER_LIMIT: u32 = 800;
pub const DEFAULT_OVERLAY_MARKER_MODE: &str = "Show All";
// Upstream: D4Companion.Entities/SettingsD4.cs line for TypeAreaOffsetLeft, default 5.
pub const DEFAULT_TYPE_AREA_OFFSET_LEFT: u32 = 5;
// Upstream: D4Companion.Entities/SettingsD4.cs line 95, default 5.
pub const DEFAULT_TYPE_AREA_OFFSET_RIGHT: u32 = 5;
// Upstream: D4Companion.Entities/SettingsD4.cs line 13, default 300.
pub const DEFAULT_ASPECT_AREA_MAX_HEIGHT: u32 = 300;
// Upstream: D4Companion.Entities/SettingsD4.cs line 14, default 50.
pub const DEFAULT_ASPECT_ICON_OFFSET: i32 = 50;
// The four OCR area offsets (upstream `SettingsD4.cs:9-12`, seeded from the
// system preset's config when the preset changes,
// SettingsViewModel.cs:258-261).
pub const DEFAULT_AFFIX_AREA_HEIGHT_OFFSET_TOP: i32 = 10;
pub const DEFAULT_AFFIX_AREA_HEIGHT_OFFSET_BOTTOM: i32 = 10;
pub const DEFAULT_AFFIX_ASPECT_AREA_WIDTH_OFFSET: i32 = 18;
pub const DEFAULT_ASPECT_AREA_HEIGHT_OFFSET_TOP: i32 = 10;
// Upstream: D4Companion.Entities/SettingsD4.cs line 81, default "Maxroll.gg".
pub const DEFAULT_SELECTED_BUILD_IMPORT_WEBSITE: &str = "Maxroll.gg";
/// Default value of every upstream `DefaultColor*` setting.
///
/// All seven upstream defaults are `Colors.Green`
/// (`D4Companion.Entities/SettingsD4.cs:18-24`); WPF's `Colors.Green` is
/// `#008000`, which is also the string form
/// [`PresetColor::GREEN`](crate::PresetColor::GREEN) renders as.
pub const DEFAULT_COLOR: &str = "#008000";

// Paragon layout (upstream `SettingsD4.cs:71-75`, edited from the
// ParagonConfig dialog). The numeric ranges mirror the dialog's own
// NumericUpDown Minimum/Maximum values (`ParagonConfigView.xaml:39-104`).
pub const DEFAULT_PARAGON_BORDER_SIZE: u32 = 2;
pub const PARAGON_BORDER_SIZE_MIN: u32 = 1;
pub const PARAGON_BORDER_SIZE_MAX: u32 = 10;
pub const DEFAULT_PARAGON_NODE_SIZE: u32 = 40;
pub const DEFAULT_PARAGON_NODE_SIZE_COLLAPSED: u32 = 23;
pub const PARAGON_NODE_SIZE_MIN: u32 = 10;
pub const PARAGON_NODE_SIZE_MAX: u32 = 100;
pub const DEFAULT_PARAGON_LEFT_OFFSET_COLLAPSED: i32 = -200;
pub const DEFAULT_PARAGON_TOP_OFFSET_COLLAPSED: i32 = -50;
pub const PARAGON_OFFSET_MIN: i32 = -1000;
pub const PARAGON_OFFSET_MAX: i32 = 1000;

// Overlay icon placement (upstream `OverlayConfigView.xaml:66-77`, Slider
// Minimum 0 / Maximum 1000) and the item-power limit NumericUpDown range
// (`OverlayConfigView.xaml:110-116`, Minimum 100 / Maximum 800).
pub const OVERLAY_ICON_POS_MAX: u32 = 1000;
pub const ITEM_POWER_LIMIT_MIN: u32 = 100;
pub const ITEM_POWER_LIMIT_MAX: u32 = 800;

// ---------------------------------------------------------------------------
// Display backend selection (shared parser)
// ---------------------------------------------------------------------------

/// The selected display backend: Auto / X11 / Wayland.
///
/// Lives in `d4lc-config` so both binaries share the single
/// persisted-token parser: the desktop app's display-backend selection
/// seam and the CLI's overlay-start routing consume this one type instead
/// of keeping duplicate string parsers. New Linux-native scope, not
/// upstream parity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DisplayBackend {
    /// Follow the default: the X11 paths, unchanged.
    #[default]
    Auto,
    /// Force the X11 paths.
    X11,
    /// Wayland session. Persisted and validated; availability depends
    /// on the build (`WAYLAND_CAPTURE_BUILD`): with the capture feature
    /// the runtime runs the ScreenCast portal path, without it the seam
    /// reports the structured unavailability.
    Wayland,
}

impl DisplayBackend {
    /// Parse the persisted setting token. Accepts exactly
    /// `auto|x11|wayland` and rejects everything else with the same
    /// structured error style as `d4lc_capture::CaptureBackend::parse`.
    ///
    /// # Errors
    ///
    /// Returns a structured, user-visible error naming the accepted
    /// tokens; the caller surfaces it instead of guessing a fallback.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "auto" => Ok(Self::Auto),
            "x11" => Ok(Self::X11),
            "wayland" => Ok(Self::Wayland),
            other => Err(format!(
                "unsupported display backend '{other}'; expected auto|x11|wayland"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::X11 => "x11",
            Self::Wayland => "wayland",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultColors {
    pub greater: String,
    pub implicit: String,
    pub normal: String,
    pub tempered: String,
    pub aspects: String,
    pub uniques: String,
    pub runes: String,
}

impl Default for DefaultColors {
    /// The upstream defaults: every class default is `Colors.Green`
    /// (`SettingsD4.cs:18-24`), stored in d4lc's string color form.
    fn default() -> Self {
        Self {
            greater: DEFAULT_COLOR.to_string(),
            implicit: DEFAULT_COLOR.to_string(),
            normal: DEFAULT_COLOR.to_string(),
            tempered: DEFAULT_COLOR.to_string(),
            aspects: DEFAULT_COLOR.to_string(),
            uniques: DEFAULT_COLOR.to_string(),
            runes: DEFAULT_COLOR.to_string(),
        }
    }
}

impl DefaultColors {
    /// The seven upstream settings as `(upstream field name, value)`
    /// pairs, in the order `SettingsD4.cs:18-24` declares them. The
    /// stored value stays verbatim (a color name or a `#RRGGBB` /
    /// `#AARRGGBB` string);
    /// [`PresetColor::parse`](crate::PresetColor::parse) is the single
    /// interpretation point.
    pub fn fields(&self) -> [(&'static str, &str); 7] {
        [
            ("DefaultColorGreater", &self.greater),
            ("DefaultColorImplicit", &self.implicit),
            ("DefaultColorNormal", &self.normal),
            ("DefaultColorTempered", &self.tempered),
            ("DefaultColorAspects", &self.aspects),
            ("DefaultColorUniques", &self.uniques),
            ("DefaultColorRunes", &self.runes),
        ]
    }

    /// Overwrite one field by its stable key (`"greater"`, `"implicit"`,
    /// …). Returns `false` for an unknown key so callers can reject a
    /// misspelled update instead of silently ignoring it.
    pub fn set_field(&mut self, key: &str, value: &str) -> bool {
        let slot = match key {
            "greater" => &mut self.greater,
            "implicit" => &mut self.implicit,
            "normal" => &mut self.normal,
            "tempered" => &mut self.tempered,
            "aspects" => &mut self.aspects,
            "uniques" => &mut self.uniques,
            "runes" => &mut self.runes,
            _ => return false,
        };
        *slot = value.to_string();
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
    pub selected_system_preset: String,
    pub selected_affix_language: String,
    /// Culture id of the UI language, persisted as the upstream
    /// `SelectedAppLanguage` key (`SettingsD4.cs:79`, written by
    /// `SettingsViewModel.cs:231`). Separate from the game-data
    /// `selected_affix_language`.
    pub selected_app_language: String,
    /// Name of the affix preset the user last selected, persisted as
    /// the upstream `SelectedAffixPreset` key.
    ///
    /// Upstream: `D4Companion.Entities/SettingsD4.cs:80` (default
    /// empty string), written by `AffixViewModel.cs:372-373` on every
    /// preset switch and by `AffixManager.RenamePreset` on renames.
    /// Files written before this field existed load with the default
    /// (empty = no persisted selection).
    pub selected_affix_preset: String,
    pub capture_backend: String,
    /// Display backend selection (Auto / X11 / Wayland), persisted as
    /// the lowercase token with the `auto` default. Kept as a lenient
    /// `String` here — exactly like `capture_backend` — so the storage
    /// layer stays tolerant; the shared [`DisplayBackend::parse`] is the
    /// single validation point both binaries run before acting on the
    /// stored token.
    pub display_backend: String,
    /// Single-use ScreenCast portal restore token from the last
    /// successful Wayland capture session. `None` means "no stored
    /// session": the next Wayland runtime start shows the compositor
    /// consent dialog. Portal restore tokens are single-use — every
    /// successful session hands out a fresh token that replaces this
    /// one, and a consumed token never restores.
    pub wayland_restore_token: Option<String>,
    pub controller_mode: bool,
    pub debug_mode: bool,
    /// Upstream `SettingsD4.DevMode` (SettingsD4.cs:25, default `false`).
    ///
    /// Upstream binds the Debug Mode toggle's *visibility* to it
    /// (`SettingsViewModel.cs:144-154` → `IsDevModeEnabled`; the switch
    /// carries the visibility binding at `SettingsView.xaml:85-90`) and
    /// has no UI control that writes it — the field is hand-edited in
    /// `Config/Settings.json`. The port parses and re-writes the key so a
    /// hand-edited file survives a d4lc save, and the app seeds the
    /// settings screen's `dev-mode` property from it (default `false`
    /// keeps the toggle hidden).
    pub dev_mode: bool,
    /// Upstream `SettingsD4.IsDebugInfoEnabled` (SettingsD4.cs:29,
    /// default `false`): the Debug view's DebugInfo toggle. Upstream
    /// saves it on every toggle (`DebugViewModel.cs:168-178`) and the
    /// live pipeline gates its debug drawing on it. Files written
    /// before this field existed load with the default (`false`).
    pub is_debug_info_enabled: bool,
    pub check_for_updates: bool,
    pub launch_minimized: bool,
    pub minimize_to_tray: bool,
    /// Persisted main-window geometry snapshot for the desktop shell.
    pub window_geometry: Option<Rect>,
    pub paragon_mode_active: bool,
    pub scan_height_percent: f64,
    /// Upstream `SettingsD4.ScreenCaptureDelay` (`SettingsD4.cs:83`,
    /// default 50): the pause in milliseconds between captures, read per
    /// capture round (`ScreenCaptureHandler.cs:266`). The live loop reads
    /// it per tick as a lower bound for the capture-to-capture period.
    pub screen_capture_delay: u32,
    /// Upstream `SettingsD4.OverlayUpdateDelay` (`SettingsD4.cs:70`,
    /// default 5): the pause in milliseconds after each processing pass,
    /// read per processed screen (`ScreenProcessHandler.cs:148`). The live
    /// loop reads it per tick as an additional lower bound.
    pub overlay_update_delay: u32,
    pub minimal_ocr_match_type: u8,
    pub threshold_min: u8,
    pub threshold_max: u8,
    pub threshold_similarity_tooltip: f64,
    pub threshold_similarity_affix_location: f64,
    pub threshold_similarity_aspect_location: f64,
    pub threshold_similarity_socket_location: f64,
    pub threshold_similarity_splitter_location: f64,
    pub tooltip_width: u32,
    pub tooltip_max_height: u32,
    /// The four OCR area offsets (upstream `SettingsD4.cs:9-12`). Upstream
    /// keeps them on `Settings` — the live OCR region assembly reads the
    /// settings values, and a system-preset change re-seeds them from the
    /// preset's config (`SettingsViewModel.cs:258-261`,
    /// `ScreenProcessHandler.cs:951-959,1372-1377`). Files written before
    /// these fields existed load with the upstream defaults.
    pub affix_area_height_offset_top: i32,
    pub affix_area_height_offset_bottom: i32,
    pub affix_aspect_area_width_offset: i32,
    pub aspect_area_height_offset_top: i32,
    pub is_socket_detection_enabled: bool,
    pub is_tempered_affix_detection_enabled: bool,
    pub overlay_enabled: bool,
    pub is_top_most: bool,
    pub overlay_font_size: u32,
    // Upstream: D4Companion.Entities/SettingsD4.cs line 83, default true.
    pub show_current_item: bool,
    pub show_overlay_icon: bool,
    pub overlay_icon_pos_x: u32,
    pub overlay_icon_pos_y: u32,
    pub type_area_offset_left: u32,
    pub type_area_offset_right: u32,
    pub aspect_area_max_height: u32,
    /// Upstream `Settings.AspectIconOffset` (`SettingsD4.cs:14`). Stored only:
    /// upstream hides its control (`AspectConfigView.xaml:48-61`,
    /// `Visibility="Collapsed"`) and nothing reads the value.
    pub aspect_icon_offset: i32,
    pub overlay_menu_locked: bool,
    pub is_import_paragon_d2core_enabled: bool,
    // Upstream: D4Companion.Entities/SettingsD4.cs lines 31-34, default true.
    pub is_import_paragon_d4builds_enabled: bool,
    // Upstream: D4Companion.Entities/SettingsD4.cs line 32, default true.
    pub is_import_paragon_infinity_builds_enabled: bool,
    pub is_import_paragon_maxroll_enabled: bool,
    pub is_import_paragon_mobalytics_enabled: bool,
    // Upstream: D4Companion.Entities/SettingsD4.cs line 81,
    // default "Maxroll.gg". Persisted as the upstream website
    // display name (e.g. "Maxroll.gg"); the provider mapping is
    // owned by the build-import workflow.
    pub selected_build_import_website: String,
    /// Directory the build-import dialogs read and write the cached
    /// provider build JSONs from.
    ///
    /// d4lc-native field, no upstream counterpart: every upstream
    /// `BuildsManager*` hardcodes `.\Builds\{Provider}` relative to the
    /// app directory (`BuildsManagerD2Core.cs:213-214,592`,
    /// `BuildsManagerMaxroll.cs:490-491,553`), so there is nothing to
    /// mirror in `SettingsD4`.
    ///
    /// `None` — the default, and the value every file written before
    /// this field existed loads — keeps the upstream-equivalent
    /// `{config_root}/Builds/{provider}` home as the resolved default; a
    /// configured directory is used verbatim by both the dialog listing
    /// and the download worker.
    pub builds_dir: Option<String>,
    // Upstream: D4Companion.Entities/SettingsD4.cs line 60, default false.
    pub is_tuning_prism_hints_enabled: bool,
    pub is_show_browser_mobalytics_enabled: bool,
    /// Default highlight colors per affix class (upstream
    /// `SettingsD4.cs:18-24`, `DefaultColor*`). Stored as color strings;
    /// the ColorsConfig dialog edits them and the add/import flows read
    /// them when a new preset entry is created without an explicit
    /// color.
    pub default_colors: DefaultColors,
    // Paragon layout (upstream `SettingsD4.cs:71-75`). The five numeric
    // values size and place the paragon board markers; the collapsed
    // board switch itself lives on `rule_settings`
    // (`is_collapsed_paragonboard_enabled`).
    pub paragon_border_size: u32,
    pub paragon_node_size: u32,
    pub paragon_node_size_collapsed: u32,
    pub paragon_left_offset_collapsed: i32,
    pub paragon_top_offset_collapsed: i32,
    pub rule_settings: RuleSettingsConfig,
    /// Hotkey binding config.
    ///
    /// Owns the persisted binding strings + enabled flag for every
    /// known upstream hotkey target. The app's hotkey backend reads
    /// these bindings and registers them.
    pub hotkey_config: HotkeyConfig,
    /// Local crash-log diagnostics export (default `false`).
    ///
    /// Opt-in only: when enabled, the Debug screen's crash-log
    /// export action may copy a local crash log to a folder the
    /// user can attach to a GitHub issue they open manually. The
    /// application itself never uploads anything. Files written
    /// before this field existed load with the default (`false`).
    pub is_diagnostics_export_enabled: bool,
    /// Check-only release banner (default `true`).
    ///
    /// When enabled, the app checks this project's GitHub Releases
    /// for a newer version on a bounded interval (at most every 16
    /// hours) and shows a non-blocking banner with the release
    /// link. The check never downloads, installs, or uploads
    /// anything; the `--no-updates` launch flag suppresses it for
    /// one session. Files written before this field existed load
    /// with the default (`true`).
    pub update_check_enabled: bool,
}

/// A single hotkey binding.
///
/// Matches the `KeyBindingConfig*` setting names in
/// `D4Companion.Entities/SettingsD4.cs:93-139`. The `name` is the
/// stable action identifier (e.g. `"SwitchPreset"`), `label` is
/// the human-readable display name used by the HotkeyConfig
/// dialog, `binding` is the modifier+key string (e.g.
/// `"Ctrl+F5"`), and `enabled` is the user-toggleable on/off
/// flag. `enabled=true` is a *request* to register the grab — it
/// does not mean the OS grab succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub name: String,
    pub label: String,
    pub binding: String,
    pub enabled: bool,
}

/// All hotkey bindings owned by `AppSettings`.
///
/// Default: 7 entries, one per known upstream target, with the
/// upstream default bindings and all disabled. New installs and
/// settings files without hotkey entries fall back to this default.
/// On disk each entry is an upstream `KeyBindingConfig*` object; the
/// older `HotkeyBindings` array is still read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub bindings: Vec<HotkeyBinding>,
}

/// All known upstream hotkey target names with their default
/// bindings.
///
/// Stable identifiers match the `KeyBindingConfig*` setting names
/// in `D4Companion.Entities/SettingsD4.cs:93-139`. The display
/// labels and binding defaults mirror upstream.
pub const HOTKEY_TARGETS: &[(&str, &str, &str)] = &[
    ("SwitchPreset", "Switch preset", "Ctrl+F5"),
    ("SwitchOverlay", "Switch overlay", "Ctrl+F6"),
    ("TakeScreenshot", "Take screenshot", "Ctrl+F10"),
    ("ToggleController", "Toggle controller mode", "Ctrl+F9"),
    ("ToggleOverlay", "Toggle overlay on/off", "Ctrl+F12"),
    (
        "ToggleDebugLockScreencapture",
        "Debug lock capture",
        "Ctrl+F11",
    ),
    // Upstream: SettingsD4.cs KeyBindingConfigToggleTuningPrismHints
    // (line 131): IsEnabled = false, KeyGestureKey = Key.F7,
    // KeyGestureModifier = ModifierKeys.Control.
    (
        "ToggleTuningPrismHints",
        "Toggle tuning prism hints",
        "Ctrl+F7",
    ),
];

impl HotkeyConfig {
    /// Construct the upstream-shaped default: one entry per
    /// known target, all disabled. This is the value used both
    /// by `AppSettings::default()` and by the JSON parser when the
    /// file carries no hotkey entries.
    pub fn default_bindings() -> Self {
        let bindings = HOTKEY_TARGETS
            .iter()
            .map(|(name, label, binding)| HotkeyBinding {
                name: (*name).to_string(),
                label: (*label).to_string(),
                binding: (*binding).to_string(),
                enabled: false,
            })
            .collect();
        Self { bindings }
    }

    /// Look up a binding by its stable action name.
    pub fn find_by_name(&self, name: &str) -> Option<&HotkeyBinding> {
        self.bindings.iter().find(|b| b.name == name)
    }

    /// Look up a binding by its stable action name (mutable).
    pub fn find_by_name_mut(&mut self, name: &str) -> Option<&mut HotkeyBinding> {
        self.bindings.iter_mut().find(|b| b.name == name)
    }

    /// Upsert a binding by name. If an entry with the same
    /// `name` exists, replace it; otherwise insert a new entry.
    pub fn upsert(&mut self, binding: HotkeyBinding) {
        if let Some(existing) = self.find_by_name_mut(&binding.name) {
            *existing = binding;
        } else {
            self.bindings.push(binding);
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self::default_bindings()
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            selected_system_preset: DEFAULT_SYSTEM_PRESET.to_string(),
            selected_affix_language: DEFAULT_AFFIX_LANGUAGE.to_string(),
            selected_app_language: DEFAULT_APP_LANGUAGE.to_string(),
            selected_affix_preset: String::new(),
            capture_backend: DEFAULT_CAPTURE_BACKEND.to_string(),
            display_backend: DEFAULT_DISPLAY_BACKEND.to_string(),
            wayland_restore_token: None,
            controller_mode: false,
            debug_mode: false,
            dev_mode: false,
            is_debug_info_enabled: false,
            check_for_updates: true,
            launch_minimized: false,
            minimize_to_tray: false,
            window_geometry: None,
            paragon_mode_active: false,
            scan_height_percent: DEFAULT_SCAN_HEIGHT_PERCENT,
            screen_capture_delay: DEFAULT_SCREEN_CAPTURE_DELAY,
            overlay_update_delay: DEFAULT_OVERLAY_UPDATE_DELAY,
            minimal_ocr_match_type: DEFAULT_MINIMAL_OCR_MATCH_TYPE,
            threshold_min: DEFAULT_THRESHOLD_MIN,
            threshold_max: DEFAULT_THRESHOLD_MAX,
            threshold_similarity_tooltip: DEFAULT_THRESHOLD_SIMILARITY_TOOLTIP,
            threshold_similarity_affix_location: DEFAULT_THRESHOLD_SIMILARITY_AFFIX_LOCATION,
            threshold_similarity_aspect_location: DEFAULT_THRESHOLD_SIMILARITY_ASPECT_LOCATION,
            threshold_similarity_socket_location: DEFAULT_THRESHOLD_SIMILARITY_SOCKET_LOCATION,
            threshold_similarity_splitter_location: DEFAULT_THRESHOLD_SIMILARITY_SPLITTER_LOCATION,
            tooltip_width: DEFAULT_TOOLTIP_WIDTH,
            tooltip_max_height: DEFAULT_TOOLTIP_MAX_HEIGHT,
            affix_area_height_offset_top: DEFAULT_AFFIX_AREA_HEIGHT_OFFSET_TOP,
            affix_area_height_offset_bottom: DEFAULT_AFFIX_AREA_HEIGHT_OFFSET_BOTTOM,
            affix_aspect_area_width_offset: DEFAULT_AFFIX_ASPECT_AREA_WIDTH_OFFSET,
            aspect_area_height_offset_top: DEFAULT_ASPECT_AREA_HEIGHT_OFFSET_TOP,
            is_socket_detection_enabled: true,
            is_tempered_affix_detection_enabled: true,
            overlay_enabled: false,
            is_top_most: false,
            overlay_font_size: DEFAULT_OVERLAY_FONT_SIZE,
            // Upstream default (SettingsD4.cs:83): the trade screen shows
            // the current-item card unless the user turns it off.
            show_current_item: true,
            show_overlay_icon: false,
            overlay_icon_pos_x: 0,
            overlay_icon_pos_y: 0,
            type_area_offset_left: DEFAULT_TYPE_AREA_OFFSET_LEFT,
            type_area_offset_right: DEFAULT_TYPE_AREA_OFFSET_RIGHT,
            aspect_area_max_height: DEFAULT_ASPECT_AREA_MAX_HEIGHT,
            aspect_icon_offset: DEFAULT_ASPECT_ICON_OFFSET,
            overlay_menu_locked: false,
            is_import_paragon_d2core_enabled: true,
            is_import_paragon_d4builds_enabled: true,
            is_import_paragon_infinity_builds_enabled: true,
            is_import_paragon_maxroll_enabled: true,
            is_import_paragon_mobalytics_enabled: true,
            selected_build_import_website: DEFAULT_SELECTED_BUILD_IMPORT_WEBSITE.to_string(),
            builds_dir: None,
            is_tuning_prism_hints_enabled: false,
            is_show_browser_mobalytics_enabled: false,
            default_colors: DefaultColors::default(),
            paragon_border_size: DEFAULT_PARAGON_BORDER_SIZE,
            paragon_node_size: DEFAULT_PARAGON_NODE_SIZE,
            paragon_node_size_collapsed: DEFAULT_PARAGON_NODE_SIZE_COLLAPSED,
            paragon_left_offset_collapsed: DEFAULT_PARAGON_LEFT_OFFSET_COLLAPSED,
            paragon_top_offset_collapsed: DEFAULT_PARAGON_TOP_OFFSET_COLLAPSED,
            rule_settings: RuleSettingsConfig::default(),
            hotkey_config: HotkeyConfig::default(),
            is_diagnostics_export_enabled: false,
            update_check_enabled: true,
        }
    }
}

impl AppSettings {
    /// The one effective update-check switch.
    ///
    /// Upstream has a single gate — `SettingsD4.CheckForUpdates`
    /// (SettingsD4.cs, serialized by `SettingsManager.cs:47-68`) — read
    /// by `ReleaseManager.UpdateAvailableReleases` (ReleaseManager.cs:72)
    /// before the release check runs. The port added a second,
    /// port-specific release-banner toggle (`UpdateCheckEnabled`,
    /// Settings.json-only) and the check historically honored only that
    /// one, leaving the upstream toggle dead.
    ///
    /// Both persisted spellings now feed this single predicate instead of
    /// two independent gates, so the upstream `CheckForUpdates` toggle is
    /// honored and a user who disabled the port-specific row keeps the
    /// check off. Both default to `true`, so a fresh or upstream-authored
    /// configuration checks for updates.
    pub const fn update_check_effective(&self) -> bool {
        self.check_for_updates && self.update_check_enabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMarkerModeSetting {
    ShowAll,
    HideUnwanted,
}

impl OverlayMarkerModeSetting {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShowAll => "Show All",
            Self::HideUnwanted => "Hide Unwanted",
        }
    }

    fn from_upstream(value: &str) -> Self {
        if value.eq_ignore_ascii_case("Hide Unwanted") {
            Self::HideUnwanted
        } else {
            Self::ShowAll
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigilDisplayModeSetting {
    Whitelisting,
    Blacklisting,
}

impl SigilDisplayModeSetting {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Whitelisting => "Whitelisting",
            Self::Blacklisting => "Blacklisting",
        }
    }

    /// Parse the upstream `SelectedSigilDisplayMode` string
    /// (`SigilConfigViewModel.cs:85-98` writes the `SigilDisplayModes`
    /// entries verbatim). Unknown values fall back to `Whitelisting`, the
    /// upstream default (`SettingsD4.cs`).
    pub fn from_upstream(value: &str) -> Self {
        if value.eq_ignore_ascii_case("Blacklisting") {
            Self::Blacklisting
        } else {
            Self::Whitelisting
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemPowerRuleSettings {
    pub enabled: bool,
    pub limit: u32,
}

impl Default for ItemPowerRuleSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            limit: DEFAULT_ITEM_POWER_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassToggleSettings {
    pub core: bool,
    pub barbarian: bool,
    pub druid: bool,
    pub necromancer: bool,
    pub paladin: bool,
    pub rogue: bool,
    pub sorcerer: bool,
    pub spiritborn: bool,
    pub warlock: bool,
}

impl Default for ClassToggleSettings {
    fn default() -> Self {
        Self {
            core: true,
            barbarian: true,
            druid: true,
            necromancer: true,
            paladin: true,
            rogue: true,
            sorcerer: true,
            spiritborn: true,
            warlock: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleToggleSettings {
    pub class_toggles: ClassToggleSettings,
    pub dungeons: bool,
    pub positive: bool,
    pub minor: bool,
    pub major: bool,
    pub rune_condition: bool,
    pub rune_effect: bool,
}

impl Default for RuleToggleSettings {
    fn default() -> Self {
        Self {
            class_toggles: ClassToggleSettings::default(),
            dungeons: true,
            positive: true,
            minor: true,
            major: true,
            rune_condition: true,
            rune_effect: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiBuildRuleConfig {
    pub name: String,
    pub index: usize,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSettingsConfig {
    pub item_power: ItemPowerRuleSettings,
    pub minimal_affix_value_enabled: bool,
    pub multi_build_enabled: bool,
    pub trade_overlay_enabled: bool,
    pub overlay_marker_mode: OverlayMarkerModeSetting,
    pub aspect_detection_enabled: bool,
    pub unique_detection_enabled: bool,
    pub rune_detection_enabled: bool,
    pub sigil_detection_enabled: bool,
    pub sigil_display_mode: SigilDisplayModeSetting,
    pub dungeon_tiers_enabled: bool,
    // Upstream: SettingsD4.cs line 28 (`IsCollapsedParagonboardEnabled`,
    // default false). Selects the collapsed paragon overlay draw path
    // (`OverlayHandler.DrawGraphicsParagonCollapsed`, OH:704-839) over
    // the full board (`DrawGraphicsParagon`, OH:568-702).
    pub is_collapsed_paragonboard_enabled: bool,
    pub toggles: RuleToggleSettings,
    pub multi_builds: Vec<MultiBuildRuleConfig>,
}

impl Default for RuleSettingsConfig {
    fn default() -> Self {
        Self {
            item_power: ItemPowerRuleSettings::default(),
            minimal_affix_value_enabled: false,
            multi_build_enabled: false,
            trade_overlay_enabled: true,
            overlay_marker_mode: OverlayMarkerModeSetting::ShowAll,
            aspect_detection_enabled: true,
            unique_detection_enabled: true,
            rune_detection_enabled: true,
            sigil_detection_enabled: true,
            sigil_display_mode: SigilDisplayModeSetting::Whitelisting,
            dungeon_tiers_enabled: true,
            is_collapsed_paragonboard_enabled: false,
            toggles: RuleToggleSettings::default(),
            multi_builds: Vec::new(),
        }
    }
}

impl RuleSettingsConfig {
    pub fn to_rule_runtime_settings(&self) -> RuleRuntimeSettings {
        RuleRuntimeSettings {
            item_power_enabled: self.item_power.enabled,
            item_power_limit: self.item_power.limit,
            minimal_affix_value_enabled: self.minimal_affix_value_enabled,
            multi_build_enabled: self.multi_build_enabled,
            trade_overlay_enabled: self.trade_overlay_enabled,
            overlay_marker_mode: self.overlay_marker_mode,
            multi_builds: self
                .multi_builds
                .iter()
                .map(|build| RuleRuntimeMultiBuild {
                    build_name: build.name.clone(),
                    preset_name: build.name.clone(),
                    index: build.index,
                    color: build.color.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRuntimeSettings {
    pub item_power_enabled: bool,
    pub item_power_limit: u32,
    pub minimal_affix_value_enabled: bool,
    pub multi_build_enabled: bool,
    pub trade_overlay_enabled: bool,
    pub overlay_marker_mode: OverlayMarkerModeSetting,
    pub multi_builds: Vec<RuleRuntimeMultiBuild>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRuntimeMultiBuild {
    pub build_name: String,
    pub preset_name: String,
    pub index: usize,
    pub color: String,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- display-backend shared parser tests ----

    #[test]
    fn display_backend_parse_accepts_stored_tokens_and_rejects_invalid_values() {
        // The three persisted tokens parse to their variants (the
        // selector only ever writes these).
        assert_eq!(DisplayBackend::parse("auto"), Ok(DisplayBackend::Auto));
        assert_eq!(DisplayBackend::parse("x11"), Ok(DisplayBackend::X11));
        assert_eq!(
            DisplayBackend::parse("wayland"),
            Ok(DisplayBackend::Wayland)
        );
        assert_eq!(
            DisplayBackend::parse(" wayland "),
            Ok(DisplayBackend::Wayland)
        );
        // The canonical token round-trips through as_str.
        assert_eq!(DisplayBackend::default().as_str(), DEFAULT_DISPLAY_BACKEND);

        // An invalid stored value is rejected at the parse point with
        // the structured error naming the accepted tokens — never a
        // silent fallback to a default backend.
        for invalid in ["x11-native", "Wayland", "", "pipewire"] {
            let error = DisplayBackend::parse(invalid)
                .expect_err("invalid display backend must be rejected");
            assert!(
                error.starts_with("unsupported display backend")
                    && error.contains("auto|x11|wayland"),
                "structured error must name the accepted tokens: {error}"
            );
        }
    }

    #[test]
    fn minimal_ocr_match_type_defaults_to_upstream_value() {
        let settings = AppSettings::default();
        assert_eq!(settings.minimal_ocr_match_type, 75);
        assert_eq!(settings.type_area_offset_left, 5);
        assert!(!settings.rule_settings.item_power.enabled);
        assert_eq!(settings.rule_settings.item_power.limit, 800);
        assert!(settings.rule_settings.trade_overlay_enabled);
        assert_eq!(
            settings.rule_settings.overlay_marker_mode,
            OverlayMarkerModeSetting::ShowAll
        );
        assert_eq!(
            settings.rule_settings.sigil_display_mode,
            SigilDisplayModeSetting::Whitelisting
        );
        assert!(settings.rule_settings.dungeon_tiers_enabled);
        assert!(settings.rule_settings.toggles.class_toggles.core);
        assert!(settings.rule_settings.toggles.rune_condition);
        assert!(settings.rule_settings.multi_builds.is_empty());
    }

    #[test]
    fn type_area_offset_left_defaults_to_upstream_value() {
        let settings = AppSettings::default();
        assert_eq!(settings.type_area_offset_left, 5);
    }

    #[test]
    fn type_area_offset_right_and_aspect_area_max_height_default_to_upstream_values() {
        let settings = AppSettings::default();
        // Upstream SettingsD4.cs: TypeAreaOffsetRight = 5, AspectAreaMaxHeight = 300.
        assert_eq!(settings.type_area_offset_right, 5);
        assert_eq!(settings.aspect_area_max_height, 300);
    }

    #[test]
    fn window_geometry_defaults_to_none() {
        let settings = AppSettings::default();
        assert_eq!(settings.window_geometry, None);
    }

    #[test]
    fn rule_settings_expose_rules_adapter_values() {
        let settings = RuleSettingsConfig {
            item_power: ItemPowerRuleSettings {
                enabled: true,
                limit: 900,
            },
            minimal_affix_value_enabled: true,
            multi_build_enabled: true,
            trade_overlay_enabled: false,
            overlay_marker_mode: OverlayMarkerModeSetting::HideUnwanted,
            aspect_detection_enabled: false,
            unique_detection_enabled: true,
            rune_detection_enabled: false,
            sigil_detection_enabled: true,
            sigil_display_mode: SigilDisplayModeSetting::Blacklisting,
            dungeon_tiers_enabled: true,
            is_collapsed_paragonboard_enabled: true,
            toggles: RuleToggleSettings::default(),
            multi_builds: vec![MultiBuildRuleConfig {
                name: "Speed".to_string(),
                index: 2,
                color: "Green".to_string(),
            }],
        };

        let adapter = settings.to_rule_runtime_settings();
        assert!(adapter.item_power_enabled);
        assert_eq!(adapter.item_power_limit, 900);
        assert!(adapter.minimal_affix_value_enabled);
        assert!(adapter.multi_build_enabled);
        assert!(!adapter.trade_overlay_enabled);
        assert_eq!(adapter.overlay_marker_mode.as_str(), "Hide Unwanted");
        assert_eq!(adapter.multi_builds[0].preset_name, "Speed");
    }

    #[test]
    fn hotkey_config_default_has_seven_disabled_entries() {
        let config = HotkeyConfig::default();
        assert_eq!(config.bindings.len(), 7);
        // All disabled by default — OS grab is opt-in.
        assert!(config.bindings.iter().all(|b| !b.enabled));
        // The seven stable upstream target names are present in
        // declaration order with the upstream default bindings.
        let names: Vec<&str> = config.bindings.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "SwitchPreset",
                "SwitchOverlay",
                "TakeScreenshot",
                "ToggleController",
                "ToggleOverlay",
                "ToggleDebugLockScreencapture",
                "ToggleTuningPrismHints",
            ]
        );
        // Default binding strings are the upstream defaults.
        assert_eq!(
            config
                .find_by_name("SwitchPreset")
                .expect("default hotkey config should include SwitchPreset")
                .binding,
            "Ctrl+F5"
        );
        assert_eq!(
            config
                .find_by_name("SwitchOverlay")
                .expect("default hotkey config should include SwitchOverlay")
                .binding,
            "Ctrl+F6"
        );
        assert_eq!(
            config
                .find_by_name("TakeScreenshot")
                .expect("default hotkey config should include TakeScreenshot")
                .binding,
            "Ctrl+F10"
        );
        assert_eq!(
            config
                .find_by_name("ToggleController")
                .expect("default hotkey config should include ToggleController")
                .binding,
            "Ctrl+F9"
        );
        assert_eq!(
            config
                .find_by_name("ToggleOverlay")
                .expect("default hotkey config should include ToggleOverlay")
                .binding,
            "Ctrl+F12"
        );
        assert_eq!(
            config
                .find_by_name("ToggleDebugLockScreencapture")
                .expect("default hotkey config should include ToggleDebugLockScreencapture")
                .binding,
            "Ctrl+F11"
        );
        assert_eq!(
            config
                .find_by_name("ToggleTuningPrismHints")
                .expect("default hotkey config should include ToggleTuningPrismHints")
                .binding,
            "Ctrl+F7"
        );
    }

    #[test]
    fn hotkey_config_upsert_replaces_existing_entry() {
        let mut config = HotkeyConfig::default();
        let replacement = HotkeyBinding {
            name: "SwitchPreset".to_string(),
            label: "Switch preset".to_string(),
            binding: "Ctrl+Shift+F5".to_string(),
            enabled: true,
        };
        config.upsert(replacement.clone());
        assert_eq!(config.bindings.len(), 7);
        assert_eq!(config.find_by_name("SwitchPreset"), Some(&replacement));
    }

    #[test]
    fn hotkey_config_upsert_appends_unknown_target() {
        let mut config = HotkeyConfig::default();
        config.upsert(HotkeyBinding {
            name: "CustomAction".to_string(),
            label: "Custom action".to_string(),
            binding: "Alt+F1".to_string(),
            enabled: false,
        });
        assert_eq!(config.bindings.len(), 8);
        assert!(config.find_by_name("CustomAction").is_some());
    }

    #[test]
    fn app_settings_default_includes_hotkey_config_with_seven_entries() {
        let settings = AppSettings::default();
        assert_eq!(settings.hotkey_config.bindings.len(), 7);
        assert!(settings.hotkey_config.bindings.iter().all(|b| !b.enabled));
    }

    #[test]
    fn update_check_effective_is_the_single_gate_for_both_persisted_switches() {
        // Upstream reads one gate — `SettingsD4.CheckForUpdates`
        // (ReleaseManager.cs:72) — while the port added a second,
        // port-specific release-banner toggle. `update_check_effective`
        // is the one predicate the release check consults, so the
        // upstream toggle is honored and the
        // port-specific row keeps its off semantics.
        let defaults = AppSettings::default();
        assert!(defaults.check_for_updates, "upstream default is on");
        assert!(defaults.update_check_enabled, "port default is on");
        assert!(
            defaults.update_check_effective(),
            "a fresh configuration checks for updates"
        );

        let upstream_toggle_off = AppSettings {
            check_for_updates: false,
            ..AppSettings::default()
        };
        assert!(
            !upstream_toggle_off.update_check_effective(),
            "turning off the upstream 'Check for Updates' row stops the check"
        );

        let port_toggle_off = AppSettings {
            update_check_enabled: false,
            ..AppSettings::default()
        };
        assert!(
            !port_toggle_off.update_check_effective(),
            "turning off the port-specific release-banner row stops the check"
        );
    }
}
