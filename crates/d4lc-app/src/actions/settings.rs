//! Settings action handlers for the Slint desktop app.
//!
//! ## Upstream evidence
//!
//! - `SettingsD4.cs:14-62` — setting field definitions
//! - `SettingsManager.cs` — JSON load/save
//! - `SettingsViewModel.cs:94-99` — command definitions (deferred for dialog commands)
//!
//! ## Linux adaptation
//!
//! Config root is XDG config home (`~/.config/d4lc/`)
//! with `D4LC_CONFIG_ROOT` env var override for tests.
//! Upstream uses `Config/Settings.json` relative to an application root.
//! The upstream-shaped relative path `Config/Settings.json` is preserved
//! under the chosen XDG root.
//!
//! ## Settings-family config dialogs
//!
//! The five settings-family config dialogs
//! (`ColorsConfig`, `ControllerConfig`, `HotkeysConfig`,
//! `OverlayConfig`, `ParagonConfig`) each have a confirm body.
//! Per-kind confirm dispatch lives in
//! [`handle_dialog_confirm_with_router`]:
//!
//! - `ControllerConfig` — applies `SettingsPanelState.controller_mode`
//!   plus the selected controller image list (written to
//!   `Config/Controllers.json`) and persists.
//! - `OverlayConfig` — applies the overlay rows
//!   (`overlay_enabled`, `overlay_top_most`, `overlay_font_size`
//!   clamped 10..=50, `overlay_menu_locked`, `type_area_offset_left`
//!   clamped 0..=25, overlay icon visibility + placement clamped
//!   0..=1000, the item-power rule clamped 100..=800, the overlay marker
//!   mode, the trade-overlay cross-entry, the socket / tempered
//!   detection gates, and the capture delay, update delay (5..=500) and
//!   scan height (10..=100)) and persists.
//! - `ParagonConfig` — applies `paragon_mode_active`,
//!   `mobalytics_show_browser`, the collapsed-board switch and the five
//!   paragon layout numerics (ranges mirroring the upstream dialog) and
//!   persists.
//! - `ColorsConfig` — validates the seven `DefaultColor*` values as
//!   colors, applies them and persists.
//! - `HotkeysConfig` — persists the binding drafts and restarts the
//!   hotkey backend so the registrations are live without an app restart.
//!
//! Form values are projected on the root `AppWindow` as the
//! `settings-controller-*`, `settings-hotkey-*`, `settings-overlay-*`,
//! `settings-paragon-*` and `settings-colors-*` in-out properties (see
//! `main_window.slint`). The matching `handle_open_*_config`
//! functions seed those props from `state.shell.settings` on
//! open, and `handle_dialog_confirm_with_router` reads them
//! back on confirm.

mod app_language;
mod confirm;
mod dialogs;
mod hotkey_config;
mod http_fetch;
mod panel;
mod preset_catalog;
mod preset_download;
mod preset_download_job;
mod preset_extract;
mod preset_install;
mod presets;
mod root_props;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod test_support;

pub(crate) use app_language::project_tab_labels;
pub use app_language::{apply_app_language_projection, handle_app_language_selected};
pub use confirm::{
    confirm_controller_config, confirm_tuning_prism_hints_config, set_overlay_enabled,
    set_paragon_mode_active,
};
#[cfg(test)]
pub use confirm::{confirm_paragon_config, ParagonConfigDraft};
pub use dialogs::{
    handle_controller_image_selected, handle_dialog_cancel_with_router,
    handle_dialog_confirm_with_router, handle_open_colors_config, handle_open_controller_config,
    handle_open_overlay_config, handle_open_paragon_config,
};
pub use hotkey_config::{
    current_hotkey_receiver, handle_hotkey_row_edit, handle_hotkey_row_toggled,
    handle_open_hotkeys_config, hotkey_status_label, install_hotkey_runtime,
    restart_hotkey_backend,
};
#[cfg(any(feature = "update-check", test))]
pub(crate) use http_fetch::effective_authority;
#[cfg(feature = "preset-download")]
pub(crate) use http_fetch::fetch_bounded;
#[cfg(feature = "update-check")]
pub(crate) use http_fetch::fetch_bounded_with_timeout;
#[cfg(all(test, feature = "update-check"))]
pub(crate) use http_fetch::tests::server as http_stub;
pub(crate) use http_fetch::UrlPolicy;
pub(crate) use panel::persist_settings_with_warnings;
pub use panel::{
    apply_settings_display_backend_projection, apply_settings_language_projection,
    handle_display_backend_changed, handle_language_selected, handle_settings_changed,
    resolve_config_root,
};
pub use preset_catalog::{start_system_preset_catalog_fetch, SystemPresetCatalog};
#[cfg(test)]
pub(crate) use preset_download::ENV_LOCK;
pub use preset_download::{DownloadError, Downloader, UreqDownloader};
#[cfg(test)]
pub use preset_download::{ENV_OVERRIDE_BASE_URL, PRESETS_IMAGES_SUBDIR};
pub use preset_download_job::start_preset_download;
pub use preset_install::recover_interrupted_preset_installs;
pub use presets::{apply_settings_preset_projection, handle_preset_selected};
pub use root_props::project_overlay_gate;
