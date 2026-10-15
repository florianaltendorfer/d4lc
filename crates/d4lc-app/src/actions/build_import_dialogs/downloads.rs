//! Background provider downloads and the main-thread event drain.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use d4lc_config::AppSettings;

use super::add_preset::load_app_settings;
use super::dialog::{
    cached_build, get_provider_url, refresh_provider_builds, refresh_provider_profiles,
    set_provider_selected_build,
};
use super::gates::{
    can_add_d2core_build, can_add_d4builds_build, can_add_infinity_builds_build,
    can_add_maxroll_build, can_add_mobalytics_build,
};
use super::import_diagnostics::{record_import, ImportRecord};
use super::paths::{downloaded_builds_dir_in, downloaded_profiles_dir_in, resolve_asset_repo_root};
use super::provider_downloads::{
    build_maxroll_download_url, download_infinity_builds_build, download_maxroll_build,
    validate_maxroll_build_id, ProviderDownloader,
};
use super::{
    PROVIDER_D2CORE, PROVIDER_D4BUILDS, PROVIDER_INFINITY_BUILDS, PROVIDER_MAXROLL,
    PROVIDER_MOBALYTICS,
};
use crate::actions::build_download::{
    BrowserError, BrowserLaunchProfile, BrowserSession, CancelOutcome, ImportControl, ImportError,
    ImportErrorKind,
};

/// Starts the browser session of a browser-driven flow. Production uses
/// [`crate::actions::build_download::launch_chrome_session`]; tests inject
/// scripted sessions to drive the real worker.
type BrowserLauncher =
    dyn Fn(BrowserLaunchProfile, &ImportControl) -> Result<Box<dyn BrowserSession>, BrowserError>;
use crate::app_state::{ActiveDownload, AppState, BuildImportEvent, CompletedImport};
use crate::slint_generated::AppWindow;

// ---------------------------------------------------------------------------
// Background downloads.
//
// Upstream fires `Task.Factory.StartNew(Download...)` and shows a nested
// "Downloading. Please wait." CustomDialog whose Done button is disabled
// until the manager's completed message arrives. Maxroll runs a plain
// synchronous HTTP GET with no dialog (`AddMaxrollBuildExecute`, :749-752).
//
// The port: the UI handler validates the gate, opens the compact progress
// panel (every provider, Maxroll included, so every import can be
// cancelled and every failure is visible), and spawns a `std::thread`
// worker with everything it needs precomputed — the worker never locks
// `AppState` and never touches `AppWindow`; it appends job-tagged
// [`BuildImportEvent`] values that the main-thread drain timer projects.
//
// Each job owns an [`ImportControl`]: Cancel flags it, every browser call
// and wait of the worker ends on the flag, and the build reaches the
// builds directory only through the control's commit gate (the flow writes
// into a per-job staging directory; the commit renames the file into
// place). A cancelled job therefore never stores a build, and its late
// events are ignored because the drain only projects the active job id.
// ---------------------------------------------------------------------------

/// One download job for a worker thread. Everything is resolved on the
/// main thread so the worker needs no `AppState` access.
pub(super) enum DownloadJob {
    D2Core { build_id: String },
    D4Builds { build_id: String },
    Mobalytics { url: String },
    InfinityBuilds { url: String },
    Maxroll { build_id: String },
}

impl DownloadJob {
    fn provider(&self) -> &'static str {
        match self {
            DownloadJob::D2Core { .. } => PROVIDER_D2CORE,
            DownloadJob::D4Builds { .. } => PROVIDER_D4BUILDS,
            DownloadJob::Mobalytics { .. } => PROVIDER_MOBALYTICS,
            DownloadJob::InfinityBuilds { .. } => PROVIDER_INFINITY_BUILDS,
            DownloadJob::Maxroll { .. } => PROVIDER_MAXROLL,
        }
    }

    /// What the job downloads, for the log lines: the page URL for the
    /// URL-based flows and D2Core, the build id for D4Builds (exact
    /// upstream copy, `Failed to download from D4Builds ({buildIdD4Builds})`)
    /// and the planner endpoint for Maxroll.
    fn subject(&self) -> String {
        match self {
            DownloadJob::D2Core { build_id } => {
                crate::actions::build_download::build_d2core_planner_url(build_id)
            }
            DownloadJob::D4Builds { build_id } => build_id.clone(),
            DownloadJob::Mobalytics { url } => url.clone(),
            DownloadJob::InfinityBuilds { url } => url.clone(),
            DownloadJob::Maxroll { build_id } => build_maxroll_download_url(build_id),
        }
    }
}

/// The Add button of `provider`'s tab (upstream `Add{Provider}BuildExecute`):
/// validate the gate, open the progress panel, and spawn the worker. The
/// provider comes from the clicked tab's callback, so the job always
/// matches the button the user pressed.
pub fn handle_add_clicked(
    app: &AppWindow,
    state: &mut AppState,
    provider: &str,
) -> Result<(), String> {
    let started = Instant::now();
    let url = get_provider_url(app, provider);
    let job = build_download_job(provider, &url)?;
    start_download(app, state, job, started)
}

/// Re-download a stored build (upstream `Update{Provider}BuildExecute`):
/// by `Id` (InfinityBuilds / Mobalytics: by stored `Url`).
pub fn handle_build_update(
    app: &AppWindow,
    state: &mut AppState,
    provider: &str,
    index: i32,
) -> Result<(), String> {
    let started = Instant::now();
    let build = cached_build(state, provider, index)?;
    let job = match provider {
        PROVIDER_D2CORE => DownloadJob::D2Core {
            build_id: build.id.clone(),
        },
        PROVIDER_D4BUILDS => DownloadJob::D4Builds {
            build_id: build.id.clone(),
        },
        PROVIDER_INFINITY_BUILDS => {
            if build.url.is_empty() {
                return Err(format!(
                    "stored InfinityBuilds build '{}' carries no Url to re-download",
                    build.id
                ));
            }
            DownloadJob::InfinityBuilds {
                url: build.url.clone(),
            }
        }
        PROVIDER_MAXROLL => DownloadJob::Maxroll {
            build_id: build.id.clone(),
        },
        // Upstream `UpdateMobalyticsBuildExecute` re-downloads by the
        // stored `Url`.
        PROVIDER_MOBALYTICS => {
            if build.url.is_empty() {
                return Err(format!(
                    "stored Mobalytics build '{}' carries no Url to re-download",
                    build.id
                ));
            }
            DownloadJob::Mobalytics {
                url: build.url.clone(),
            }
        }
        other => return Err(format!("unknown build-import provider '{other}'")),
    };
    start_download(app, state, job, started)
}

/// Validate the per-provider gate and shape the download job.
fn build_download_job(provider: &str, url: &str) -> Result<DownloadJob, String> {
    let refused = |what: &str| format!("'{}' is not a valid {provider} {what}", url.trim());
    match provider {
        PROVIDER_D2CORE => {
            let build_id = can_add_d2core_build(url).ok_or_else(|| refused("planner URL"))?;
            Ok(DownloadJob::D2Core { build_id })
        }
        PROVIDER_D4BUILDS => {
            let build_id = can_add_d4builds_build(url).ok_or_else(|| refused("build URL"))?;
            Ok(DownloadJob::D4Builds { build_id })
        }
        PROVIDER_INFINITY_BUILDS => {
            let share_url =
                can_add_infinity_builds_build(url).ok_or_else(|| refused("share URL"))?;
            Ok(DownloadJob::InfinityBuilds { url: share_url })
        }
        PROVIDER_MAXROLL => {
            let build_id = can_add_maxroll_build(url).ok_or_else(|| refused("planner id"))?;
            validate_maxroll_build_id(&build_id)?;
            Ok(DownloadJob::Maxroll { build_id })
        }
        PROVIDER_MOBALYTICS => {
            let page_url =
                can_add_mobalytics_build(url).ok_or_else(|| refused("build or profile URL"))?;
            Ok(DownloadJob::Mobalytics { url: page_url })
        }
        other => Err(format!("unknown build-import provider '{other}'")),
    }
}

/// Open the progress panel and spawn the worker thread. `started` is the
/// click that requested the job (the diagnostics measure from there).
pub(super) fn start_download(
    app: &AppWindow,
    state: &mut AppState,
    job: DownloadJob,
    started: Instant,
) -> Result<(), String> {
    // Single-active-download guard: upstream serializes downloads through
    // one modal CustomDialog. The port blocks re-entry with the modal
    // progress panel plus this explicit refusal (a double click on Add
    // lands here before the panel is drawn).
    if state.build_import.active_download.is_some() || app.get_import_download_active() {
        return Err("a build download is already in progress".to_string());
    }
    // Fail fast on capability gaps before any panel is shown.
    ensure_download_capability(&job)?;

    let id = state.build_import.next_download_id;
    state.build_import.next_download_id += 1;
    let control = ImportControl::new();
    let provider = job.provider();
    state.build_import.active_download = Some(ActiveDownload {
        id,
        provider: provider.to_string(),
        subject: job.subject(),
        mobalytics_url: match &job {
            DownloadJob::Mobalytics { url } => Some(url.clone()),
            _ => None,
        },
        control: control.clone(),
        started,
    });
    state.bump_rules_input_revision();

    // Claim the single-download slot before the worker exists so a fast
    // completion can never slip past an unset guard.
    app.set_import_download_active(true);
    // A stale error line from an earlier action must not sit above a new,
    // running import.
    app.set_dialog_error_visible(false);
    app.set_import_progress_provider(provider.into());
    app.set_import_progress_step(initial_step(&job).into());
    app.set_import_progress_status(slint::SharedString::default());
    app.set_import_progress_build_name(slint::SharedString::default());
    app.set_import_progress_variants(slint::ModelRc::new(slint::VecModel::from(Vec::<
        slint::SharedString,
    >::new())));
    app.set_import_progress_elapsed(slint::SharedString::default());
    app.set_import_progress_failed(false);
    app.set_import_progress_error_title(slint::SharedString::default());
    app.set_import_progress_error(slint::SharedString::default());
    app.set_import_progress_visible(true);
    spawn_download(state, job, id, control, started);
    Ok(())
}

/// The first step the panel shows.
fn initial_step(job: &DownloadJob) -> &'static str {
    match job {
        DownloadJob::D2Core { .. }
        | DownloadJob::D4Builds { .. }
        | DownloadJob::Mobalytics { .. } => STEP_PREPARING_BROWSER,
        DownloadJob::InfinityBuilds { .. } | DownloadJob::Maxroll { .. } => STEP_DOWNLOADING,
    }
}

/// The import steps the panel headline shows (English msgids).
const STEP_PREPARING_BROWSER: &str = "Preparing browser";
const STEP_LOADING_PAGE: &str = "Loading build page";
const STEP_DOWNLOADING: &str = "Downloading build";
const STEP_READING: &str = "Reading build data";
const STEP_CONVERTING: &str = "Converting build";
const STEP_SAVING: &str = "Saving build";

/// The step a provider status line starts (the flows report the upstream
/// status copy; the panel headline names the phase it belongs to).
fn step_of_status(status: &str) -> Option<&'static str> {
    if status.starts_with("Converting ") || status.starts_with("Converted ") {
        Some(STEP_CONVERTING)
    } else if ["Downloaded ", "Exporting ", "Exported ", "Paragon: "]
        .iter()
        .any(|prefix| status.starts_with(prefix))
    {
        Some(STEP_READING)
    } else {
        None
    }
}

/// Synchronous capability precheck so a build without the browser / HTTP
/// features reports the gap without spawning a doomed worker.
/// With the features enabled this is a no-op: the worker performs the real
/// launch, so the precheck must not start a second browser here.
fn ensure_download_capability(job: &DownloadJob) -> Result<(), String> {
    match job {
        DownloadJob::D2Core { .. }
        | DownloadJob::D4Builds { .. }
        | DownloadJob::Mobalytics { .. } => {
            #[cfg(not(feature = "browser-download"))]
            {
                crate::actions::build_download::launch_chrome_session(
                    BrowserLaunchProfile::D2Core,
                    &ImportControl::new(),
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            }
            #[cfg(feature = "browser-download")]
            {
                let _ = job;
                Ok(())
            }
        }
        DownloadJob::InfinityBuilds { .. } | DownloadJob::Maxroll { .. } => {
            ProviderDownloader::for_provider(job.provider())
                .map(|_| ())
                .ok_or_else(|| format!("no downloader for provider '{}'", job.provider()))
        }
    }
}

/// Spawn the worker thread. The worker owns the event buffer clone and
/// precomputed inputs; it must never lock `AppState` or touch Slint.
fn spawn_download(
    state: &mut AppState,
    job: DownloadJob,
    job_id: u64,
    control: ImportControl,
    started: Instant,
) {
    let events = state.build_import_events.clone();
    let builds_dir_override = state.build_import.builds_dir.clone();
    let settings = load_app_settings(state);
    let thread_name = format!("d4lc-import-{}", job.provider().to_ascii_lowercase());
    let provider = job.provider().to_string();
    let worker_control = control.clone();
    let spawned = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            // A panic in the provider flow must land in the same failure
            // surface as a returned error: the modal panel and the
            // single-download guard both wait for a `Completed` event.
            run_download_job_guarded(&events, job_id, &provider, &worker_control, || {
                run_download_job(
                    &events,
                    job_id,
                    started,
                    builds_dir_override,
                    settings,
                    job,
                    &worker_control,
                    &crate::actions::build_download::launch_chrome_session,
                )
            });
        });
    if let Err(error) = spawned {
        // A failed spawn must not look like a started download: report the
        // completion with the spawn error so the panel cannot hang.
        control.mark_finished();
        state.build_import_events.push(BuildImportEvent::Completed {
            job_id,
            provider: state
                .build_import
                .active_download
                .as_ref()
                .map(|active| active.provider.clone())
                .unwrap_or_default(),
            result: Err(ImportError::new(
                ImportErrorKind::Internal,
                format!("failed to spawn the download worker: {error}"),
            )),
            phases: Vec::new(),
            finished_at: Instant::now(),
        });
    }
}

/// Run `body` and report the standard worker failure when it panics
/// instead of letting the thread die silently: the modal panel and the
/// single-download guard wait for a `Completed` event. The job is marked
/// finished either way.
fn run_download_job_guarded(
    events: &crate::app_state::BuildImportEventBuffer,
    job_id: u64,
    provider: &str,
    control: &ImportControl,
    body: impl FnOnce(),
) {
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        let detail = panic_detail(payload);
        eprintln!(
            "[d4lc] {} download worker panicked{detail}",
            provider.to_ascii_lowercase()
        );
        events.push(BuildImportEvent::Completed {
            job_id,
            provider: provider.to_string(),
            result: Err(ImportError::new(
                ImportErrorKind::Internal,
                format!("the download worker panicked{detail}"),
            )),
            phases: control.phases(),
            finished_at: Instant::now(),
        });
    }
    control.mark_finished();
}

/// Best-effort human-readable message from a panic payload (the default
/// panic hook already printed the full panic + backtrace to stderr).
fn panic_detail(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        format!(": {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!(": {message}")
    } else {
        String::new()
    }
}

/// The worker body: run the provider flow and report step / status /
/// completion events. Runs on its own thread — no Slint, no `AppState`.
/// The browser session is closed only after the completion event, so the
/// dialog never waits for Chromium's shutdown.
#[allow(clippy::too_many_arguments)]
fn run_download_job(
    events: &crate::app_state::BuildImportEventBuffer,
    job_id: u64,
    started: Instant,
    builds_dir_override: Option<PathBuf>,
    settings: AppSettings,
    job: DownloadJob,
    control: &ImportControl,
    launcher: &BrowserLauncher,
) {
    control.record("ui_dispatch", started.elapsed());
    let provider = job.provider();
    let sink = WorkerSink {
        events,
        job_id,
        provider,
        last_step: std::cell::Cell::new(""),
    };
    let mut browser: Option<Box<dyn BrowserSession>> = None;
    let result = run_provider_flow(
        &mut browser,
        launcher,
        builds_dir_override,
        &settings,
        &job,
        control,
        job_id,
        &sink,
    );
    if let Err(error) = &result {
        if error.kind != ImportErrorKind::Cancelled {
            eprintln!(
                "[d4lc] {} download failed ({}): {}",
                provider.to_ascii_lowercase(),
                error.kind.code(),
                error.message
            );
        }
    }
    events.push(BuildImportEvent::Completed {
        job_id,
        provider: provider.to_string(),
        result,
        phases: control.phases(),
        finished_at: Instant::now(),
    });
    let closing = Instant::now();
    drop(browser);
    if closing.elapsed() > Duration::from_millis(1) {
        eprintln!(
            "[d4lc] import #{job_id}: browser closed in {} ms (after completion)",
            closing.elapsed().as_millis()
        );
    }
}

/// The worker's event sink for one job.
struct WorkerSink<'a> {
    events: &'a crate::app_state::BuildImportEventBuffer,
    job_id: u64,
    provider: &'static str,
    last_step: std::cell::Cell<&'static str>,
}

impl WorkerSink<'_> {
    fn step(&self, step: &'static str) {
        if self.last_step.replace(step) == step {
            return;
        }
        self.events.push(BuildImportEvent::Step {
            job_id: self.job_id,
            step: step.to_string(),
        });
    }

    fn status(&self, status: &str, build_name: &str, variant_names: Vec<String>) {
        if let Some(step) = step_of_status(status) {
            self.step(step);
        }
        self.events.push(BuildImportEvent::Status {
            job_id: self.job_id,
            provider: self.provider.to_string(),
            status: status.to_string(),
            build_name: build_name.to_string(),
            variant_names,
        });
    }

    fn log(&self, line: &str) {
        self.events.push(BuildImportEvent::Log {
            job_id: self.job_id,
            line: line.to_string(),
        });
    }
}

/// A per-job staging directory inside the target directory: the flows
/// write there, and [`Staging::commit`] renames the finished file into the
/// target through the job's commit gate. Dropping it removes whatever the
/// job left behind (a cancelled or failed job stores nothing). Directory
/// listings only read `*.json` files, so the staging directory never shows
/// up as a build.
struct Staging {
    dir: PathBuf,
}

impl Staging {
    fn new(target_dir: &Path, job_id: u64) -> Result<Self, ImportError> {
        let dir = target_dir.join(format!(".d4lc-import-{job_id}"));
        // A same-named leftover can only come from an earlier process.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|error| {
            ImportError::new(
                ImportErrorKind::Storage,
                format!("create staging dir {}: {error}", dir.display()),
            )
        })?;
        Ok(Self { dir })
    }

    fn dir(&self) -> &Path {
        &self.dir
    }

    /// Move the staged `file` into `target_dir` unless the job was
    /// cancelled. Returns the stored path.
    fn commit(
        &self,
        control: &ImportControl,
        file: &Path,
        target_dir: &Path,
    ) -> Result<PathBuf, ImportError> {
        let name = file.file_name().ok_or_else(|| {
            ImportError::new(
                ImportErrorKind::Storage,
                format!("staged file {} has no name", file.display()),
            )
        })?;
        let target = target_dir.join(name);
        let started = Instant::now();
        control
            .commit(|| std::fs::rename(file, &target))
            .map_err(ImportError::from)?
            .map_err(|error| {
                ImportError::new(
                    ImportErrorKind::Storage,
                    format!("store {}: {error}", target.display()),
                )
            })?;
        control.record("commit", started.elapsed());
        Ok(target)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The file stem of a stored build: the id the build list shows.
fn stored_id(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Launch the browser for a flow into `slot` (kept by the worker so the
/// session closes after the completion event).
fn launch_browser<'a>(
    slot: &'a mut Option<Box<dyn BrowserSession>>,
    launcher: &BrowserLauncher,
    profile: BrowserLaunchProfile,
    control: &ImportControl,
    sink: &WorkerSink<'_>,
) -> Result<&'a dyn BrowserSession, ImportError> {
    sink.step(STEP_PREPARING_BROWSER);
    let session = control
        .timed("browser_launch", || launcher(profile, control))
        .map_err(ImportError::from)?;
    sink.step(STEP_LOADING_PAGE);
    Ok(&**slot.insert(session))
}

fn asset_root(provider: &str) -> Result<PathBuf, ImportError> {
    resolve_asset_repo_root().ok_or_else(|| {
        ImportError::new(
            ImportErrorKind::Parse,
            format!(
                "No localized import data available (assets/original/data). The {provider} \
                 download needs the bundled asset data; run from a checkout or install with \
                 the bundled assets."
            ),
        )
    })
}

/// The per-provider flow: download into the staging directory, then
/// commit the stored file.
#[allow(clippy::too_many_arguments)]
fn run_provider_flow(
    browser: &mut Option<Box<dyn BrowserSession>>,
    launcher: &BrowserLauncher,
    builds_dir_override: Option<PathBuf>,
    settings: &AppSettings,
    job: &DownloadJob,
    control: &ImportControl,
    job_id: u64,
    sink: &WorkerSink<'_>,
) -> Result<CompletedImport, ImportError> {
    let prepare_started = Instant::now();
    let builds_dir = resolve_download_builds_dir(job.provider(), builds_dir_override)?;
    let staging = Staging::new(&builds_dir, job_id)?;
    control.record("prepare", prepare_started.elapsed());
    let status = |status: &str, build_name: &str, variant_names: Vec<String>| {
        sink.status(status, build_name, variant_names)
    };
    match job {
        DownloadJob::D2Core { build_id } => {
            let session = launch_browser(
                browser,
                launcher,
                BrowserLaunchProfile::D2Core,
                control,
                sink,
            )?;
            status(
                &format!(
                    "Downloading {}.",
                    crate::actions::build_download::build_d2core_planner_url(build_id)
                ),
                "",
                Vec::new(),
            );
            let outcome = crate::actions::build_download::download_d2core_build(
                build_id,
                staging.dir(),
                session,
            )?;
            let name = outcome
                .build_name
                .clone()
                .unwrap_or_else(|| outcome.build_id.clone());
            status(&format!("Downloaded {name}."), &name, Vec::new());
            sink.step(STEP_SAVING);
            let path = staging.commit(control, &outcome.path, &builds_dir)?;
            Ok(CompletedImport {
                name,
                build_id: stored_id(&path),
                variant_names: Vec::new(),
            })
        }
        DownloadJob::D4Builds { build_id } => {
            let repo_root = asset_root("D4Builds")?;
            let session = launch_browser(
                browser,
                launcher,
                BrowserLaunchProfile::D4Builds,
                control,
                sink,
            )?;
            let outcome = crate::actions::build_download::download_d4builds_build(
                build_id,
                staging.dir(),
                session,
                &repo_root,
                settings.is_import_paragon_d4builds_enabled,
                &status,
            )?;
            sink.step(STEP_SAVING);
            let path = staging.commit(control, &outcome.path, &builds_dir)?;
            Ok(CompletedImport {
                name: outcome.build_name,
                build_id: stored_id(&path),
                variant_names: outcome.variant_names,
            })
        }
        DownloadJob::Mobalytics { url } => {
            let repo_root = asset_root("Mobalytics")?;
            let profiles_dir = resolve_download_profiles_dir()?;
            let profiles_staging = Staging::new(&profiles_dir, job_id)?;
            // Upstream skips `--headless` when
            // `IsShowBrowserMobalyticsEnabled` is on (InitSelenium :376-385);
            // the persisted toggle reaches this launch site through the
            // worker's settings snapshot.
            let session = launch_browser(
                browser,
                launcher,
                crate::actions::build_download::mobalytics_launch_profile(
                    settings.is_show_browser_mobalytics_enabled,
                ),
                control,
                sink,
            )?;
            let outcome = crate::actions::build_download::download_mobalytics_build(
                url,
                staging.dir(),
                profiles_staging.dir(),
                session,
                &repo_root,
                settings.is_import_paragon_mobalytics_enabled,
                &status,
            )?;
            for line in &outcome.log_lines {
                sink.log(line);
            }
            sink.step(STEP_SAVING);
            let (stage, target) = if outcome.path.starts_with(profiles_staging.dir()) {
                (&profiles_staging, profiles_dir.as_path())
            } else {
                (&staging, builds_dir.as_path())
            };
            let path = stage.commit(control, &outcome.path, target)?;
            Ok(CompletedImport {
                name: outcome.name,
                build_id: stored_id(&path),
                variant_names: outcome.variant_names,
            })
        }
        DownloadJob::InfinityBuilds { url } => {
            let downloader = ProviderDownloader::for_provider(PROVIDER_INFINITY_BUILDS)
                .ok_or_else(|| {
                    ImportError::new(
                        ImportErrorKind::Internal,
                        "InfinityBuilds downloader unavailable: unknown provider",
                    )
                })?;
            let repo_root = asset_root("InfinityBuilds")?;
            status(&format!("Downloading {url}."), "", Vec::new());
            let outcome = download_infinity_builds_build(
                url,
                staging.dir(),
                &repo_root,
                &downloader,
                settings,
                control,
            )?;
            sink.step(STEP_SAVING);
            let path = staging.commit(control, &outcome.cache_path, &builds_dir)?;
            Ok(CompletedImport {
                name: outcome.build_name,
                build_id: stored_id(&path),
                variant_names: outcome.variant_names,
            })
        }
        DownloadJob::Maxroll { build_id } => {
            let downloader =
                ProviderDownloader::for_provider(PROVIDER_MAXROLL).ok_or_else(|| {
                    ImportError::new(
                        ImportErrorKind::Internal,
                        "Maxroll downloader unavailable: unknown provider",
                    )
                })?;
            status(
                &format!("Downloading {}.", build_maxroll_download_url(build_id)),
                "",
                Vec::new(),
            );
            let outcome = download_maxroll_build(build_id, staging.dir(), &downloader, control)?;
            sink.step(STEP_SAVING);
            let path = staging.commit(control, &outcome.path, &builds_dir)?;
            Ok(CompletedImport {
                name: outcome.build_name,
                build_id: stored_id(&path),
                variant_names: Vec::new(),
            })
        }
    }
}

/// The worker-side builds directory: the configured directory wins
/// (`BuildImportState::builds_dir`, hydrated from the persisted
/// `AppSettings::builds_dir`), otherwise the upstream-equivalent
/// `{config_root}/Builds/{provider}` home, created on demand. Pure
/// filesystem — safe off the UI thread.
fn resolve_download_builds_dir(
    provider: &str,
    override_dir: Option<PathBuf>,
) -> Result<PathBuf, ImportError> {
    let dir = match override_dir {
        Some(dir) => dir,
        None => {
            downloaded_builds_dir_in(&crate::actions::settings::resolve_config_root(), provider)
        }
    };
    std::fs::create_dir_all(&dir).map_err(|error| {
        ImportError::new(
            ImportErrorKind::Storage,
            format!("create builds dir {}: {error}", dir.display()),
        )
    })?;
    Ok(dir)
}

/// The worker-side profiles directory for a Mobalytics job: the builds
/// override is a BUILDS dir, so the profiles home always resolves from the
/// config root (`.\Profiles\Mobalytics` upstream, `ParseJsonProfile`).
fn resolve_download_profiles_dir() -> Result<PathBuf, ImportError> {
    let dir = downloaded_profiles_dir_in(&crate::actions::settings::resolve_config_root());
    std::fs::create_dir_all(&dir).map_err(|error| {
        ImportError::new(
            ImportErrorKind::Storage,
            format!("create profiles dir {}: {error}", dir.display()),
        )
    })?;
    Ok(dir)
}

/// `12 s` / `1 min 05 s` for the panel's elapsed time.
fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min {:02} s", secs / 60, secs % 60)
    }
}

/// The main-thread drain (a 50 ms `slint::Timer` in `wiring.rs`): move the
/// active job's events into the window properties, finish the job on its
/// completion, and drop every event of any other job (a cancelled or
/// replaced job can never change the dialog).
pub fn drain_build_import_events(app: &AppWindow, state: &mut AppState) {
    if let Some(active) = &state.build_import.active_download {
        let elapsed = format_elapsed(active.started.elapsed());
        if app.get_import_progress_elapsed().as_str() != elapsed {
            app.set_import_progress_elapsed(elapsed.into());
        }
    }
    let detached = state.build_import.detached_downloads.len();
    state
        .build_import
        .detached_downloads
        .retain(|control| !control.is_finished());
    if state.build_import.detached_downloads.len() != detached {
        state.bump_rules_input_revision();
    }
    for event in state.build_import_events.drain() {
        let active_id = state.build_import.active_download.as_ref().map(|a| a.id);
        let job_id = match &event {
            BuildImportEvent::Step { job_id, .. }
            | BuildImportEvent::Status { job_id, .. }
            | BuildImportEvent::Log { job_id, .. }
            | BuildImportEvent::Completed { job_id, .. } => *job_id,
        };
        if active_id != Some(job_id) {
            continue;
        }
        match event {
            BuildImportEvent::Step { step, .. } => {
                app.set_import_progress_step(step.into());
            }
            BuildImportEvent::Status {
                status,
                build_name,
                variant_names,
                ..
            } => {
                if !status.is_empty() {
                    app.set_import_progress_status(status.into());
                }
                if !build_name.is_empty() {
                    app.set_import_progress_build_name(build_name.into());
                }
                if !variant_names.is_empty() {
                    let rows: Vec<slint::SharedString> = variant_names
                        .iter()
                        .map(|variant| variant.clone().into())
                        .collect();
                    app.set_import_progress_variants(slint::ModelRc::new(slint::VecModel::from(
                        rows,
                    )));
                }
            }
            BuildImportEvent::Log { line, .. } => {
                state.append_log_message(line, crate::clock::hh_mm_now());
            }
            BuildImportEvent::Completed {
                provider,
                result,
                phases,
                finished_at,
                ..
            } => finish_download(app, state, &provider, result, phases, finished_at),
        }
    }
}

/// Project the active job's completion: a stored build closes the panel and
/// shows the build selected in its list; a failure keeps the panel open
/// with the classified error until the user closes it.
fn finish_download(
    app: &AppWindow,
    state: &mut AppState,
    provider: &str,
    result: Result<CompletedImport, ImportError>,
    phases: Vec<crate::actions::build_download::PhaseTiming>,
    finished_at: Instant,
) {
    let Some(active) = state.build_import.active_download.take() else {
        return;
    };
    state.bump_rules_input_revision();
    app.set_import_download_active(false);
    match &result {
        Ok(done) => {
            // Upstream reloads the list after completion
            // (`{Provider}BuildsLoadedMessage`).
            refresh_provider_builds(app, state, provider);
            if provider == PROVIDER_MOBALYTICS {
                // A profile download reloads the profiles list
                // (`MobalyticsProfilesLoadedMessage`), and upstream
                // `AddMobalyticsBuildExecute` (:778-786) selects the nested
                // tab from the requested URL.
                refresh_provider_profiles(app, state, provider);
                if let Some(url) = &active.mobalytics_url {
                    let lowered = url.to_lowercase();
                    let sub_tab = if lowered.contains("profile") && lowered.contains("builds") {
                        Some(0)
                    } else if lowered.contains("profile") {
                        Some(1)
                    } else {
                        None
                    };
                    if let Some(sub_tab) = sub_tab {
                        app.set_import_mobalytics_selected_sub_tab(sub_tab);
                    }
                }
            }
            select_stored_build(app, state, provider, &done.build_id);
            app.set_import_progress_visible(false);
            state.append_log_message(
                format!(
                    "Imported {provider} build '{}' ({}) in {}.",
                    done.name,
                    active.subject,
                    format_secs(active.started.elapsed())
                ),
                crate::clock::hh_mm_now(),
            );
        }
        Err(error) => {
            app.set_import_progress_failed(true);
            app.set_import_progress_error_title(error.kind.headline().into());
            app.set_import_progress_error(error.message.clone().into());
            app.set_import_progress_visible(true);
            state.append_log_message(
                format!(
                    "Failed to download from {provider} ({}): {}",
                    active.subject, error.message
                ),
                crate::clock::hh_mm_now(),
            );
        }
    }
    record_import(&ImportRecord {
        job_id: active.id,
        provider,
        subject: &active.subject,
        result: result.as_ref().map(|done| done.name.as_str()),
        phases: &phases,
        ui_finish: Some(finished_at.elapsed()),
        total: active.started.elapsed(),
    });
}

/// Select the just-stored build in its list (and list its variants), so
/// the import ends on the build the user asked for.
fn select_stored_build(app: &AppWindow, state: &AppState, provider: &str, build_id: &str) {
    let Some(index) = state
        .build_import
        .provider_builds
        .get(provider)
        .and_then(|builds| builds.iter().position(|build| build.id == build_id))
    else {
        return;
    };
    let index = i32::try_from(index).unwrap_or(-1);
    if super::row_actions::handle_build_selected(app, state, provider, index).is_err() {
        set_provider_selected_build(app, provider, -1);
    }
}

fn format_secs(elapsed: Duration) -> String {
    format!("{:.1} s", elapsed.as_secs_f64())
}

/// The progress panel's Cancel (and Escape while an import runs): cancel
/// the active job at once. The panel closes, the guard is released, and
/// a new import can start immediately; the old worker winds down on its
/// own (its events are ignored, it stores nothing). Repeated clicks are
/// harmless. When the job already stored its build, the cancel came too
/// late: the panel stays and the completion arrives momentarily.
pub fn handle_progress_cancel(app: &AppWindow, state: &mut AppState) {
    let Some(active) = state.build_import.active_download.take() else {
        // Nothing runs (a failed import's panel): just close it.
        app.set_import_progress_visible(false);
        return;
    };
    if active.control.cancel() == CancelOutcome::AlreadyCommitted {
        app.set_import_progress_step(STEP_SAVING.into());
        state.build_import.active_download = Some(active);
        return;
    }
    state.bump_rules_input_revision();
    app.set_import_download_active(false);
    app.set_import_progress_visible(false);
    state.append_log_message(
        format!(
            "Import from {} cancelled ({}).",
            active.provider, active.subject
        ),
        crate::clock::hh_mm_now(),
    );
    record_import(&ImportRecord {
        job_id: active.id,
        provider: &active.provider,
        subject: &active.subject,
        result: Err(&ImportError::cancelled()),
        phases: &active.control.phases(),
        ui_finish: None,
        total: active.started.elapsed(),
    });
    state.build_import.detached_downloads.push(active.control);
}

/// The failed panel's Close: hide it (the job already finished).
pub fn handle_progress_close(app: &AppWindow, state: &mut AppState) {
    if state.build_import.active_download.is_some() {
        handle_progress_cancel(app, state);
    } else {
        app.set_import_progress_visible(false);
    }
}

/// Escape on the import dialog: while the progress panel is up it cancels
/// the running import (or closes the failure panel). Returns whether the
/// key was used; otherwise the dialog keeps upstream's key parity.
pub fn handle_progress_escape(app: &AppWindow, state: &mut AppState) -> bool {
    if !app.get_import_progress_visible() {
        return false;
    }
    handle_progress_close(app, state);
    true
}

/// App exit: cancel the running import and give every worker a bounded
/// moment to shut its browser down, so no Chromium outlives d4lc.
pub fn shutdown_downloads(state: &mut AppState) {
    if let Some(active) = state.build_import.active_download.take() {
        active.control.cancel();
        state.build_import.detached_downloads.push(active.control);
    }
    state.bump_rules_input_revision();
    let deadline = Instant::now() + Duration::from_secs(4);
    for control in state.build_import.detached_downloads.drain(..) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !control.wait_finished(remaining) {
            eprintln!("[d4lc] build import: a download worker did not finish before exit");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::actions::build_download::{CapturedResponse, PhaseTiming};
    use crate::actions::build_import_dialogs::paths::dialog_builds_dir;
    use crate::actions::build_import_dialogs::test_support::local_tab_root;
    use crate::app_state::{AppState, BuildImportEventBuffer};

    // -------------------------------------------------------------------
    // Worker: the real `run_download_job` with scripted browser sessions.
    // -------------------------------------------------------------------

    /// How the scripted D2Core page behaves.
    #[derive(Clone, Copy)]
    enum Page {
        /// The planner response arrives.
        Responds,
        /// The page never delivers data; the wait ends only on cancel
        /// (exactly how the Chromium session ends its waits).
        WaitsUntilCancelled,
        /// The response arrives, but the user cancels while the worker
        /// processes it.
        CancelledWhileProcessing,
    }

    struct ScriptedPage {
        page: Page,
        control: ImportControl,
    }

    fn d2core_body(build_id: &str) -> String {
        let inner = format!(
            r#"{{"data":{{"_id":"{build_id}","title":"Blazing Scream","variants":[{{"name":"Endgame","gear":{{}}}}]}}}}"#
        );
        format!(
            "{{\"data\":{{\"response_data\":{}}}}}",
            serde_json::to_string(&inner).unwrap()
        )
    }

    impl BrowserSession for ScriptedPage {
        fn navigate_collecting_json(
            &self,
            url: &str,
            _url_filter: &str,
            _inactivity: Duration,
            _accept: Option<&dyn Fn(&str) -> bool>,
        ) -> Result<Vec<CapturedResponse>, BrowserError> {
            match self.page {
                Page::WaitsUntilCancelled => {
                    while !self.control.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(BrowserError::Cancelled)
                }
                Page::Responds | Page::CancelledWhileProcessing => {
                    if matches!(self.page, Page::CancelledWhileProcessing) {
                        self.control.cancel();
                    }
                    Ok(vec![CapturedResponse {
                        url: url.to_string(),
                        mime_type: "application/json".to_string(),
                        body: d2core_body("23tb"),
                    }])
                }
            }
        }

        fn evaluate(&self, _script: &str) -> Result<String, BrowserError> {
            Ok(String::new())
        }

        // Like the Chromium session: flow phases land in the job control.
        fn record_phase(&self, phase: &'static str, duration: Duration) {
            self.control.record(phase, duration);
        }
    }

    fn page_launcher(
        page: Page,
    ) -> impl Fn(BrowserLaunchProfile, &ImportControl) -> Result<Box<dyn BrowserSession>, BrowserError>
    {
        move |profile, control| {
            assert_eq!(profile, BrowserLaunchProfile::D2Core);
            Ok(Box::new(ScriptedPage {
                page,
                control: control.clone(),
            }))
        }
    }

    /// A browser that never finishes starting: the launch ends only on
    /// cancel, like the Chromium launch.
    fn hanging_launcher(
        _profile: BrowserLaunchProfile,
        control: &ImportControl,
    ) -> Result<Box<dyn BrowserSession>, BrowserError> {
        while !control.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(BrowserError::Cancelled)
    }

    fn d2core_job() -> DownloadJob {
        DownloadJob::D2Core {
            build_id: "23tb".to_string(),
        }
    }

    /// Run the real worker for `job` on its own thread (like
    /// `spawn_download`), returning its control and a handle.
    fn spawn_worker(
        events: &BuildImportEventBuffer,
        builds_dir: &Path,
        job: DownloadJob,
        launcher: impl Fn(
                BrowserLaunchProfile,
                &ImportControl,
            ) -> Result<Box<dyn BrowserSession>, BrowserError>
            + Send
            + 'static,
    ) -> (ImportControl, std::thread::JoinHandle<()>) {
        let control = ImportControl::new();
        let worker_control = control.clone();
        let events = events.clone();
        let builds_dir = builds_dir.to_path_buf();
        let handle = std::thread::spawn(move || {
            run_download_job_guarded(&events, 1, PROVIDER_D2CORE, &worker_control, || {
                run_download_job(
                    &events,
                    1,
                    Instant::now(),
                    Some(builds_dir),
                    AppSettings::default(),
                    job,
                    &worker_control,
                    &launcher,
                )
            });
        });
        (control, handle)
    }

    fn completed(
        events: &BuildImportEventBuffer,
    ) -> (Result<CompletedImport, ImportError>, Vec<PhaseTiming>) {
        let drained = events.drain();
        let mut completions = drained.into_iter().filter_map(|event| match event {
            BuildImportEvent::Completed { result, phases, .. } => Some((result, phases)),
            _ => None,
        });
        let first = completions.next().expect("one completion");
        assert!(
            completions.next().is_none(),
            "exactly one completion per job"
        );
        first
    }

    fn json_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_worker_stores_the_build_through_the_commit_and_measures_every_phase() {
        let root = tempfile::TempDir::new().unwrap();
        let events = BuildImportEventBuffer::default();
        let (control, handle) = spawn_worker(
            &events,
            root.path(),
            d2core_job(),
            page_launcher(Page::Responds),
        );
        handle.join().unwrap();
        assert!(control.is_finished());

        let (result, phases) = completed(&events);
        let done = result.expect("stored");
        assert_eq!(done.name, "Blazing Scream");
        assert_eq!(done.build_id, "23tb");
        assert_eq!(
            json_files(root.path()),
            ["23tb.json"],
            "the build is stored and the staging directory is gone"
        );
        // The stored file reads back through the dialog's list loader.
        let listed =
            super::super::paths::load_builds_for_provider(PROVIDER_D2CORE, root.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "23tb");
        assert_eq!(listed[0].variant_names, ["Endgame"]);
        let names: Vec<&str> = phases.iter().map(|phase| phase.phase).collect();
        for phase in [
            "ui_dispatch",
            "prepare",
            "browser_launch",
            "extract",
            "store",
            "commit",
        ] {
            assert!(
                names.contains(&phase),
                "phase {phase} missing from {names:?}"
            );
        }
    }

    #[test]
    fn cancelling_while_the_browser_starts_stores_nothing_and_completes_cancelled() {
        let root = tempfile::TempDir::new().unwrap();
        let events = BuildImportEventBuffer::default();
        let (control, handle) = spawn_worker(&events, root.path(), d2core_job(), hanging_launcher);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(control.cancel(), CancelOutcome::Cancelled);
        handle.join().unwrap();
        let (result, _) = completed(&events);
        assert_eq!(result.unwrap_err().kind, ImportErrorKind::Cancelled);
        assert!(
            json_files(root.path()).is_empty(),
            "nothing stored, no staging left"
        );
    }

    #[test]
    fn cancelling_while_waiting_for_the_page_stores_nothing_and_completes_cancelled() {
        let root = tempfile::TempDir::new().unwrap();
        let events = BuildImportEventBuffer::default();
        let (control, handle) = spawn_worker(
            &events,
            root.path(),
            d2core_job(),
            page_launcher(Page::WaitsUntilCancelled),
        );
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        control.cancel();
        assert_eq!(
            control.cancel(),
            CancelOutcome::AlreadyCancelled,
            "repeated cancel is safe"
        );
        handle.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the wait ends on cancel"
        );
        let (result, _) = completed(&events);
        assert_eq!(result.unwrap_err().kind, ImportErrorKind::Cancelled);
        assert!(json_files(root.path()).is_empty());
    }

    #[test]
    fn cancelling_while_processing_refuses_the_commit() {
        // The data arrived and was written to staging, but the cancel came
        // before the commit: the builds directory must stay untouched and
        // the staging file must be removed.
        let root = tempfile::TempDir::new().unwrap();
        std::fs::write(root.path().join("23tb.json"), "previous build").unwrap();
        let events = BuildImportEventBuffer::default();
        let (_control, handle) = spawn_worker(
            &events,
            root.path(),
            d2core_job(),
            page_launcher(Page::CancelledWhileProcessing),
        );
        handle.join().unwrap();
        let (result, _) = completed(&events);
        assert_eq!(result.unwrap_err().kind, ImportErrorKind::Cancelled);
        assert_eq!(json_files(root.path()), ["23tb.json"]);
        assert_eq!(
            std::fs::read_to_string(root.path().join("23tb.json")).unwrap(),
            "previous build",
            "a cancelled update keeps the previously stored build"
        );
    }

    #[test]
    #[cfg(not(feature = "browser-download"))]
    fn d4builds_worker_failure_names_the_missing_browser() {
        let events = BuildImportEventBuffer::default();
        let root = tempfile::TempDir::new().unwrap();
        run_download_job(
            &events,
            3,
            Instant::now(),
            Some(root.path().to_path_buf()),
            AppSettings::default(),
            DownloadJob::D4Builds {
                build_id: "ancients-barbarian-endgame".to_string(),
            },
            &ImportControl::new(),
            &crate::actions::build_download::launch_chrome_session,
        );
        let (result, _) = completed(&events);
        assert_eq!(
            result.unwrap_err().kind,
            ImportErrorKind::BrowserUnavailable
        );
    }

    #[test]
    fn a_panicking_worker_reports_an_internal_failure_and_finishes() {
        let events = BuildImportEventBuffer::default();
        let control = ImportControl::new();
        run_download_job_guarded(&events, 4, PROVIDER_D4BUILDS, &control, || {
            panic!("controlled worker panic")
        });
        assert!(control.is_finished());
        let drained = events.drain();
        assert!(
            drained.iter().any(|event| matches!(event,
                BuildImportEvent::Completed { job_id: 4, result: Err(error), .. }
                    if error.kind == ImportErrorKind::Internal
                        && error.message.contains("controlled worker panic"))),
            "{drained:?}"
        );
    }

    // -------------------------------------------------------------------
    // Gates of the Add entry.
    // -------------------------------------------------------------------

    #[test]
    fn the_first_add_of_a_planner_url_shapes_the_d2core_job() {
        // BUG-02: the URL the user pasted must produce the job on the very
        // first click; the provider comes from the clicked tab.
        let job = build_download_job(PROVIDER_D2CORE, "https://www.d2core.com/d4/planner?bd=23tb")
            .expect("gate passes");
        assert!(matches!(job, DownloadJob::D2Core { ref build_id } if build_id == "23tb"));
        let error = build_download_job(PROVIDER_D2CORE, "https://www.d2core.com/d4/builds")
            .err()
            .expect("an overview URL is no build");
        assert!(
            error.contains("is not a valid D2Core planner URL"),
            "{error}"
        );
    }

    // -------------------------------------------------------------------
    // Drain, cancel, and the single-download guard on the real window.
    // -------------------------------------------------------------------

    /// Put `state` into "job `id` of `provider` is running" exactly like
    /// `start_download` does, without spawning a worker.
    fn seed_active(
        app: &AppWindow,
        state: &mut AppState,
        id: u64,
        provider: &str,
    ) -> ImportControl {
        let control = ImportControl::new();
        state.build_import.active_download = Some(ActiveDownload {
            id,
            provider: provider.to_string(),
            subject: "subject".to_string(),
            mobalytics_url: None,
            control: control.clone(),
            started: Instant::now(),
        });
        state.build_import.next_download_id = id + 1;
        app.set_import_download_active(true);
        app.set_import_progress_visible(true);
        app.set_import_progress_failed(false);
        control
    }

    fn completion(
        job_id: u64,
        provider: &str,
        result: Result<CompletedImport, ImportError>,
    ) -> BuildImportEvent {
        BuildImportEvent::Completed {
            job_id,
            provider: provider.to_string(),
            result,
            phases: Vec::new(),
            finished_at: Instant::now(),
        }
    }

    #[test]
    fn a_failed_import_keeps_the_classified_error_open_and_releases_the_guard() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.build_import.builds_dir = Some(std::env::temp_dir().join("d4lc-import-drain-failed"));
        seed_active(&app, &mut state, 1, PROVIDER_MAXROLL);
        state.build_import_events.push(completion(
            1,
            PROVIDER_MAXROLL,
            Err(ImportError::new(
                ImportErrorKind::Timeout,
                "timed out reading response",
            )),
        ));

        drain_build_import_events(&app, &mut state);

        assert!(
            app.get_import_progress_visible(),
            "the failure stays visible"
        );
        assert!(app.get_import_progress_failed());
        assert_eq!(app.get_import_progress_error_title().as_str(), "Timed out");
        assert_eq!(
            app.get_import_progress_error().as_str(),
            "timed out reading response"
        );
        assert!(
            !app.get_import_download_active(),
            "the finished job releases the guard"
        );
        assert!(state.build_import.active_download.is_none());
        assert!(state.logging.messages.iter().any(|message| message
            .message
            .contains("Failed to download from Maxroll (subject): timed out reading response")));

        // Close dismisses the panel.
        handle_progress_close(&app, &mut state);
        assert!(!app.get_import_progress_visible());
    }

    #[test]
    fn a_stored_import_closes_the_panel_and_switches_the_mobalytics_sub_tab() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.build_import.builds_dir =
            Some(std::env::temp_dir().join("d4lc-import-drain-success"));
        seed_active(&app, &mut state, 5, PROVIDER_MOBALYTICS);
        state
            .build_import
            .active_download
            .as_mut()
            .unwrap()
            .mobalytics_url =
            Some("https://mobalytics.gg/diablo-4/profile/some-user/builds/abc123".to_string());
        app.set_import_mobalytics_selected_sub_tab(1);
        state.build_import_events.push(completion(
            5,
            PROVIDER_MOBALYTICS,
            Ok(CompletedImport {
                name: "Build".to_string(),
                build_id: "abc123".to_string(),
                variant_names: Vec::new(),
            }),
        ));

        drain_build_import_events(&app, &mut state);

        assert!(!app.get_import_progress_visible());
        assert!(!app.get_import_download_active());
        assert!(state.build_import.active_download.is_none());
        assert_eq!(app.get_import_mobalytics_selected_sub_tab(), 0);
        assert!(state.logging.messages.iter().any(|message| message
            .message
            .starts_with("Imported Mobalytics build 'Build'")));
    }

    #[test]
    fn events_of_another_job_never_touch_the_dialog() {
        // A cancelled job's late completion (job 1) must not close or fail
        // the panel of the job that runs now (job 2).
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        seed_active(&app, &mut state, 2, PROVIDER_D2CORE);
        state.build_import_events.push(BuildImportEvent::Step {
            job_id: 1,
            step: "Saving build".to_string(),
        });
        state.build_import_events.push(completion(
            1,
            PROVIDER_D2CORE,
            Err(ImportError::new(ImportErrorKind::Network, "late failure")),
        ));
        drain_build_import_events(&app, &mut state);

        assert!(app.get_import_progress_visible());
        assert!(!app.get_import_progress_failed());
        assert!(app.get_import_download_active());
        assert_eq!(state.build_import.active_download.as_ref().unwrap().id, 2);
        assert_ne!(app.get_import_progress_step().as_str(), "Saving build");
    }

    #[test]
    fn cancel_closes_at_once_and_a_new_import_can_start_without_mixing_jobs() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let control = seed_active(&app, &mut state, 7, PROVIDER_D2CORE);

        handle_progress_cancel(&app, &mut state);

        assert!(control.is_cancelled(), "the worker got the cancel signal");
        assert!(!app.get_import_progress_visible());
        assert!(!app.get_import_download_active());
        assert!(state.build_import.active_download.is_none());
        assert_eq!(
            state.build_import.detached_downloads,
            std::slice::from_ref(&control)
        );
        assert!(state
            .logging
            .messages
            .iter()
            .any(|message| message.message == "Import from D2Core cancelled (subject)."));
        // A second click is harmless.
        handle_progress_cancel(&app, &mut state);
        assert!(!app.get_import_progress_visible());

        // The next import runs as job 8; the cancelled job's late success
        // is ignored and never claims the new job's panel.
        let next = seed_active(&app, &mut state, 8, PROVIDER_D2CORE);
        state.build_import_events.push(completion(
            7,
            PROVIDER_D2CORE,
            Ok(CompletedImport {
                name: "old".to_string(),
                build_id: "23tb".to_string(),
                variant_names: Vec::new(),
            }),
        ));
        drain_build_import_events(&app, &mut state);
        assert!(app.get_import_progress_visible());
        assert_eq!(
            state.build_import.active_download.as_ref().unwrap().control,
            next
        );

        // Exit cancels the running job and waits for its worker.
        next.mark_finished();
        control.mark_finished();
        shutdown_downloads(&mut state);
        assert!(next.is_cancelled());
        assert!(state.build_import.active_download.is_none());
        assert!(state.build_import.detached_downloads.is_empty());
    }

    #[test]
    fn a_cancel_after_the_commit_keeps_the_panel_until_the_completion() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let control = seed_active(&app, &mut state, 9, PROVIDER_D2CORE);
        control.commit(|| Ok::<(), ()>(())).unwrap().unwrap();

        handle_progress_cancel(&app, &mut state);

        assert!(
            app.get_import_progress_visible(),
            "the build is already stored"
        );
        assert!(state.build_import.active_download.is_some());
    }

    #[test]
    fn escape_cancels_a_running_import_and_is_unused_without_the_panel() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        assert!(!handle_progress_escape(&app, &mut state));
        let control = seed_active(&app, &mut state, 3, PROVIDER_D4BUILDS);
        assert!(handle_progress_escape(&app, &mut state));
        assert!(control.is_cancelled());
        assert!(!app.get_import_progress_visible());
    }

    #[test]
    fn a_second_download_is_refused_while_one_is_active() {
        // A double click on Add: the second click finds the guard claimed.
        // A refused job touches neither the panel nor the active job.
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let control = seed_active(&app, &mut state, 1, PROVIDER_MOBALYTICS);
        app.set_import_progress_visible(false);
        let error = start_download(
            &app,
            &mut state,
            DownloadJob::Maxroll {
                build_id: "to4erl0e".to_string(),
            },
            Instant::now(),
        )
        .expect_err("a second download must be refused while one is active");
        assert!(error.contains("already in progress"), "got: {error}");
        assert!(!app.get_import_progress_visible());
        assert_eq!(
            state.build_import.active_download.as_ref().unwrap().control,
            control
        );
    }

    #[test]
    #[cfg(not(feature = "browser-download"))]
    fn start_download_claims_no_guard_when_the_capability_is_missing() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        let error = start_download(
            &app,
            &mut state,
            DownloadJob::D2Core {
                build_id: "abcd".to_string(),
            },
            Instant::now(),
        )
        .expect_err("a no-browser build cannot launch the D2Core download");
        assert!(!error.contains("already in progress"), "got: {error}");
        assert!(!app.get_import_download_active());
        assert!(!app.get_import_progress_visible());
        assert!(state.build_import.active_download.is_none());
    }

    #[test]
    fn persisted_builds_dir_reaches_the_dialog_and_the_download_worker() {
        // The configured builds directory must survive the exact startup
        // path (`main.rs` loads `Config/Settings.json`, then hydrates
        // `build_import`) so a restart keeps listing and downloading in the
        // configured directory.
        let root = local_tab_root("builds-dir-persistence");
        let configured = root.join("configured-builds");
        let config = d4lc_config::AppSettings {
            builds_dir: Some(configured.display().to_string()),
            ..d4lc_config::AppSettings::default()
        };
        d4lc_config::save_app_settings(&root, &config).expect("save configured settings");

        let loaded = d4lc_config::load_app_settings(&root).expect("load configured settings");
        let mut state = AppState::default_startup();
        state.hydrate_build_import_from_settings(&loaded);

        assert_eq!(
            state.build_import.builds_dir.as_deref(),
            Some(configured.as_path())
        );
        assert_eq!(
            dialog_builds_dir(PROVIDER_MAXROLL, &state).as_deref(),
            Some(configured.as_path())
        );
        assert_eq!(
            resolve_download_builds_dir(PROVIDER_MAXROLL, state.build_import.builds_dir.clone())
                .expect("a configured directory is used as-is"),
            configured
        );

        std::fs::write(
            root.join(d4lc_config::SETTINGS_PATH),
            r##"{ "SelectedBuildImportWebsite": "Maxroll.gg" }"##,
        )
        .expect("write legacy settings");
        let legacy = d4lc_config::load_app_settings(&root).expect("load legacy settings");
        let mut state = AppState::default_startup();
        state.hydrate_build_import_from_settings(&legacy);
        assert_eq!(state.build_import.builds_dir, None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Live end-to-end import through the real worker: the Add gate, the
    /// real browser / HTTP flow, staging and commit, then the dialog's own
    /// list loader. Prints the measured phases as the diagnostics JSON.
    ///   D4LC_LIVE_IMPORT='D2Core=https://www.d2core.com/d4/planner?bd=23tb' \
    ///     cargo test -p d4lc-app --bin d4lc-app live_import_through_the_worker -- --ignored --nocapture
    /// Several `Provider=url` pairs may be separated by `;`; they run in order
    /// in one process (the second run of a provider shows the warm path).
    #[test]
    #[ignore = "live: real provider sites, real Chromium; set D4LC_LIVE_IMPORT"]
    fn live_import_through_the_worker() {
        let spec = std::env::var("D4LC_LIVE_IMPORT").expect("set D4LC_LIVE_IMPORT");
        let mut failures = Vec::new();
        for pair in spec.split(';').filter(|pair| !pair.trim().is_empty()) {
            let (provider, url) = pair.trim().split_once('=').expect("Provider=url");
            let root = tempfile::TempDir::new().unwrap();
            // D4LC_LIVE_IMPORT_KEEP=<dir> keeps a copy of every stored build
            // for comparing it with the provider page.
            let keep = std::env::var_os("D4LC_LIVE_IMPORT_KEEP").map(PathBuf::from);
            let job = build_download_job(provider, url).expect("the Add gate accepts the URL");
            let subject = job.subject();
            let events = BuildImportEventBuffer::default();
            let control = ImportControl::new();
            let started = Instant::now();
            run_download_job(
                &events,
                1,
                started,
                Some(root.path().to_path_buf()),
                AppSettings::default(),
                job,
                &control,
                &crate::actions::build_download::launch_chrome_session,
            );
            let drained = events.drain();
            let statuses: Vec<String> = drained
                .iter()
                .filter_map(|event| match event {
                    BuildImportEvent::Status { status, .. } => Some(status.clone()),
                    BuildImportEvent::Step { step, .. } => Some(format!("[{step}]")),
                    _ => None,
                })
                .collect();
            let (result, phases, finished_at) = drained
                .into_iter()
                .find_map(|event| match event {
                    BuildImportEvent::Completed {
                        result,
                        phases,
                        finished_at,
                        ..
                    } => Some((result, phases, finished_at)),
                    _ => None,
                })
                .expect("completion");
            let record = super::super::import_diagnostics::import_record_json(
                &ImportRecord {
                    job_id: 1,
                    provider,
                    subject: &subject,
                    result: result.as_ref().map(|done| done.name.as_str()),
                    phases: &phases,
                    ui_finish: None,
                    total: finished_at - started,
                },
                0,
            );
            eprintln!("[live] {provider}: {record}");
            eprintln!("[live] {provider}: statuses {statuses:?}");
            match &result {
                Ok(done) => {
                    let builds = if provider == PROVIDER_MOBALYTICS
                        && !root
                            .path()
                            .join(format!("{}.json", done.build_id))
                            .is_file()
                    {
                        eprintln!("[live] {provider}: stored as a profile '{}'", done.name);
                        Vec::new()
                    } else {
                        let builds =
                            super::super::paths::load_builds_for_provider(provider, root.path())
                                .expect("the stored build lists");
                        assert_eq!(builds.len(), 1, "{provider}: one stored build");
                        builds
                    };
                    for build in &builds {
                        eprintln!(
                            "[live] {provider}: listed '{}' (id {}) with variants {:?}",
                            build.name, build.id, build.variant_names
                        );
                    }
                    if let Some(keep) = &keep {
                        let target = keep.join(provider);
                        std::fs::create_dir_all(&target).unwrap();
                        for name in json_files(root.path()) {
                            if name.ends_with(".json") {
                                std::fs::copy(root.path().join(&name), target.join(&name)).unwrap();
                            }
                        }
                    }
                    let leftovers: Vec<_> = json_files(root.path())
                        .into_iter()
                        .filter(|name| name.starts_with(".d4lc-import"))
                        .collect();
                    assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");
                }
                Err(error) => {
                    failures.push(format!("{provider}: {}: {}", error.kind.code(), error))
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn elapsed_time_reads_naturally() {
        assert_eq!(format_elapsed(Duration::from_millis(4_900)), "4 s");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1 min 05 s");
    }
}
