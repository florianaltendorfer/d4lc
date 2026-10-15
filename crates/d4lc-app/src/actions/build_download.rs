//! Headless-browser build downloads (D2Core, D4Builds, Mobalytics).
//!
//! Upstream downloads the D2Core, D4Builds, and Mobalytics builds by
//! driving a headless Chrome through Selenium/DevTools (CDP) and sniffing
//! the page's network traffic — the planner / build pages are
//! client-rendered shells, so no direct-HTTP shortcut exists. The port
//! mirrors those mechanics:
//!
//! - [`BrowserSession`] is the injectable seam (navigate + collect
//!   matching JSON response bodies / evaluate JS). The production
//!   Chromium implementation lives behind the `browser-download` cargo
//!   feature; tests inject a fake session so the default suite stays
//!   network- and chrome-free.
//! - D2Core: navigate `https://www.d2core.com/d4/planner?bd={id}`, sniff
//!   the `web?env=diablocore` JSON responses, accept only the body carrying
//!   `{"response_data":"{\"data\":{\"_id\":\"{id}\"` (the planner RPC
//!   response) and store the inner `response_data` string verbatim as
//!   `{builds_dir}/{id}.json` — the shape `d4lc_rules::import_d2core_build_variant`
//!   parses. Upstream: `BuildsManagerD2Core.cs` (navigate :532, sniffing
//!   :173, envelope gate :203, verbatim store :213-214, 10 s inactivity
//!   :52-53).
//! - D4Builds: navigate `https://d4builds.gg/builds/{id}/?var=0`, wait for
//!   the build-name marker (upstream WebDriverWait 10 s) and for the
//!   rendered content (bounded by upstream's 5 s render sleep), then walk
//!   the variant dropdown and the `builder__navigation__link` tabs into
//!   `d4lc_rules::D4BuildsSiteBuild`; the conversion is pure d4lc-rules
//!   (`convert_d4builds_site_variants`). Upstream: `BuildsManagerD4Builds.cs`
//!   (navigate :302, marker :306, sleep :316, dropdown walk :610-635, tab
//!   walk :637-714, paragon :822-913).
//! - Mobalytics: `/builds/{GUID}` pages sniff the `api/diablo4` JSON
//!   responses and accept the `userGeneratedDocumentById` GraphQL body;
//!   slug and profile pages read `window.__PRELOADED_STATE__.diablo4State`
//!   (a bare `/profile/<user>` URL loads its `/builds` page, the only one
//!   whose server-rendered state carries the profile). The pure d4lc-rules
//!   conversion stores builds as `{builds_dir}/{id}.json` and profiles as
//!   `{profiles_dir}/{display name}.json`. Upstream:
//!   `BuildsManagerMobalytics.cs` (navigate :452, CDP acceptance :264,
//!   profile flow :454-476, slug flow :477-499, `HasBuildUrlSlugFormat`
//!   :1017-1038, 10 s inactivity :253-258).
//!
//! Every capture ends as soon as the needed response is complete; the
//! inactivity timer only bounds the give-up path. Each session belongs to
//! one import job ([`ImportControl`]): a cancel ends every browser call and
//! wait at once, and failures carry an [`ImportErrorKind`] for the dialog.
//!
//! Without the `browser-download` feature the UI handlers report the
//! structured capability gap ("rebuild with --features
//! browser-download") after the URL / id gates have run — no fake
//! download, no fake capture.

use std::time::Duration;

// ---------------------------------------------------------------------------
// Browser seam.
// ---------------------------------------------------------------------------

/// Name of the Chromium binary override, also quoted in the
/// browser-not-found message of every build.
const CHROME_BIN_ENV_NAME: &str = "D4LC_CHROME_BIN";

/// Environment override for the Chromium binary. Unset or empty falls
/// back to the standard browser discovery; a set value must name an
/// existing file.
#[cfg(feature = "browser-download")]
pub const CHROME_BIN_ENV: &str = CHROME_BIN_ENV_NAME;

/// Why a browser-driven flow could not run.
///
/// The `allow(dead_code)` covers the default non-test build, where only
/// [`BrowserError::FeatureDisabled`] is constructed: `BrowserNotFound` /
/// `Navigation` / `Timeout` are constructed by the feature-gated
/// Chromium implementation, and the fake sessions construct them in
/// tests.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserError {
    /// The build was compiled without the `browser-download` feature;
    /// the message names the rebuild command.
    FeatureDisabled,
    /// No Chromium/Chrome binary was found: discovery failed, or the
    /// `D4LC_CHROME_BIN` override names no binary. `override_set` says
    /// whether the override was in effect (the message then does not ask
    /// to set it).
    BrowserNotFound { detail: String, override_set: bool },
    /// The browser or its CDP session failed (launch, command, evaluate).
    Session(String),
    /// Navigation to the page failed.
    Navigation(String),
    /// The inactivity timeout elapsed without the needed traffic.
    Timeout { url: String, inactivity_secs: u64 },
    /// The page never fired its load event within the CDP request timeout.
    PageLoadTimeout { url: String },
    /// The import job was cancelled while the browser worked.
    Cancelled,
}

impl From<Cancelled> for BrowserError {
    fn from(_: Cancelled) -> Self {
        BrowserError::Cancelled
    }
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserError::FeatureDisabled => write!(
                f,
                "the `browser-download` feature (headless Chromium) is not enabled in this \
                 build; rebuild with --features browser-download"
            ),
            BrowserError::BrowserNotFound {
                detail,
                override_set: true,
            } => write!(f, "no Chromium/Chrome binary found: {detail}"),
            BrowserError::BrowserNotFound {
                detail,
                override_set: false,
            } => write!(
                f,
                "no Chromium/Chrome binary found (set {CHROME_BIN_ENV_NAME}): {detail}"
            ),
            BrowserError::Session(message) => write!(f, "browser session error: {message}"),
            BrowserError::Navigation(message) => write!(f, "navigation error: {message}"),
            BrowserError::Timeout {
                url,
                inactivity_secs,
            } => write!(
                f,
                "no JSON response within {inactivity_secs}s of inactivity for {url} (upstream \
                 10s inactivity timer)"
            ),
            BrowserError::PageLoadTimeout { url } => {
                write!(f, "{url} did not finish loading within 30 s")
            }
            BrowserError::Cancelled => write!(f, "the import was cancelled"),
        }
    }
}

impl std::error::Error for BrowserError {}

/// What went wrong in an import, as far as the user can act on it. The
/// dialog shows the kind as the error headline and the message below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportErrorKind {
    /// The user cancelled the import.
    Cancelled,
    /// No usable Chromium/Chrome (missing binary or feature).
    BrowserUnavailable,
    /// The browser process or its DevTools session failed.
    Browser,
    /// The provider could not be reached (DNS, TLS, HTTP error status).
    Network,
    /// The provider did not deliver the build data in time.
    Timeout,
    /// The provider refused automated access (bot protection).
    Blocked,
    /// The entered URL / id is not a build of this provider.
    InvalidUrl,
    /// The provider answered, but not with the expected build data.
    InvalidResponse,
    /// The build data could not be read or converted.
    Parse,
    /// The build could not be saved.
    Storage,
    /// A defect in d4lc itself (worker panic, failed thread spawn).
    Internal,
}

impl ImportErrorKind {
    /// Short English headline for the progress dialog.
    pub fn headline(self) -> &'static str {
        match self {
            ImportErrorKind::Cancelled => "Import cancelled",
            ImportErrorKind::BrowserUnavailable => "Browser not available",
            ImportErrorKind::Browser => "Browser error",
            ImportErrorKind::Network => "Network error",
            ImportErrorKind::Timeout => "Timed out",
            ImportErrorKind::Blocked => "Blocked by the provider",
            ImportErrorKind::InvalidUrl => "Invalid build URL",
            ImportErrorKind::InvalidResponse => "Unexpected provider response",
            ImportErrorKind::Parse => "Could not read the build",
            ImportErrorKind::Storage => "Could not save the build",
            ImportErrorKind::Internal => "Internal error",
        }
    }

    /// Stable machine name for the import diagnostics log.
    pub fn code(self) -> &'static str {
        match self {
            ImportErrorKind::Cancelled => "cancelled",
            ImportErrorKind::BrowserUnavailable => "browser_unavailable",
            ImportErrorKind::Browser => "browser",
            ImportErrorKind::Network => "network",
            ImportErrorKind::Timeout => "timeout",
            ImportErrorKind::Blocked => "blocked",
            ImportErrorKind::InvalidUrl => "invalid_url",
            ImportErrorKind::InvalidResponse => "invalid_response",
            ImportErrorKind::Parse => "parse",
            ImportErrorKind::Storage => "storage",
            ImportErrorKind::Internal => "internal",
        }
    }
}

/// A classified import failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    pub kind: ImportErrorKind,
    pub message: String,
}

impl ImportError {
    pub fn new(kind: ImportErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn cancelled() -> Self {
        Self::new(ImportErrorKind::Cancelled, "the import was cancelled")
    }

    /// A failure with this kind (closure shorthand for `map_err`).
    pub fn of(kind: ImportErrorKind) -> impl Fn(String) -> Self {
        move |message| Self::new(kind, message)
    }

    /// Prefix the message with what was being done, keeping the kind.
    pub fn context(self, doing: impl std::fmt::Display) -> Self {
        if self.kind == ImportErrorKind::Cancelled {
            return self;
        }
        Self {
            kind: self.kind,
            message: format!("{doing}: {}", self.message),
        }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

impl From<BrowserError> for ImportError {
    fn from(error: BrowserError) -> Self {
        let kind = match &error {
            BrowserError::FeatureDisabled | BrowserError::BrowserNotFound { .. } => {
                ImportErrorKind::BrowserUnavailable
            }
            BrowserError::Session(_) => ImportErrorKind::Browser,
            BrowserError::Navigation(_) => ImportErrorKind::Network,
            BrowserError::Timeout { .. } | BrowserError::PageLoadTimeout { .. } => {
                ImportErrorKind::Timeout
            }
            BrowserError::Cancelled => ImportErrorKind::Cancelled,
        };
        Self::new(kind, error.to_string())
    }
}

impl From<Cancelled> for ImportError {
    fn from(_: Cancelled) -> Self {
        Self::cancelled()
    }
}

/// One application/json response captured while the page loaded — the
/// CDP `Network.responseReceived` + `GetResponseBody` pair upstream
/// collects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedResponse {
    pub url: String,
    pub mime_type: String,
    pub body: String,
}

/// Injectable browser seam — the analog of upstream's Selenium +
/// DevTools session.
///
/// Implementations drive one browser: navigate a tab, record every
/// application/json response body whose URL contains `url_filter` (an
/// empty filter records every JSON response), and evaluate JavaScript in
/// the page context.
///
/// The collection ends as soon as a recorded body satisfies `accept` —
/// the needed data is complete, so analytics or polling traffic cannot
/// delay the import. Without an accepted body it ends after the document
/// loaded and `inactivity` passed without a new network response
/// (upstream's inactivity timer), or at an overall deadline. The
/// production session also caps the recorded responses and body bytes
/// (`chromium::CollectionLimits`).
pub trait BrowserSession {
    fn navigate_collecting_json(
        &self,
        url: &str,
        url_filter: &str,
        inactivity: Duration,
        accept: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Vec<CapturedResponse>, BrowserError>;

    /// Navigate and return once the document's load event fired, keeping
    /// no response body. The default collects every JSON response and
    /// drops it; the production session waits for the load event only.
    fn navigate(&self, url: &str) -> Result<(), BrowserError> {
        self.navigate_collecting_json(url, "", Duration::ZERO, None)
            .map(drop)
    }

    /// Sleep between page steps. The production session wakes early and
    /// fails with [`BrowserError::Cancelled`] when the job is cancelled.
    fn pause(&self, duration: Duration) -> Result<(), BrowserError> {
        std::thread::sleep(duration);
        Ok(())
    }

    /// Record one measured flow phase in the job's import diagnostics.
    fn record_phase(&self, phase: &'static str, duration: Duration) {
        let _ = (phase, duration);
    }

    /// Evaluate a JavaScript expression in the open page and return its
    /// string result (upstream `ExecuteScript`, used for
    /// `window.__PRELOADED_STATE__`).
    fn evaluate(&self, script: &str) -> Result<String, BrowserError>;

    /// Whether this session's browser window is visible to the user (the
    /// Mobalytics "show browser" setting). The Mobalytics flow waits for a
    /// manual bot-protection check only when the window is shown.
    fn window_is_shown(&self) -> bool {
        false
    }
}

/// Which upstream manager a browser session serves. Upstream configures
/// per-manager Chrome options in each `InitSelenium`
/// (`BuildsManagerD2Core.cs:238-278`, `BuildsManagerD4Builds.cs:234-266`,
/// `BuildsManagerMobalytics.cs:371-416`): all flows share the headless
/// base flag set, the cache disables belong to the D2Core and Mobalytics
/// managers, and the `Diablo4Companion/1.0` user agent belongs to
/// Mobalytics only (`BuildsManagerMobalytics.cs:403`; InfinityBuilds'
/// equivalent is commented out upstream). The Mobalytics variant carries
/// the `IsShowBrowserMobalyticsEnabled` decision: upstream SKIPS the
/// headless argument when the show-browser setting is on
/// (`BuildsManagerMobalytics.cs:376-385`), so the flow launches headful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserLaunchProfile {
    D2Core,
    D4Builds,
    Mobalytics { headless: bool },
}

/// The Mobalytics launch profile for a show-browser setting (upstream
/// `InitSelenium`: `--headless` is added only when
/// `IsShowBrowserMobalyticsEnabled` is false).
pub fn mobalytics_launch_profile(show_browser: bool) -> BrowserLaunchProfile {
    BrowserLaunchProfile::Mobalytics {
        headless: !show_browser,
    }
}

impl BrowserLaunchProfile {
    /// The upstream Chrome arguments beyond the shared base flags.
    fn extra_args(self) -> &'static [&'static str] {
        match self {
            BrowserLaunchProfile::D2Core => &[
                // Cache related settings (BuildsManagerD2Core.cs:271-274).
                "--disable-cache",
                "--disk-cache-size=0",
                "--media-cache-size=0",
            ],
            BrowserLaunchProfile::D4Builds => &[],
            BrowserLaunchProfile::Mobalytics { .. } => &[
                // Cache related settings plus the explicit user agent
                // (BuildsManagerMobalytics.cs:398-403).
                "--disable-cache",
                "--disk-cache-size=0",
                "--media-cache-size=0",
                "--user-agent=Diablo4Companion/1.0",
            ],
        }
    }
}

/// Shared browser flags from upstream's `InitSelenium`
/// (`BuildsManagerD2Core.cs:241-268` and siblings), except `--no-sandbox`:
/// imported pages must run inside Chromium's default sandbox.
/// Upstream drives the locally installed Chrome through Selenium's
/// chromedriver (Selenium.WebDriver 4.49 provisions the driver via
/// Selenium Manager and never bundles a browser); this port drives the
/// same locally installed browser directly over CDP — chromiumoxide's
/// `fetcher` feature stays off, so no browser is ever downloaded, and
/// the binary comes from `D4LC_CHROME_BIN` or the standard discovery,
/// mirroring Selenium's installed-Chrome lookup.
const BASE_CHROME_ARGS: &[&str] = &[
    "--disable-gpu",
    "--disable-extensions",
    "--disable-popup-blocking",
    "--disable-notifications",
    "--dns-prefetch-disable",
    "--disable-dev-shm-usage",
    "--window-size=1600,900",
];

/// Keeps the invisible window off the desktop. It applies to headless
/// sessions only: the shown Mobalytics window has to stay visible so the
/// user can complete a bot-protection check (upstream keeps the off-screen
/// position for both, which leaves the shown window off-screen).
const HEADLESS_WINDOW_POSITION: &str = "--window-position=-32000,-32000";

/// The browser flags for `profile`: the shared base set, the off-screen
/// position for headless sessions, then the profile's own flags.
pub fn session_chrome_args(profile: BrowserLaunchProfile) -> Vec<&'static str> {
    let headless = match profile {
        BrowserLaunchProfile::Mobalytics { headless } => headless,
        _ => true,
    };
    let mut args = BASE_CHROME_ARGS.to_vec();
    if headless {
        args.push(HEADLESS_WINDOW_POSITION);
    }
    args.extend(profile.extra_args());
    args
}

/// Launch the production browser session for `profile`, bound to the
/// import job's `control` (cancellation and phase timings).
///
/// With the `browser-download` feature this launches headless Chromium
/// over CDP with the upstream flag set for that provider (binary: the
/// `D4LC_CHROME_BIN` override, then the standard browser discovery).
/// Without it this returns the [`BrowserError::FeatureDisabled`]
/// capability gap.
pub fn launch_chrome_session(
    profile: BrowserLaunchProfile,
    control: &ImportControl,
) -> Result<Box<dyn BrowserSession>, BrowserError> {
    #[cfg(feature = "browser-download")]
    {
        Ok(Box::new(ChromiumSession::launch(profile, control.clone())?))
    }
    #[cfg(not(feature = "browser-download"))]
    {
        let _ = (profile, control);
        Err(BrowserError::FeatureDisabled)
    }
}

/// Shared capture inactivity policy: upstream Mobalytics restarts its
/// 10 s inactivity timer on every CDP response
/// (`BuildsManagerMobalytics.cs:253-258`) and upstream D2Core uses a fixed
/// 10 s timer (`BuildsManagerD2Core.cs:52-53`). The port only applies it
/// as the give-up bound once the document loaded: an accepted response
/// ends the capture at once.
pub const MOBALYTICS_CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a shown Mobalytics window waits for the user to complete a
/// bot-protection check before the challenge is reported.
pub const MOBALYTICS_CHALLENGE_WAIT: Duration = Duration::from_secs(120);

/// Reads the open page's title and the first 64 KiB of its markup for
/// [`bot_protection_page`]. Always yields a JSON string.
const BOT_PROTECTION_PROBE_SCRIPT: &str = "JSON.stringify({title: String(document.title || ''), \
     html: document.documentElement ? document.documentElement.outerHTML.slice(0, 65536) : ''})";

/// The probe answer of [`BOT_PROTECTION_PROBE_SCRIPT`].
#[derive(serde::Deserialize)]
struct BotProtectionProbe {
    #[serde(default)]
    title: String,
    #[serde(default)]
    html: String,
}

/// A Cloudflare interstitial served instead of the provider page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BotProtectionPage {
    /// The "Just a moment..." challenge; it may pass on a later visit.
    Challenge,
    /// The "Attention Required!" block page; retrying does not help.
    Blocked,
}

/// Classify a page as a Cloudflare interstitial. Blocked: the block-page
/// title. Challenge: the challenge title, or the challenge-page bootstrap
/// (`window._cf_chl_opt`, the `orchestrate/chl_page` script). Cloudflare's
/// passive `/cdn-cgi/challenge-platform/scripts/jsd/` beacon also appears on
/// normal pages and is deliberately not a marker.
fn bot_protection_page(title: &str, html: &str) -> Option<BotProtectionPage> {
    let title = title.trim();
    if title.starts_with("Attention Required! | Cloudflare") {
        Some(BotProtectionPage::Blocked)
    } else if title.eq_ignore_ascii_case("Just a moment...")
        || html.contains("window._cf_chl_opt")
        || html.contains("/orchestrate/chl_page/")
    {
        Some(BotProtectionPage::Challenge)
    } else {
        None
    }
}

/// `Some(message)` when the open page is a bot-protection page. The
/// flows call this on their failure paths so the user sees the real
/// cause instead of a timeout. A failing probe yields `None`: it only
/// refines an error that is reported either way. d4lc never tries to pass
/// the challenge.
fn bot_protection_failure(
    session: &dyn BrowserSession,
    provider: &str,
    url: &str,
    remedy: &str,
) -> Option<String> {
    Some(bot_protection_message(
        provider,
        url,
        bot_protection_page_from_session(session)?,
        remedy,
    ))
}

/// The open page's bot-protection state, or `None` for a normal page. A
/// failing probe also yields `None`: it only refines an error that is
/// reported either way.
fn bot_protection_page_from_session(session: &dyn BrowserSession) -> Option<BotProtectionPage> {
    let answer = session.evaluate(BOT_PROTECTION_PROBE_SCRIPT).ok()?;
    let probe: BotProtectionProbe = serde_json::from_str(&answer).ok()?;
    bot_protection_page(&probe.title, &probe.html)
}

/// Poll the open page until the bot-protection challenge is gone or
/// `budget` elapses — the user completes a check in the shown window.
/// `true` when the page is no longer a challenge.
fn wait_for_bot_protection_to_clear(
    session: &dyn BrowserSession,
    budget: Duration,
    poll_step: Duration,
) -> Result<bool, BrowserError> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        match bot_protection_page_from_session(session) {
            None => return Ok(true),
            Some(BotProtectionPage::Blocked) => return Ok(false),
            Some(BotProtectionPage::Challenge) => {}
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        session.pause(poll_step)?;
    }
}

/// The user-facing message for a bot-protection page. `remedy` names what
/// the user can do about a challenge; a block page has no remedy.
fn bot_protection_message(
    provider: &str,
    url: &str,
    page: BotProtectionPage,
    remedy: &str,
) -> String {
    match page {
        BotProtectionPage::Challenge => format!(
            "{provider} answered {url} with a bot-protection challenge (Cloudflare \
             \"Just a moment...\" page) instead of the build page; d4lc does not bypass bot \
             protection. {remedy}"
        ),
        BotProtectionPage::Blocked => format!(
            "{provider} answered {url} with a Cloudflare block page: access blocked by bot \
             protection; d4lc does not bypass bot protection"
        ),
    }
}

mod control;
mod d2core;
mod d4builds;
mod mobalytics;

pub use control::{CancelOutcome, Cancelled, ImportControl, PhaseTiming};
pub use d2core::*;
pub use d4builds::*;
pub use mobalytics::*;

#[cfg(feature = "browser-download")]
mod chromium;

#[cfg(feature = "browser-download")]
use chromium::ChromiumSession;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
