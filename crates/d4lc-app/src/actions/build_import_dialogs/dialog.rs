//! Dialog open, per-provider seeding and property plumbing, the URL
//! form updates, and the dialog-family confirm / cancel entry points.

use std::path::Path;

use d4lc_rules::ImportBuildDescriptor;

use super::gates::{
    can_add_d2core_build, can_add_d4builds_build, can_add_infinity_builds_build,
    can_add_maxroll_build, can_add_mobalytics_build, mobalytics_url_status,
};
use super::merge_tab::DEFAULT_MERGE_COLOR;
use super::paths::{
    dialog_builds_dir, load_builds_for_provider, load_profiles_for_provider, resolve_profiles_dir,
};
use super::{
    tab_index_for_website, ALL_BUILD_IMPORT_PROVIDERS, ALL_BUILD_IMPORT_WEBSITES, PROVIDER_D2CORE,
    PROVIDER_D4BUILDS, PROVIDER_INFINITY_BUILDS, PROVIDER_MAXROLL, PROVIDER_MOBALYTICS,
};
use crate::app_state::{AppState, DialogKind};
use crate::slint_generated::AppWindow;

// ---------------------------------------------------------------------------
// Dialog open + per-provider seeding.
// ---------------------------------------------------------------------------

/// Open the tabbed import dialog on the tab mapped from the AffixScreen's
/// website selection (upstream `ImportAffixPresetCommandExecute`,
/// `AffixViewModel.cs:1779-1794`). The selection is persisted
/// (remember-last-website, upstream `InitBuildImportWebsites`).
pub fn handle_open_import_preset(app: &AppWindow, state: &mut AppState) {
    let website = app.get_build_import_provider().to_string();
    if ALL_BUILD_IMPORT_WEBSITES.contains(&website.as_str())
        && state.shell.settings.selected_build_import_website != website
    {
        state.shell.settings.selected_build_import_website = website.clone();
        state.bump_rules_input_revision();
    }
    open_import_dialog(app, state, tab_index_for_website(&website));
}

/// Open the tabbed import dialog on `tab` (the AffixScreen provider chips;
/// upstream's per-provider dialogs collapse into one dialog here).
pub fn open_import_dialog(app: &AppWindow, state: &mut AppState, tab: i32) {
    app.set_import_tab_index(tab);
    seed_preset_models(app, state);
    for provider in ALL_BUILD_IMPORT_PROVIDERS {
        seed_provider_state(app, state, provider);
    }
    seed_merge_state(app, state);
    // Overlays start closed.
    // The progress panel stays up while an import still runs (it is the
    // only way to follow or cancel it); a finished import's panel closes.
    let running = state.build_import.active_download.is_some();
    app.set_import_progress_visible(running);
    if !running {
        app.set_import_progress_failed(false);
    }
    app.set_import_name_visible(false);
    app.set_import_delete_visible(false);
    app.set_import_color_picker_visible(false);
    state.build_import.pending_import = None;
    state.bump_rules_input_revision();
    state.build_import.pending_merge = None;
    state.bump_rules_input_revision();
    crate::actions::dialogs::open(app, state, DialogKind::ImportPreset, None);
}

/// Reset the Merge tab's rows (upstream constructs a fresh
/// `ImportAffixPresetViewModel` per dialog open: the colors start at
/// `Settings.DefaultColorNormal`, the checkboxes are off, and the preset
/// selections are empty).
fn seed_merge_state(app: &AppWindow, _state: &AppState) {
    app.set_import_merge_build1(slint::SharedString::default());
    app.set_import_merge_build2(slint::SharedString::default());
    app.set_import_merge_change_color1(false);
    app.set_import_merge_change_color2(false);
    app.set_import_merge_change_color12(false);
    app.set_import_merge_color1(DEFAULT_MERGE_COLOR.into());
    app.set_import_merge_color2(DEFAULT_MERGE_COLOR.into());
    app.set_import_merge_color12(DEFAULT_MERGE_COLOR.into());
    app.set_import_color_picker_value(DEFAULT_MERGE_COLOR.into());
    app.set_import_color_picker_row(0);
}

/// Seed the per-provider form: builds list (left), variants of the
/// selected build (right), URL / extracted-id form, and the toggles.
/// Upstream `Update{Provider}Builds` resets the selection to none.
fn seed_provider_state(app: &AppWindow, state: &mut AppState, provider: &str) {
    let builds = dialog_builds_dir(provider, state)
        .map(|dir| {
            load_builds_for_provider(provider, &dir).unwrap_or_else(|error| {
                log_list_failure(state, &format!("{provider} builds: {error}"));
                Vec::new()
            })
        })
        .unwrap_or_default();
    state
        .build_import
        .provider_builds
        .insert(provider.to_string(), builds.clone());
    state.bump_rules_input_revision();
    set_provider_builds(app, provider, &builds);
    set_provider_selected_build(app, provider, -1);
    set_provider_variants(app, provider, &[]);
    set_provider_selected_variant(app, provider, -1);
    // A fresh open starts with an empty URL form and a disabled Add.
    set_provider_url(app, provider, "");
    set_provider_extracted_id(app, provider, "");
    set_provider_add_enabled(app, provider, false);
    set_provider_paragon(
        app,
        provider,
        state.build_import.paragon_toggles.get(provider),
    );
    if provider == PROVIDER_MOBALYTICS {
        app.set_import_mobalytics_url_status(mobalytics_url_status("").into());
        app.set_import_mobalytics_show_browser(state.shell.settings.mobalytics_show_browser);
        // Upstream opens the nested tab control on "Builds" and seeds the
        // profiles list (UpdateMobalyticsProfiles resets the profile
        // selection to none).
        app.set_import_mobalytics_selected_sub_tab(0);
        refresh_provider_profiles(app, state, provider);
    }
}

/// The stored profiles of `provider` in `profiles_dir` (none without a
/// directory). Every skipped file and a listing failure are logged.
fn load_profiles_logged(
    state: &mut AppState,
    provider: &str,
    profiles_dir: Option<&std::path::Path>,
) -> Vec<d4lc_rules::MobalyticsProfileFile> {
    let Some(dir) = profiles_dir else {
        return Vec::new();
    };
    match load_profiles_for_provider(provider, dir) {
        Ok((profiles, skipped)) => {
            for message in skipped {
                log_list_failure(state, &format!("{provider} profile skipped: {message}"));
            }
            profiles
        }
        Err(error) => {
            log_list_failure(state, &format!("{provider} profiles: {error}"));
            Vec::new()
        }
    }
}

/// Report a stored build/profile list problem on the Logging screen and
/// stderr; the dialog still shows what could be loaded.
fn log_list_failure(state: &mut AppState, message: &str) {
    eprintln!("[d4lc] build import: {message}");
    state.append_log_message(
        format!("Build import: {message}"),
        crate::clock::hh_mm_now(),
    );
}

/// Reload the provider's stored profiles from disk into the dialog and
/// reset the selection (upstream `UpdateMobalyticsProfiles` after a
/// `MobalyticsProfilesLoadedMessage`).
pub(super) fn refresh_provider_profiles(app: &AppWindow, state: &mut AppState, provider: &str) {
    let profiles = load_profiles_logged(state, provider, resolve_profiles_dir(provider).as_deref());
    state
        .build_import
        .provider_profiles
        .insert(provider.to_string(), profiles.clone());
    state.bump_rules_input_revision();
    let rows: Vec<crate::slint_generated::ImportBuildRow> = profiles
        .iter()
        .map(|profile| crate::slint_generated::ImportBuildRow {
            id: profile.id.clone().into(),
            name: profile.name.clone().into(),
            date: String::new().into(),
        })
        .collect();
    app.set_import_mobalytics_profiles(slint::ModelRc::new(slint::VecModel::from(rows)));
    app.set_import_mobalytics_selected_profile_index(-1);
    app.set_import_mobalytics_profile_variants(slint::ModelRc::new(slint::VecModel::from(Vec::<
        crate::slint_generated::ImportBuildRow,
    >::new(
    ))));
    app.set_import_mobalytics_selected_profile_variant_index(-1);
}

/// The cached profile entity for (provider, index): the dialog's profile
/// list is a projection of this cache.
pub(super) fn cached_profile(
    state: &AppState,
    provider: &str,
    index: i32,
) -> Result<d4lc_rules::MobalyticsProfileFile, String> {
    if index < 0 {
        return Err("no profile selected".to_string());
    }
    state
        .build_import
        .provider_profiles
        .get(provider)
        .and_then(|profiles| profiles.get(index as usize))
        .cloned()
        .ok_or_else(|| format!("no profile row {index} for provider '{provider}'"))
}

/// The per-tab preset combo (upstream `UpdateAffixPresets` fills the list
/// and selects the first preset — a dialog-local selection).
pub(super) fn seed_preset_models(app: &AppWindow, state: &AppState) {
    let names: Vec<slint::SharedString> = state
        .affix_preset_editor
        .presets
        .iter()
        .map(|preset| preset.name.clone().into())
        .collect();
    app.set_import_preset_names(slint::ModelRc::new(slint::VecModel::from(names)));
    app.set_import_selected_preset(
        state
            .affix_preset_editor
            .presets
            .first()
            .map(|preset| preset.name.clone())
            .unwrap_or_default()
            .into(),
    );
}

// ---------------------------------------------------------------------------
// Per-provider property plumbing. Slint properties are static, so the
// provider token dispatches to the concrete setter; the match must stay
// total over ALL_BUILD_IMPORT_PROVIDERS.
// ---------------------------------------------------------------------------

fn set_provider_url(app: &AppWindow, provider: &str, value: &str) {
    match provider {
        PROVIDER_D2CORE => app.set_import_d2core_url(value.into()),
        PROVIDER_D4BUILDS => app.set_import_d4builds_url(value.into()),
        PROVIDER_INFINITY_BUILDS => app.set_import_infinity_url(value.into()),
        PROVIDER_MAXROLL => app.set_import_maxroll_url(value.into()),
        PROVIDER_MOBALYTICS => app.set_import_mobalytics_url(value.into()),
        _ => {}
    }
}

fn set_provider_extracted_id(app: &AppWindow, provider: &str, value: &str) {
    match provider {
        PROVIDER_D2CORE => app.set_import_d2core_extracted_id(value.into()),
        PROVIDER_D4BUILDS => app.set_import_d4builds_extracted_id(value.into()),
        PROVIDER_INFINITY_BUILDS => app.set_import_infinity_extracted_id(value.into()),
        PROVIDER_MAXROLL => app.set_import_maxroll_extracted_id(value.into()),
        PROVIDER_MOBALYTICS => app.set_import_mobalytics_extracted_id(value.into()),
        _ => {}
    }
}

fn set_provider_add_enabled(app: &AppWindow, provider: &str, enabled: bool) {
    match provider {
        PROVIDER_D2CORE => app.set_import_d2core_add_enabled(enabled),
        PROVIDER_D4BUILDS => app.set_import_d4builds_add_enabled(enabled),
        PROVIDER_INFINITY_BUILDS => app.set_import_infinity_add_enabled(enabled),
        PROVIDER_MAXROLL => app.set_import_maxroll_add_enabled(enabled),
        PROVIDER_MOBALYTICS => app.set_import_mobalytics_add_enabled(enabled),
        _ => {}
    }
}

fn set_provider_paragon(app: &AppWindow, provider: &str, enabled: bool) {
    match provider {
        PROVIDER_D2CORE => app.set_import_d2core_paragon_enabled(enabled),
        PROVIDER_D4BUILDS => app.set_import_d4builds_paragon_enabled(enabled),
        PROVIDER_INFINITY_BUILDS => app.set_import_infinity_paragon_enabled(enabled),
        PROVIDER_MAXROLL => app.set_import_maxroll_paragon_enabled(enabled),
        PROVIDER_MOBALYTICS => app.set_import_mobalytics_paragon_enabled(enabled),
        _ => {}
    }
}

fn set_provider_builds(app: &AppWindow, provider: &str, builds: &[ImportBuildDescriptor]) {
    let rows: Vec<crate::slint_generated::ImportBuildRow> = builds
        .iter()
        .map(|build| crate::slint_generated::ImportBuildRow {
            id: build.id.clone().into(),
            name: build.name.clone().into(),
            date: build.date.clone().into(),
        })
        .collect();
    match provider {
        PROVIDER_D2CORE => {
            app.set_import_d2core_builds(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_D4BUILDS => {
            app.set_import_d4builds_builds(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_INFINITY_BUILDS => {
            app.set_import_infinity_builds(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_MAXROLL => {
            app.set_import_maxroll_builds(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_MOBALYTICS => {
            app.set_import_mobalytics_builds(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        _ => {}
    }
}

pub(super) fn set_provider_selected_build(app: &AppWindow, provider: &str, index: i32) {
    match provider {
        PROVIDER_D2CORE => app.set_import_d2core_selected_build_index(index),
        PROVIDER_D4BUILDS => app.set_import_d4builds_selected_build_index(index),
        PROVIDER_INFINITY_BUILDS => app.set_import_infinity_selected_build_index(index),
        PROVIDER_MAXROLL => app.set_import_maxroll_selected_build_index(index),
        PROVIDER_MOBALYTICS => app.set_import_mobalytics_selected_build_index(index),
        _ => {}
    }
}

pub(super) fn set_provider_variants(app: &AppWindow, provider: &str, variants: &[String]) {
    let rows: Vec<slint::SharedString> = variants
        .iter()
        .map(|variant| variant.clone().into())
        .collect();
    match provider {
        PROVIDER_D2CORE => {
            app.set_import_d2core_variants(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_D4BUILDS => {
            app.set_import_d4builds_variants(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_INFINITY_BUILDS => {
            app.set_import_infinity_variants(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_MAXROLL => {
            app.set_import_maxroll_variants(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        PROVIDER_MOBALYTICS => {
            app.set_import_mobalytics_variants(slint::ModelRc::new(slint::VecModel::from(rows)))
        }
        _ => {}
    }
}

pub(super) fn set_provider_selected_variant(app: &AppWindow, provider: &str, index: i32) {
    match provider {
        PROVIDER_D2CORE => app.set_import_d2core_selected_variant_index(index),
        PROVIDER_D4BUILDS => app.set_import_d4builds_selected_variant_index(index),
        PROVIDER_INFINITY_BUILDS => app.set_import_infinity_selected_variant_index(index),
        PROVIDER_MAXROLL => app.set_import_maxroll_selected_variant_index(index),
        PROVIDER_MOBALYTICS => app.set_import_mobalytics_selected_variant_index(index),
        _ => {}
    }
}

pub(super) fn get_provider_url(app: &AppWindow, provider: &str) -> String {
    match provider {
        PROVIDER_D2CORE => app.get_import_d2core_url().to_string(),
        PROVIDER_D4BUILDS => app.get_import_d4builds_url().to_string(),
        PROVIDER_INFINITY_BUILDS => app.get_import_infinity_url().to_string(),
        PROVIDER_MAXROLL => app.get_import_maxroll_url().to_string(),
        PROVIDER_MOBALYTICS => app.get_import_mobalytics_url().to_string(),
        _ => String::new(),
    }
}

/// Reload the provider's stored builds from disk into the dialog and reset
/// the selection (upstream `Update{Provider}Builds` after a
/// `{Provider}BuildsLoadedMessage`).
pub(super) fn refresh_provider_builds(app: &AppWindow, state: &mut AppState, provider: &str) {
    let builds = dialog_builds_dir(provider, state)
        .map(|dir| {
            load_builds_for_provider(provider, &dir).unwrap_or_else(|error| {
                log_list_failure(state, &format!("{provider} builds: {error}"));
                Vec::new()
            })
        })
        .unwrap_or_default();
    state
        .build_import
        .provider_builds
        .insert(provider.to_string(), builds.clone());
    state.bump_rules_input_revision();
    set_provider_builds(app, provider, &builds);
    set_provider_selected_build(app, provider, -1);
    set_provider_variants(app, provider, &[]);
    set_provider_selected_variant(app, provider, -1);
}

/// The cached descriptor row for (provider, index): the dialog's list is a
/// projection of this cache.
pub(super) fn cached_build(
    state: &AppState,
    provider: &str,
    index: i32,
) -> Result<ImportBuildDescriptor, String> {
    if index < 0 {
        return Err("no build selected".to_string());
    }
    state
        .build_import
        .provider_builds
        .get(provider)
        .and_then(|builds| builds.get(index as usize))
        .cloned()
        .ok_or_else(|| format!("no build row {index} for provider '{provider}'"))
}

// ---------------------------------------------------------------------------
// URL form updates (the Add gate live on every keystroke, upstream
// `UpdateSourceTrigger=PropertyChanged`).
// ---------------------------------------------------------------------------

/// Re-evaluate the Add gate for `provider` after a URL edit: refresh the
/// extracted-id box, the Add enabled flag, and (Mobalytics) the status
/// line.
pub fn handle_url_changed(app: &AppWindow, provider: &str, url: &str) {
    let extracted = match provider {
        PROVIDER_D2CORE => can_add_d2core_build(url),
        PROVIDER_D4BUILDS => can_add_d4builds_build(url),
        PROVIDER_INFINITY_BUILDS => can_add_infinity_builds_build(url),
        PROVIDER_MAXROLL => can_add_maxroll_build(url),
        PROVIDER_MOBALYTICS => can_add_mobalytics_build(url),
        _ => None,
    };
    set_provider_extracted_id(app, provider, extracted.as_deref().unwrap_or(""));
    set_provider_add_enabled(app, provider, extracted.is_some());
    if provider == PROVIDER_MOBALYTICS {
        app.set_import_mobalytics_url_status(mobalytics_url_status(url).into());
    }
}

// ---------------------------------------------------------------------------
// Confirm / cancel entry points.
//
// The router in `actions::dialogs` calls `handle_confirm` and
// `handle_cancel` for every dialog whose family is
// `DialogFamily::BuildImport`. The tabbed dialog's Done button closes
// (upstream `ImportAffixPresetDoneExecute` → `CloseCommand`); the real
// import work runs through the Confirm-name overlay flow, not the dialog
// confirm.
// ---------------------------------------------------------------------------

pub fn handle_confirm(
    _app: &AppWindow,
    state: &mut AppState,
    _config_root: &Path,
) -> Result<(), String> {
    let kind = state.dialog.kind;
    match kind {
        // Done only closes: the multi-build dialog saves every edit at once
        // (upstream `MultiBuildConfigDoneExecute`).
        DialogKind::ImportPreset | DialogKind::MultiBuildConfig => Ok(()),
        other => Err(format!(
            "build_import_dialogs::handle_confirm: unimplemented kind {other}"
        )),
    }
}

pub fn handle_cancel(app: &AppWindow, state: &mut AppState) {
    // Abandon any pending Confirm-name flow; the Slint root props hold the
    // form values and are re-seeded on the next open.
    state.build_import.pending_import = None;
    state.bump_rules_input_revision();
    state.build_import.pending_merge = None;
    state.bump_rules_input_revision();
    let _ = app;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_corrupt_profile_is_logged_and_the_others_still_load() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Good.json"),
            r#"{"Id":"p1","Name":"Good","Url":"https://mobalytics.gg/diablo-4/profile/p1","Variants":[]}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("Broken.json"), "{ not json").unwrap();
        let mut state = AppState::default_startup();

        let profiles = load_profiles_logged(&mut state, PROVIDER_MOBALYTICS, Some(dir.path()));

        assert_eq!(
            profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Good"]
        );
        let logged: Vec<&str> = state
            .logging
            .messages
            .iter()
            .map(|entry| entry.message.as_str())
            .collect();
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(
            logged[0].starts_with("Build import: Mobalytics profile skipped: failed to parse ")
                && logged[0].contains("Broken.json"),
            "{logged:?}"
        );

        // An unlistable directory is logged too, never a silent empty list.
        let missing = dir.path().join("missing");
        assert!(load_profiles_logged(&mut state, PROVIDER_MOBALYTICS, Some(&missing)).is_empty());
        assert!(state.logging.messages[1]
            .message
            .starts_with("Build import: Mobalytics profiles: failed to read "));
        assert!(load_profiles_logged(&mut state, PROVIDER_MOBALYTICS, None).is_empty());
        assert_eq!(state.logging.messages.len(), 2);
    }
}
