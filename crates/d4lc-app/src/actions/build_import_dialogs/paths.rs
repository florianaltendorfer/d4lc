//! Builds/profiles directory resolution, stored-build enumeration and
//! source-path resolution for the build-import dialog.

use std::path::{Path, PathBuf};

use d4lc_rules::{
    load_available_d2core_builds, load_available_d4builds_builds,
    load_available_infinity_builds_builds, load_available_maxroll_builds,
    load_available_mobalytics_builds, ImportBuildDescriptor,
};

use super::{
    PROVIDER_D2CORE, PROVIDER_D4BUILDS, PROVIDER_INFINITY_BUILDS, PROVIDER_MAXROLL,
    PROVIDER_MOBALYTICS,
};
use crate::app_state::AppState;

// ---------------------------------------------------------------------------
// Builds directory resolution.
//
// Upstream saves every downloaded build to `.\Builds\{Provider}\{id}.json`
// relative to the app directory and enumerates that same directory (there is
// no configurable builds home upstream — `BuildsManagerD2Core.cs:213-214,592`
// and the four sibling managers hardcode the path). The Linux port resolves
// the per-provider directory in this order:
//
//   1. The configured path (`AppSettings::builds_dir`, hydrated onto
//      `BuildImportState::builds_dir` at startup) when set. It is used
//      verbatim — listing and downloading both read/write it instead of the
//      per-provider home.
//   2. `{config_root}/Builds/{provider}` — the upstream-equivalent
//      downloaded-builds home when it exists. Downloads land
//      here, so imports see them; this is the home an unset
//      `AppSettings::builds_dir` keeps.
//   3. The bundled d4lc-rules fixture directory
//      (`crates/d4lc-rules/tests/fixtures/imports/<provider>`) when it
//      exists at runtime. This is the offline fallback for
//      `cargo test` / dev runs and does NOT require live network access.
//   4. `None` — the dialog surfaces an empty list rather than fake data.
// ---------------------------------------------------------------------------

/// Resolve the builds directory for the given provider.
///
/// Returns `None` if neither the downloaded-builds home nor a
/// workspace fixture directory exists. Callers must treat `None`
/// as an empty state, not as an error.
///
/// This is the *unconfigured* resolution — it never consults the
/// persisted `AppSettings::builds_dir`. Callers that hold the app state
/// apply that override first (`dialog_builds_dir`,
/// `resolve_download_builds_dir`).
pub fn resolve_builds_dir(provider: &str) -> Option<PathBuf> {
    resolve_builds_dir_with_config_root(provider, &crate::actions::settings::resolve_config_root())
}

/// Resolve the profiles directory for the given provider — the upstream
/// `.\Profiles\{Provider}` home first, then the bundled fixture directory.
pub fn resolve_profiles_dir(provider: &str) -> Option<PathBuf> {
    resolve_profiles_dir_with_config_root(
        provider,
        &crate::actions::settings::resolve_config_root(),
    )
}

/// [`resolve_profiles_dir`] against an explicit config root.
pub(crate) fn resolve_profiles_dir_with_config_root(
    provider: &str,
    config_root: &Path,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    candidates.push(downloaded_profiles_dir_in(config_root));
    candidates.extend(candidate_profiles_dirs(provider));
    candidates.into_iter().find(|path| path.is_dir())
}

fn candidate_profiles_dirs(provider: &str) -> Vec<PathBuf> {
    match provider {
        PROVIDER_MOBALYTICS => vec![workspace_rules_fixtures_dir("mobalytics/profiles")],
        _ => Vec::new(),
    }
}

/// Enumerate the stored profiles of `provider` from `profiles_dir` through
/// the matching `d4lc_rules` loader (upstream
/// `LoadAvailableMobalyticsProfiles`): the profiles plus one message per
/// unreadable file that was skipped. Unknown providers error.
pub fn load_profiles_for_provider(
    provider: &str,
    profiles_dir: &Path,
) -> Result<(Vec<d4lc_rules::MobalyticsProfileFile>, Vec<String>), String> {
    match provider {
        PROVIDER_MOBALYTICS => d4lc_rules::load_available_mobalytics_profiles(profiles_dir),
        other => Err(format!("unknown build-import provider '{other}'")),
    }
}

/// [`resolve_builds_dir`] against an explicit config root — the pure,
/// hermetic seam the unit tests use.
pub(crate) fn resolve_builds_dir_with_config_root(
    provider: &str,
    config_root: &Path,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    candidates.push(downloaded_builds_dir_in(config_root, provider));
    candidates.extend(candidate_builds_dirs(provider));
    candidates.into_iter().find(|path| path.is_dir())
}

/// The downloaded-builds home for `provider` under `config_root` — the
/// upstream `.\Builds\{Provider}` layout. Callers create
/// the directory on demand when downloading.
pub(super) fn downloaded_builds_dir_in(config_root: &Path, provider: &str) -> PathBuf {
    config_root.join("Builds").join(provider)
}

/// The downloaded-profiles home for `provider` under `config_root` — the
/// upstream `.\Profiles\{Provider}` layout. Callers create the directory
/// on demand when downloading.
pub(super) fn downloaded_profiles_dir_in(config_root: &Path) -> PathBuf {
    config_root.join("Profiles").join(PROVIDER_MOBALYTICS)
}

fn candidate_builds_dirs(provider: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let provider_subdir = match provider {
        PROVIDER_D2CORE => "d2core/source",
        PROVIDER_D4BUILDS => "d4builds",
        PROVIDER_MAXROLL => "maxroll/source",
        PROVIDER_MOBALYTICS => "mobalytics/source",
        PROVIDER_INFINITY_BUILDS => "infinitybuilds/source",
        other => {
            paths.push(workspace_rules_fixtures_dir(other));
            return paths;
        }
    };
    paths.push(workspace_rules_fixtures_dir(provider_subdir));
    paths
}

/// The workspace root (dev checkout). One of the candidates
/// [`resolve_asset_repo_root`] probes for the localized import data under
/// `assets/original/data`.
pub(super) fn workspace_root_dir() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // d4lc-app
    root.pop(); // crates
    root
}

pub(super) fn workspace_rules_fixtures_dir(sub: &str) -> PathBuf {
    // `d4lc-app/Cargo.toml` lives at `<workspace>/crates/d4lc-app`,
    // so `../../../` reaches the workspace root.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut root = manifest_dir;
    // Pop "d4lc-app".
    root.pop();
    // Pop "crates".
    root.pop();
    root.push("crates");
    root.push("d4lc-rules");
    root.push("tests");
    root.push("fixtures");
    root.push("imports");
    root.push(sub);
    root
}

/// Resolve the asset-repository root that carries the localized import
/// data (`assets/original/data`, the lookup tables behind
/// `ImportLocalizedData`): CWD-first (installed bundle layout — packaging
/// copies the whole `assets/` tree into the bundle and starts the binary
/// with the bundle root as CWD), then the compile-time dev-checkout root
/// (the same dev-first resolution the d4lc-rules import tests use).
///
/// The builds directory is NOT a repository root — it only holds the
/// provider build JSONs — so passing it as the `AssetRepository` root
/// makes every import fail with "missing localized data file".
/// `None` when neither location carries the data; callers must surface
/// an error instead of importing against an empty repository.
pub(super) fn resolve_asset_repo_root() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    candidates.push(workspace_root_dir());
    candidates
        .into_iter()
        .find(|root| root.join(d4lc_assets::DATA_DIR).is_dir())
}

// ---------------------------------------------------------------------------
// Stored-build enumeration (the dialog's left lists).
// ---------------------------------------------------------------------------

/// Enumerate the stored builds of `provider` from `builds_dir` through the
/// matching `d4lc_rules::load_available_{provider}_builds` loader.
/// Returns an empty-list-worthy `Err` only for an unknown provider or an
/// unreadable directory. Maxroll additionally tolerates one malformed
/// cached file (skipped with a structured warning through
/// `d4lc_rules::load_available_maxroll_builds`): upstream's list load
/// never hard-fails the dialog — `BuildsManagerMaxroll
/// .LoadAvailableMaxrollBuilds` catches per-file failures and logs, so
/// the rows that do parse still list.
pub fn load_builds_for_provider(
    provider: &str,
    builds_dir: &Path,
) -> Result<Vec<ImportBuildDescriptor>, String> {
    match provider {
        PROVIDER_D2CORE => load_available_d2core_builds(builds_dir),
        PROVIDER_D4BUILDS => load_available_d4builds_builds(builds_dir),
        PROVIDER_MAXROLL => load_available_maxroll_builds(builds_dir).map(|list| list.builds),
        PROVIDER_MOBALYTICS => load_available_mobalytics_builds(builds_dir),
        PROVIDER_INFINITY_BUILDS => load_available_infinity_builds_builds(builds_dir),
        other => Err(format!("unknown build-import provider '{other}'")),
    }
}

/// The builds directory the dialog enumerates: the configured path first
/// (`BuildImportState::builds_dir`, hydrated from the persisted
/// `AppSettings::builds_dir` at startup), then the resolution order of
/// [`resolve_builds_dir`] when nothing is configured.
pub(super) fn dialog_builds_dir(provider: &str, state: &AppState) -> Option<PathBuf> {
    state
        .build_import
        .builds_dir
        .clone()
        .or_else(|| resolve_builds_dir(provider))
}

// ---------------------------------------------------------------------------
// Source-path resolution: turn (provider, source_build_id) into a
// concrete file path under the offline builds directory.
// ---------------------------------------------------------------------------

/// Resolve the on-disk JSON path for a (provider, source_build_id) pair
/// inside `builds_dir`: the file whose stem equals `source_build_id` (the
/// upstream `{id}.json` cache layout) first, then the file whose own
/// top-level id field carries it (providers whose file name does not
/// mirror the build id: D4Builds `Id`, Maxroll `id`).
///
/// `None` means "this build has no JSON here" — the directory is never
/// searched for a merely plausible file. Substituting the first other
/// JSON of the directory would import and evaluate a build the user did
/// not select, so the caller reports the miss instead.
pub fn resolve_source_path(
    provider: &str,
    builds_dir: &Path,
    source_build_id: &str,
) -> Option<PathBuf> {
    let entries = std::fs::read_dir(builds_dir).ok()?;
    let mut stems: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    stems.sort();
    // Try exact stem match first.
    for path in &stems {
        if path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case(source_build_id))
            .unwrap_or(false)
        {
            return Some(path.clone());
        }
    }
    stems
        .into_iter()
        .find(|path| source_path_matches_build_id(provider, path, source_build_id))
}

fn source_path_matches_build_id(provider: &str, path: &Path, source_build_id: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };

    match provider {
        PROVIDER_D4BUILDS => {
            text.contains(&format!("\"Id\":\"{source_build_id}\""))
                || text.contains(&format!("\"Id\": \"{source_build_id}\""))
        }
        PROVIDER_MAXROLL => {
            text.contains(&format!("\"id\":\"{source_build_id}\""))
                || text.contains(&format!("\"id\": \"{source_build_id}\""))
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    use crate::actions::build_import_dialogs::{
        ALL_BUILD_IMPORT_PROVIDERS, PROVIDER_D2CORE, PROVIDER_D4BUILDS, PROVIDER_INFINITY_BUILDS,
        PROVIDER_MAXROLL,
    };

    // -----------------------------------------------------------------------
    // Build dir resolution
    // -----------------------------------------------------------------------

    /// The bundled fixture builds dir for `provider`, resolved hermetically:
    /// a nonexistent config root can never shadow the fixtures with a real
    /// downloaded-builds home, so these tests stay green regardless of the
    /// developer machine's real `~/.config/d4lc` content (one malformed
    /// cached download there would fail them).
    fn fixture_builds_dir(provider: &str) -> PathBuf {
        resolve_builds_dir_with_config_root(provider, Path::new("nonexistent-config-root"))
            .unwrap_or_else(|| panic!("bundled fixture dir for {provider}"))
    }

    #[test]
    fn resolve_builds_dir_returns_none_for_unknown_provider() {
        // Unknown provider must not panic and must not return
        // a fabricated path. Hermetic: an empty config root has
        // no `Builds/{provider}` home.
        let empty_root = crate::test_root::TestRoot::new("builds-dir-none");
        let path = resolve_builds_dir_with_config_root("Unknown", &empty_root);
        assert!(path.is_none());
    }

    #[test]
    fn resolve_builds_dir_returns_workspace_fixture_for_known_provider() {
        // The bundled fixture directory is present in the
        // d4lc-rules crate's tests/fixtures/imports/ tree. We
        // assert the resolution falls through to it for each
        // known provider when no downloaded-builds home exists
        // (dev / `cargo test` runs). Hermetic: the empty config
        // root has no `Builds/{provider}` directory.
        let empty_root = crate::test_root::TestRoot::new("builds-dir-fixture");
        for provider in ALL_BUILD_IMPORT_PROVIDERS.iter() {
            let path = resolve_builds_dir_with_config_root(provider, &empty_root);
            assert!(
                path.is_some(),
                "expected bundled fixture dir for provider '{provider}'"
            );
            assert!(path.unwrap().is_dir(), "fixture path must be a directory");
        }
    }

    #[test]
    fn resolve_builds_dir_prefers_the_downloaded_builds_home() {
        // `{config_root}/Builds/{provider}` mirrors
        // upstream's `.\Builds\{Provider}`; when it exists it wins over
        // the bundled fixtures so imports see freshly downloaded builds.
        let root = std::env::temp_dir().join("d4lc-builds-dir-home-test");
        let home = root.join("Builds").join(PROVIDER_MAXROLL);
        let _ = std::fs::create_dir_all(&home);
        let resolved = resolve_builds_dir_with_config_root(PROVIDER_MAXROLL, &root);
        assert_eq!(
            resolved.as_deref(),
            Some(home.as_path()),
            "downloaded-builds home must shadow the fixture dir"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn default_builds_dir_creates_the_upstream_builds_layout() {
        // Downloads land under `{config_root}/Builds/{provider}` — the
        // Linux equivalent of upstream `.\Builds\{Provider}` — created on
        // demand.
        let root = std::env::temp_dir().join("d4lc-builds-dir-create-test");
        let _ = std::fs::remove_dir_all(&root);
        let dir = downloaded_builds_dir_in(&root, PROVIDER_INFINITY_BUILDS);
        assert_eq!(
            dir,
            root.join("Builds").join("InfinityBuilds"),
            "provider layout must mirror upstream's Builds/{{Provider}}"
        );
    }

    #[test]
    fn resolve_asset_repo_root_carries_localized_import_data() {
        // The add-as-preset path roots the AssetRepository here; the root
        // must carry `assets/original/data` (the `ImportLocalizedData`
        // lookup tables) and must NOT be the provider builds directory —
        // rooting at the builds dir made every import fail with "missing
        // localized data file".
        let root = resolve_asset_repo_root().expect("dev checkout must carry the asset data");
        assert!(root.join(d4lc_assets::DATA_DIR).is_dir());
        for provider in ALL_BUILD_IMPORT_PROVIDERS.iter() {
            let builds_dir = fixture_builds_dir(provider);
            assert_ne!(
                root.canonicalize().unwrap_or_else(|_| root.clone()),
                builds_dir
                    .canonicalize()
                    .unwrap_or_else(|_| builds_dir.clone()),
                "asset repo root must differ from the '{provider}' builds dir"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Stored-build enumeration
    // -----------------------------------------------------------------------

    #[test]
    fn load_builds_for_provider_enumerates_every_provider_fixture() {
        for provider in ALL_BUILD_IMPORT_PROVIDERS.iter() {
            let dir = fixture_builds_dir(provider);
            let builds = load_builds_for_provider(provider, &dir)
                .unwrap_or_else(|error| panic!("{provider}: {error}"));
            assert!(
                !builds.is_empty(),
                "expected at least one stored build for {provider}"
            );
            assert!(
                builds
                    .iter()
                    .all(|build| !build.id.is_empty() && !build.name.is_empty()),
                "rows must carry id + name"
            );
        }
    }

    #[test]
    fn load_builds_for_provider_skips_malformed_maxroll_cached_file() {
        // Observed with a real cached download: one malformed file must not
        // fail the whole provider list and leave the dialog empty. The list
        // load must skip the bad file and keep the valid rows; the structured
        // warning naming the file is asserted on the `d4lc-rules` loader test.
        let dir = std::env::temp_dir().join("d4lc-app-maxroll-tolerance");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(
            fixture_builds_dir(PROVIDER_MAXROLL).join("whirlwind_speedfarm.json"),
            dir.join("whirlwind_speedfarm.json"),
        )
        .unwrap();
        // The nested planner payload is not JSON.
        std::fs::write(
            dir.join("hidaul0h.json"),
            r#"{"data":"not json","date":"2026-09-01T00:00:00Z","id":"hidaul0h","name":"Broken Cached Build"}"#,
        )
        .unwrap();

        let builds = load_builds_for_provider(PROVIDER_MAXROLL, &dir).unwrap_or_else(|error| {
            panic!("a malformed cached file must not fail the list: {error}")
        });

        assert_eq!(builds.len(), 1, "only the valid build lists");
        assert_eq!(builds[0].id, "barb-whirlwind-speedfarm");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_builds_for_unknown_provider_returns_err() {
        let dir = std::env::temp_dir();
        let result = load_builds_for_provider("Unknown", &dir);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Source path resolution
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_source_path_finds_a_stem_matching_json() {
        // Maxroll's cached envelope file carries its own id (`id`), but a
        // cache written under the build id (`{id}.json`, the upstream
        // layout) is found by the stem alone.
        let dir = std::env::temp_dir().join("d4lc-resolve-source-stem");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("barb-hota-endgame.json"), "{}").unwrap();
        let path = resolve_source_path(PROVIDER_MAXROLL, &dir, "barb-hota-endgame");
        assert_eq!(path, Some(dir.join("barb-hota-endgame.json")));
        // A different build id resolves to nothing — never to that file.
        assert!(resolve_source_path(PROVIDER_MAXROLL, &dir, "other-build").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_source_path_matches_d4builds_top_level_id() {
        let dir = fixture_builds_dir(PROVIDER_D4BUILDS);
        let path = resolve_source_path(PROVIDER_D4BUILDS, &dir, "barb-whirlwind-endgame")
            .expect("expected source path for D4Builds id");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("whirlwind_endgame.json")
        );
    }

    #[test]
    fn resolve_source_path_matches_maxroll_top_level_id() {
        let dir = fixture_builds_dir(PROVIDER_MAXROLL);
        let path = resolve_source_path(PROVIDER_MAXROLL, &dir, "barb-whirlwind-speedfarm")
            .expect("expected source path for Maxroll id");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("whirlwind_speedfarm.json")
        );
    }

    #[test]
    fn resolve_source_path_returns_none_for_missing_dir() {
        let path =
            resolve_source_path(PROVIDER_D2CORE, Path::new("/this/path/does/not/exist"), "x");
        assert!(path.is_none());
    }
}
