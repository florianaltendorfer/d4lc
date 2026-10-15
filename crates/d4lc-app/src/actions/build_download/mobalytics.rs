//! Mobalytics: complete download flow (build id / slug / profile).
//!
//! Upstream `DownloadMobalyticsBuild` (BuildsManagerMobalytics.cs:431-517)
//! drives one Chrome through all three URL shapes: the CDP id flow sniffs
//! the `api/diablo4` JSON responses and accepts only the GraphQL document
//! bodies (:264); the slug and profile flows wait for
//! `window.__PRELOADED_STATE__` (WebDriverWait 10 s) and parse its
//! `diablo4State` (:454-476, :477-499). The captured bodies are the pure
//! inputs to the d4lc-rules conversion, which stores the converted entity
//! as `{builds_dir}/{id}.json` and the profile as
//! `{profiles_dir}/{display name}.json` — the same files the import path
//! and the dialog lists consume.
//!
//! Timing: the GUID flow ends the moment the `userGeneratedDocumentById`
//! body arrived; without it the 10 s inactivity bound applies once the
//! document loaded (upstream restarts the timer on every CDP response,
//! :253-258). The slug and profile flows read the server-rendered
//! preloaded state right after the load event, polled under upstream's
//! 10 s WebDriverWait bound.
//!
//! Every path that stores nothing is an error: upstream finalizes some of
//! them silently (no document, no `diablo4State`) or with a status line
//! only ("Timeout occurred.", "Failed. Invalid json."), which left the user
//! with a seemingly finished import and no build.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{
    bot_protection_message, bot_protection_page_from_session, wait_for_bot_protection_to_clear,
    BotProtectionPage, BrowserSession, CapturedResponse, ImportError, ImportErrorKind,
    MOBALYTICS_CAPTURE_TIMEOUT, MOBALYTICS_CHALLENGE_WAIT,
};

/// The response-URL substring upstream Mobalytics sniffs for
/// (`BuildsManagerMobalytics.cs:264`).
pub const MOBALYTICS_SNIFF_URL_FILTER: &str = "api/diablo4";

/// Response bodies must start with this GraphQL envelope
/// (`BuildsManagerMobalytics.cs:264`).
pub const MOBALYTICS_GRAPHQL_PREFIX: &str =
    r#"{"data":{"game":{"documents":{"userGeneratedDocumentById":"#;

/// Profile pages carry the build data in this JS global instead of the
/// GraphQL response (`BuildsManagerMobalytics.cs:454-476`); the script
/// always yields a string so `evaluate` never fails on an unset global.
pub const MOBALYTICS_PRELOADED_STATE_SCRIPT: &str =
    "String(window.__PRELOADED_STATE__ ? JSON.stringify(window.__PRELOADED_STATE__) : '')";

/// Outcome of one successful Mobalytics download (build or profile).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobalyticsDownloadOutcome {
    /// The stored JSON path (build cache or profile cache).
    pub path: PathBuf,
    /// The build / profile display name.
    pub name: String,
    /// The exported variant names (build variants, or a profile's
    /// published builds).
    pub variant_names: Vec<String>,
    /// Messages upstream reports on the Logging screen while exporting the
    /// build (`ErrorOccurredMessage`, e.g. `No nodes found for paragon
    /// board {name}.`).
    pub log_lines: Vec<String>,
}

/// Status sink of the Mobalytics flow — the same shape the import worker
/// pushes into the event buffer, mirroring the upstream
/// `MobalyticsStatusUpdateMessage` payloads.
pub type MobalyticsStatusSink<'a> = dyn Fn(&str, &str, Vec<String>) + 'a;

/// Upstream WebDriverWait timeout (`InitSelenium`,
/// BuildsManagerMobalytics.cs:411): bounds the preloaded-state poll.
const MOBALYTICS_ELEMENT_WAIT: Duration = Duration::from_secs(10);

/// Marker-poll cadence (the port's bounded equivalent of upstream's
/// `WebDriverWait.Until` re-probing).
const MOBALYTICS_POLL_STEP: Duration = Duration::from_millis(100);

/// Upstream profile-page classification (`DownloadMobalyticsBuild` :454):
/// contains `/profile/` but no `/builds/`. Callers pass the LOWERCASED url.
pub(super) fn mobalytics_is_profile_url(url: &str) -> bool {
    url.contains("/profile/") && !url.contains("/builds/")
}

/// Upstream `HasBuildUrlSlugFormat` (:1017-1038): `/builds/` without
/// `/profile/` is slug format; both words are slug format only when NO
/// `/` segment is a build id (36 characters with 4 hyphens). Callers pass
/// the LOWERCASED url.
pub(super) fn has_build_url_slug_format(url: &str) -> bool {
    let has_profile = url.contains("/profile/");
    let has_builds = url.contains("/builds/");
    if !has_profile && has_builds {
        return true;
    }
    if has_profile && has_builds {
        return !url
            .split('/')
            .any(|part| part.len() == 36 && part.matches('-').count() == 4);
    }
    false
}

/// The page the flow loads for `url` (already lowercased). A bare profile
/// page (`/profile/<user>`) is served as an empty client shell whose
/// preloaded state carries no profile queries; the site itself redirects
/// it to `/profile/<user>/builds`, whose server-rendered state carries the
/// `ngf-creator-profile-documents` and `mgp-header` queries the profile
/// parser reads (verified against the live site, 2026-10-15). Every other
/// URL is loaded as given.
pub fn mobalytics_page_url(url: &str) -> String {
    let (path, query) = match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    };
    let path = path.trim_end_matches('/');
    let bare_profile = path
        .split_once("/profile/")
        .is_some_and(|(_, rest)| !rest.is_empty() && !rest.contains('/'));
    if !bare_profile {
        return url.to_string();
    }
    match query {
        Some(query) => format!("{path}/builds?{query}"),
        None => format!("{path}/builds"),
    }
}

/// Wait for the page's `window.__PRELOADED_STATE__` and return its
/// `diablo4State` JSON (upstream WebDriverWait :459-463 / :482-486 plus
/// the `diablo4State` extraction :468-474 / :491-497). `Ok(None)` when the
/// global exists but carries no `diablo4State`.
fn mobalytics_preloaded_diablo4_state(
    session: &dyn BrowserSession,
    url: &str,
) -> Result<Option<String>, ImportError> {
    let deadline = Instant::now() + MOBALYTICS_ELEMENT_WAIT;
    let state = loop {
        let state = session
            .evaluate(MOBALYTICS_PRELOADED_STATE_SCRIPT)
            .map_err(|error| {
                ImportError::from(error).context(format!("Mobalytics preloaded state of '{url}'"))
            })?;
        if !state.is_empty() {
            break state;
        }
        if Instant::now() >= deadline {
            return Err(ImportError::new(
                ImportErrorKind::Timeout,
                format!(
                    "the Mobalytics page '{url}' never exposed window.__PRELOADED_STATE__ within \
                     {} s (upstream WebDriverWait)",
                    MOBALYTICS_ELEMENT_WAIT.as_secs()
                ),
            ));
        }
        session.pause(MOBALYTICS_POLL_STEP)?;
    };
    let value: serde_json::Value = serde_json::from_str(&state).map_err(|error| {
        ImportError::new(
            ImportErrorKind::InvalidResponse,
            format!("Mobalytics preloaded state was not valid JSON: {error}"),
        )
    })?;
    Ok(value.get("diablo4State").map(|state| state.to_string()))
}

/// Run `load` (one navigation of the page) and handle a Cloudflare page in
/// its place: a block page fails at once; a challenge is waited for while
/// the user can solve it in the shown window, or retried once headless (a
/// challenge that answered can leave its clearance cookie for the next
/// navigation). d4lc never tries to pass the check itself.
fn load_through_bot_check<T>(
    session: &dyn BrowserSession,
    url: &str,
    load: &dyn Fn() -> Result<T, ImportError>,
) -> Result<T, ImportError> {
    let shown = session.window_is_shown();
    let remedy = if shown {
        "Complete the check in the browser window; the import continues once it is gone."
    } else {
        "Enable \"Show browser\" on the Mobalytics tab to complete the check yourself, or \
         try again later."
    };
    let blocked = |page| {
        ImportError::new(
            ImportErrorKind::Blocked,
            bot_protection_message("Mobalytics", url, page, remedy),
        )
    };
    let loaded = load();
    // A timeout on a challenge page is reported as the challenge.
    if let Err(error) = &loaded {
        if error.kind != ImportErrorKind::Timeout {
            return loaded;
        }
    }
    let Some(page) = bot_protection_page_from_session(session) else {
        return loaded;
    };
    let retry = match page {
        BotProtectionPage::Blocked => false,
        BotProtectionPage::Challenge if shown => wait_for_bot_protection_to_clear(
            session,
            MOBALYTICS_CHALLENGE_WAIT,
            MOBALYTICS_POLL_STEP,
        )?,
        BotProtectionPage::Challenge => true,
    };
    if !retry {
        return Err(blocked(page));
    }
    let loaded = load();
    match bot_protection_page_from_session(session) {
        Some(page) => Err(blocked(page)),
        None => loaded,
    }
}

/// Download a Mobalytics build or profile through the injected browser
/// session and store the converted entity — build URLs as
/// `{builds_dir}/{id}.json`, profile URLs as
/// `{profiles_dir}/{display name}.json`. The upstream status sequence is
/// emitted through `push_status` verbatim. Does **not** mutate
/// `AppState`; the caller projects the outcome to the UI.
pub fn download_mobalytics_build(
    url: &str,
    builds_dir: &Path,
    profiles_dir: &Path,
    session: &dyn BrowserSession,
    repo_root: &Path,
    paragon_enabled: bool,
    push_status: &MobalyticsStatusSink<'_>,
) -> Result<MobalyticsDownloadOutcome, ImportError> {
    // Upstream lowercases the URL before every use (`buildUrl.ToLower()`):
    // navigation, the profile/slug classification, and the stored `Url`.
    let url = url.to_lowercase();
    push_status(&format!("Downloading {url}."), "", Vec::new());
    let page_url = mobalytics_page_url(&url);
    let load_error = |error: super::BrowserError| {
        ImportError::from(error).context(format!("Mobalytics page load for '{url}'"))
    };

    if mobalytics_is_profile_url(&url) {
        // Profile page: the data lives in the server-rendered preloaded
        // state (BuildsManagerMobalytics.cs:454-476).
        load_through_bot_check(session, &page_url, &|| {
            session.navigate(&page_url).map_err(load_error)
        })?;
        let started = Instant::now();
        let state = mobalytics_preloaded_diablo4_state(session, &page_url)?;
        session.record_phase("data_wait", started.elapsed());
        let started = Instant::now();
        let no_profile = || {
            ImportError::new(
                ImportErrorKind::InvalidResponse,
                format!(
                    "the Mobalytics page '{page_url}' carried no profile data (no \
                     ngf-creator-profile-documents / mgp-header query in its preloaded state)"
                ),
            )
        };
        let state = state.ok_or_else(no_profile)?;
        let profile = d4lc_rules::parse_mobalytics_profile_state(&state, &url)
            .map_err(ImportError::of(ImportErrorKind::Parse))?
            .ok_or_else(no_profile)?;
        push_status(
            &format!("Exporting {}.", profile.name),
            &profile.name,
            Vec::new(),
        );
        let mut exported = Vec::new();
        for build in &profile.variants {
            exported.push(build.name.clone());
            push_status(
                &format!("Exported {}.", build.name),
                &profile.name,
                exported.clone(),
            );
        }
        session.record_phase("extract", started.elapsed());
        let started = Instant::now();
        let path = d4lc_rules::export_mobalytics_profile_cache(&profile, profiles_dir)
            .map_err(ImportError::of(ImportErrorKind::Storage))?;
        session.record_phase("store", started.elapsed());
        Ok(MobalyticsDownloadOutcome {
            path,
            name: profile.name.clone(),
            variant_names: exported,
            log_lines: Vec::new(),
        })
    } else if has_build_url_slug_format(&url) {
        // Build page with slug name: the document lives in the
        // server-rendered preloaded state (BuildsManagerMobalytics.cs:477-499).
        load_through_bot_check(session, &page_url, &|| {
            session.navigate(&page_url).map_err(load_error)
        })?;
        let started = Instant::now();
        let state = mobalytics_preloaded_diablo4_state(session, &page_url)?;
        session.record_phase("data_wait", started.elapsed());
        let no_document = || {
            ImportError::new(
                ImportErrorKind::InvalidResponse,
                format!(
                    "the Mobalytics page '{page_url}' carried no build document in its preloaded \
                     state (is the URL a build page?)"
                ),
            )
        };
        let state = state.ok_or_else(no_document)?;
        convert_and_store_mobalytics_build(
            session,
            &state,
            &url,
            builds_dir,
            repo_root,
            paragon_enabled,
            push_status,
        )?
        .ok_or_else(no_document)
    } else {
        // Build page by id: the page fetches the document over GraphQL
        // (CDP flow, BuildsManagerMobalytics.cs:264).
        let accept = |body: &str| body.starts_with(MOBALYTICS_GRAPHQL_PREFIX);
        let bodies = load_through_bot_check(session, &page_url, &|| {
            session
                .navigate_collecting_json(
                    &page_url,
                    MOBALYTICS_SNIFF_URL_FILTER,
                    MOBALYTICS_CAPTURE_TIMEOUT,
                    Some(&accept),
                )
                .map_err(load_error)
        })?;
        mobalytics_outcome_from_sniffed_bodies(
            session,
            &bodies,
            &url,
            builds_dir,
            repo_root,
            paragon_enabled,
            push_status,
        )
    }
}

/// The CDP id flow: accept only the GraphQL document bodies
/// (BuildsManagerMobalytics.cs:264).
fn mobalytics_outcome_from_sniffed_bodies(
    session: &dyn BrowserSession,
    bodies: &[CapturedResponse],
    url: &str,
    builds_dir: &Path,
    repo_root: &Path,
    paragon_enabled: bool,
    push_status: &MobalyticsStatusSink<'_>,
) -> Result<MobalyticsDownloadOutcome, ImportError> {
    let Some(matched) = bodies
        .iter()
        .find(|captured| captured.body.starts_with(MOBALYTICS_GRAPHQL_PREFIX))
    else {
        return Err(ImportError::new(
            ImportErrorKind::Timeout,
            format!(
                "Mobalytics sent no build document (userGeneratedDocumentById) for '{url}' \
                 before the page went quiet ({} api/diablo4 JSON response(s) of other shapes)",
                bodies.len()
            ),
        ));
    };
    convert_and_store_mobalytics_build(
        session,
        &matched.body,
        url,
        builds_dir,
        repo_root,
        paragon_enabled,
        push_status,
    )?
    .ok_or_else(|| {
        ImportError::new(
            ImportErrorKind::InvalidResponse,
            format!(
                "the Mobalytics build document for '{url}' was empty (deleted or private build?)"
            ),
        )
    })
}

/// Convert a captured Mobalytics GraphQL body / preloaded state into the
/// stored entity and persist it — the pure half of the build flows, split
/// out so the live smoke can dump the captured input before converting.
/// Emits the upstream `Exporting {name}.` / `Exporting {v}.` /
/// `Exported {v}.` / `Converting {v}.` / `Converted {v}.` statuses.
/// `Ok(None)` when the input carries no usable document (upstream
/// `ParseJsonBuildBySlug` skips the null document).
fn convert_and_store_mobalytics_build(
    session: &dyn BrowserSession,
    graphql_json: &str,
    build_url: &str,
    builds_dir: &Path,
    repo_root: &Path,
    paragon_enabled: bool,
    push_status: &MobalyticsStatusSink<'_>,
) -> Result<Option<MobalyticsDownloadOutcome>, ImportError> {
    let started = Instant::now();
    let Some((build, log_lines)) = d4lc_rules::parse_mobalytics_graphql_build_with_warnings(
        graphql_json,
        build_url,
        paragon_enabled,
    )
    .map_err(ImportError::of(ImportErrorKind::Parse))?
    else {
        return Ok(None);
    };
    if build.variants.is_empty() {
        return Err(ImportError::new(
            ImportErrorKind::InvalidResponse,
            format!("the Mobalytics build '{}' has no variants", build.name),
        ));
    }

    push_status(
        &format!("Exporting {}.", build.name),
        &build.name,
        Vec::new(),
    );
    for variant in &build.variants {
        push_status(
            &format!("Exporting {}.", variant.name),
            &build.name,
            vec![variant.name.clone()],
        );
        push_status(
            &format!("Exported {}.", variant.name),
            &build.name,
            vec![variant.name.clone()],
        );
    }
    session.record_phase("extract", started.elapsed());

    let started = Instant::now();
    let repo = d4lc_assets::AssetRepository::new(repo_root);
    let lookups = d4lc_rules::MobalyticsConversionLookups::load(&repo).map_err(|error| {
        ImportError::new(
            ImportErrorKind::Parse,
            format!("Mobalytics conversion data unavailable for '{build_url}': {error}"),
        )
    })?;
    let mut warning_count = 0usize;
    for variant in &build.variants {
        push_status(
            &format!("Converting {}.", variant.name),
            &build.name,
            Vec::new(),
        );
        let converted = lookups
            .convert_variant(variant)
            .map_err(ImportError::of(ImportErrorKind::Parse))?;
        warning_count += converted.warnings.len();
        push_status(
            &format!("Converted {}.", variant.name),
            &build.name,
            Vec::new(),
        );
    }
    if warning_count > 0 {
        // Upstream reports nothing for site texts that do not map, so this
        // stays a diagnostic on stderr only.
        eprintln!(
            "[d4lc] mobalytics download '{build_url}': {warning_count} site text(s) did not \
             map to canonical ids"
        );
    }
    session.record_phase("convert", started.elapsed());

    let started = Instant::now();
    let variant_names = build
        .variants
        .iter()
        .map(|variant| variant.name.clone())
        .collect();
    let path = d4lc_rules::export_mobalytics_build_cache(&build, builds_dir)
        .map_err(ImportError::of(ImportErrorKind::Storage))?;
    session.record_phase("store", started.elapsed());
    Ok(Some(MobalyticsDownloadOutcome {
        path,
        name: build.name.clone(),
        variant_names,
        log_lines,
    }))
}
