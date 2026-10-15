//! Loading and saving `Config/Settings.json`, including the schema-version
//! check and the migrations between settings versions.

use super::codec::{app_settings_json, parse_app_settings};
use super::AppSettings;
use crate::persistence::{self, ConfigError};
use crate::{json, SETTINGS_PATH};
use std::fs;
use std::path::Path;

/// Schema version of the persisted `Config/Settings.json` shape written
/// by this build.
///
/// The version lives in the file, not in `AppSettings`: it is file
/// metadata owned by load/save. Files written before versioning existed
/// carry no `schema_version` field and are treated as version 0. Bump
/// this constant whenever a settings change needs a data
/// transformation, and add one `migrate_vN_to_vN+1` step (with a
/// fixture test) per bumped version to `migrate_app_settings`.
pub const SETTINGS_SCHEMA_VERSION: u32 = 2;

/// A non-fatal condition detected while loading `Config/Settings.json`.
///
/// Warnings never block loading; they are reported through
/// [`load_app_settings_with_warnings`] so callers can react — for
/// example by refusing to save over newer user state. The plain
/// [`load_app_settings`] entry point keeps returning the loaded
/// settings and discards warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSettingsWarning {
    /// The file was written by a newer schema version than this build
    /// supports. The known fields were loaded read-only, no migration
    /// ran, and [`save_app_settings`] refuses to overwrite the file so
    /// unknown newer fields are never silently dropped.
    NewerSchemaVersion {
        /// `schema_version` detected in the loaded file.
        file_version: u32,
        /// Newest `schema_version` this build understands.
        supported_version: u32,
    },
}

impl std::fmt::Display for AppSettingsWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewerSchemaVersion {
                file_version,
                supported_version,
            } => write!(
                f,
                "Config/Settings.json schema_version {file_version} is newer than supported version {supported_version}; settings loaded read-only"
            ),
        }
    }
}

pub fn load_app_settings(root: &Path) -> Result<AppSettings, String> {
    load_app_settings_with_warnings(root).map(|(settings, _)| settings)
}

/// Load app settings and report structured [`AppSettingsWarning`]s.
///
/// A missing file yields defaults with no warnings. A file whose
/// `schema_version` is older than [`SETTINGS_SCHEMA_VERSION`] is
/// migrated step by step before it is returned. A file with a newer
/// `schema_version` is returned read-only (known fields only) together
/// with an [`AppSettingsWarning::NewerSchemaVersion`]; [`save_app_settings`]
/// refuses to overwrite such a file.
pub fn load_app_settings_with_warnings(
    root: &Path,
) -> Result<(AppSettings, Vec<AppSettingsWarning>), String> {
    let path = root.join(SETTINGS_PATH);
    if !path.is_file() {
        return Ok((AppSettings::default(), Vec::new()));
    }

    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_app_settings_with_warnings(&text)
}

/// Load the settings the way upstream does at startup and write them back
/// (`SettingsManager.LoadSettings`, SettingsManager.cs:46-57): the first run
/// creates `Config/Settings.json` with the defaults, and an existing file is
/// rewritten in the current shape.
///
/// Returns the loaded settings and, when the write-back was refused or
/// failed (a file from a newer schema version is never overwritten), the
/// reason. The settings are usable either way.
///
/// # Errors
///
/// Returns the load error; nothing is written then, so an unreadable file is
/// never replaced by defaults.
pub fn load_app_settings_and_save(root: &Path) -> Result<(AppSettings, Option<String>), String> {
    let (settings, _warnings) = load_app_settings_with_warnings(root)?;
    let save_error = save_app_settings(root, &settings).err();
    Ok((settings, save_error))
}

pub fn save_app_settings(root: &Path, settings: &AppSettings) -> Result<(), String> {
    let path = root.join(SETTINGS_PATH);
    if path.is_file() {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        // Fail closed on files written by a newer schema version:
        // overwriting them with the current shape would silently drop
        // the fields this build does not know.
        let file_version = settings_schema_version(&text);
        if file_version > SETTINGS_SCHEMA_VERSION {
            return Err(String::from(ConfigError::NewerSchema {
                path: SETTINGS_PATH.to_string(),
                version: file_version,
                supported_version: SETTINGS_SCHEMA_VERSION,
            }));
        }
    }
    keep_settings_migration_backup(root).map_err(String::from)?;
    persistence::write_atomic(root, SETTINGS_PATH, &app_settings_json(settings))
        .map_err(String::from)?;
    Ok(())
}

/// Keep a versioned copy of an older settings file before it is
/// rewritten in the current schema shape.
///
/// The migration is only reversible through the previous bytes; the
/// backup is written once per older version and never overwritten, so
/// rolling a migration back is a file copy, not a re-initialization.
pub(crate) fn keep_settings_migration_backup(root: &Path) -> Result<(), ConfigError> {
    let path = root.join(SETTINGS_PATH);
    if !path.is_file() {
        return Ok(());
    }
    let text = fs::read_to_string(&path).map_err(ConfigError::io(&path, "read"))?;
    let file_version = settings_schema_version(&text);
    if file_version >= SETTINGS_SCHEMA_VERSION {
        return Ok(());
    }
    let backup = format!("Config/Settings.v{file_version}.bak.json");
    if root.join(&backup).exists() {
        return Ok(());
    }
    persistence::write_atomic(root, &backup, &text).map(|_| ())
}

/// Persist the latest single-use ScreenCast restore token after a
/// successful Wayland capture session.
///
/// Loads the current settings, replaces only the
/// [`AppSettings::wayland_restore_token`] field, and saves the file so
/// the next Wayland runtime start can attempt a dialog-free restore.
/// Passing `None` clears a stored token (for example after the portal
/// stopped issuing tokens for the session). The write goes through
/// [`save_app_settings`], which refuses to overwrite settings files
/// written by a newer schema version.
///
/// # Errors
///
/// Returns the load or save error string; the caller surfaces it
/// instead of silently dropping the rotated token.
pub fn save_wayland_restore_token(root: &Path, token: Option<&str>) -> Result<(), String> {
    let mut settings = load_app_settings(root)?;
    settings.wayland_restore_token = token.map(str::to_string);
    save_app_settings(root, &settings)
}

/// Load the settings for a write: an unreadable file is an error, and a
/// file from a newer schema version is refused instead of overwritten
/// with the fields this build knows.
pub(crate) fn load_settings_for_write(root: &Path) -> Result<AppSettings, ConfigError> {
    let (settings, warnings) = load_app_settings_with_warnings(root).map_err(|detail| {
        ConfigError::malformed(
            root.join(SETTINGS_PATH),
            format!("settings could not be read: {detail}"),
        )
    })?;
    match newer_schema_warning(&warnings) {
        Some((version, supported_version)) => Err(ConfigError::NewerSchema {
            path: SETTINGS_PATH.to_string(),
            version,
            supported_version,
        }),
        None => Ok(settings),
    }
}

/// The file/supported version pair of a [`AppSettingsWarning::NewerSchemaVersion`],
/// when the load reported one.
fn newer_schema_warning(warnings: &[AppSettingsWarning]) -> Option<(u32, u32)> {
    warnings.first().map(|warning| {
        let AppSettingsWarning::NewerSchemaVersion {
            file_version,
            supported_version,
        } = warning;
        (*file_version, *supported_version)
    })
}

/// Detect the `schema_version` integer of a settings JSON document.
/// Files written before versioning existed (and documents without a
/// parseable integer) report version 0.
fn settings_schema_version(text: &str) -> u32 {
    json::parse_object(text, SETTINGS_PATH)
        .ok()
        .and_then(|document| json::Fields::new(&document, SETTINGS_PATH).int("schema_version"))
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

fn parse_app_settings_with_warnings(
    text: &str,
) -> Result<(AppSettings, Vec<AppSettingsWarning>), String> {
    let file_version = settings_schema_version(text);
    let settings = parse_app_settings(text)?;
    if file_version > SETTINGS_SCHEMA_VERSION {
        // Future schema: load read-only instead of guessing a
        // migration path; the warning tells callers why saving is
        // refused.
        return Ok((
            settings,
            vec![AppSettingsWarning::NewerSchemaVersion {
                file_version,
                supported_version: SETTINGS_SCHEMA_VERSION,
            }],
        ));
    }
    let settings = migrate_app_settings(file_version, settings)?;
    Ok((settings, Vec::new()))
}

/// Apply every schema migration between the file's `schema_version`
/// and [`SETTINGS_SCHEMA_VERSION`], oldest step first.
///
/// Each version step owns one `migrate_vN_to_vN+1` function. Bump
/// [`SETTINGS_SCHEMA_VERSION`] and append the new step (with a fixture
/// test) whenever a settings change needs a data transformation. Files
/// with a version newer than supported never reach this function —
/// they are reported read-only with a warning instead.
fn migrate_app_settings(file_version: u32, settings: AppSettings) -> Result<AppSettings, String> {
    let mut settings = settings;
    let mut version = file_version;
    while version < SETTINGS_SCHEMA_VERSION {
        settings = match version {
            0 => migrate_v0_to_v1(settings),
            1 => migrate_v1_to_v2(settings),
            other => {
                return Err(format!(
                    "Config/Settings.json schema_version {other} has no migration step to {}",
                    other + 1
                ))
            }
        };
        version += 1;
    }
    Ok(settings)
}

/// Version 0 is every settings file written before `schema_version`
/// existed. The versioned v1 shape is field-identical to v0 — the
/// lenient parser already maps missing fields to their upstream
/// defaults — so this step only establishes the version baseline and
/// transforms no data. The next data-bearing settings change bumps the
/// schema version and adds a real transformation as its own step.
fn migrate_v0_to_v1(settings: AppSettings) -> AppSettings {
    settings
}

/// Version 2 writes the hotkeys, colors and multi-build colors in the shapes
/// upstream's `System.Text.Json` serializes (`KeyBindingConfig*` objects,
/// `Media.Color` objects). The parser reads both shapes into the same
/// in-memory settings, so this step transforms no data; it exists so a v1
/// file gets its `Settings.v1.bak.json` before the next write replaces it,
/// and so builds that only know v1 refuse the file instead of dropping the
/// new shapes.
fn migrate_v1_to_v2(settings: AppSettings) -> AppSettings {
    settings
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::{create_file, create_temp_root, remove_dir};
    use crate::*;
    use std::fs;

    #[test]
    fn v0_settings_fixture_migrates_to_current_and_stamps_version_on_save() {
        let root =
            create_temp_root("v0_settings_fixture_migrates_to_current_and_stamps_version_on_save");
        let path = root.join(SETTINGS_PATH);
        // Fixture: a settings file written before `schema_version`
        // existed (version 0 — no version field, pre-versioning shape).
        create_file(
            &path,
            r##"{
  "SelectedSystemPreset": "1080p_SMF",
  "SelectedAffixLanguage": "deDE",
  "ControllerMode": true,
  "ThresholdMin": 71,
  "IsTuningPrismHintsEnabled": true,
  "MultiBuildList": [
    { "Name": "Speed", "Index": 0, "Color": "Blue" }
  ]
}"##,
        );

        let (settings, warnings) = load_app_settings_with_warnings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert!(warnings.is_empty());
        // Migrated struct: every fixture value survived the v0 -> v1
        // step and missing fields kept their upstream defaults.
        assert_eq!(settings.selected_system_preset, "1080p_SMF");
        assert_eq!(settings.selected_affix_language, "deDE");
        assert!(settings.controller_mode);
        assert_eq!(settings.threshold_min, 71);
        assert!(settings.is_tuning_prism_hints_enabled);
        assert_eq!(settings.rule_settings.multi_builds.len(), 1);
        // A v0 file carries no `DisplayBackend` key: the lenient parser
        // backfills the `auto` default (older builds in turn ignore the
        // unknown key — no schema-version bump either direction).
        assert_eq!(settings.display_backend, DEFAULT_DISPLAY_BACKEND);
        // ...and no `WaylandRestoreToken` key either: no stored portal
        // session, so a Wayland start shows the consent dialog.
        assert_eq!(settings.wayland_restore_token, None);

        // Write-back on the next save stamps the current version.
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let written = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read back settings: {error}"));
        assert_eq!(settings_schema_version(&written), SETTINGS_SCHEMA_VERSION);

        // Reloading the saved file needs no further migration and
        // keeps the migrated user values.
        let (reloaded, warnings) = load_app_settings_with_warnings(&root)
            .unwrap_or_else(|error| panic!("failed to reload settings: {error}"));
        assert!(warnings.is_empty());
        assert_eq!(reloaded, settings);

        remove_dir(&root);
    }

    #[test]
    fn v1_settings_keep_a_backup_and_are_rewritten_with_the_upstream_shapes() {
        let root = create_temp_root("v1_settings_keep_a_backup_and_are_rewritten");
        let path = root.join(SETTINGS_PATH);
        // Fixture: a v1 file in the shapes d4lc wrote before the upstream
        // ones: a HotkeyBindings array and string colors.
        let v1 = r##"{
  "schema_version": 1,
  "SelectedSystemPreset": "1080p_SMF",
  "DefaultColorRunes": "#FFAA00",
  "MultiBuildList": [
    { "Name": "Speed", "Index": 0, "Color": "Blue" }
  ],
  "HotkeyBindings": [
    { "Name": "SwitchPreset", "Label": "Switch preset", "Binding": "Ctrl+Shift+F5", "Enabled": true }
  ]
}"##;
        create_file(&path, v1);

        let (settings, save_error) = load_app_settings_and_save(&root)
            .unwrap_or_else(|error| panic!("startup load failed: {error}"));

        assert_eq!(save_error, None);
        assert_eq!(
            fs::read_to_string(root.join("Config/Settings.v1.bak.json")).unwrap_or_default(),
            v1,
            "the v1 bytes are kept before the write-back"
        );
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap_or_default())
                .expect("rewritten settings are JSON");
        assert_eq!(written["schema_version"], 2);
        assert!(written.get("HotkeyBindings").is_none());
        assert_eq!(
            written["KeyBindingConfigSwitchPreset"],
            serde_json::json!({
                "IsEnabled": true,
                "Name": "Switch Preset",
                "KeyGestureKey": 94,
                "KeyGestureModifier": 6
            })
        );
        assert_eq!(written["DefaultColorRunes"]["G"], 170);
        assert_eq!(written["MultiBuildList"][0]["Color"]["B"], 255);
        // The migrated values survive.
        assert_eq!(settings.default_colors.runes, "#FFAA00");
        assert_eq!(
            settings
                .hotkey_config
                .find_by_name("SwitchPreset")
                .map(|b| b.binding.as_str()),
            Some("Ctrl+Shift+F5")
        );

        // A build that only knows v1 refuses the v2 file.
        assert_eq!(
            settings_schema_version(&fs::read_to_string(&path).unwrap_or_default()),
            2
        );

        remove_dir(&root);
    }

    #[test]
    fn wayland_restore_token_round_trips_and_rotates_on_disk() {
        let root = create_temp_root("wayland_restore_token_round_trips_and_rotates_on_disk");

        // Fresh settings have no stored session.
        assert_eq!(AppSettings::default().wayland_restore_token, None);

        // A stored token round-trips; an empty stored token is treated
        // as "no stored session".
        create_file(
            &root.join(SETTINGS_PATH),
            concat!(
                "{\n",
                "  \"DisplayBackend\": \"wayland\",\n",
                "  \"WaylandRestoreToken\": \"token-first-session\"\n",
                "}\n"
            ),
        );
        let loaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(
            loaded.wayland_restore_token.as_deref(),
            Some("token-first-session")
        );

        create_file(
            &root.join(SETTINGS_PATH),
            concat!(
                "{\n",
                "  \"DisplayBackend\": \"wayland\",\n",
                "  \"WaylandRestoreToken\": \"\"\n",
                "}\n"
            ),
        );
        let loaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(
            loaded.wayland_restore_token, None,
            "an empty stored token must not persist as a session"
        );

        // Rotation: the first successful session stores its fresh
        // token, the next successful session replaces it (tokens are
        // single-use, so the old value must never survive).
        save_wayland_restore_token(&root, Some("token-second-session"))
            .unwrap_or_else(|error| panic!("token store should save: {error}"));
        let rotated = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to reload settings: {error}"));
        assert_eq!(
            rotated.wayland_restore_token.as_deref(),
            Some("token-second-session")
        );

        // Clearing drops the stored value and writes an explicit null,
        // so the file keeps a single stable shape.
        save_wayland_restore_token(&root, None)
            .unwrap_or_else(|error| panic!("token clear should save: {error}"));
        let written = fs::read_to_string(root.join(SETTINGS_PATH))
            .unwrap_or_else(|error| panic!("failed to read back settings: {error}"));
        assert!(
            written.contains("\"WaylandRestoreToken\": null"),
            "cleared token must serialize as null: {written}"
        );
        let cleared = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to reload settings: {error}"));
        assert_eq!(cleared.wayland_restore_token, None);

        remove_dir(&root);
    }

    #[test]
    fn current_version_settings_load_without_warnings() {
        let root = create_temp_root("current_version_settings_load_without_warnings");
        let settings = AppSettings::default();

        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("settings should save: {error}"));
        let (reloaded, warnings) = load_app_settings_with_warnings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));

        assert!(warnings.is_empty());
        assert_eq!(reloaded, settings);

        remove_dir(&root);
    }

    #[test]
    fn future_schema_version_loads_read_only_with_warning_and_save_is_refused() {
        let root = create_temp_root(
            "future_schema_version_loads_read_only_with_warning_and_save_is_refused",
        );
        let path = root.join(SETTINGS_PATH);
        // Fixture: a file from a future build with a schema version
        // newer than this build supports plus a field this build does
        // not know.
        create_file(
            &path,
            r##"{
  "schema_version": 3,
  "SelectedSystemPreset": "1080p_SMF",
  "IsSomeFutureFieldEnabled": true
}"##,
        );

        let (settings, warnings) = load_app_settings_with_warnings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(
            warnings,
            vec![AppSettingsWarning::NewerSchemaVersion {
                file_version: 3,
                supported_version: SETTINGS_SCHEMA_VERSION,
            }]
        );
        // Known fields still load; nothing panicked, nothing migrated.
        assert_eq!(settings.selected_system_preset, "1080p_SMF");
        // The plain entry point keeps loading the same read-only view.
        let plain = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(plain.selected_system_preset, "1080p_SMF");

        // Saving over the newer file is refused and leaves the file
        // byte-identical, so the unknown fields survive.
        let before = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read settings: {error}"));
        match save_app_settings(&root, &settings) {
            Ok(()) => panic!("save must refuse a file with a newer schema_version"),
            Err(error) => assert!(error.contains("schema_version 3")),
        }
        let after = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to re-read settings: {error}"));
        assert_eq!(before, after);

        remove_dir(&root);
    }

    #[test]
    fn startup_load_creates_the_default_settings_file_on_a_fresh_install() {
        // SettingsManager.cs:46-57: LoadSettings ends with SaveSettings.
        let root = create_temp_root("startup_load_creates_the_default_settings_file");

        let (settings, save_error) = load_app_settings_and_save(&root)
            .unwrap_or_else(|error| panic!("startup load failed: {error}"));

        assert_eq!(settings, AppSettings::default());
        assert_eq!(save_error, None);
        let reloaded = load_app_settings(&root)
            .unwrap_or_else(|error| panic!("failed to load settings: {error}"));
        assert_eq!(reloaded, AppSettings::default());
        assert!(root.join(SETTINGS_PATH).is_file());

        remove_dir(&root);
    }

    #[test]
    fn startup_load_rewrites_an_existing_file_in_the_current_shape() {
        let root = create_temp_root("startup_load_rewrites_an_existing_file");
        let path = root.join(SETTINGS_PATH);
        create_file(&path, r##"{ "SelectedSystemPreset": "1080p_SMF" }"##);

        let (settings, save_error) = load_app_settings_and_save(&root)
            .unwrap_or_else(|error| panic!("startup load failed: {error}"));

        assert_eq!(save_error, None);
        assert_eq!(settings.selected_system_preset, "1080p_SMF");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read settings: {error}"));
        assert!(text.contains("\"SelectedSystemPreset\": \"1080p_SMF\""));
        assert!(text.contains("\"KeyBindingConfigSwitchPreset\""));

        remove_dir(&root);
    }

    #[test]
    fn startup_load_never_replaces_an_unreadable_or_newer_file() {
        let root = create_temp_root("startup_load_never_replaces_an_unreadable_or_newer_file");
        let path = root.join(SETTINGS_PATH);

        create_file(&path, "{ broken");
        assert!(load_app_settings_and_save(&root).is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap_or_default(),
            "{ broken",
            "an unreadable file is left as it is"
        );

        let newer = r##"{ "schema_version": 3, "IsSomeFutureFieldEnabled": true }"##;
        create_file(&path, newer);
        let (_, save_error) = load_app_settings_and_save(&root)
            .unwrap_or_else(|error| panic!("startup load failed: {error}"));
        assert!(save_error.is_some_and(|error| error.contains("schema_version 3")));
        assert_eq!(fs::read_to_string(&path).unwrap_or_default(), newer);

        remove_dir(&root);
    }

    // -------------------------------------------------------------------
    // Affix-preset persistence (Config/AffixPresets-v2.json)
    // -------------------------------------------------------------------
}
