//! The three roots of a CLI run.
//!
//! - **Assets** (read-only app and game data: `assets/`, the vendored
//!   presets, the upstream metadata of a developer checkout): `--root`, else
//!   `D4LC_ASSET_ROOT`, else the current directory. The packaged launchers
//!   export `D4LC_ASSET_ROOT` and leave the working directory alone.
//! - **Config** (the user configuration the desktop app reads and writes:
//!   `Config/Settings.json`, `Config/Controllers.json`, the extracted presets
//!   under `Images/`): `--config-root`, else `D4LC_CONFIG_ROOT`, else
//!   `$XDG_CONFIG_HOME/d4lc` (`~/.config/d4lc`), the desktop app's order.
//! - **Work** (the current directory): the base of every relative path the
//!   user passes and of the default `debug-output/` directories. Nothing is
//!   written below the asset root unless the user names it.

use d4lc_assets::{AssetRepository, EXTRACTED_PRESETS_DIR, SYSTEM_PRESETS_DIR};
use d4lc_config::{load_app_settings, load_controller_config, AppSettings};
use d4lc_config::{CONTROLLERS_PATH, SETTINGS_PATH};
use serde_json::{json, Map, Value};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Environment variable the packaged launchers set to the bundle root.
pub(crate) const ASSET_ROOT_ENV: &str = "D4LC_ASSET_ROOT";

/// The process state the roots resolve from. Tests build it directly, so
/// they never read or change the real environment or `~/.config`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Invocation {
    pub(crate) cwd: PathBuf,
    /// `D4LC_ASSET_ROOT`, when set and not empty.
    pub(crate) asset_root: Option<PathBuf>,
    /// `D4LC_CONFIG_ROOT`, when set and not empty.
    pub(crate) config_root: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME/d4lc`; `None` when no home directory is known.
    pub(crate) xdg_config_root: Option<PathBuf>,
}

impl Invocation {
    pub(crate) fn from_process() -> Result<Self, String> {
        Ok(Self {
            cwd: std::env::current_dir()
                .map_err(|error| format!("cannot read the current directory: {error}"))?,
            asset_root: std::env::var_os(ASSET_ROOT_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            config_root: d4lc_config::config_root_from_env(),
            xdg_config_root: d4lc_config::xdg_config_root(),
        })
    }

    /// The asset root when `--root` is not given.
    pub(crate) fn default_asset_root(&self) -> PathBuf {
        self.asset_root.clone().unwrap_or_else(|| self.cwd.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigRootSource {
    Flag,
    Env,
    Xdg,
}

impl ConfigRootSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "--config-root",
            Self::Env => d4lc_config::CONFIG_ROOT_ENV,
            Self::Xdg => "xdg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigRoot {
    /// Absolute path.
    pub(crate) path: PathBuf,
    pub(crate) source: ConfigRootSource,
}

#[derive(Debug, Clone)]
pub(crate) struct Roots {
    pub(crate) assets: PathBuf,
    /// Absolute current directory.
    pub(crate) work: PathBuf,
    config: Result<ConfigRoot, String>,
}

impl Roots {
    pub(crate) fn resolve(
        asset_root: &Path,
        config_root_flag: Option<&Path>,
        invocation: &Invocation,
    ) -> Self {
        let config = if let Some(path) = config_root_flag {
            Ok((path.to_path_buf(), ConfigRootSource::Flag))
        } else if let Some(path) = &invocation.config_root {
            Ok((path.clone(), ConfigRootSource::Env))
        } else if let Some(path) = &invocation.xdg_config_root {
            Ok((path.clone(), ConfigRootSource::Xdg))
        } else {
            Err(format!(
                "no configuration root: pass --config-root <dir> or set {} \
                 (neither XDG_CONFIG_HOME nor HOME names a config directory)",
                d4lc_config::CONFIG_ROOT_ENV
            ))
        };
        Self {
            assets: asset_root.to_path_buf(),
            work: invocation.cwd.clone(),
            config: config.map(|(path, source)| ConfigRoot {
                path: invocation.cwd.join(path),
                source,
            }),
        }
    }

    /// Assets and work at `root`, and a config root that does not exist:
    /// the defaults of a fresh user.
    #[cfg(test)]
    pub(crate) fn for_tests(root: &Path) -> Self {
        Self {
            assets: root.to_path_buf(),
            work: root.to_path_buf(),
            config: Ok(ConfigRoot {
                path: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/no-config-root")),
                source: ConfigRootSource::Flag,
            }),
        }
    }

    pub(crate) fn config(&self) -> Result<&ConfigRoot, String> {
        self.config.as_ref().map_err(Clone::clone)
    }

    pub(crate) fn config_path(&self) -> Result<&Path, String> {
        self.config().map(|config| config.path.as_path())
    }

    /// A user-given path (output or input): relative paths resolve against
    /// the current directory, absolute paths stay as given.
    pub(crate) fn work_path(&self, path: &Path) -> PathBuf {
        self.work.join(path)
    }

    /// The repository serving `preset`: the config root when it carries the
    /// preset (`Images/{preset}` from a user download, or a vendored tree),
    /// else the asset root. The same precedence as the app's OCR preparation
    /// (`PreparedTooltipOcr::prepare_with_game_data_root`).
    pub(crate) fn preset_repo(&self, preset: &str) -> Result<AssetRepository, String> {
        let config = self.config_path()?;
        let in_config_root = [EXTRACTED_PRESETS_DIR, SYSTEM_PRESETS_DIR]
            .iter()
            .any(|dir| config.join(dir).join(preset).is_dir());
        Ok(AssetRepository::new(if in_config_root {
            config
        } else {
            &self.assets
        }))
    }

    /// The settings of the config root; an unreadable or invalid file is an
    /// error, a missing one yields the defaults (as in the desktop app).
    pub(crate) fn settings(&self) -> Result<AppSettings, String> {
        self.config_files()?.into_settings()
    }

    pub(crate) fn config_files(&self) -> Result<ConfigFiles, String> {
        let config = self.config()?.clone();
        let settings = load_config_file(&config.path, SETTINGS_PATH, load_app_settings);
        let controllers = load_config_file(&config.path, CONTROLLERS_PATH, load_controller_config);
        Ok(ConfigFiles {
            root: config,
            settings,
            controllers,
        })
    }
}

/// How a config file was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileStatus {
    /// The file exists and parsed.
    Loaded,
    /// No file: the defaults apply.
    Missing,
    /// The file exists but does not parse.
    Invalid(String),
    /// The file cannot be read (permissions, not a regular file, ...).
    Unreadable(String),
}

impl FileStatus {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Missing => "missing",
            Self::Invalid(_) => "invalid",
            Self::Unreadable(_) => "unreadable",
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match self {
            Self::Invalid(error) | Self::Unreadable(error) => Some(error),
            Self::Loaded | Self::Missing => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigFile<T> {
    /// Absolute path.
    pub(crate) path: PathBuf,
    pub(crate) status: FileStatus,
    /// The file's values, or the defaults when it is missing; `None` when it
    /// is invalid or unreadable.
    pub(crate) value: Option<T>,
}

impl<T> ConfigFile<T> {
    fn into_value(self) -> Result<T, String> {
        let status = self.status.as_str();
        match (self.value, self.status.error()) {
            (Some(value), _) => Ok(value),
            (None, error) => Err(format!(
                "{} is {status}: {}",
                self.path.display(),
                error.unwrap_or_default()
            )),
        }
    }

    fn identity_json(&self) -> Value {
        json!({
            "path": self.path.display().to_string(),
            "status": self.status.as_str(),
            "error": self.status.error(),
            // A missing file runs on the defaults; a broken one stops the
            // command, it is never replaced by the defaults.
            "effect": match self.status {
                FileStatus::Loaded => "file_values",
                FileStatus::Missing => "defaults",
                FileStatus::Invalid(_) | FileStatus::Unreadable(_) => "aborted",
            },
        })
    }
}

/// `missing` is decided here, not by the loaders: they return the defaults
/// for an absent file and would let a report claim `loaded`.
fn load_config_file<T: Default>(
    root: &Path,
    relative: &str,
    load: impl FnOnce(&Path) -> Result<T, String>,
) -> ConfigFile<T> {
    let path = root.join(relative);
    let (status, value) = match fs::metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            (FileStatus::Missing, Some(T::default()))
        }
        Err(error) => (FileStatus::Unreadable(error.to_string()), None),
        Ok(metadata) if !metadata.is_file() => (
            FileStatus::Unreadable("not a regular file".to_string()),
            None,
        ),
        Ok(_) => match load(root) {
            Ok(value) => (FileStatus::Loaded, Some(value)),
            Err(error) if fs::File::open(&path).is_err() => (FileStatus::Unreadable(error), None),
            Err(error) => (FileStatus::Invalid(error), None),
        },
    };
    ConfigFile {
        path,
        status,
        value,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigFiles {
    pub(crate) root: ConfigRoot,
    pub(crate) settings: ConfigFile<AppSettings>,
    pub(crate) controllers: ConfigFile<Vec<String>>,
}

impl ConfigFiles {
    pub(crate) fn into_settings(self) -> Result<AppSettings, String> {
        self.settings.into_value()
    }

    pub(crate) fn into_controllers(self) -> Result<Vec<String>, String> {
        self.controllers.into_value()
    }

    /// Where the effective configuration came from; `overrides` are the
    /// explicit CLI flags that replace file values.
    pub(crate) fn identity_json(&self, overrides: Value) -> Value {
        json!({
            "config_root": self.root.path.display().to_string(),
            "config_root_source": self.root.source.as_str(),
            "settings": self.settings.identity_json(),
            "controllers": self.controllers.identity_json(),
            "cli_overrides": overrides,
        })
    }
}

/// The given flags only, by flag name.
pub(crate) fn cli_overrides_json(flags: &[(&str, Value)]) -> Value {
    Value::Object(
        flags
            .iter()
            .filter(|(_, value)| !value.is_null() && value != &json!([]))
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect::<Map<String, Value>>(),
    )
}
