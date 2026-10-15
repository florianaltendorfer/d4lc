//! Production browser session (`browser-download` feature).
//!
//! chromiumoxide drives Chromium over CDP. Its runtime is tokio-only
//! (async-tungstenite `tokio-runtime`, tokio rt/time/process — crates.io
//! metadata for 0.9.1), so — like the ksni tray — the session owns a
//! private current-thread Tokio runtime. Browser calls block the download
//! worker, not the Slint event loop. The workspace zbus keeps its
//! async-io flavor process-wide: no dependency in this feature tree
//! enables zbus's `tokio` feature (see the ksni note in
//! `Cargo.toml`), and no tokio context is installed on the Slint threads.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::{BrowserError, CapturedResponse, ImportControl, CHROME_BIN_ENV};
use chromiumoxide::cdp::browser_protocol::network::{
    EventLoadingFinished, EventResponseReceived, GetResponseBodyParams, RequestId,
};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::future::Either;
use futures::StreamExt;

/// Launch flags retain upstream's cache, headless, GPU, /dev/shm and
/// window-size settings, but keep Chromium's sandbox enabled:
/// headless is chromiumoxide's launch default, the rest are passed
/// verbatim. The binary comes from `D4LC_CHROME_BIN` (override) or
/// the standard browser discovery.
pub struct ChromiumSession {
    runtime: tokio::runtime::Runtime,
    browser: Browser,
    page: RefCell<Option<Page>>,
    /// Whether the launch profile asked for a visible window.
    window_is_shown: bool,
    /// Process id of the spawned browser: a browser that dies mid-call
    /// fails the call at once instead of after the CDP request timeout.
    browser_pid: Option<u32>,
    /// The import job this session works for: every browser call ends
    /// with [`BrowserError::Cancelled`] once the job is cancelled.
    control: ImportControl,
    // Keep the private profile until the browser handle has been dropped.
    _profile: tempfile::TempDir,
}

use super::{session_chrome_args, BrowserLaunchProfile};

/// Split a `--key[=value]` flag into the parts chromiumoxide expects: its
/// `Arg` adds the `--` prefix itself, so passing the full flag produced
/// `----key=value`, which Chromium ignores (every launch flag, including
/// the Mobalytics user agent, was silently dropped).
pub(super) fn chrome_arg_parts(flag: &str) -> (&str, Option<&str>) {
    let flag = flag.strip_prefix("--").unwrap_or(flag);
    match flag.split_once('=') {
        Some((key, value)) => (key, Some(value)),
        None => (flag, None),
    }
}

impl ChromiumSession {
    pub fn launch(
        profile: BrowserLaunchProfile,
        control: ImportControl,
    ) -> Result<Self, BrowserError> {
        control.check()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| BrowserError::Session(format!("tokio runtime: {error}")))?;
        let profile_dir = tempfile::tempdir()
            .map_err(|error| BrowserError::Session(format!("browser profile: {error}")))?;
        let mut config = BrowserConfig::builder().user_data_dir(profile_dir.path());
        let binary_override = chrome_binary_override(
            std::env::var(CHROME_BIN_ENV).ok().as_deref(),
            std::env::var_os("PATH").as_deref(),
        )?;
        let override_set = binary_override.is_some();
        if let Some(binary) = binary_override {
            config = config.chrome_executable(binary);
        }
        // Upstream skips the `--headless` argument when the Mobalytics
        // show-browser setting is on (InitSelenium); chromiumoxide
        // launches headless by default, so headful needs the explicit
        // `with_head` opt-in.
        let headless = match profile {
            BrowserLaunchProfile::Mobalytics { headless } => headless,
            _ => true,
        };
        if !headless {
            config = config.with_head();
        }
        for flag in session_chrome_args(profile) {
            config = match chrome_arg_parts(flag) {
                (key, Some(value)) => config.arg((key, value)),
                (key, None) => config.arg(key),
            };
        }
        // `build` fails only when the standard discovery finds no binary.
        let config = config
            .build()
            .map_err(|detail| BrowserError::BrowserNotFound {
                detail,
                override_set: false,
            })?;
        let mut browser = runtime
            .block_on(async {
                // `Browser::launch` returns the browser plus its CDP
                // `Handler`; the handler must be driven for the
                // session lifetime, spawned on this private runtime
                // so it dies with the runtime when the session drops.
                let launch = Box::pin(Browser::launch(config));
                let cancelled = Box::pin(job_cancelled(&control));
                let (browser, mut handler) = match futures::future::select(launch, cancelled).await
                {
                    Either::Left((launched, _)) => {
                        launched.map_err(|error| Some(error.to_string()))?
                    }
                    // Dropping the launch future kills the spawned
                    // child (`kill_on_drop`).
                    Either::Right(((), _)) => return Err(None),
                };
                tokio::task::spawn(async move { while handler.next().await.is_some() {} });
                Ok::<Browser, Option<String>>(browser)
            })
            .map_err(|message| match message {
                Some(message) => map_launch_error(message, override_set),
                None => BrowserError::Cancelled,
            })?;
        let browser_pid = browser
            .get_mut_child()
            .and_then(|child| child.as_mut_inner().id());
        Ok(Self {
            runtime,
            browser,
            page: RefCell::new(None),
            window_is_shown: !headless,
            browser_pid,
            control,
            _profile: profile_dir,
        })
    }
}

/// The `D4LC_CHROME_BIN` override: `Ok(None)` when unset or blank (the
/// standard discovery runs). A value with a `/` is a path and must name an
/// existing file; a bare command name (`chromium`) is looked up in
/// `search_path` (`$PATH`) like the process spawn would, skipping empty
/// entries, and resolves to the first executable regular file. Anything
/// else is [`BrowserError::BrowserNotFound`], so a mistyped override never
/// surfaces as a generic spawn failure.
pub(super) fn chrome_binary_override(
    value: Option<&str>,
    search_path: Option<&OsStr>,
) -> Result<Option<PathBuf>, BrowserError> {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let not_found = |reason: &str| BrowserError::BrowserNotFound {
        detail: format!("{CHROME_BIN_ENV}={raw} {reason}"),
        override_set: true,
    };
    if raw.contains('/') {
        let path = PathBuf::from(raw);
        return if path.is_file() {
            Ok(Some(path))
        } else {
            Err(not_found("is not an existing file"))
        };
    }
    let is_executable_file = |path: &std::path::Path| {
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    search_path
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(raw))
        .find(|candidate| is_executable_file(candidate))
        .map(Some)
        .ok_or_else(|| not_found("was not found as an executable on PATH"))
}

/// Bounds of one [`navigate_collecting_json`](super::BrowserSession::navigate_collecting_json)
/// call. The inactivity timer alone never ends on a page with continuous
/// traffic (analytics, polling), so listening for responses also stops at
/// `deadline`, and the body fetches end at most [`BODY_FETCH_GRACE`] after
/// it. The number of recorded responses and the retained body bytes are
/// capped; `max_responses: 0` only waits for the page to go quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CollectionLimits {
    pub deadline: Duration,
    pub max_responses: usize,
    pub max_body_bytes: usize,
    pub max_total_bytes: usize,
}

/// How long the body fetches may run past [`CollectionLimits::deadline`].
/// Chromium answers `GetResponseBody` from its buffer at once (also with an
/// error for a body still in flight), so this only bounds a stalled CDP.
pub(super) const BODY_FETCH_GRACE: Duration = Duration::from_secs(5);

impl CollectionLimits {
    /// Measured real bodies: D2Core planner `response_data`
    /// ≈ 300 KB; the flows need one matching body each. The flows' own
    /// inactivity is 10 s.
    pub(super) const PRODUCTION: Self = Self {
        deadline: Duration::from_secs(60),
        max_responses: 256,
        max_body_bytes: 16 * 1024 * 1024,
        max_total_bytes: 64 * 1024 * 1024,
    };
}

/// Running accounting of one collection against its [`CollectionLimits`].
#[derive(Debug)]
pub(super) struct CaptureBudget {
    limits: CollectionLimits,
    responses: usize,
    bytes: usize,
    pub skipped_responses: usize,
    pub skipped_bodies: usize,
}

impl CaptureBudget {
    pub(super) fn new(limits: CollectionLimits) -> Self {
        Self {
            limits,
            responses: 0,
            bytes: 0,
            skipped_responses: 0,
            skipped_bodies: 0,
        }
    }

    /// Whether one more matching response may be recorded.
    pub(super) fn admit_response(&mut self) -> bool {
        if self.responses < self.limits.max_responses {
            self.responses += 1;
            true
        } else {
            self.skipped_responses += 1;
            false
        }
    }

    /// Whether a fetched body of `len` bytes may be retained.
    pub(super) fn admit_body(&mut self, len: usize) -> bool {
        let total = self.bytes.saturating_add(len);
        if len <= self.limits.max_body_bytes && total <= self.limits.max_total_bytes {
            self.bytes = total;
            true
        } else {
            self.skipped_bodies += 1;
            false
        }
    }
}

/// Whether process `pid` still runs. A killed child stays a zombie until
/// it is reaped, so the `/proc` state is checked, not only its presence.
fn process_running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X")
    })
}

/// Resolves when the browser process has exited; never without a pid.
async fn browser_exited(pid: Option<u32>) {
    let Some(pid) = pid else {
        return std::future::pending().await;
    };
    while process_running(pid) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn map_launch_error(message: String, override_set: bool) -> BrowserError {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("executable") || lowered.contains("cannot find") {
        BrowserError::BrowserNotFound {
            detail: message,
            override_set,
        }
    } else {
        BrowserError::Session(message)
    }
}

/// One event of a collection: a network response header, a finished body,
/// or the end of the document navigation (its load event).
enum CollectionEvent {
    Response(Arc<EventResponseReceived>),
    Finished(Arc<EventLoadingFinished>),
    Loaded(Result<(), chromiumoxide::error::CdpError>),
}

/// The navigation error for a failed `goto`: chromiumoxide's own request
/// timeout (30 s without the load event) is a timeout, everything else
/// (DNS, TLS, refused connection, aborted load) a navigation failure.
fn navigation_error(url: &str, error: chromiumoxide::error::CdpError) -> BrowserError {
    match error {
        chromiumoxide::error::CdpError::Timeout => BrowserError::PageLoadTimeout {
            url: url.to_string(),
        },
        other => BrowserError::Navigation(format!("navigate {url}: {other}")),
    }
}

/// Resolves once the job is cancelled.
async fn job_cancelled(control: &ImportControl) {
    while !control.is_cancelled() {
        tokio::time::sleep(CANCEL_POLL).await;
    }
}

/// Upper bound of one page-script evaluation.
const EVALUATE_TIMEOUT: Duration = Duration::from_secs(15);

/// How often a running browser call checks for cancellation.
const CANCEL_POLL: Duration = Duration::from_millis(50);

impl ChromiumSession {
    /// Open a fresh tab for `url`, closing the previous one so leaked pages
    /// cannot accumulate (one flow drives one tab).
    async fn open_page(&self, url: &str) -> Result<Page, BrowserError> {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|error| BrowserError::Navigation(format!("open page for {url}: {error}")))?;
        if let Some(previous) = self.page.replace(Some(page.clone())) {
            let _ = previous.close().await;
        }
        Ok(page)
    }

    /// [`navigate_collecting_json`](super::BrowserSession::navigate_collecting_json)
    /// under explicit `limits`.
    ///
    /// Event driven: a matching JSON response's body is fetched as soon as
    /// Chromium reports it complete (`Network.loadingFinished`), and the
    /// collection returns the moment a body satisfies `accept`. Without an
    /// accepted body it ends once the document loaded and `inactivity`
    /// passed without network events (upstream's inactivity timer; before
    /// the load event only the deadline applies, so a slow first byte is not
    /// mistaken for a quiet page), or at `limits.deadline`. Hitting the
    /// deadline or a cap ends or thins the collection (noted on stderr),
    /// never fails it: the flows then report the missing response
    /// themselves.
    pub(super) fn collect_json_within(
        &self,
        url: &str,
        url_filter: &str,
        inactivity: Duration,
        limits: CollectionLimits,
        accept: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Vec<CapturedResponse>, BrowserError> {
        self.while_browser_runs(&format!("loading {url}"), async {
            let page = self.open_page(url).await?;
            // Subscribe before navigation so even the first response is captured.
            let responses = page
                .event_listener::<EventResponseReceived>()
                .await
                .map_err(|error| BrowserError::Session(format!("network domain: {error}")))?;
            let finished = page
                .event_listener::<EventLoadingFinished>()
                .await
                .map_err(|error| BrowserError::Session(format!("network domain: {error}")))?;
            let started = tokio::time::Instant::now();
            let navigation =
                futures::stream::once(Box::pin(async { page.goto(url).await.map(drop) }));
            let mut events = futures::stream::select(
                futures::stream::select(
                    responses.map(CollectionEvent::Response),
                    finished.map(CollectionEvent::Finished),
                ),
                navigation.map(CollectionEvent::Loaded),
            );
            // Matching responses waiting for their body. The response and
            // loading-finished events arrive on two streams without a
            // mutual order, so a finish seen first is remembered too.
            let mut pending: HashMap<RequestId, String> = HashMap::new();
            let mut finished_first: std::collections::HashSet<RequestId> =
                std::collections::HashSet::new();
            let mut captured = Vec::new();
            let mut budget = CaptureBudget::new(limits);
            let mut loaded_at: Option<tokio::time::Instant> = None;
            let deadline_hit;
            loop {
                let remaining = limits.deadline.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    deadline_hit = true;
                    break;
                }
                let wait = if loaded_at.is_some() {
                    inactivity.min(remaining)
                } else {
                    remaining
                };
                let event = match tokio::time::timeout(wait, events.next()).await {
                    Ok(Some(event)) => event,
                    // Every stream ended: the page's target is gone.
                    Ok(None) => {
                        return Err(BrowserError::Session(format!(
                            "the browser closed the page while loading {url}"
                        )))
                    }
                    Err(_elapsed) => {
                        deadline_hit = loaded_at.is_none();
                        break;
                    }
                };
                match event {
                    CollectionEvent::Loaded(Ok(())) => {
                        let now = tokio::time::Instant::now();
                        self.control.record("page_load", now - started);
                        loaded_at = Some(now);
                    }
                    // A load event that never comes (a hanging third-party
                    // resource) is no failure while the needed response can
                    // still arrive: the inactivity timer and the deadline
                    // bound the rest.
                    CollectionEvent::Loaded(Err(chromiumoxide::error::CdpError::Timeout)) => {
                        eprintln!(
                            "[d4lc] {url} fired no load event within the CDP request timeout; \
                             still waiting for its data"
                        );
                        let now = tokio::time::Instant::now();
                        self.control.record("page_load", now - started);
                        loaded_at = Some(now);
                    }
                    CollectionEvent::Loaded(Err(error)) => {
                        return Err(navigation_error(url, error))
                    }
                    CollectionEvent::Response(event) => {
                        let response = &event.response;
                        if response.mime_type.eq_ignore_ascii_case("application/json")
                            && (url_filter.is_empty() || response.url.contains(url_filter))
                            && budget.admit_response()
                        {
                            pending.insert(event.request_id.clone(), response.url.clone());
                            if finished_first.remove(&event.request_id) {
                                if let Some(done) = self
                                    .fetch_collected_body(
                                        &page,
                                        &event.request_id,
                                        &mut pending,
                                        &mut budget,
                                        &mut captured,
                                        accept,
                                    )
                                    .await
                                {
                                    let waited_since = loaded_at.unwrap_or(started);
                                    self.control.record("data_wait", waited_since.elapsed());
                                    return Ok(done);
                                }
                            }
                        }
                    }
                    CollectionEvent::Finished(event) => {
                        if !pending.contains_key(&event.request_id) {
                            // Bounded by the page's own request count.
                            finished_first.insert(event.request_id.clone());
                            continue;
                        }
                        if let Some(done) = self
                            .fetch_collected_body(
                                &page,
                                &event.request_id,
                                &mut pending,
                                &mut budget,
                                &mut captured,
                                accept,
                            )
                            .await
                        {
                            let waited_since = loaded_at.unwrap_or(started);
                            self.control.record("data_wait", waited_since.elapsed());
                            return Ok(done);
                        }
                    }
                }
            }
            // Responses whose finish was never reported (e.g. a body the
            // page did not read) get one last fetch, like the body fetches
            // after quiescence of upstream.
            let unfinished: Vec<RequestId> = pending.keys().cloned().collect();
            let fetch_until = tokio::time::Instant::now() + BODY_FETCH_GRACE;
            for request_id in unfinished {
                if tokio::time::Instant::now() >= fetch_until {
                    break;
                }
                if let Some(done) = self
                    .fetch_collected_body(
                        &page,
                        &request_id,
                        &mut pending,
                        &mut budget,
                        &mut captured,
                        accept,
                    )
                    .await
                {
                    captured = done;
                    break;
                }
            }
            if loaded_at.is_some() {
                self.control.record(
                    "data_wait",
                    loaded_at.map_or(Duration::ZERO, |loaded| loaded.elapsed()),
                );
            }
            budget.skipped_bodies += pending.len();
            // With `max_responses: 0` skipping every response is the intent.
            let responses_over_cap = if limits.max_responses == 0 {
                0
            } else {
                budget.skipped_responses
            };
            if deadline_hit || responses_over_cap > 0 || budget.skipped_bodies > 0 {
                eprintln!(
                    "[d4lc] browser collection for {url} bounded: deadline {}s reached: \
                     {deadline_hit}; {} response(s) over the {} cap ignored; {} body(ies) \
                     unavailable or over the size caps dropped",
                    limits.deadline.as_secs(),
                    responses_over_cap,
                    limits.max_responses,
                    budget.skipped_bodies
                );
            }
            Ok(captured)
        })
    }

    /// Fetch the completed body of the pending response `request_id` into
    /// `captured`. Returns the whole capture when the body satisfies
    /// `accept` (the collection is done). A body Chromium no longer has, or
    /// one over the size caps, is skipped.
    async fn fetch_collected_body(
        &self,
        page: &Page,
        request_id: &RequestId,
        pending: &mut HashMap<RequestId, String>,
        budget: &mut CaptureBudget,
        captured: &mut Vec<CapturedResponse>,
        accept: Option<&dyn Fn(&str) -> bool>,
    ) -> Option<Vec<CapturedResponse>> {
        let response_url = pending.remove(request_id)?;
        let fetch = page.execute(GetResponseBodyParams::new(request_id.clone()));
        let body = match tokio::time::timeout(BODY_FETCH_GRACE, fetch).await {
            Ok(Ok(response)) => response.result.body,
            Ok(Err(error)) => {
                eprintln!("[d4lc] browser collection: no body for {response_url}: {error}");
                budget.skipped_bodies += 1;
                return None;
            }
            Err(_elapsed) => {
                budget.skipped_bodies += 1;
                return None;
            }
        };
        if !budget.admit_body(body.len()) {
            return None;
        }
        let accepted = accept.is_some_and(|accept| accept(&body));
        captured.push(CapturedResponse {
            url: response_url,
            mime_type: "application/json".to_string(),
            body,
        });
        accepted.then(|| std::mem::take(captured))
    }

    /// Navigate and wait for the document's load event only.
    fn load_page(&self, url: &str) -> Result<(), BrowserError> {
        self.while_browser_runs(&format!("loading {url}"), async {
            let page = self.open_page(url).await?;
            let started = tokio::time::Instant::now();
            match page.goto(url).await {
                Ok(_) => {}
                // The document is there, only a subresource hangs: the
                // flows' own bounded marker polls decide whether the page
                // is usable.
                Err(chromiumoxide::error::CdpError::Timeout) => eprintln!(
                    "[d4lc] {url} fired no load event within the CDP request timeout; \
                     continuing with the loaded document"
                ),
                Err(error) => return Err(navigation_error(url, error)),
            }
            self.control.record("page_load", started.elapsed());
            Ok(())
        })
    }

    /// Run one browser call on the session runtime, failing it as soon as
    /// the browser process exits (`doing` names the call in the error) or
    /// the import job is cancelled.
    fn while_browser_runs<T>(
        &self,
        doing: &str,
        call: impl std::future::Future<Output = Result<T, BrowserError>>,
    ) -> Result<T, BrowserError> {
        self.control.check()?;
        self.runtime.block_on(async {
            let stop = futures::future::select(
                Box::pin(browser_exited(self.browser_pid)),
                Box::pin(job_cancelled(&self.control)),
            );
            match futures::future::select(Box::pin(call), stop).await {
                Either::Left((result, _)) => result,
                Either::Right((Either::Left(((), _)), _)) => Err(BrowserError::Session(format!(
                    "the browser exited unexpectedly while {doing}"
                ))),
                Either::Right((Either::Right(((), _)), _)) => Err(BrowserError::Cancelled),
            }
        })
    }
}

#[cfg(test)]
impl ChromiumSession {
    /// The process id of the spawned browser, for tests that end it.
    pub(super) fn browser_pid(&self) -> Option<u32> {
        self.browser_pid
    }
}

impl super::BrowserSession for ChromiumSession {
    fn window_is_shown(&self) -> bool {
        self.window_is_shown
    }

    fn navigate_collecting_json(
        &self,
        url: &str,
        url_filter: &str,
        inactivity: Duration,
        accept: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Vec<CapturedResponse>, BrowserError> {
        self.collect_json_within(
            url,
            url_filter,
            inactivity,
            CollectionLimits::PRODUCTION,
            accept,
        )
    }

    fn navigate(&self, url: &str) -> Result<(), BrowserError> {
        self.load_page(url)
    }

    fn pause(&self, duration: Duration) -> Result<(), BrowserError> {
        self.control.sleep(duration).map_err(BrowserError::from)
    }

    fn record_phase(&self, phase: &'static str, duration: Duration) {
        self.control.record(phase, duration);
    }

    fn evaluate(&self, script: &str) -> Result<String, BrowserError> {
        self.while_browser_runs("evaluating a page script", async {
            let page =
                self.page.borrow().clone().ok_or_else(|| {
                    BrowserError::Session("no open page; navigate first".to_string())
                })?;
            // chromiumoxide waits for the page's execution context without a
            // bound; a page stuck mid-load kept one probe waiting for minutes
            // (measured: 263 s on the D2Core planner).
            let result = tokio::time::timeout(EVALUATE_TIMEOUT, page.evaluate(script))
                .await
                .map_err(|_elapsed| {
                    BrowserError::Session(format!(
                        "the page did not run a script within {} s",
                        EVALUATE_TIMEOUT.as_secs()
                    ))
                })?
                .map_err(|error| BrowserError::Session(format!("evaluate: {error}")))?;
            result
                .into_value::<String>()
                .map_err(|error| BrowserError::Session(format!("evaluate result: {error}")))
        })
    }
}

impl Drop for ChromiumSession {
    fn drop(&mut self) {
        // Upstream ends every flow by killing the chromedriver PID;
        // here the browser closes gracefully (bounded) and the spawned
        // Chromium child is killed when the handle drops. The close
        // future must be constructed INSIDE the runtime context —
        // chromiumoxide's close grabs tokio handles eagerly, so
        // building the argument outside `block_on` panics with
        // "no reactor running" and a panic in Drop aborts the process.
        // A cancelled job skips the graceful close. Either way the process
        // is reaped before the private profile directory is removed, so
        // no Chromium keeps writing into a deleted profile.
        let graceful = !self.control.is_cancelled();
        self.runtime.block_on(async {
            if graceful {
                let _ = tokio::time::timeout(Duration::from_secs(3), self.browser.close()).await;
                if let Ok(Ok(_)) =
                    tokio::time::timeout(Duration::from_secs(2), self.browser.wait()).await
                {
                    return;
                }
            }
            let _ = self.browser.kill().await;
            let _ = tokio::time::timeout(Duration::from_secs(2), self.browser.wait()).await;
        });
    }
}
