//! The Debug screen's settings-backed numeric steppers and their
//! re-projection when the settings file changes.

use std::path::Path;

use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

/// One Debug-screen numeric stepper (`DebugView.xaml:74-184`).
///
/// Upstream binds each control to a `DebugViewModel` setter that writes
/// `SettingsD4` and saves on every change (`DebugViewModel.cs:106-454`).
/// The port keeps the same control order and ranges; every one of the
/// seventeen controls owns an `AppSettings` field — including the four
/// area offsets, which the OCR region assembly reads from the settings
/// (upstream keeps them on `SettingsD4`, `SettingsD4.cs:9-12`) and a
/// system-preset change re-seeds (`SettingsViewModel.cs:258-273`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugStepper {
    TooltipWidth,
    TooltipMaxHeight,
    ThresholdMin,
    ThresholdMax,
    AffixAreaHeightOffsetTop,
    AffixAreaHeightOffsetBottom,
    AffixAspectAreaWidthOffset,
    AspectAreaHeightOffsetTop,
    TypeAreaOffsetLeft,
    TypeAreaOffsetRight,
    AspectAreaMaxHeight,
    MinimalOcrMatchType,
    ThresholdSimilarityTooltip,
    ThresholdSimilarityAffixLocation,
    ThresholdSimilarityAspectLocation,
    ThresholdSimilaritySocketLocation,
    ThresholdSimilaritySplitterLocation,
}

/// Every stepper id the Slint screen sends through `debug-stepper-changed`,
/// in upstream `DebugView.xaml:74-184` order. Test-only: the tests check
/// each id resolves through [`DebugStepper::from_id`].
#[cfg(test)]
pub const DEBUG_STEPPER_IDS: [&str; 17] = [
    "tooltip_width",
    "tooltip_max_height",
    "threshold_min",
    "threshold_max",
    "affix_area_height_offset_top",
    "affix_area_height_offset_bottom",
    "affix_aspect_area_width_offset",
    "aspect_area_height_offset_top",
    "type_area_offset_left",
    "type_area_offset_right",
    "aspect_area_max_height",
    "minimal_ocr_match_type",
    "threshold_similarity_tooltip",
    "threshold_similarity_affix_location",
    "threshold_similarity_aspect_location",
    "threshold_similarity_socket_location",
    "threshold_similarity_splitter_location",
];

impl DebugStepper {
    /// Resolve the id the screen sent. `None` for an id this build does
    /// not know — the handler records that instead of guessing.
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "tooltip_width" => Self::TooltipWidth,
            "tooltip_max_height" => Self::TooltipMaxHeight,
            "threshold_min" => Self::ThresholdMin,
            "threshold_max" => Self::ThresholdMax,
            "affix_area_height_offset_top" => Self::AffixAreaHeightOffsetTop,
            "affix_area_height_offset_bottom" => Self::AffixAreaHeightOffsetBottom,
            "affix_aspect_area_width_offset" => Self::AffixAspectAreaWidthOffset,
            "aspect_area_height_offset_top" => Self::AspectAreaHeightOffsetTop,
            "type_area_offset_left" => Self::TypeAreaOffsetLeft,
            "type_area_offset_right" => Self::TypeAreaOffsetRight,
            "aspect_area_max_height" => Self::AspectAreaMaxHeight,
            "minimal_ocr_match_type" => Self::MinimalOcrMatchType,
            "threshold_similarity_tooltip" => Self::ThresholdSimilarityTooltip,
            "threshold_similarity_affix_location" => Self::ThresholdSimilarityAffixLocation,
            "threshold_similarity_aspect_location" => Self::ThresholdSimilarityAspectLocation,
            "threshold_similarity_socket_location" => Self::ThresholdSimilaritySocketLocation,
            "threshold_similarity_splitter_location" => Self::ThresholdSimilaritySplitterLocation,
            _ => return None,
        })
    }

    /// The stable id (same token the Slint screen sends).
    #[cfg(test)]
    pub const fn id(self) -> &'static str {
        match self {
            Self::TooltipWidth => "tooltip_width",
            Self::TooltipMaxHeight => "tooltip_max_height",
            Self::ThresholdMin => "threshold_min",
            Self::ThresholdMax => "threshold_max",
            Self::AffixAreaHeightOffsetTop => "affix_area_height_offset_top",
            Self::AffixAreaHeightOffsetBottom => "affix_area_height_offset_bottom",
            Self::AffixAspectAreaWidthOffset => "affix_aspect_area_width_offset",
            Self::AspectAreaHeightOffsetTop => "aspect_area_height_offset_top",
            Self::TypeAreaOffsetLeft => "type_area_offset_left",
            Self::TypeAreaOffsetRight => "type_area_offset_right",
            Self::AspectAreaMaxHeight => "aspect_area_max_height",
            Self::MinimalOcrMatchType => "minimal_ocr_match_type",
            Self::ThresholdSimilarityTooltip => "threshold_similarity_tooltip",
            Self::ThresholdSimilarityAffixLocation => "threshold_similarity_affix_location",
            Self::ThresholdSimilarityAspectLocation => "threshold_similarity_aspect_location",
            Self::ThresholdSimilaritySocketLocation => "threshold_similarity_socket_location",
            Self::ThresholdSimilaritySplitterLocation => "threshold_similarity_splitter_location",
        }
    }

    /// The upstream control range (`DebugView.xaml:74-184`). A stepper
    /// edit is clamped to it before it reaches the settings state, so a
    /// stray value from the UI can never widen the persisted range.
    ///
    /// `TypeAreaOffsetLeft` is 0..=25 here and in the overlay config
    /// dialog (`DebugView.xaml:108-109`).
    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::TooltipWidth | Self::TooltipMaxHeight => (100.0, 1000.0),
            Self::ThresholdMin | Self::ThresholdMax => (0.0, 255.0),
            Self::AffixAreaHeightOffsetTop
            | Self::AffixAreaHeightOffsetBottom
            | Self::AffixAspectAreaWidthOffset
            | Self::AspectAreaHeightOffsetTop => (0.0, 50.0),
            Self::TypeAreaOffsetLeft => (0.0, 25.0),
            Self::TypeAreaOffsetRight => (0.0, 250.0),
            Self::AspectAreaMaxHeight => (100.0, 999.0),
            Self::MinimalOcrMatchType => (1.0, 100.0),
            Self::ThresholdSimilarityTooltip
            | Self::ThresholdSimilarityAffixLocation
            | Self::ThresholdSimilarityAspectLocation
            | Self::ThresholdSimilaritySocketLocation
            | Self::ThresholdSimilaritySplitterLocation => (0.01, 0.09),
        }
    }

    fn clamp(self, value: f64) -> f64 {
        let (min, max) = self.range();
        value.clamp(min, max)
    }
}

/// Handle one Debug-screen stepper edit.
///
/// Mirrors the wired `type_area_offset_left` pattern: clamp to the
/// upstream range, write the value into the in-memory settings state,
/// project it back to the root property, and persist through the shared
/// settings save path (`persist_settings_with_warnings` surfaces a
/// refused save on the Logging screen). An unknown id is an
/// no-op with a log line.
pub fn handle_debug_stepper_changed(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    id: &str,
    value: f64,
) {
    if let Some(stepper) = apply_debug_stepper_change(state, config_root, id, value) {
        project_stepper_value(app, state, stepper);
    }
}

/// State-only portion of [`handle_debug_stepper_changed`]: resolve the
/// id, clamp to the upstream range, store the value and persist it.
/// Returns the applied stepper, or `None` for an unknown id (the refusal
/// is logged). Public so the headless tests can drive the persistence
/// path without a Slint window.
pub fn apply_debug_stepper_change(
    state: &mut AppState,
    config_root: &Path,
    id: &str,
    value: f64,
) -> Option<DebugStepper> {
    let Some(stepper) = DebugStepper::from_id(id) else {
        state.append_log_message(
            format!("debug stepper '{id}' is unknown in this build; edit ignored"),
            crate::clock::hh_mm_now(),
        );
        return None;
    };

    let clamped = stepper.clamp(value);
    apply_stepper_value(&mut state.shell.settings, stepper, clamped);
    state.bump_rules_input_revision();
    crate::actions::settings::persist_settings_with_warnings(state, config_root);
    Some(stepper)
}

/// Write one clamped stepper value into the in-memory settings state.
fn apply_stepper_value(
    settings: &mut d4lc_gui::SettingsPanelState,
    stepper: DebugStepper,
    value: f64,
) {
    let as_u32 = |v: f64| v.round().max(0.0) as u32;
    let as_u8 = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    let as_i32 = |v: f64| v.round() as i32;
    match stepper {
        DebugStepper::TooltipWidth => settings.tooltip_width = as_u32(value),
        DebugStepper::TooltipMaxHeight => settings.tooltip_max_height = as_u32(value),
        DebugStepper::AffixAreaHeightOffsetTop => {
            settings.affix_area_height_offset_top = as_i32(value);
        }
        DebugStepper::AffixAreaHeightOffsetBottom => {
            settings.affix_area_height_offset_bottom = as_i32(value);
        }
        DebugStepper::AffixAspectAreaWidthOffset => {
            settings.affix_aspect_area_width_offset = as_i32(value);
        }
        DebugStepper::AspectAreaHeightOffsetTop => {
            settings.aspect_area_height_offset_top = as_i32(value);
        }
        DebugStepper::ThresholdMin => settings.threshold_min = as_u8(value),
        DebugStepper::ThresholdMax => settings.threshold_max = as_u8(value),
        DebugStepper::TypeAreaOffsetLeft => settings.type_area_offset_left = as_u32(value),
        DebugStepper::TypeAreaOffsetRight => settings.type_area_offset_right = as_u32(value),
        DebugStepper::AspectAreaMaxHeight => settings.aspect_area_max_height = as_u32(value),
        DebugStepper::MinimalOcrMatchType => settings.minimal_ocr_match_type = as_u8(value),
        DebugStepper::ThresholdSimilarityTooltip => {
            settings.threshold_similarity_tooltip = value;
        }
        DebugStepper::ThresholdSimilarityAffixLocation => {
            settings.threshold_similarity_affix_location = value;
        }
        DebugStepper::ThresholdSimilarityAspectLocation => {
            settings.threshold_similarity_aspect_location = value;
        }
        DebugStepper::ThresholdSimilaritySocketLocation => {
            settings.threshold_similarity_socket_location = value;
        }
        DebugStepper::ThresholdSimilaritySplitterLocation => {
            settings.threshold_similarity_splitter_location = value;
        }
    }
}

/// Project every Debug-screen stepper value into the root `AppWindow`
/// properties the screen binds two-way.
///
/// All seventeen values come from the in-memory settings state (the same
/// state every save writes) — including the four area offsets, which the
/// OCR region assembly reads from the settings and a preset change
/// re-seeds.
pub fn project_debug_stepper_values(app: &AppWindow, state: &AppState) {
    let settings = &state.shell.settings;
    app.set_debug_tooltip_width(settings.tooltip_width as f32);
    app.set_debug_tooltip_max_height(settings.tooltip_max_height as f32);
    app.set_debug_affix_area_height_offset_top(settings.affix_area_height_offset_top as f32);
    app.set_debug_affix_area_height_offset_bottom(settings.affix_area_height_offset_bottom as f32);
    app.set_debug_affix_aspect_area_width_offset(settings.affix_aspect_area_width_offset as f32);
    app.set_debug_aspect_area_height_offset_top(settings.aspect_area_height_offset_top as f32);
    app.set_debug_threshold_min(f32::from(settings.threshold_min));
    app.set_debug_threshold_max(f32::from(settings.threshold_max));
    app.set_type_area_offset_left(settings.type_area_offset_left as f32);
    app.set_debug_type_area_offset_right(settings.type_area_offset_right as f32);
    app.set_debug_aspect_area_max_height(settings.aspect_area_max_height as f32);
    app.set_debug_minimal_ocr_match_type(f32::from(settings.minimal_ocr_match_type));
    app.set_debug_threshold_similarity_tooltip(settings.threshold_similarity_tooltip as f32);
    app.set_debug_threshold_similarity_affix_location(
        settings.threshold_similarity_affix_location as f32,
    );
    app.set_debug_threshold_similarity_aspect_location(
        settings.threshold_similarity_aspect_location as f32,
    );
    app.set_debug_threshold_similarity_socket_location(
        settings.threshold_similarity_socket_location as f32,
    );
    app.set_debug_threshold_similarity_splitter_location(
        settings.threshold_similarity_splitter_location as f32,
    );
}

/// Re-projects the Debug-screen stepper values when `Config/Settings.json`
/// changes on disk.
///
/// This is the port's equivalent of upstream's
/// `HandleSystemPresetChangedMessage` (`DebugViewModel.cs:574-587`), which
/// re-reads the offset/threshold fields into the panel when the system
/// preset changes. The settings file is the single source both the
/// Settings tab and an out-of-app edit write, so one stamp comparison
/// covers every writer.
#[derive(Debug)]
pub struct DebugSettingsProjection {
    last_stamp: Option<(std::time::SystemTime, u64)>,
}

impl DebugSettingsProjection {
    /// Start watching `config_root`'s settings file.
    pub fn new(config_root: &Path) -> Self {
        Self {
            last_stamp: settings_stamp(config_root),
        }
    }

    /// Re-project the stepper values when the settings file changed since
    /// the previous call. Returns `true` when a re-projection happened.
    pub fn observe_and_reproject(
        &mut self,
        app: &AppWindow,
        state: &mut AppState,
        config_root: &Path,
    ) -> bool {
        if !self.reproject_steppers_from_settings(state, config_root) {
            return false;
        }
        project_debug_stepper_values(app, state);
        true
    }

    /// State-only portion of [`Self::observe_and_reproject`]: reload the
    /// settings file when its stamp changed and copy the stepper fields
    /// into the in-memory settings state. Returns `true` on a reload.
    pub fn reproject_steppers_from_settings(
        &mut self,
        state: &mut AppState,
        config_root: &Path,
    ) -> bool {
        let stamp = settings_stamp(config_root);
        if stamp == self.last_stamp {
            return false;
        }
        self.last_stamp = stamp;
        let Ok(settings) = d4lc_config::load_app_settings(config_root) else {
            return false;
        };
        apply_settings_to_debug_steppers(state, &settings);
        true
    }
}

/// Cheap (mtime, size) stamp of `Config/Settings.json`; `None` when absent.
fn settings_stamp(config_root: &Path) -> Option<(std::time::SystemTime, u64)> {
    let path = config_root.join(d4lc_config::SETTINGS_PATH);
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Copy the persisted stepper fields from a freshly loaded settings file
/// into the in-memory settings state (the 13 fields the Debug screen
/// owns; every other field keeps whatever its own writer set).
fn apply_settings_to_debug_steppers(state: &mut AppState, settings: &d4lc_config::AppSettings) {
    state.bump_rules_input_revision();
    let panel = &mut state.shell.settings;
    panel.tooltip_width = settings.tooltip_width;
    panel.tooltip_max_height = settings.tooltip_max_height;
    panel.threshold_min = settings.threshold_min;
    panel.threshold_max = settings.threshold_max;
    panel.type_area_offset_left = settings.type_area_offset_left;
    panel.type_area_offset_right = settings.type_area_offset_right;
    panel.aspect_area_max_height = settings.aspect_area_max_height;
    panel.minimal_ocr_match_type = settings.minimal_ocr_match_type;
    panel.threshold_similarity_tooltip = settings.threshold_similarity_tooltip;
    panel.threshold_similarity_affix_location = settings.threshold_similarity_affix_location;
    panel.threshold_similarity_aspect_location = settings.threshold_similarity_aspect_location;
    panel.threshold_similarity_socket_location = settings.threshold_similarity_socket_location;
    panel.threshold_similarity_splitter_location = settings.threshold_similarity_splitter_location;
}

/// Project one stepper value back into its root property (the screen's
/// two-way binding source) after a clamped edit.
fn project_stepper_value(app: &AppWindow, state: &AppState, stepper: DebugStepper) {
    let settings = &state.shell.settings;
    match stepper {
        DebugStepper::TooltipWidth => app.set_debug_tooltip_width(settings.tooltip_width as f32),
        DebugStepper::TooltipMaxHeight => {
            app.set_debug_tooltip_max_height(settings.tooltip_max_height as f32);
        }
        DebugStepper::ThresholdMin => {
            app.set_debug_threshold_min(f32::from(settings.threshold_min));
        }
        DebugStepper::ThresholdMax => {
            app.set_debug_threshold_max(f32::from(settings.threshold_max));
        }
        DebugStepper::TypeAreaOffsetLeft => {
            app.set_type_area_offset_left(settings.type_area_offset_left as f32);
        }
        DebugStepper::TypeAreaOffsetRight => {
            app.set_debug_type_area_offset_right(settings.type_area_offset_right as f32);
        }
        DebugStepper::AspectAreaMaxHeight => {
            app.set_debug_aspect_area_max_height(settings.aspect_area_max_height as f32);
        }
        DebugStepper::MinimalOcrMatchType => {
            app.set_debug_minimal_ocr_match_type(f32::from(settings.minimal_ocr_match_type));
        }
        DebugStepper::ThresholdSimilarityTooltip => {
            app.set_debug_threshold_similarity_tooltip(settings.threshold_similarity_tooltip as f32)
        }
        DebugStepper::ThresholdSimilarityAffixLocation => app
            .set_debug_threshold_similarity_affix_location(
                settings.threshold_similarity_affix_location as f32,
            ),
        DebugStepper::ThresholdSimilarityAspectLocation => app
            .set_debug_threshold_similarity_aspect_location(
                settings.threshold_similarity_aspect_location as f32,
            ),
        DebugStepper::ThresholdSimilaritySocketLocation => app
            .set_debug_threshold_similarity_socket_location(
                settings.threshold_similarity_socket_location as f32,
            ),
        DebugStepper::ThresholdSimilaritySplitterLocation => app
            .set_debug_threshold_similarity_splitter_location(
                settings.threshold_similarity_splitter_location as f32,
            ),
        DebugStepper::AffixAreaHeightOffsetTop => {
            app.set_debug_affix_area_height_offset_top(settings.affix_area_height_offset_top as f32)
        }
        DebugStepper::AffixAreaHeightOffsetBottom => app.set_debug_affix_area_height_offset_bottom(
            settings.affix_area_height_offset_bottom as f32,
        ),
        DebugStepper::AffixAspectAreaWidthOffset => app.set_debug_affix_aspect_area_width_offset(
            settings.affix_aspect_area_width_offset as f32,
        ),
        DebugStepper::AspectAreaHeightOffsetTop => app
            .set_debug_aspect_area_height_offset_top(settings.aspect_area_height_offset_top as f32),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::actions::debug::test_support::{debug_test_root, fresh_state};

    /// Named decision point: every settings-backed Debug stepper persists
    /// its (clamped) value through the shared settings save path, and the
    /// value survives a reload. Oracle: readback of the settings file.
    #[test]
    fn debug_stepper_changes_persist_and_clamp_to_the_ui_range() {
        let root = debug_test_root("stepper-persist");
        let mut state = fresh_state();

        // In range: stored verbatim.
        apply_debug_stepper_change(&mut state, &root, "tooltip_width", 640.0);
        // Above the upstream maximum: clamped to 1000.
        apply_debug_stepper_change(&mut state, &root, "tooltip_width", 4000.0);
        // Below the minimum: clamped to 100.
        apply_debug_stepper_change(&mut state, &root, "tooltip_max_height", 5.0);
        // Fractional similarity value stays fractional.
        apply_debug_stepper_change(
            &mut state,
            &root,
            "threshold_similarity_affix_location",
            0.03,
        );
        // The type-area offset and aspect-area height steppers.
        apply_debug_stepper_change(&mut state, &root, "type_area_offset_right", 250.0);
        apply_debug_stepper_change(&mut state, &root, "aspect_area_max_height", 999.0);

        assert_eq!(state.shell.settings.tooltip_width, 1000);
        assert_eq!(state.shell.settings.tooltip_max_height, 100);
        assert_eq!(
            state.shell.settings.threshold_similarity_affix_location,
            0.03
        );
        assert_eq!(state.shell.settings.type_area_offset_right, 250);
        assert_eq!(state.shell.settings.aspect_area_max_height, 999);

        let reloaded = d4lc_config::load_app_settings(&root).unwrap();
        assert_eq!(reloaded.tooltip_width, 1000);
        assert_eq!(reloaded.tooltip_max_height, 100);
        assert_eq!(reloaded.threshold_similarity_affix_location, 0.03);
        assert_eq!(reloaded.type_area_offset_right, 250);
        assert_eq!(reloaded.aspect_area_max_height, 999);

        std::fs::remove_dir_all(&root).ok();
    }

    /// `DebugView.xaml:108-109`: TypeAreaOffsetLeft is 0..=25, so a stepper
    /// edit above 25 clamps there.
    #[test]
    fn type_area_offset_left_stepper_clamps_to_the_upstream_range() {
        let root = debug_test_root("stepper-type-area-left");
        let mut state = fresh_state();

        apply_debug_stepper_change(&mut state, &root, "type_area_offset_left", 60.0);
        assert_eq!(state.shell.settings.type_area_offset_left, 25);
        apply_debug_stepper_change(&mut state, &root, "type_area_offset_left", -4.0);
        assert_eq!(state.shell.settings.type_area_offset_left, 0);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Named decision point: the four area offsets are live settings now
    /// (upstream keeps them on `SettingsD4`, `SettingsD4.cs:9-12`), so a
    /// stepper edit writes the settings value and persists it, and the
    /// persisted file carries the upstream-shaped offset keys.
    #[test]
    fn area_offset_stepper_edits_persist_as_settings() {
        let root = debug_test_root("stepper-offsets");
        let mut state = fresh_state();

        let seeded = [
            ("affix_area_height_offset_top", 12.0),
            ("affix_area_height_offset_bottom", 14.0),
            ("affix_aspect_area_width_offset", 20.0),
            ("aspect_area_height_offset_top", 16.0),
        ];
        for (id, value) in seeded {
            let applied = apply_debug_stepper_change(&mut state, &root, id, value);
            assert_eq!(applied.map(DebugStepper::id), Some(id));
        }
        let panel = &state.shell.settings;
        assert_eq!(panel.affix_area_height_offset_top, 12);
        assert_eq!(panel.affix_area_height_offset_bottom, 14);
        assert_eq!(panel.affix_aspect_area_width_offset, 20);
        assert_eq!(panel.aspect_area_height_offset_top, 16);

        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert_eq!(persisted.affix_area_height_offset_top, 12);
        assert_eq!(persisted.aspect_area_height_offset_top, 16);

        // An unknown id is still refused with a log line (no silent drop).
        apply_debug_stepper_change(&mut state, &root, "not_a_stepper", 1.0);
        assert!(state
            .logging
            .messages
            .iter()
            .any(|message| message.message.contains("not_a_stepper")
                && message.message.contains("unknown")));

        std::fs::remove_dir_all(&root).ok();
    }

    /// Named decision point: the Debug screen re-projects its stepper
    /// values when `Config/Settings.json` changes on disk (upstream
    /// re-reads them when the system preset changes,
    /// DebugViewModel.cs:574-587). Oracle: the in-memory settings state
    /// after the file was rewritten by another writer.
    #[test]
    fn settings_file_change_reprojects_debug_steppers() {
        let root = debug_test_root("reproject");
        let mut state = fresh_state();
        let mut settings = d4lc_config::AppSettings {
            threshold_min: 70,
            ..d4lc_config::AppSettings::default()
        };
        d4lc_config::save_app_settings(&root, &settings).unwrap();

        let mut projection = DebugSettingsProjection::new(&root);
        assert!(
            !projection.reproject_steppers_from_settings(&mut state, &root),
            "an unchanged file must not re-project"
        );

        settings.threshold_min = 91;
        settings.type_area_offset_right = 17;
        d4lc_config::save_app_settings(&root, &settings).unwrap();

        assert!(
            projection.reproject_steppers_from_settings(&mut state, &root),
            "a changed file must re-project"
        );
        assert_eq!(state.shell.settings.threshold_min, 91);
        assert_eq!(state.shell.settings.type_area_offset_right, 17);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Named decision point: a stepper id the screen sends is applied to
    /// its own settings field only (no cross-field bleed).
    #[test]
    fn stepper_ids_map_to_their_own_settings_field() {
        let root = debug_test_root("stepper-ids");
        let mut state = fresh_state();

        for id in DEBUG_STEPPER_IDS {
            let stepper = DebugStepper::from_id(id)
                .unwrap_or_else(|| panic!("{id} must resolve to a stepper"));
            assert_eq!(stepper.id(), id, "id round-trip must be stable");
        }
        assert!(DebugStepper::from_id("nope").is_none());

        let before = state.shell.settings.clone();
        apply_debug_stepper_change(&mut state, &root, "minimal_ocr_match_type", 88.0);
        assert_eq!(state.shell.settings.minimal_ocr_match_type, 88);
        assert_eq!(state.shell.settings.threshold_min, before.threshold_min);
        assert_eq!(state.shell.settings.tooltip_width, before.tooltip_width);

        std::fs::remove_dir_all(&root).ok();
    }
}
