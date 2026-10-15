//! Check-only update banner backend.
//!
//! The app checks this project's GitHub Releases for a newer version
//! and shows a non-blocking banner with the release link. It is a
//! check-only updater: check, notify, open the release page — never
//! download, never install, never upload anything.
//!
//! ## Boundaries
//!
//! - The check runs on a dedicated background thread; the Slint UI
//!   thread only receives the final result through
//!   `slint::invoke_from_event_loop`. Nothing here ever touches the
//!   capture/OCR hot path (owned by `d4lc-runtime`).
//! - One check performs one bounded HTTP GET (1 MiB body cap, 10 s
//!   deadline excluding DNS) against the GitHub latest-release API
//!   resolved from the workspace `repository` field. Redirects are
//!   followed only to `https://api.github.com`. `D4LC_UPDATE_CHECK_URL`
//!   overrides the endpoint (operator escape hatch for tests and
//!   proxies); its exact `host:port` is then allowed too.
//! - The release page URL from the payload is shown and opened only when
//!   it is `https://github.com/{owner}/{repo}/releases/...` of this
//!   project.
//! - Failure is silent in the UI: every error path is logged via
//!   `eprintln!` only and the banner simply never appears.

use std::cmp::Ordering;
use std::sync::{Arc, Mutex};

#[cfg(feature = "update-check")]
use slint::ComponentHandle as _;

#[cfg(any(feature = "update-check", test))]
use crate::actions::settings::effective_authority;
use crate::actions::settings::UrlPolicy;
#[cfg(any(feature = "update-check", test))]
use crate::actions::APP_VERSION;
use crate::slint_generated::AppWindow;

/// Delay before the first release check after startup. The check must
/// never sit on the startup path; the dedicated thread sleeps first.
pub const FIRST_CHECK_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// Interval between release checks: 16 hours (bounded, per the
/// check-only update banner contract).
pub const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(16 * 60 * 60);

/// Hard timeout for one release-check HTTP GET. A hung connection must
/// never keep the background thread stuck on a single cycle.
pub const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Maximum release-payload size. A larger body is an error, never a
/// truncated parse. A latest-release JSON with a dozen assets is about
/// 20 KiB.
pub const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024;

/// The only host the release check contacts (redirects included) unless
/// [`ENV_OVERRIDE_URL`] is set.
const RELEASE_API_HOSTS: &[&str] = &["api.github.com"];

/// Request headers of the release check. GitHub's API requires a
/// `User-Agent`; this one names only the app and its version.
#[cfg(feature = "update-check")]
const RELEASE_CHECK_HEADERS: &[(&str, &str)] = &[
    ("User-Agent", concat!("d4lc/", env!("CARGO_PKG_VERSION"))),
    ("Accept", "application/vnd.github+json"),
];

/// Environment override for the release-check endpoint, following the
/// `D4LC_*` pattern. When set to a non-empty http(s) URL it replaces
/// the endpoint resolved from the workspace `repository` field.
pub const ENV_OVERRIDE_URL: &str = "D4LC_UPDATE_CHECK_URL";

/// A release advertised by the GitHub latest-release endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// Release tag as published (e.g. `v1.2.0` or `1.2.0-rc.1`).
    pub version: String,
    /// Release page URL (the payload's `html_url`).
    pub url: String,
}

// ---------------------------------------------------------------------------
// Pure decision functions (unit-tested offline, no IO)
// ---------------------------------------------------------------------------

/// Strip the optional `v` / `V` tag prefix and trim whitespace.
fn normalize_tag(tag: &str) -> &str {
    let trimmed = tag.trim();
    trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
}

/// Split a version into (core, pre-release) with build metadata
/// removed. `1.1.0-rc.1+build.5` → `("1.1.0", Some("rc.1"))`.
fn split_version(version: &str) -> (&str, Option<&str>) {
    let without_build = version.split('+').next().unwrap_or(version);
    match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    }
}

/// Compare the dot-separated numeric core of two versions. Missing
/// trailing components count as zero (`1.0` == `1.0.0`). Returns
/// `None` when a component is not purely numeric — an unparsable
/// version is never ordered.
fn compare_cores(current: &str, candidate: &str) -> Option<Ordering> {
    let current_numbers: Option<Vec<u64>> = current
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect();
    let candidate_numbers: Option<Vec<u64>> = candidate
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect();
    let (Some(current_numbers), Some(candidate_numbers)) = (current_numbers, candidate_numbers)
    else {
        return None;
    };
    let len = current_numbers.len().max(candidate_numbers.len());
    for index in 0..len {
        let current_value = current_numbers.get(index).copied().unwrap_or(0);
        let candidate_value = candidate_numbers.get(index).copied().unwrap_or(0);
        let order = current_value.cmp(&candidate_value);
        if order != Ordering::Equal {
            return Some(order);
        }
    }
    Some(Ordering::Equal)
}

/// Compare two pre-release identifier lists per semver precedence:
/// numeric identifiers compare numerically, alphanumeric identifiers
/// compare as ASCII, numeric sorts below alphanumeric, and a shorter
/// list sorts below a longer one when all preceding identifiers are
/// equal. A version without a pre-release sorts above one with one.
fn compare_pre_releases(current: Option<&str>, candidate: Option<&str>) -> Option<Ordering> {
    match (current, candidate) {
        (None, None) => Some(Ordering::Equal),
        (None, Some(_)) => Some(Ordering::Greater),
        (Some(_), None) => Some(Ordering::Less),
        (Some(current), Some(candidate)) => {
            let mut current_ids = current.split('.');
            let mut candidate_ids = candidate.split('.');
            loop {
                match (current_ids.next(), candidate_ids.next()) {
                    (None, None) => return Some(Ordering::Equal),
                    (None, Some(_)) => return Some(Ordering::Less),
                    (Some(_), None) => return Some(Ordering::Greater),
                    (Some(current_id), Some(candidate_id)) => {
                        let order = match (current_id.parse::<u64>(), candidate_id.parse::<u64>()) {
                            (Ok(current_number), Ok(candidate_number)) => {
                                current_number.cmp(&candidate_number)
                            }
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => current_id.cmp(candidate_id),
                        };
                        if order != Ordering::Equal {
                            return Some(order);
                        }
                    }
                }
            }
        }
    }
}

/// Pure version comparison: `Some(ordering)` when both sides are
/// comparable release versions, `None` when either side is unparsable.
/// Tolerates the `v` tag prefix, missing trailing components, build
/// metadata, and pre-release tags.
pub fn compare_versions(current: &str, candidate: &str) -> Option<Ordering> {
    let (current_core, current_pre) = split_version(normalize_tag(current));
    let (candidate_core, candidate_pre) = split_version(normalize_tag(candidate));
    let core_order = compare_cores(current_core, candidate_core)?;
    if core_order != Ordering::Equal {
        return Some(core_order);
    }
    compare_pre_releases(current_pre, candidate_pre)
}

/// Pure banner decision: `true` only when the latest release tag is a
/// strictly newer version than the running one. Equal or older tags —
/// and unparsable tags — produce no banner.
pub fn is_newer_release(current_version: &str, latest_tag: &str) -> bool {
    matches!(
        compare_versions(current_version, latest_tag),
        Some(Ordering::Less)
    )
}

/// Pure gating decision for one check cycle: the persisted Settings
/// toggle must be enabled and the `--no-updates` session flag must be
/// absent.
pub fn should_run_check(setting_enabled: bool, cli_suppressed: bool) -> bool {
    setting_enabled && !cli_suppressed
}

/// Resolve the GitHub latest-release API endpoint from a project
/// repository URL. Only `github.com` web URLs with an `owner/repo`
/// slug resolve; anything else is rejected so the check can never be
/// pointed at an unrelated host by the manifest value.
pub fn resolve_release_api_url(repository_url: &str) -> Result<String, String> {
    let slug = repository_slug(repository_url)?;
    Ok(format!(
        "https://api.github.com/repos/{slug}/releases/latest"
    ))
}

/// The `owner/repo` slug of a `github.com` project URL.
fn repository_slug(repository_url: &str) -> Result<&str, String> {
    let trimmed = repository_url.trim().trim_end_matches('/');
    let slug = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .ok_or_else(|| format!("repository '{repository_url}' is not a github.com project URL"))?;
    let slug = slug.strip_suffix(".git").unwrap_or(slug);
    let mut parts = slug.split('/');
    let owner = parts.next().filter(|part| !part.is_empty());
    let repo = parts.next().filter(|part| !part.is_empty());
    match (owner, repo, parts.next()) {
        (Some(_), Some(_), None) => Ok(slug),
        _ => Err(format!(
            "repository '{repository_url}' does not end in an owner/repo slug"
        )),
    }
}

/// Resolve the endpoint for this session: the `D4LC_UPDATE_CHECK_URL`
/// override when set to a non-empty value, otherwise the endpoint
/// resolved from the crate's `repository` manifest field.
pub fn release_api_url() -> Result<String, String> {
    match env_override_url() {
        Some(override_url) => Ok(override_url),
        None => resolve_release_api_url(env!("CARGO_PKG_REPOSITORY")),
    }
}

/// The trimmed, non-empty `D4LC_UPDATE_CHECK_URL` value.
fn env_override_url() -> Option<String> {
    std::env::var(ENV_OVERRIDE_URL)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// URL policy of the release check, applied to the request and every
/// redirect: `https://api.github.com`, plus exactly the `host:port` of the
/// endpoint override when one is set (over `http` too). A malformed
/// override adds nothing, so the fetch fails closed.
#[cfg(any(feature = "update-check", test))]
fn release_url_policy(override_url: Option<&str>) -> UrlPolicy {
    let policy = UrlPolicy::exact_hosts(RELEASE_API_HOSTS);
    match override_url.and_then(effective_authority) {
        Some(authority) => policy.with_local_authority(&authority),
        None => policy,
    }
}

/// Accept `url` as this project's release page:
/// `https://github.com/{owner}/{repo}/releases/...` with the slug of
/// `repository_url` (ASCII case-insensitive, as GitHub treats it), no
/// credentials, default port, no whitespace or control characters.
pub fn validate_release_page_url(url: &str, repository_url: &str) -> Result<(), String> {
    let prefix = format!(
        "https://github.com/{}/releases/",
        repository_slug(repository_url)?
    );
    UrlPolicy::exact_hosts(&["github.com"])
        .check(url)
        .map_err(|error| format!("release page URL refused: {error}"))?;
    let matches_prefix = url.len() > prefix.len()
        && url
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(&prefix));
    if matches_prefix {
        Ok(())
    } else {
        Err(format!("release page URL '{url}' is not under {prefix}"))
    }
}

/// Parse the bounded latest-release payload into a [`ReleaseInfo`].
/// Both top-level `tag_name` and `html_url` (the release page) must be
/// present and non-empty.
pub fn parse_release_payload(body: &str) -> Option<ReleaseInfo> {
    let document: serde_json::Value = serde_json::from_str(body).ok()?;
    let field = |key: &str| {
        document
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Some(ReleaseInfo {
        version: field("tag_name")?,
        url: field("html_url")?,
    })
}

// ---------------------------------------------------------------------------
// Network seam (single bounded GET, hard timeout, injectable for tests)
// ---------------------------------------------------------------------------

/// Release-check network seam. Production implementation is
/// [`UreqReleaseFetcher`] (gated on the `update-check` Cargo feature);
/// tests inject a mock so the default validation never touches the
/// network.
pub trait ReleaseFetcher {
    /// Fetch the release payload at `url`. Returns the bounded
    /// response body text or an error description. Implementations
    /// must not perform side effects beyond the single HTTP GET.
    fn fetch(&self, url: &str) -> Result<String, String>;
}

/// One complete release check: fetch the payload, parse it, and accept
/// its release page only under this project's GitHub releases
/// ([`validate_release_page_url`]).
pub fn check_latest_release(
    fetcher: &dyn ReleaseFetcher,
    api_url: &str,
) -> Result<ReleaseInfo, String> {
    let body = fetcher.fetch(api_url)?;
    let release = parse_release_payload(&body)
        .ok_or_else(|| format!("release payload from '{api_url}' had no tag_name/html_url"))?;
    validate_release_page_url(&release.url, env!("CARGO_PKG_REPOSITORY"))?;
    Ok(release)
}

/// `ureq`-backed [`ReleaseFetcher`]. Only compiled when the
/// `update-check` Cargo feature is enabled; disabled builds never
/// start the check thread at all (no-op, no fake banner).
#[cfg(feature = "update-check")]
pub struct UreqReleaseFetcher;

#[cfg(feature = "update-check")]
impl ReleaseFetcher for UreqReleaseFetcher {
    fn fetch(&self, url: &str) -> Result<String, String> {
        let policy = release_url_policy(env_override_url().as_deref());
        fetch_release_payload(&policy, url, CHECK_TIMEOUT)
    }
}

/// One bounded release-payload GET under `policy`: at most
/// [`MAX_PAYLOAD_BYTES`], `timeout` overall (DNS excluded), redirects
/// checked hop by hop.
#[cfg(feature = "update-check")]
fn fetch_release_payload(
    policy: &UrlPolicy,
    url: &str,
    timeout: std::time::Duration,
) -> Result<String, String> {
    let fetched = crate::actions::settings::fetch_bounded_with_timeout(
        policy,
        url,
        MAX_PAYLOAD_BYTES,
        timeout,
        RELEASE_CHECK_HEADERS,
    )
    .map_err(|error| error.to_string())?;
    String::from_utf8(fetched.bytes)
        .map_err(|_| format!("release payload from '{url}' is not UTF-8"))
}

// ---------------------------------------------------------------------------
// Wiring (background thread + UI projection)
// ---------------------------------------------------------------------------

/// Wire the check-only release banner.
///
/// Resolves the endpoint once, then starts a dedicated background
/// thread that sleeps first (the check never sits on the startup
/// path), re-reads the persisted Settings toggle every cycle, and
/// posts a newer-release finding to the UI thread via
/// `slint::invoke_from_event_loop`. Suppressed entirely by
/// `--no-updates`; every failure is logged only.
pub fn wire_update_check(
    app: &AppWindow,
    app_state: Arc<Mutex<crate::app_state::AppState>>,
    cli_suppressed: bool,
) {
    if cli_suppressed {
        eprintln!("[d4lc] update check: disabled for this session (--no-updates)");
        return;
    }
    #[cfg(feature = "update-check")]
    {
        let api_url = match release_api_url() {
            Ok(api_url) => api_url,
            Err(error) => {
                eprintln!("[d4lc] update check: not started: {error}");
                return;
            }
        };
        let app_weak = app.as_weak();
        let spawn_result = std::thread::Builder::new()
            .name("d4lc-update-check".to_string())
            .spawn(move || check_loop(app_weak, Arc::clone(&app_state), api_url));
        if let Err(error) = spawn_result {
            eprintln!(
                "[d4lc] update check: not started: could not spawn the check thread: {error}"
            );
        }
    }
    #[cfg(not(feature = "update-check"))]
    {
        let _ = (app, app_state);
        eprintln!(
            "[d4lc] update check: not started: d4lc was built without the update-check HTTP transport."
        );
    }
}

/// The background loop: one check shortly after startup, then one per
/// [`CHECK_INTERVAL`]. Exits with the process.
#[cfg(feature = "update-check")]
fn check_loop(
    app_weak: slint::Weak<AppWindow>,
    app_state: Arc<Mutex<crate::app_state::AppState>>,
    api_url: String,
) {
    std::thread::sleep(FIRST_CHECK_DELAY);
    loop {
        let setting_enabled = {
            let state = app_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // One effective switch: the upstream `CheckForUpdates` row and
            // the port-specific release-banner row both feed this gate
            // (`SettingsPanelState::update_check_effective`), so the
            // upstream toggle is honored instead of being shadowed.
            state.shell.settings.update_check_effective()
        };
        if should_run_check(setting_enabled, false) {
            run_single_check(&app_weak, &app_state, &api_url);
        }
        std::thread::sleep(CHECK_INTERVAL);
    }
}

/// Run one bounded release check and surface a newer release on the
/// UI thread. All failures stay here: log only, never block, never
/// touch the UI on error.
#[cfg(feature = "update-check")]
fn run_single_check(
    app_weak: &slint::Weak<AppWindow>,
    app_state: &Arc<Mutex<crate::app_state::AppState>>,
    api_url: &str,
) {
    let fetcher = UreqReleaseFetcher;
    match check_latest_release(&fetcher, api_url) {
        Ok(release) => {
            if !is_newer_release(APP_VERSION, &release.version) {
                eprintln!(
                    "[d4lc] update check: running {APP_VERSION} is up to date (latest release {})",
                    release.version
                );
                return;
            }
            eprintln!(
                "[d4lc] update check: release {} available at {} (running {APP_VERSION})",
                release.version, release.url
            );
            // Upstream surfaces the finding as an info line naming the
            // version (`MainWindowViewModel.cs:145-148`) and keeps
            // `ReleaseManager.UpdateAvailable` set
            // (`MainWindowViewModel.cs:143`) so the window-state and
            // launch-minimized guards see it. The port replaces the
            // updater prompt with the check-only banner, so the log line
            // carries the version string and the flag feeds the same two
            // guards.
            let app_weak = app_weak.clone();
            let app_state = Arc::clone(app_state);
            let message = format!(
                "New version available: {} (running {APP_VERSION})",
                release.version
            );
            if let Err(error) = slint::invoke_from_event_loop(move || {
                {
                    let mut state = app_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.window_lifecycle.update_pending = true;
                    state.append_log_message(message.clone(), crate::clock::hh_mm_now());
                }
                if let Some(app) = app_weak.upgrade() {
                    app.set_update_available(true);
                    app.set_update_version(release.version.clone().into());
                    app.set_update_url(release.url.clone().into());
                    crate::actions::logging::project_log_messages(
                        &app,
                        &app_state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .logging
                            .messages,
                    );
                }
            }) {
                eprintln!("[d4lc] update check: could not schedule the banner update: {error}");
            }
        }
        Err(error) => {
            eprintln!("[d4lc] update check failed (staying silent in the UI): {error}");
        }
    }
}

/// Open the release page in the user's browser. Only this project's
/// release pages ([`validate_release_page_url`]) are opened. Errors are
/// logged only; the banner never blocks on a failed open.
pub fn open_release_url(url: &str) {
    if url.trim().is_empty() {
        return;
    }
    if let Err(error) = validate_release_page_url(url, env!("CARGO_PKG_REPOSITORY")) {
        eprintln!("[d4lc] update banner: not opening: {error}");
        return;
    }
    match webbrowser::open(url) {
        Ok(()) => eprintln!("[d4lc] update banner: opened release page {url}"),
        Err(error) => eprintln!("[d4lc] update banner: could not open '{url}': {error}"),
    }
}

// ---------------------------------------------------------------------------
// Tests — fully offline. The network seam is injected; no test below
// opens a socket.
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex as TestMutex;

    /// Serializes every test that resolves the endpoint, because
    /// `release_api_url()` reads the process-global env override.
    static ENV_LOCK: TestMutex<()> = TestMutex::new(());

    // -------------------------------------------------------------------
    // compare_versions / is_newer_release
    // -------------------------------------------------------------------

    #[test]
    fn compare_versions_orders_basic_versions() {
        use Ordering::{Equal, Greater, Less};
        assert_eq!(compare_versions("1.0.0", "1.0.1"), Some(Less));
        assert_eq!(compare_versions("1.0.0", "1.1.0"), Some(Less));
        assert_eq!(compare_versions("1.0.0", "2.0.0"), Some(Less));
        assert_eq!(compare_versions("1.0.1", "1.0.0"), Some(Greater));
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Some(Equal));
    }

    #[test]
    fn compare_versions_tolerates_v_prefix_and_case() {
        use Ordering::{Equal, Greater, Less};
        assert_eq!(compare_versions("1.0.0", "v1.0.0"), Some(Equal));
        assert_eq!(compare_versions("1.0.0", "V1.0.0"), Some(Equal));
        assert_eq!(compare_versions("v1.0.0", "1.0.1"), Some(Less));
        assert_eq!(compare_versions("v1.0.0", "v1.1.0"), Some(Less));
        assert_eq!(compare_versions("v1.0.0", "v0.9.0"), Some(Greater));
    }

    #[test]
    fn compare_versions_tolerates_missing_components_and_build_metadata() {
        use Ordering::Equal;
        assert_eq!(compare_versions("1.0", "1.0.0"), Some(Equal));
        assert_eq!(compare_versions("1.0.0", "1.0.0.0"), Some(Equal));
        assert_eq!(compare_versions("1.0.0", "1.0.0+build.5"), Some(Equal));
        assert_eq!(compare_versions("1.0.0+metadata", "1.0.0"), Some(Equal));
    }

    #[test]
    fn compare_versions_orders_pre_release_tags() {
        use Ordering::{Greater, Less};
        // A pre-release sorts below its release.
        assert_eq!(compare_versions("1.1.0-rc.1", "1.1.0"), Some(Less));
        assert_eq!(compare_versions("1.0.0", "1.1.0-rc.1"), Some(Less));
        // Numeric identifiers compare numerically.
        assert_eq!(compare_versions("1.1.0-rc.2", "1.1.0-rc.1"), Some(Greater));
        // Numeric sorts below alphanumeric.
        assert_eq!(compare_versions("1.1.0-1", "1.1.0-alpha"), Some(Less));
        // Alphanumeric identifiers compare as ASCII.
        assert_eq!(
            compare_versions("1.1.0-rc.1", "1.1.0-beta.1"),
            Some(Greater)
        );
        // Shorter pre-release lists sort below longer equal prefixes.
        assert_eq!(compare_versions("1.1.0-rc", "1.1.0-rc.1"), Some(Less));
    }

    #[test]
    fn compare_versions_returns_none_for_unparsable_input() {
        assert_eq!(compare_versions("1.0.0", "not-a-version"), None);
        assert_eq!(compare_versions("garbled", "1.0.0"), None);
        assert_eq!(compare_versions("1.0.x", "1.0.0"), None);
        assert_eq!(compare_versions("", "1.0.0"), None);
    }

    #[test]
    fn is_newer_release_only_fires_for_strictly_newer_tags() {
        assert!(is_newer_release("1.0.0", "v1.0.1"));
        assert!(is_newer_release("1.0.0", "v1.1.0-rc.1"));
        assert!(!is_newer_release("1.0.0", "v1.0.0"), "equal → no banner");
        assert!(!is_newer_release("1.0.0", "v0.9.0"), "older → no banner");
        assert!(
            !is_newer_release("1.0.0", "1.0.0-rc.1"),
            "pre-release of running version → no banner"
        );
        assert!(
            !is_newer_release("1.0.0", "garbled"),
            "unparsable → no banner"
        );
    }

    #[test]
    fn running_version_compares_against_workspace_release_tags() {
        // The running binary version must be comparable to the tags
        // this project publishes.
        assert!(compare_versions(APP_VERSION, APP_VERSION).is_some());
    }

    // -------------------------------------------------------------------
    // should_run_check gating matrix
    // -------------------------------------------------------------------

    #[test]
    fn should_run_check_gating_matrix() {
        assert!(should_run_check(true, false), "default: check runs");
        assert!(!should_run_check(false, false), "settings toggle off");
        assert!(!should_run_check(true, true), "--no-updates flag");
        assert!(!should_run_check(false, true), "both suppressed");
    }

    // -------------------------------------------------------------------
    // Release source resolution
    // -------------------------------------------------------------------

    #[test]
    fn workspace_repository_resolves_to_github_latest_release_api() {
        let url = resolve_release_api_url("https://github.com/florianaltendorfer/d4lc")
            .expect("workspace repository must resolve");
        assert_eq!(
            url,
            "https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest"
        );
    }

    #[test]
    fn release_api_url_default_uses_manifest_repository() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let expected = resolve_release_api_url(env!("CARGO_PKG_REPOSITORY"))
            .expect("manifest repository must resolve");
        assert_eq!(expected, release_api_url().expect("default must resolve"));
        assert!(
            expected.starts_with("https://api.github.com/repos/"),
            "default endpoint must be the GitHub API, got: {expected}"
        );
    }

    #[test]
    fn resolve_release_api_url_tolerates_trailing_slash_and_git_suffix() {
        assert_eq!(
            resolve_release_api_url("https://github.com/florianaltendorfer/d4lc/").as_deref(),
            Ok("https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest")
        );
        assert_eq!(
            resolve_release_api_url("https://github.com/florianaltendorfer/d4lc.git").as_deref(),
            Ok("https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest")
        );
    }

    #[test]
    fn resolve_release_api_url_rejects_non_github_and_malformed_slugs() {
        for bad in [
            "https://gitlab.com/florianaltendorfer/d4lc",
            "https://github.com/florianaltendorfer",
            "https://github.com/",
            "not-a-url",
            "",
        ] {
            assert!(
                resolve_release_api_url(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn env_override_takes_precedence_over_repository() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());

        std::env::set_var(ENV_OVERRIDE_URL, "http://127.0.0.1:9/api/releases/latest");
        assert_eq!(
            release_api_url().as_deref(),
            Ok("http://127.0.0.1:9/api/releases/latest")
        );
        std::env::remove_var(ENV_OVERRIDE_URL);
        let default = release_api_url().expect("default must resolve after removing override");
        assert!(default.starts_with("https://api.github.com/repos/"));

        // Empty or whitespace overrides fall back to the repository.
        std::env::set_var(ENV_OVERRIDE_URL, "   ");
        let fallback = release_api_url().expect("blank override must fall back");
        assert!(fallback.starts_with("https://api.github.com/repos/"));
        std::env::remove_var(ENV_OVERRIDE_URL);
    }

    // -------------------------------------------------------------------
    // Payload parsing
    // -------------------------------------------------------------------

    const SAMPLE_PAYLOAD: &str = r#"{
  "url": "https://api.github.com/repos/florianaltendorfer/d4lc/releases/1",
  "html_url": "https://github.com/florianaltendorfer/d4lc/releases/tag/v1.2.0",
  "id": 1,
  "tag_name": "v1.2.0",
  "name": "d4lc 1.2.0",
  "draft": false,
  "prerelease": false,
  "assets": [
    {
      "html_url": "https://github.com/florianaltendorfer/d4lc/releases/download/v1.2.0/d4lc.tar.gz",
      "name": "d4lc.tar.gz"
    }
  ]
}"#;

    #[test]
    fn parse_release_payload_reads_tag_and_release_page() {
        let release = parse_release_payload(SAMPLE_PAYLOAD).expect("sample payload must parse");
        assert_eq!(release.version, "v1.2.0");
        // The first `html_url` occurrence is the release page, not an
        // asset download link.
        assert_eq!(
            release.url,
            "https://github.com/florianaltendorfer/d4lc/releases/tag/v1.2.0"
        );
    }

    #[test]
    fn parse_release_payload_decodes_json_escapes() {
        let body = r#"{"tag_name": "v1.2.0", "html_url": "https:\/\/github.com\/florianaltendorfer\/d4lc\/releases\/tag\/v1.2.0"}"#;
        let release = parse_release_payload(body).expect("escaped payload must parse");
        assert_eq!(release.version, "v1.2.0");
        assert_eq!(
            release.url,
            "https://github.com/florianaltendorfer/d4lc/releases/tag/v1.2.0"
        );
    }

    #[test]
    fn parse_release_payload_rejects_missing_or_empty_fields() {
        assert!(parse_release_payload(SAMPLE_PAYLOAD).is_some());
        assert_eq!(parse_release_payload("{\"html_url\": \"https://x\"}"), None);
        assert_eq!(parse_release_payload("{\"tag_name\": \"v1.2.0\"}"), None);
        assert_eq!(
            parse_release_payload("{\"tag_name\": \"\", \"html_url\": \"https://x\"}"),
            None
        );
        assert_eq!(parse_release_payload(""), None);
        assert_eq!(parse_release_payload("gateway timeout page"), None);
    }

    #[test]
    fn release_payload_reads_the_top_level_fields() {
        // `html_url` also appears in nested objects (the author); only
        // the top-level release page counts.
        let body = r#"{"author": {"html_url": "https://github.com/someone"},
            "description": "see tag_name below", "tag_name": "v2.0.0",
            "html_url": "https://github.com/o/r/releases/v2.0.0"}"#;
        let info = parse_release_payload(body).expect("valid payload");
        assert_eq!(info.version, "v2.0.0");
        assert_eq!(info.url, "https://github.com/o/r/releases/v2.0.0");
    }

    // -------------------------------------------------------------------
    // Network seam — injected mock, no socket
    // -------------------------------------------------------------------

    struct MockReleaseFetcher {
        body: Result<String, String>,
        seen_urls: TestMutex<Vec<String>>,
    }

    impl MockReleaseFetcher {
        fn ok(body: &str) -> Self {
            Self {
                body: Ok(body.to_string()),
                seen_urls: TestMutex::new(Vec::new()),
            }
        }
        fn err(message: &str) -> Self {
            Self {
                body: Err(message.to_string()),
                seen_urls: TestMutex::new(Vec::new()),
            }
        }
    }

    impl ReleaseFetcher for MockReleaseFetcher {
        fn fetch(&self, url: &str) -> Result<String, String> {
            self.seen_urls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(url.to_string());
            self.body.clone()
        }
    }

    #[test]
    fn check_latest_release_parses_a_newer_release() {
        let mock = MockReleaseFetcher::ok(SAMPLE_PAYLOAD);
        let release = check_latest_release(
            &mock,
            "https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest",
        )
        .expect("check must succeed");
        assert_eq!(release.version, "v1.2.0");
        let seen = mock.seen_urls.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0],
            "https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest"
        );
    }

    #[test]
    fn check_latest_release_propagates_transport_errors() {
        let mock = MockReleaseFetcher::err("transport error: dns failure");
        let error = check_latest_release(
            &mock,
            "https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest",
        )
        .expect_err("transport failure must propagate");
        assert!(error.contains("dns failure"), "got: {error}");
        assert_eq!(mock.seen_urls.lock().unwrap().len(), 1);
    }

    #[test]
    fn check_latest_release_rejects_unparsable_payloads() {
        let mock = MockReleaseFetcher::ok("<html>not json</html>");
        let error = check_latest_release(
            &mock,
            "https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest",
        )
        .expect_err("unparsable payload must fail");
        assert!(error.contains("tag_name"), "got: {error}");
    }

    #[test]
    fn banner_decision_end_to_end_with_injected_payload() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // Full offline path: payload → parse → compare → banner decision.
        let mock = MockReleaseFetcher::ok(SAMPLE_PAYLOAD);
        let api_url = release_api_url().expect("default endpoint must resolve");
        let release = check_latest_release(&mock, &api_url).expect("check must succeed");
        assert!(
            is_newer_release("0.0.1", &release.version),
            "a lower running version must see the banner"
        );
        assert!(
            !is_newer_release("99.0.0", &release.version),
            "a higher running version must not see the banner"
        );
    }

    // -------------------------------------------------------------------
    // Release URL policy and release page validation
    // -------------------------------------------------------------------

    #[test]
    fn release_url_policy_allows_only_https_api_github_plus_the_override() {
        let policy = release_url_policy(None);
        policy
            .check("https://api.github.com/repos/o/r/releases/latest")
            .expect("the GitHub API over https");
        for bad in [
            "http://api.github.com/repos/o/r/releases/latest",
            "https://github.com/repos/o/r/releases/latest",
            "https://user:token@api.github.com/x",
            "https://api.github.com:8443/x",
            "http://127.0.0.1:9/api/releases/latest",
            "ftp://api.github.com/x",
            "not-a-url",
            "",
        ] {
            assert!(policy.check(bad).is_err(), "{bad}");
        }
        let with_override = release_url_policy(Some("http://127.0.0.1:9/api/releases/latest"));
        with_override
            .check("http://127.0.0.1:9/other")
            .expect("exactly the override authority");
        assert!(with_override.check("http://127.0.0.1:10/other").is_err());
        assert!(with_override.check("https://evil.example/").is_err());
        let malformed = release_url_policy(Some("not a url"));
        assert!(malformed.check("http://127.0.0.1:9/").is_err());
    }

    #[test]
    fn release_page_urls_must_be_this_projects_github_releases() {
        let repository = "https://github.com/florianaltendorfer/d4lc";
        for good in [
            "https://github.com/florianaltendorfer/d4lc/releases/tag/v1.2.0",
            "https://GitHub.com/FlorianAltendorfer/d4lc/releases/tag/v1.2.0",
        ] {
            validate_release_page_url(good, repository).unwrap_or_else(|e| panic!("{good}: {e}"));
        }
        for bad in [
            "https://github.com/florianaltendorfer/d4lc/releases/",
            "https://github.com/florianaltendorfer/d4lc/issues/1",
            "https://github.com/someone/else/releases/tag/v1",
            "https://github.com/florianaltendorfer/d4lc-evil/releases/tag/v1",
            "http://github.com/florianaltendorfer/d4lc/releases/tag/v1",
            "https://github.com.evil.example/florianaltendorfer/d4lc/releases/tag/v1",
            "https://x@github.com/florianaltendorfer/d4lc/releases/tag/v1",
            "https://github.com:8443/florianaltendorfer/d4lc/releases/tag/v1",
            "https://github.com/florianaltendorfer/d4lc/releases/tag/v1 x",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "",
        ] {
            assert!(validate_release_page_url(bad, repository).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_release_with_a_foreign_page_url_is_not_reported() {
        let body = r#"{"tag_name": "v9.0.0", "html_url": "https://evil.example/download"}"#;
        let error = check_latest_release(
            &MockReleaseFetcher::ok(body),
            "https://api.github.com/repos/florianaltendorfer/d4lc/releases/latest",
        )
        .expect_err("a foreign release page must not reach the banner");
        assert!(error.contains("release page URL"), "{error}");
    }

    #[cfg(feature = "update-check")]
    mod loopback {
        use super::super::*;
        use crate::actions::settings::http_stub::{ok_with_length, redirect, Stub};
        use crate::actions::settings::UrlPolicy;
        use std::time::Duration;

        fn policy(stub: &Stub) -> UrlPolicy {
            release_url_policy(Some(&stub.url("/latest")))
        }

        #[test]
        fn the_release_fetch_is_bounded_and_redirect_checked() {
            let payload = br#"{"tag_name": "v1.0.0", "html_url": "https://github.com/o/r/releases/tag/v1.0.0"}"#;
            let oversized = vec![b' '; MAX_PAYLOAD_BYTES as usize + 1];
            let stub = Stub::start(vec![
                ok_with_length(payload.to_vec()),
                ok_with_length(oversized),
                redirect(302, "https://evil.example/latest"),
                redirect(302, "http://api.github.com/latest"),
            ]);
            let url = stub.url("/latest");
            let timeout = Duration::from_secs(10);
            assert_eq!(
                fetch_release_payload(&policy(&stub), &url, timeout)
                    .unwrap()
                    .as_bytes(),
                payload
            );
            let head = stub.heads.lock().unwrap()[0].to_ascii_lowercase();
            assert!(
                head.contains(&format!(
                    "user-agent: d4lc/{}\r\n",
                    env!("CARGO_PKG_VERSION")
                )),
                "{head}"
            );
            let error = fetch_release_payload(&policy(&stub), &url, timeout).unwrap_err();
            assert!(error.contains("response too large"), "{error}");
            for _ in 0..2 {
                let error = fetch_release_payload(&policy(&stub), &url, timeout).unwrap_err();
                assert!(error.contains("redirect refused"), "{error}");
            }
            stub.join();
        }
    }

    #[cfg(feature = "update-check")]
    #[test]
    fn ureq_fetcher_rejects_bad_urls_without_opening_a_socket() {
        let fetcher = UreqReleaseFetcher;
        let error = fetcher
            .fetch("ftp://api.github.com/releases/latest")
            .expect_err("non-http scheme must be rejected before any socket");
        assert!(error.contains("not http(s)"), "got: {error}");
    }
}
