//! Stored build and Mobalytics profile row actions: select, update,
//! web, remove and variant add.

use std::path::Path;

use d4lc_rules::ImportBuildDescriptor;

use super::dialog::{
    cached_build, cached_profile, get_provider_url, handle_url_changed, refresh_provider_builds,
    refresh_provider_profiles, set_provider_selected_build, set_provider_selected_variant,
    set_provider_variants,
};
use super::downloads::{start_download, DownloadJob};
use super::gates::{can_add_d4builds_build, can_add_maxroll_build, mobalytics_effective_url};
use super::paths::{dialog_builds_dir, resolve_profiles_dir};
use super::provider_downloads::validate_provider_build_url;
use super::{
    PROVIDER_D2CORE, PROVIDER_D4BUILDS, PROVIDER_INFINITY_BUILDS, PROVIDER_MAXROLL,
    PROVIDER_MOBALYTICS,
};
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

// ---------------------------------------------------------------------------
// Row actions: Web / Visit (browser), Remove (no confirmation, upstream
// parity), and the variant Add (Confirm-name overlay).
// ---------------------------------------------------------------------------

/// The stored-build page URL (upstream `Web{Provider}BuildExecute`): the
/// URL the build was downloaded from when the stored file carries one on
/// the provider's domain, else the page built from the stored id.
fn build_web_url(provider: &str, build: &ImportBuildDescriptor) -> Result<String, String> {
    if !build.url.is_empty() && validate_provider_build_url(provider, &build.url).is_ok() {
        return Ok(build.url.clone());
    }
    match provider {
        PROVIDER_D2CORE => Ok(format!("https://www.d2core.com/d4/planner?bd={}", build.id)),
        PROVIDER_D4BUILDS => Ok(format!("https://d4builds.gg/builds/{}", build.id)),
        PROVIDER_INFINITY_BUILDS => Ok(format!("https://infinitybuilds.gg/en/builds/{}", build.id)),
        PROVIDER_MAXROLL => Ok(format!("https://maxroll.gg/d4/planner/{}", build.id)),
        PROVIDER_MOBALYTICS if !build.url.is_empty() => Ok(build.url.clone()),
        PROVIDER_MOBALYTICS => Err(format!(
            "stored Mobalytics build '{}' carries no URL",
            build.id
        )),
        other => Err(format!("unknown build-import provider '{other}'")),
    }
}

/// Open a stored build's provider page in the browser.
pub fn handle_build_web(state: &AppState, provider: &str, index: i32) -> Result<(), String> {
    let build = cached_build(state, provider, index)?;
    let url = build_web_url(provider, &build)?;
    open_url(&url)
}

/// The provider's build overview page (upstream `Visit{Provider}Execute`).
fn provider_overview_url(provider: &str) -> Result<&'static str, String> {
    match provider {
        PROVIDER_D2CORE => Ok("https://www.d2core.com/d4/builds"),
        PROVIDER_D4BUILDS => Ok("https://d4builds.gg/"),
        PROVIDER_INFINITY_BUILDS => Ok("https://infinitybuilds.gg/en/builds"),
        PROVIDER_MAXROLL => Ok("https://maxroll.gg/d4/build-guides"),
        PROVIDER_MOBALYTICS => Ok("https://mobalytics.gg/diablo-4/builds"),
        other => Err(format!("unknown build-import provider '{other}'")),
    }
}

/// What the URL row's Web button opens for the `entered` text:
///
/// - empty field: the provider's build overview page (upstream
///   `Visit{Provider}Execute`);
/// - an https URL on the provider's domain: exactly that URL (query and
///   build id intact — upstream always opened the overview, which lost
///   the entered build);
/// - a bare D4Builds / Maxroll build id the Add gate accepts: that
///   build's page;
/// - anything else: an error — an invalid entry is never silently
///   replaced by the overview page.
pub(super) fn visit_web_url(provider: &str, entered: &str) -> Result<String, String> {
    let entered = entered.trim();
    if entered.is_empty() {
        return provider_overview_url(provider).map(str::to_string);
    }
    if validate_provider_build_url(provider, entered).is_ok() {
        return Ok(entered.to_string());
    }
    let bare_id = !entered.contains('/') && !entered.contains(':');
    match provider {
        PROVIDER_D4BUILDS if bare_id => {
            if let Some(id) = can_add_d4builds_build(entered) {
                return Ok(format!("https://d4builds.gg/builds/{id}/"));
            }
        }
        PROVIDER_MAXROLL if bare_id => {
            if let Some(id) = can_add_maxroll_build(entered) {
                return Ok(format!("https://maxroll.gg/d4/planner/{id}"));
            }
        }
        _ => {}
    }
    Err(format!(
        "'{entered}' is not a {provider} build address (https on the provider's site); \
         nothing was opened"
    ))
}

/// The URL row's Web button: open the entered build, or the provider's
/// overview page while the field is empty (see [`visit_web_url`]).
pub fn handle_visit_web(app: &AppWindow, provider: &str) -> Result<(), String> {
    let url = visit_web_url(provider, &get_provider_url(app, provider))?;
    open_url(&url)
}

pub(super) fn open_url(url: &str) -> Result<(), String> {
    crate::actions::open_url(url)
}

/// Remove a stored build file without confirmation (upstream
/// `Remove{Provider}BuildExecute` → `File.Delete`), then reload the list.
pub fn handle_build_remove(
    app: &AppWindow,
    state: &mut AppState,
    provider: &str,
    index: i32,
) -> Result<(), String> {
    let build = cached_build(state, provider, index)?;
    let builds_dir = dialog_builds_dir(provider, state)
        .ok_or_else(|| format!("No builds directory available for provider '{provider}'."))?;
    remove_build_file(&builds_dir, &build.id)?;
    refresh_provider_builds(app, state, provider);
    Ok(())
}

/// Delete the stored JSON of a build id (upstream
/// `Remove{Provider}BuildExecute` → `File.Delete`, no confirmation). The
/// id comes from the stored build JSON, so it passes the port-side
/// cache-file safety gate first: a hostile `"id": "../x"` must never
/// delete files above the builds directory. Returns whether a file was
/// removed; a missing file is not an error (the list refreshes anyway).
fn remove_build_file(builds_dir: &Path, build_id: &str) -> Result<bool, String> {
    d4lc_rules::ensure_cache_filename_safe(build_id)?;
    let path = builds_dir.join(format!("{build_id}.json"));
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| format!("remove {}: {error}", path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Build-row click: select the build and list its variants
/// (upstream `Select{Provider}BuildExecute`).
pub fn handle_build_selected(
    app: &AppWindow,
    state: &AppState,
    provider: &str,
    index: i32,
) -> Result<(), String> {
    let build = cached_build(state, provider, index)?;
    set_provider_selected_build(app, provider, index);
    set_provider_variants(app, provider, &build.variant_names);
    set_provider_selected_variant(app, provider, -1);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mobalytics profile rows (upstream `SelectMobalyticsProfileExecute`,
// `UpdateMobalyticsProfileExecute`, `WebMobalyticsProfileExecute`,
// `RemoveMobalyticsProfileExecute`, and `AddMobalyticsProfileBuildVariant
// Execute` — the nested "Profiles" tab of the Mobalytics provider tab).
// ---------------------------------------------------------------------------

/// Profile-row click: select the profile and list its published builds
/// (upstream `SelectMobalyticsProfileExecute`).
pub fn handle_profile_selected(
    app: &AppWindow,
    state: &AppState,
    provider: &str,
    index: i32,
) -> Result<(), String> {
    let profile = cached_profile(state, provider, index)?;
    app.set_import_mobalytics_selected_profile_index(index);
    let rows: Vec<crate::slint_generated::ImportBuildRow> = profile
        .variants
        .iter()
        .map(|variant| crate::slint_generated::ImportBuildRow {
            id: variant.id.clone().into(),
            name: variant.name.clone().into(),
            date: variant.date.clone().into(),
        })
        .collect();
    app.set_import_mobalytics_profile_variants(slint::ModelRc::new(slint::VecModel::from(rows)));
    app.set_import_mobalytics_selected_profile_variant_index(-1);
    Ok(())
}

/// Profile-row Upd click: re-download the profile by its stored `Url`
/// (upstream `UpdateMobalyticsProfileExecute`).
pub fn handle_profile_update(
    app: &AppWindow,
    state: &mut AppState,
    provider: &str,
    index: i32,
) -> Result<(), String> {
    let profile = cached_profile(state, provider, index)?;
    if profile.url.is_empty() {
        return Err(format!(
            "stored Mobalytics profile '{}' carries no Url to re-download",
            profile.name
        ));
    }
    start_download(
        app,
        state,
        DownloadJob::Mobalytics {
            url: profile.url.clone(),
        },
        std::time::Instant::now(),
    )
}

/// Profile-row Web click: open the stored profile URL
/// (upstream `WebMobalyticsProfileExecute`).
pub fn handle_profile_web(state: &AppState, provider: &str, index: i32) -> Result<(), String> {
    let profile = cached_profile(state, provider, index)?;
    open_url(&profile.url)
}

/// Profile-row X click: delete the profile file by display NAME (upstream
/// `RemoveMobalyticsProfileExecute` → `RemoveMobalyticsProfile`, no
/// confirmation), then reload the list.
pub fn handle_profile_remove(
    app: &AppWindow,
    state: &mut AppState,
    provider: &str,
    index: i32,
) -> Result<(), String> {
    let profile = cached_profile(state, provider, index)?;
    let profiles_dir = resolve_profiles_dir(provider)
        .ok_or_else(|| format!("No profiles directory available for provider '{provider}'."))?;
    d4lc_rules::remove_mobalytics_profile(&profiles_dir, &profile.name)?;
    refresh_provider_profiles(app, state, provider);
    Ok(())
}

/// Profile-variant Add click (upstream
/// `AddMobalyticsProfileBuildVariantExecute`): fill the URL field with the
/// variant URL — the `?query…` strip of the `BuildIdMobalytics` setter
/// happens through [`mobalytics_effective_url`] — and re-evaluate the Add
/// gate. The user then presses Add; the variant is NOT imported directly.
pub fn handle_profile_variant_add(
    app: &AppWindow,
    state: &AppState,
    provider: &str,
    variant_index: i32,
) -> Result<(), String> {
    let profile_index = app.get_import_mobalytics_selected_profile_index();
    let profile = cached_profile(state, provider, profile_index)?;
    if variant_index < 0 || variant_index as usize >= profile.variants.len() {
        return Err(format!(
            "no profile variant row {variant_index} for profile '{}'",
            profile.name
        ));
    }
    let variant_url = profile.variants[variant_index as usize].url.clone();
    let effective = mobalytics_effective_url(&variant_url);
    app.set_import_mobalytics_url(effective.clone().into());
    handle_url_changed(app, provider, &effective);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn web_opens_the_entered_build_url_with_its_query_intact() {
        // BUG-01: the D2Core Web button opened the builds overview instead
        // of the entered planner build.
        assert_eq!(
            visit_web_url(PROVIDER_D2CORE, "https://www.d2core.com/d4/planner?bd=23tb").unwrap(),
            "https://www.d2core.com/d4/planner?bd=23tb"
        );
        assert_eq!(
            visit_web_url(
                PROVIDER_D2CORE,
                "  https://www.d2core.com/d4/planner?bd=23tb&lang=en "
            )
            .unwrap(),
            "https://www.d2core.com/d4/planner?bd=23tb&lang=en",
            "only surrounding whitespace is normalized"
        );
        for (provider, url) in [
            (PROVIDER_D4BUILDS, "https://d4builds.gg/builds/blazing-scream-warlock-endgame/?var=2"),
            (PROVIDER_MAXROLL, "https://maxroll.gg/d4/planner/to4erl0e#2"),
            (
                PROVIDER_INFINITY_BUILDS,
                "https://infinitybuilds.gg/en/builds/mekunas-blazing-scream-warlock-rn824uhPl3",
            ),
            (
                PROVIDER_MOBALYTICS,
                "https://mobalytics.gg/diablo-4/profile/kerkyboi/builds/kerkyboi-s-s14-swarmageddon",
            ),
        ] {
            assert_eq!(visit_web_url(provider, url).unwrap(), url, "{provider}");
        }
    }

    #[test]
    fn web_opens_the_overview_only_for_an_empty_field() {
        for provider in super::super::ALL_BUILD_IMPORT_PROVIDERS {
            assert_eq!(
                visit_web_url(provider, "   ").unwrap(),
                provider_overview_url(provider).unwrap()
            );
        }
    }

    #[test]
    fn web_opens_the_page_of_a_bare_id_and_refuses_invalid_entries() {
        assert_eq!(
            visit_web_url(PROVIDER_MAXROLL, "to4erl0e").unwrap(),
            "https://maxroll.gg/d4/planner/to4erl0e"
        );
        assert_eq!(
            visit_web_url(PROVIDER_D4BUILDS, "blazing-scream-warlock-endgame").unwrap(),
            "https://d4builds.gg/builds/blazing-scream-warlock-endgame/"
        );
        for (provider, entered) in [
            (PROVIDER_D2CORE, "23tb"),
            (PROVIDER_D2CORE, "http://www.d2core.com/d4/planner?bd=23tb"),
            (PROVIDER_D2CORE, "https://evil.example/d4/planner?bd=23tb"),
            (PROVIDER_MOBALYTICS, "https://d4builds.gg/builds/x"),
            (PROVIDER_MAXROLL, "not an id"),
        ] {
            let error = visit_web_url(provider, entered).expect_err(entered);
            assert!(error.contains("nothing was opened"), "{error}");
        }
    }

    fn descriptor(id: &str, url: &str) -> ImportBuildDescriptor {
        ImportBuildDescriptor {
            id: id.to_string(),
            name: String::new(),
            url: url.to_string(),
            date: String::new(),
            variant_names: Vec::new(),
            source_path: String::new(),
        }
    }

    #[test]
    fn stored_build_web_prefers_the_stored_provider_url() {
        let build = descriptor("23tb", "");
        assert_eq!(
            build_web_url(PROVIDER_D2CORE, &build).unwrap(),
            "https://www.d2core.com/d4/planner?bd=23tb"
        );
        let stored = descriptor(
            "rn824uhPl3",
            "https://infinitybuilds.gg/en/builds/mekunas-blazing-scream-warlock-rn824uhPl3",
        );
        assert_eq!(
            build_web_url(PROVIDER_INFINITY_BUILDS, &stored).unwrap(),
            stored.url
        );
        let foreign = descriptor("to4erl0e", "https://evil.example/x");
        assert_eq!(
            build_web_url(PROVIDER_MAXROLL, &foreign).unwrap(),
            "https://maxroll.gg/d4/planner/to4erl0e",
            "a stored URL off the provider's domain is never opened"
        );
    }

    #[test]
    fn remove_build_file_rejects_hostile_ids_without_touching_the_filesystem() {
        // The build id in a stored JSON is content-controlled; a
        // hostile `"id": "../decoy"` must fail the safety gate before any
        // path is built, leaving files above the builds dir untouched.
        let root = std::env::temp_dir().join("d4lc-hostile-remove");
        let dir = root.join("Builds").join("D2Core");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.join("decoy.json"), b"{}").unwrap();

        let error = remove_build_file(&dir, "../decoy").expect_err("hostile id must be rejected");
        assert!(
            error.contains("cannot be used as a cache file name"),
            "{error}"
        );
        assert!(
            root.join("decoy.json").is_file(),
            "the file above the builds dir must survive"
        );
        // A legitimate id still deletes, and a missing file is not an error.
        std::fs::write(dir.join("abcd.json"), b"{}").unwrap();
        assert!(remove_build_file(&dir, "abcd").unwrap());
        assert!(!remove_build_file(&dir, "abcd").unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }
}
