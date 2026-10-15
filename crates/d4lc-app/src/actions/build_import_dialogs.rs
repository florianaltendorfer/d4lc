//! Tabbed build-import dialog backend.
//!
//! The upstream `ImportAffixPresetView` is one MahApps dialog with seven
//! tabs: `Local`, `Merge`, and one provider tab each for D2Core,
//! D4Builds, InfinityBuilds, Maxroll, and Mobalytics
//! (`ImportAffixPresetView.xaml:18-1057`). This module is the Rust side
//! of its Slint port (`ui/dialogs/import_preset_dialog.slint`):
//!
//! - **Entry**: [`handle_open_import_preset`] opens the dialog on the tab
//!   mapped from the persisted website selection (upstream
//!   `ImportAffixPresetViewModel.cs:173-179`); `open_import_dialog` opens
//!   it on an explicit tab (the AffixScreen chips).
//! - **Gates**: the exact upstream `CanAdd{Provider}BuildExecute`
//!   predicates ([`can_add_d2core_build`](gates::can_add_d2core_build),
//!   [`can_add_d4builds_build`](gates::can_add_d4builds_build),
//!   [`can_add_infinity_builds_build`](gates::can_add_infinity_builds_build),
//!   [`can_add_maxroll_build`](gates::can_add_maxroll_build),
//!   [`can_add_mobalytics_build`](gates::can_add_mobalytics_build)). Invalid
//!   input never errors — the Add button stays disabled — except
//!   Mobalytics' persistent status line
//!   ([`mobalytics_url_status`](gates::mobalytics_url_status)).
//! - **Builds / variants**: the left/right lists read
//!   `d4lc_rules::load_available_{provider}_builds` over the resolved
//!   builds directory (downloaded-builds home first, bundled fixtures as
//!   the offline dev fallback).
//! - **Downloads**: every Add / Update runs on a `std::thread` worker
//!   ([`spawn_download`](downloads::spawn_download)) that appends
//!   [`crate::app_state::BuildImportEvent`] values into the shared
//!   buffer and never touches Slint or the `AppState` lock. The
//!   main-thread 50 ms drain timer ([`drain_build_import_events`],
//!   wired in `wiring.rs`) moves the events into the window properties:
//!   the progress overlay mirrors the upstream status flow
//!   ("Status: {status}") and auto-closes on success while Done stays
//!   disabled until completion. A failed download keeps the overlay open
//!   with its failed status (upstream flashes the same status before its
//!   dialog auto-closes) until Done dismisses it. Maxroll keeps
//!   upstream's no-overlay behavior. A single-active-download guard
//!   refuses a second job while one runs; a panicked worker reports the
//!   same failure completion so the overlay and the guard cannot stick.
//!   D4Builds runs the full upstream scrape + conversion flow (status
//!   sequence and stored entity match `BuildsManagerD4Builds`); Mobalytics
//!   keeps the raw-capture download with the "conversion pending"
//!   status — raw captures are not importable builds and stay out of the
//!   lists.
//! - **Add-as-preset**: the Confirm-name overlay (suggestions, red
//!   overwrite warning, silent overwrite through the editor's
//!   `import_preset`) plus the paragon import toggles, mirroring
//!   `Add{Provider}BuildAsPresetExecute` and `CreatePresetFrom{Provider}Build`.
//!   The imported source binds strictly to the selected provider build: a
//!   vanished or renamed file fails the import visibly instead of
//!   substituting another build's JSON, and the outcome reaches the Logging
//!   screen either as a usable import or as an import naming its skipped
//!   entries with counts and reasons.
//! - **Update / Web / Remove** on stored builds with the upstream URLs
//!   and the unconfirmed `File.Delete` parity.
//! - **Local tab**: file import through the injectable
//!   [`PresetFileDialog`](local_tab::PresetFileDialog) seam (the production `rfd` portal picker),
//!   export to `Exports/{name}.json`, and the remove flow shared with
//!   the provider tabs — upstream tab 0.
//! - **Merge tab**: the Build 1 / Build 2 / Build 1+2 rows with the
//!   embedded color-picker overlay and the pure
//!   `d4lc_gui::merge_affix_presets` merge through the shared
//!   Confirm-name overlay — upstream tab 1.
//! - Errors and failures surface on the Logging screen message list
//!   (upstream `ErrorOccurredMessage` parity), never as invented
//!   fallbacks.
//!
//! ## What this module does NOT do
//!
//! - Touch `d4lc-config` schemas (persistence goes through the existing
//!   `AppSettings` load/save seam).
//! - Run downloads on the UI thread or let worker threads touch
//!   `AppWindow`.

mod add_preset;
mod dialog;
mod downloads;
mod gates;
mod import_diagnostics;
mod local_tab;
mod merge_tab;
mod multi_build;
mod paths;
mod provider_downloads;
mod row_actions;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod test_support;

pub use add_preset::{
    handle_delete_cancel, handle_delete_confirm, handle_name_cancel, handle_name_done,
    handle_name_suggestion_selected, handle_name_text_changed, handle_paragon_toggled,
    handle_preset_remove, handle_show_browser_toggled, handle_variant_add,
};
pub use dialog::{
    handle_cancel, handle_confirm, handle_open_import_preset, handle_url_changed,
    open_import_dialog,
};
pub use downloads::{
    drain_build_import_events, handle_add_clicked, handle_build_update, handle_progress_cancel,
    handle_progress_close, handle_progress_escape, shutdown_downloads,
};
#[cfg(test)]
pub use gates::{MOBALYTICS_STATUS_BUILD_URL, MOBALYTICS_STATUS_INVALID};
pub use local_tab::{handle_local_export, handle_local_import};
#[cfg(test)]
pub use local_tab::{
    set_exports_folder_opener, set_import_file_dialog, ExportsFolderOpener, PresetFileDialog,
};
pub use merge_tab::{
    handle_color_picker_cancel, handle_color_picker_done, handle_merge_clicked,
    handle_merge_color_clicked,
};
pub use multi_build::{
    handle_multi_build_add, handle_multi_build_color_clicked,
    handle_multi_build_color_picker_cancel, handle_multi_build_color_picker_done,
    handle_multi_build_preset_selected, handle_multi_build_remove, handle_open_multi_build_config,
};
#[cfg(test)]
pub use paths::{load_builds_for_provider, load_profiles_for_provider};
pub use row_actions::{
    handle_build_remove, handle_build_selected, handle_build_web, handle_profile_remove,
    handle_profile_selected, handle_profile_update, handle_profile_variant_add, handle_profile_web,
    handle_visit_web,
};

// ---------------------------------------------------------------------------
// Provider id (mirrors the upstream WPF `BuildsManager*.Provider` strings
// and matches the IDs in `d4lc_rules::D2CORE_PROVIDER` /
// `D4BUILDS_PROVIDER` / `MOBALYTICS_PROVIDER`).
// ---------------------------------------------------------------------------

pub const PROVIDER_D2CORE: &str = "D2Core";
pub const PROVIDER_D4BUILDS: &str = "D4Builds";
pub const PROVIDER_MAXROLL: &str = "Maxroll";
pub const PROVIDER_MOBALYTICS: &str = "Mobalytics";
pub const PROVIDER_INFINITY_BUILDS: &str = "InfinityBuilds";

/// The providers reachable from the tabbed import dialog.
pub const ALL_BUILD_IMPORT_PROVIDERS: [&str; 5] = [
    PROVIDER_D2CORE,
    PROVIDER_D4BUILDS,
    PROVIDER_INFINITY_BUILDS,
    PROVIDER_MAXROLL,
    PROVIDER_MOBALYTICS,
];

/// Upstream website-entry names (`AffixViewModel.InitBuildImportWebsites`).
/// Selecting a website persists `SelectedBuildImportWebsite`
/// (remember-last-website).
pub const WEBSITE_D2CORE: &str = "D2Core.com";
pub const WEBSITE_D4BUILDS: &str = "D4Builds.gg";
pub const WEBSITE_INFINITY_BUILDS: &str = "InfinityBuilds.gg";
pub const WEBSITE_MAXROLL: &str = "Maxroll.gg";
pub const WEBSITE_MOBALYTICS: &str = "Mobalytics.gg";

/// The five website entries, in upstream combo order.
pub const ALL_BUILD_IMPORT_WEBSITES: [&str; 5] = [
    WEBSITE_D2CORE,
    WEBSITE_D4BUILDS,
    WEBSITE_INFINITY_BUILDS,
    WEBSITE_MAXROLL,
    WEBSITE_MOBALYTICS,
];

/// Map the website selection to the initial tab index
/// (`ImportAffixPresetViewModel.cs:173-179`): D2Core→2, D4Builds→3,
/// InfinityBuilds→4, Maxroll→5, Mobalytics→6, unknown→0 (Local).
pub fn tab_index_for_website(website: &str) -> i32 {
    match website {
        WEBSITE_D2CORE => 2,
        WEBSITE_D4BUILDS => 3,
        WEBSITE_INFINITY_BUILDS => 4,
        WEBSITE_MAXROLL => 5,
        WEBSITE_MOBALYTICS => 6,
        _ => 0,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app_state::{AppState, DialogFamily, DialogKind, ParagonImportToggles};

    // -----------------------------------------------------------------------
    // Family dispatch
    // -----------------------------------------------------------------------

    #[test]
    fn build_import_family_has_two_kinds() {
        // The provider downloads live in the tabbed ImportPreset dialog;
        // MultiBuildConfig is its own dialog.
        let kinds: Vec<DialogKind> = DialogKind::ALL
            .iter()
            .copied()
            .filter(|k| k.family() == DialogFamily::BuildImport)
            .collect();
        assert_eq!(
            kinds,
            vec![DialogKind::MultiBuildConfig, DialogKind::ImportPreset]
        );
    }

    // -----------------------------------------------------------------------
    // Website → tab mapping (upstream initial tab selection)
    // -----------------------------------------------------------------------

    #[test]
    fn website_tabs_map_like_the_upstream_view_model() {
        assert_eq!(tab_index_for_website("D2Core.com"), 2);
        assert_eq!(tab_index_for_website("D4Builds.gg"), 3);
        assert_eq!(tab_index_for_website("InfinityBuilds.gg"), 4);
        assert_eq!(tab_index_for_website("Maxroll.gg"), 5);
        assert_eq!(tab_index_for_website("Mobalytics.gg"), 6);
        assert_eq!(tab_index_for_website("unknown"), 0);
    }

    // -----------------------------------------------------------------------
    // AppState shape
    // -----------------------------------------------------------------------

    #[test]
    fn default_app_state_does_not_carry_builds_dir() {
        let state = AppState::default_startup();
        // We do not fabricate a default builds dir; the dialog
        // resolves it from the workspace fixture when present.
        // This is the offline contract.
        assert!(state.build_import.builds_dir.is_none());
        assert!(state.build_import.multi_builds.is_empty());
        assert!(!state.shell.settings.multi_build_mode_enabled);
        assert!(state.build_import.pending_import.is_none());
        assert!(state.build_import.provider_builds.is_empty());
        // The paragon toggles default to upstream's all-true defaults.
        assert_eq!(
            state.build_import.paragon_toggles,
            ParagonImportToggles::default()
        );
        assert!(state.build_import.paragon_toggles.get(PROVIDER_D2CORE));
        assert!(state.build_import.paragon_toggles.get(PROVIDER_MAXROLL));
        // The event buffer starts empty and drains clean.
        assert!(state.build_import_events.drain().is_empty());
    }

    #[test]
    fn paragon_toggles_read_and_write_by_provider_token() {
        let mut toggles = ParagonImportToggles::default();
        toggles.set(PROVIDER_MOBALYTICS, false);
        assert!(!toggles.get(PROVIDER_MOBALYTICS));
        assert!(toggles.get(PROVIDER_INFINITY_BUILDS));
        toggles.set("Unknown", true);
        assert!(!toggles.get("Unknown"), "unknown providers stay false");
    }

    #[test]
    fn hydrate_build_import_from_settings_populates_rows_and_toggles() {
        use d4lc_config::AppSettings;
        let mut state = AppState::default_startup();
        let mut settings = AppSettings::default();
        settings.rule_settings.multi_builds = vec![
            d4lc_config::MultiBuildRuleConfig {
                name: "Speed".to_string(),
                index: 0,
                color: "Green".to_string(),
            },
            d4lc_config::MultiBuildRuleConfig {
                name: "Boss".to_string(),
                index: 1,
                color: "Orange".to_string(),
            },
        ];
        settings.rule_settings.multi_build_enabled = true;
        settings.is_import_paragon_maxroll_enabled = false;
        state.hydrate_build_import_from_settings(&settings);
        assert_eq!(state.build_import.multi_builds.len(), 2);
        assert_eq!(state.build_import.multi_builds[0].name, "Speed");
        assert!(!state.build_import.paragon_toggles.get(PROVIDER_MAXROLL));
        assert!(state.build_import.paragon_toggles.get(PROVIDER_D2CORE));
    }
}
