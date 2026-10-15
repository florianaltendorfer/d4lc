//! Settings defaults and parsing helpers for d4lc.
//!
//! Upstream reference:
//! - `D4Companion.Services/SettingsManager.cs` loads `Config/Settings.json`.
//! - `D4Companion.Services/SystemPresetManager.cs` loads `Config/Controllers.json`.
//!
//! This crate intentionally keeps the parser small and file based so the native
//! Rust port can consume the same user-facing configuration shape without a CLR
//! or WPF dependency.
//!
//! Every config file is read and written with `serde_json` (see the `json`
//! module): readers keep the lenient upstream field rules, writers keep the
//! upstream key order and indentation.
//!
//! Writes go through [`persistence`]: staged next to the target, synced and
//! atomically renamed, with a transaction record for changes that span more
//! than one file. A caller may only report a new applied revision after the
//! write really reached the disk.

use std::path::{Path, PathBuf};

mod data_files;
mod json;
pub mod persistence;
mod presets;
mod settings;
#[cfg(test)]
mod test_support;
mod wpf_color;

pub use data_files::*;
pub use persistence::{
    interrupted_transactions, CommittedRevision, ConfigError, InterruptedTransaction, Recovery,
    RecoveryOutcome,
};
pub use presets::*;
pub use settings::*;

pub const SETTINGS_PATH: &str = "Config/Settings.json";
pub const CONTROLLERS_PATH: &str = "Config/Controllers.json";
// Upstream: D4Companion.Services/AffixManager.cs:913,1049 — both the
// reader and the writer of the affix-preset file use this exact name.
pub const AFFIX_PRESETS_PATH: &str = "Config/AffixPresets-v2.json";
/// Upstream `AffixManager.InitSigilDungeonTierData`
/// (AffixManager.cs:405-424) reads `.\Config\DungeonTiers.json`.
pub const DUNGEON_TIERS_PATH: &str = "Config/DungeonTiers.json";
/// Upstream `AffixManager.InitAffixMinimalValueData`
/// (AffixManager.cs:329-351) reads `.\Config\MinimalAffixValues.json`.
pub const MINIMAL_AFFIX_VALUES_PATH: &str = "Config/MinimalAffixValues.json";
// Upstream: D4Companion.Services/TradeItemManager.cs:81,93 — both
// LoadTradeItems and SaveTradeItems use this exact name.
pub const TRADE_ITEMS_PATH: &str = "Config/TradeItems.json";

/// Environment variable that overrides the user configuration root for the
/// desktop app and the diagnostics CLI alike (tests, isolated acceptance runs).
pub const CONFIG_ROOT_ENV: &str = "D4LC_CONFIG_ROOT";

/// The configuration root named by [`CONFIG_ROOT_ENV`], when it is set and
/// not empty.
pub fn config_root_from_env() -> Option<PathBuf> {
    std::env::var_os(CONFIG_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The XDG location of the user's configuration, `$XDG_CONFIG_HOME/d4lc`
/// (`~/.config/d4lc`); `None` when the environment names no home at all.
pub fn xdg_config_root() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("d4lc"))
}

pub fn config_path(root: &Path, relative: &str) -> PathBuf {
    root.join(relative)
}
