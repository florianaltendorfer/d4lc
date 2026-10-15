//! Live-loop thread: start, pacing, settings reload and shutdown handle.

use super::events::{RuntimeEvent, RuntimeEventSink};
#[cfg(not(feature = "real-runtime"))]
use super::live_core::unevaluated_presentation;
#[cfg(feature = "real-runtime")]
use super::live_core::{finish_diagnostics, record_state_change, LiveLoopCore, LiveStep};
use super::live_state;
#[cfg(feature = "real-runtime")]
use super::live_state::OverlayPresenter;
#[cfg(not(feature = "real-runtime"))]
use super::live_state::{DebugStatus, LiveState, Presentation};
use super::live_state::{LifecycleCell, LiveLoopLinks};
#[cfg(feature = "real-runtime")]
use super::overlay::{live_overlay_target, start_live_overlay, StartedLiveOverlay, StartedOverlay};
#[cfg(feature = "real-runtime")]
use super::overlay::{
    log_overlay_status_change, overlay_status_without_surface,
    reconcile_overlay_gate_with_menu_lock,
};
use crate::app_state::AppState;
#[cfg(feature = "real-runtime")]
use d4lc_overlay_x11::OverlayStatus;
#[cfg(feature = "real-runtime")]
use d4lc_runtime::diagnostics::DiagnosticsOutcome;
use d4lc_runtime::RuntimeLifecycle;
#[cfg(feature = "real-runtime")]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Live-loop handle — owner of the background thread and shutdown signal.
// ---------------------------------------------------------------------------

/// Handle returned by `start_live_loop` that allows clean shutdown.
///
/// # Stop contract
///
/// Stopping never waits for the loop thread. [`LiveLoopHandle::request_stop`]
/// closes the loop's publication epoch — from that instant nothing the loop
/// produces reaches the UI — and signals the loop, which hides the overlay
/// before it processes anything else. The thread itself (and the native OCR
/// call it may still be waiting for) ends on its own; a reaper thread joins
/// it and records `Stopped`, or `Failed` for a panicked thread. Until then
/// the lifecycle stays `Stopping`.
///
/// Because no caller joins, holding the `AppState` lock across a stop can no
/// longer deadlock against the loop's per-frame lock.
pub struct LiveLoopHandle {
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    lifecycle: LifecycleCell,
    epoch: u64,
    links: LiveLoopLinks,
}

impl LiveLoopHandle {
    /// Invalidate and block the loop's publications now, and signal it to
    /// stop. Returns immediately.
    pub fn request_stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.links.mailbox.close_epoch(self.epoch);
        self.lifecycle.begin_stop();
    }

    /// Stop the loop without blocking the caller (see the stop contract).
    pub fn stop(mut self) {
        self.request_stop();
        self.release_thread();
    }

    /// Hand the thread to a reaper that joins it off the caller's thread.
    fn release_thread(&mut self) {
        if let Some(thread) = self.thread.take() {
            let lifecycle = self.lifecycle.clone();
            let reaper = thread::Builder::new()
                .name("d4lc-live-loop-reaper".to_string())
                .spawn(move || {
                    let panicked = thread.join().is_err();
                    lifecycle.finish(panicked);
                });
            if reaper.is_err() {
                // No reaper thread: the loop still ends on its own; its own
                // exit path records the terminal state.
                eprintln!("[d4lc] live loop: no reaper thread; the loop ends on its own");
            }
        }
    }

    /// Whether the loop is running: not asked to stop, its thread alive and
    /// its lifecycle `Starting`/`Running`. A thread that ended or panicked
    /// is never reported as running.
    pub fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::SeqCst)
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
            && self.lifecycle.get().accepts_work()
    }

    /// The loop's lifecycle. A finished thread that never recorded a
    /// terminal state panicked: `Failed`.
    #[cfg(test)]
    pub fn lifecycle(&self) -> RuntimeLifecycle {
        let current = self.lifecycle.get();
        let thread_ended = self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished());
        if thread_ended
            && !matches!(
                current,
                RuntimeLifecycle::Stopped | RuntimeLifecycle::Failed
            )
        {
            RuntimeLifecycle::Failed
        } else {
            current
        }
    }
}

#[cfg(test)]
impl LiveLoopHandle {
    /// A handle that reports `Running` until it is stopped, with no capture
    /// behind it: a deterministic "live loop is running" state for tests that
    /// must not depend on a real preparation succeeding on the test host.
    pub fn idle_for_test() -> Self {
        let links = LiveLoopLinks::new();
        let epoch = links.mailbox.open_epoch();
        let lifecycle = LifecycleCell::default();
        lifecycle.set(RuntimeLifecycle::Running);
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            shutdown,
            thread: Some(thread),
            lifecycle,
            epoch,
            links,
        }
    }
}

#[cfg(all(test, not(feature = "real-runtime")))]
impl LiveLoopHandle {
    /// Publication epoch of this loop in the UI mailbox.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The links this loop shares with its controller and the UI.
    pub fn links(&self) -> &LiveLoopLinks {
        &self.links
    }
}

impl Drop for LiveLoopHandle {
    fn drop(&mut self) {
        self.request_stop();
        self.release_thread();
    }
}

/// Start the live runtime loop on a background thread.
///
/// When the `real-runtime` feature is disabled (default), this is a structured
/// stub that demonstrates correct state conversion and channel plumbing without
/// spawning actual capture/OCR/overlay threads: it publishes the rules
/// snapshot's evaluation of the empty analysis each cycle.
/// When `real-runtime` is enabled, the loop uses `d4lc-runtime`'s
/// `process_one_frame()` to drive the real capture/detect/OCR/parse pipeline
/// each cycle.
///
/// The loop gets its own [`LiveLoopLinks`]; use
/// [`start_live_loop_with_links`] to share a controller's links.
#[cfg(not(feature = "real-runtime"))]
pub fn start_live_loop(
    app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    frame_interval: Duration,
) -> LiveLoopHandle {
    start_live_loop_with_links(app_state, event_tx, LiveLoopLinks::new(), frame_interval)
}

/// [`start_live_loop`] over the given links (structured stub build).
#[cfg(not(feature = "real-runtime"))]
pub fn start_live_loop_with_links(
    _app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    links: LiveLoopLinks,
    frame_interval: Duration,
) -> LiveLoopHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let lifecycle = LifecycleCell::default();
    let loop_lifecycle = lifecycle.clone();
    let epoch = links.mailbox.open_epoch();
    let loop_links = links.clone();

    let handle = thread::spawn(move || {
        let mut sequence: u64 = 0;
        let analysis = Arc::new(d4lc_runtime::AnalysisSnapshot::empty(0));
        let mut presentation = Arc::new(unevaluated_presentation(Arc::clone(&analysis)));

        // Notify UI that the loop has started.
        loop_lifecycle.set(RuntimeLifecycle::Running);
        event_tx.send(RuntimeEvent::Started);

        while !shutdown_clone.load(Ordering::SeqCst) && loop_links.mailbox.is_open(epoch) {
            // ── Evaluate the newest rules snapshot ───────────────────
            if let Some(rules) = loop_links.rules.current() {
                if presentation.rules_revision != Some(rules.revision()) {
                    let evaluation = rules.evaluate(&analysis, None, None);
                    presentation = Arc::new(Presentation {
                        analysis: Arc::clone(&analysis),
                        rules_revision: Some(rules.revision()),
                        descriptors: evaluation.descriptors,
                        affix_traces: evaluation.affix_traces,
                    });
                }
            }

            // ── Publish the latest state ────────────────────────────
            // In production, tooltip_found and hotpath_ms come from
            // capture + OCR + rule evaluation.  Here they are stubs.
            // The stub loop creates no overlay surface, so the snapshot
            // carries no overlay status: the UI keeps its pending text.
            loop_links.mailbox.publish(LiveState {
                epoch,
                lifecycle: RuntimeLifecycle::Running,
                presentation: Arc::clone(&presentation),
                debug: DebugStatus {
                    frame_sequence: sequence,
                    hotpath_ms: 0,
                    tooltip_found: false,
                    descriptor_count: presentation.descriptors.len(),
                    generation: presentation.generation(),
                    // The structured stub runs no OCR worker: the phase is
                    // reported as absent, never as a zero measurement.
                    ocr: None,
                    ..DebugStatus::default()
                },
                overlay: live_state::OverlaySnapshot::default(),
                // The structured stub runs no OCR worker, so it never builds
                // a debug bundle.
                debug_bundle: None,
                debug_info_enabled: false,
                published_at: std::time::SystemTime::now(),
            });

            sequence = sequence.wrapping_add(1);
            thread::sleep(frame_interval);
        }

        // ── Shutdown ───────────────────────────────────────────────
        loop_lifecycle.set(RuntimeLifecycle::Stopped);
        event_tx.send(RuntimeEvent::Stopped {
            reason: "shutdown signalled".to_string(),
        });
    });

    LiveLoopHandle {
        shutdown,
        thread: Some(handle),
        lifecycle,
        epoch,
        links,
    }
}

/// Start the live runtime loop on a background thread using the real
/// capture/detect/OCR/parse pipeline.
///
/// Requires the `real-runtime` feature. Each cycle runs one
/// [`LiveLoopCore::step`]: the frame goes through `process_one_frame` and the
/// runtime coordinator, the accepted analysis is evaluated against the newest
/// immutable rules snapshot, and the same presentation is drawn and published
/// to the UI mailbox. After preparation succeeds the loop owns the production
/// overlay start (`start_live_overlay`): the
/// display-backend-selected handle is created once and updated with every
/// frame; its `OverlayStatus` degrades headlessly without stopping the
/// loop.
///
/// When the pipeline encounters a fatal error (no capture backend available,
/// window gone), an `Error` event is emitted and the loop terminates
/// gracefully with the `Failed` lifecycle.
///
/// Preparation loads config, templates, and capture target from
/// `config_root`, and creates an OCR worker with the leptess backend
/// (requires the `leptess-backend` feature).  If preparation fails,
/// an `Error` event is emitted and no fallback placeholder is constructed.
///
/// # Arguments
///
/// * `app_state` — shared application state (read only for the debug capture
///   lock bit and written by the overlay menu-lock reconcile).
/// * `event_tx` — channel for discrete runtime events (lifecycle, log lines).
/// * `config_root` — workspace root path for loading config, templates, and OCR data.
/// * `reload_signal` — settings hot-reload seam. When the settings owner
///   requests a reload, the loop re-prepares from `config_root` on this
///   thread (keeping template/OCR I/O off the UI thread) and swaps the pair
///   in without stopping. A failed re-preparation keeps the previous pair
///   and is surfaced as a non-fatal status event.
///
/// The loop gets its own [`LiveLoopLinks`]; use
/// [`start_live_loop_with_links`] to share a controller's links.
#[cfg(all(feature = "real-runtime", test))]
pub fn start_live_loop(
    app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    config_root: PathBuf,
    reload_signal: d4lc_runtime::RuntimeReloadSignal,
) -> LiveLoopHandle {
    start_live_loop_with_links(
        app_state,
        event_tx,
        LiveLoopLinks::new(),
        config_root,
        reload_signal,
    )
}

/// `start_live_loop` over the given links.
#[cfg(feature = "real-runtime")]
pub fn start_live_loop_with_links(
    app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    links: LiveLoopLinks,
    config_root: PathBuf,
    reload_signal: d4lc_runtime::RuntimeReloadSignal,
) -> LiveLoopHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let lifecycle = LifecycleCell::default();
    let loop_lifecycle = lifecycle.clone();
    let epoch = links.mailbox.open_epoch();
    let loop_links = links.clone();

    let handle = thread::spawn(move || {
        run_live_loop(LiveLoopArgs {
            app_state,
            event_tx,
            links: loop_links,
            epoch,
            lifecycle: loop_lifecycle,
            shutdown: shutdown_clone,
            config_root,
            reload_signal,
        });
    });

    LiveLoopHandle {
        shutdown,
        thread: Some(handle),
        lifecycle,
        epoch,
        links,
    }
}

#[cfg(feature = "real-runtime")]
struct LiveLoopArgs {
    app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    links: LiveLoopLinks,
    epoch: u64,
    lifecycle: LifecycleCell,
    shutdown: Arc<AtomicBool>,
    config_root: PathBuf,
    reload_signal: d4lc_runtime::RuntimeReloadSignal,
}

/// The wait until the tick deadline: sleep-until
/// pacing. A step that overran the interval gets a zero wait — no catch-up
/// bursts, and the next tick processes only the newest frame.
#[cfg(feature = "real-runtime")]
fn remaining_wait(deadline: Option<std::time::Instant>, now: std::time::Instant) -> Duration {
    deadline
        .map(|deadline| deadline.saturating_duration_since(now))
        .unwrap_or_default()
}

/// The effective tick interval: the display pacing, raised to the upstream
/// delay settings' sum when that is larger. Upstream
/// paces its capture round with `ScreenCaptureDelay`
/// (ScreenCaptureHandler.cs:266) and sleeps `OverlayUpdateDelay` after each
/// processing pass (ScreenProcessHandler.cs:148); combined they form the
/// minimum period between processed captures.
fn delay_paced_interval(pacing_interval: Duration, delay_floor: Duration) -> Duration {
    pacing_interval.max(delay_floor)
}

/// The frame-signalled tick wait: sleep to the upstream delay floor, then
/// wake on a frame newer than `seen` (one that arrived during the sleep ends
/// the wait at once) or at the tick deadline. The stream delivers at the
/// display rate; waking on every frame ran the full-frame tooltip search
/// about 55 times a second, three times upstream's capture rate, and kept a
/// CPU core busy while nothing changed.
#[cfg(feature = "real-runtime")]
fn wait_for_paced_frame(
    signal: &d4lc_capture::FrameSignal,
    seen: u64,
    floor: Option<std::time::Instant>,
    deadline: Option<std::time::Instant>,
) -> Option<u64> {
    thread::sleep(remaining_wait(floor, std::time::Instant::now()));
    signal.wait_for(seen, remaining_wait(deadline, std::time::Instant::now()))
}

/// Handles every phase of the live loop reports through.
#[cfg(feature = "real-runtime")]
struct LoopEnv<'a> {
    app_state: &'a Mutex<AppState>,
    event_tx: &'a RuntimeEventSink,
    lifecycle: &'a LifecycleCell,
    config_root: &'a Path,
    diagnostics_context: &'a d4lc_runtime::DiagnosticsContext,
}

/// Body of the real live-loop thread.
#[cfg(feature = "real-runtime")]
fn run_live_loop(args: LiveLoopArgs) {
    let LiveLoopArgs {
        app_state,
        event_tx,
        links,
        epoch,
        lifecycle,
        shutdown,
        config_root,
        reload_signal,
    } = args;

    // `Started` follows a successful preparation: until then the loop
    // reports what it waits on (portal dialog, first stream frame) instead
    // of a running pipeline with no frame.
    event_tx.send(RuntimeEvent::Preparing {
        detail: "preparing the runtime".to_string(),
    });

    let (mut diagnostics, diagnostics_context) = start_diagnostics(&config_root);
    let env = LoopEnv {
        app_state: &app_state,
        event_tx: &event_tx,
        lifecycle: &lifecycle,
        config_root: &config_root,
        diagnostics_context: &diagnostics_context,
    };
    let mut reload = SettingsReloadSeam::new(reload_signal);
    let mut last_processed_frame: u64 = 0;

    let Some((mut runtime, mut session, engine_resolution)) =
        prepare_start_runtime(&env, &shutdown, &mut diagnostics)
    else {
        return;
    };
    session.attach_diagnostics(diagnostics_context.clone());
    session.wake_on_ocr_results(runtime.frame_signal());
    record_state_change(&diagnostics_context, 0, DiagnosticsOutcome::Ok, || {
        format!(
            "applied configuration revision {}: preset '{}'",
            reload.applied_revision,
            runtime.preset_name()
        )
    });
    publish_capture_engine(&app_state, &runtime, &engine_resolution);

    let mut published_overlay_status: Option<OverlayStatus> = None;
    let Some((presenter, absent_surface)) = start_overlay_surface(
        &env,
        &runtime,
        &mut published_overlay_status,
        &mut diagnostics,
    ) else {
        return;
    };
    // ── Game-window observer ──────────────────────────────
    // Focus, minimize, close and move of the captured game window reach the
    // coordinator through the control inbox from a thread of their own.
    let controls = links.controls.clone();
    let mut game_window = GameWindowWatch::start(&runtime, controls.clone());
    game_window.publish(&event_tx);
    let mut core = LiveLoopCore::new(epoch, links, presenter, diagnostics_context.clone());
    if let Some(status) = absent_surface {
        core.set_absent_surface(status);
    }
    lifecycle.set(RuntimeLifecycle::Running);
    event_tx.send(RuntimeEvent::Started);

    // Last-seen overlay menu-lock state for the gate reconcile: the first
    // cycle establishes the baseline, later flips mirror into the app-side
    // overlay gate (upstream ToggleOverlayMessage).
    let mut last_menu_locked: Option<bool> = None;
    // Diagnostic stop reason: a clean shutdown unless a fatal pipeline error
    // ended the loop (the runtime already recorded that discard).
    let mut stop_failure: Option<String> = None;
    // Start of the previous tick, for the per-tick period. The
    // first tick has no period and records no sample.
    let mut last_tick_start: Option<std::time::Instant> = None;
    // Newest capture-frame generation the loop has seen: the
    // Wayland wait returns when the pump delivers a newer one.
    let mut last_seen_frame_generation: u64 = 0;
    // Last menu settings applied to the running overlay surface:
    // the first tick establishes the baseline from the app state, later
    // changes are applied live to the surface.
    let mut last_menu_settings: Option<(bool, u32, u32, bool)> = None;

    while !shutdown.load(Ordering::SeqCst) && core.is_open() {
        let tick_start = std::time::Instant::now();
        reload.drain(
            &env,
            &mut runtime,
            &mut session,
            &mut game_window,
            &controls,
            last_processed_frame,
        );

        let (debug_capture_locked, delay_floor, menu_settings) = read_tick_inputs(&app_state);
        // A changed overlay menu setting reaches the running surface at
        // once (Presentation kind): icon visibility and position update the
        // menu state machine, a changed stored lock re-arms it (the running
        // overlay owns the lock again afterwards —
        // reconcile_overlay_gate_with_menu_lock mirrors observed flips back
        // into the stored value).
        if last_menu_settings != Some(menu_settings) {
            if let Some(presenter) = core.presenter_mut() {
                presenter.apply_menu_settings(
                    menu_settings.0,
                    menu_settings.1,
                    menu_settings.2,
                    Some(menu_settings.3),
                );
            }
            last_menu_settings = Some(menu_settings);
        }

        // Hand the app's bounded diagnostics ring to the next published state
        // the window is the newest events drained so far.
        core.set_recent_diagnostics(diagnostics.ring());
        let step = match core.step(&mut runtime, &mut session, debug_capture_locked) {
            Ok(step) => step,
            Err(e) => {
                // The fatal error already recorded its own discard event
                // inside the pipeline; the loop names it as its stop reason
                // so the session record does not claim a clean stop.
                stop_failure = Some(e.to_string());
                event_tx.send(RuntimeEvent::Error {
                    message: e.to_string(),
                });
                break;
            }
        };
        last_processed_frame = step.frame_sequence;
        game_window.publish(&event_tx);

        // ── Drain the runtime diagnostics into the app sinks ─────
        diagnostics.drain_runtime(diagnostics_context.handle());

        mirror_overlay_menu_lock(
            &env,
            &core,
            &mut session,
            &mut published_overlay_status,
            &mut last_menu_locked,
        );

        let waited = wait_for_next_tick(
            &runtime,
            tick_start,
            delay_floor,
            &mut last_seen_frame_generation,
        );
        if let Some(previous) = last_tick_start {
            record_loop_tick(&core, &step, waited, tick_start, previous);
        }
        last_tick_start = Some(tick_start);
    }

    finish_live_loop(
        &env,
        core,
        session,
        runtime,
        &mut diagnostics,
        last_processed_frame,
        stop_failure,
    );
}

/// Start the app diagnostics session and record the loop start.
#[cfg(feature = "real-runtime")]
fn start_diagnostics(
    config_root: &Path,
) -> (
    crate::diagnostics::AppDiagnostics,
    d4lc_runtime::DiagnosticsContext,
) {
    // ── Diagnostics session ───────────────────────────────
    // The app owns the durable sinks; the runtime session and its OCR
    // worker emit into the handle this record drains once per frame.
    let mut diagnostics = crate::diagnostics::AppDiagnostics::start(
        config_root,
        crate::diagnostics::verbosity_from_env(),
    );
    let diagnostics_context = d4lc_runtime::DiagnosticsContext::new(
        d4lc_runtime::diagnostics::next_session_id(),
        d4lc_runtime::DiagnosticsClock::new(),
        diagnostics.handle().clone(),
        // The first applied configuration of this session.
        d4lc_runtime::Revision::Applied(1),
        d4lc_runtime::Revision::Unknown,
    );
    let session_id = diagnostics_context.session_id();
    record_state_change(&diagnostics_context, 0, DiagnosticsOutcome::Ok, || {
        format!(
            "live loop started: session={session_id} verbosity={}",
            diagnostics.verbosity().as_str()
        )
    });
    if let crate::diagnostics::FileSinkStatus::Unavailable { reason } =
        diagnostics.file_sink_status().clone()
    {
        // A diagnostics sink that cannot write is reported, never silent:
        // the ring keeps the record and the drop counter carries the rest.
        record_state_change(
            &diagnostics_context,
            0,
            DiagnosticsOutcome::failed(format!("diagnostics log unavailable: {reason}")),
            || format!("diagnostics dir: {}", diagnostics.dir().display()),
        );
    }

    // Persist the start records now: preparation can block on the
    // compositor dialog, and a process that exits meanwhile must still
    // leave them on disk.
    diagnostics.drain_runtime(diagnostics_context.handle());
    (diagnostics, diagnostics_context)
}

/// Prepare the runtime from `config_root`. `None` means the loop must not
/// start (stop requested during preparation, or preparation failed); the
/// lifecycle, the events and the diagnostics record are already settled.
#[cfg(feature = "real-runtime")]
fn prepare_start_runtime(
    env: &LoopEnv<'_>,
    shutdown: &AtomicBool,
    diagnostics: &mut crate::diagnostics::AppDiagnostics,
) -> Option<(
    d4lc_runtime::PreparedRuntime,
    d4lc_runtime::RuntimeSession,
    crate::display_backend::CaptureEngineResolution,
)> {
    let diagnostics_context = env.diagnostics_context;
    // ── Prepare runtime from config_root ──────────────────────────
    // Each long-running step is announced before it starts: the status
    // line, the Logging screen, stderr (journal) and the diagnostics log
    // name the wait, so a hidden compositor dialog is never a silent hang.
    let report_progress = |step: crate::runtime_prepare::PrepareProgress| {
        let detail = step.describe();
        eprintln!("[d4lc] runtime preparation: {detail}");
        record_state_change(diagnostics_context, 0, DiagnosticsOutcome::Ok, || {
            format!("runtime preparation: {detail}")
        });
        env.event_tx.send(RuntimeEvent::LogMessage {
            timestamp: crate::clock::hh_mm_now(),
            message: format!("runtime preparation: {detail}"),
        });
        env.event_tx.send(RuntimeEvent::Preparing { detail });
    };
    let prepared = crate::runtime_prepare::prepare_app_runtime_with_progress(
        env.config_root,
        None, // default window query (Diablo title)
        None, // no manual region
        None, // default preset from config
        None, // default language from config
        None, // default mode from config
        crate::runtime_prepare::ReusableCapture::none(),
        &report_progress,
    );
    diagnostics.drain_runtime(diagnostics_context.handle());
    // A stop requested while preparation blocked (a pending compositor
    // dialog cannot be interrupted from here, so the thread only gets here
    // once the dialog is answered or dismissed) ends the loop before it
    // starts anything: the prepared capture session is released at once.
    if shutdown.load(Ordering::SeqCst) {
        drop(prepared);
        record_state_change(diagnostics_context, 0, DiagnosticsOutcome::Ok, || {
            "stopped during runtime preparation".to_string()
        });
        finish_diagnostics(
            diagnostics,
            diagnostics_context,
            "stopped during preparation",
            None,
        );
        env.lifecycle.set(RuntimeLifecycle::Stopped);
        env.event_tx.send(RuntimeEvent::Stopped {
            reason: "stopped during preparation".to_string(),
        });
        return None;
    }
    match prepared {
        Ok((rt, sess, resolution)) => Some((rt, sess, resolution)),
        Err(e) => {
            record_state_change(
                diagnostics_context,
                0,
                DiagnosticsOutcome::failed(format!("runtime preparation failed: {e}")),
                || "runtime preparation".to_string(),
            );
            finish_diagnostics(diagnostics, diagnostics_context, "preparation failed", None);
            env.lifecycle.set(RuntimeLifecycle::Failed);
            env.event_tx.send(RuntimeEvent::Error {
                message: format!("runtime preparation failed: {e}"),
            });
            env.event_tx.send(RuntimeEvent::Stopped {
                reason: "preparation failed".to_string(),
            });
            None
        }
    }
}

/// Publish the resolved capture engine.
///
/// The engine is published once per runtime start: the Debug screen's
/// "Capture engine:" row, and — on the Wayland portal engine — the stream
/// rect the paragon geometry refresh follows instead of an X11 window query
/// (which is blind on a Wayland session).
#[cfg(feature = "real-runtime")]
fn publish_capture_engine(
    app_state: &Mutex<AppState>,
    runtime: &d4lc_runtime::PreparedRuntime,
    resolution: &crate::display_backend::CaptureEngineResolution,
) {
    let wayland_stream_geometry = runtime
        .captures_wayland_stream()
        .then(|| runtime.capture_geometry());
    let mut state = app_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.set_capture_engine(
        format!(
            "{}, {}",
            resolution.status.capture_engine_text(resolution.backend),
            runtime.pacing().debug_text()
        ),
        wayland_stream_geometry,
    );
}

/// Start the overlay surface. `None` means a hard start error stopped the
/// loop (lifecycle, events and diagnostics already settled); otherwise the
/// presenter, or the status of the absent surface.
#[cfg(feature = "real-runtime")]
fn start_overlay_surface(
    env: &LoopEnv<'_>,
    runtime: &d4lc_runtime::PreparedRuntime,
    published_overlay_status: &mut Option<OverlayStatus>,
    diagnostics: &mut crate::diagnostics::AppDiagnostics,
) -> Option<(
    Option<OverlayPresenter<StartedOverlay>>,
    Option<OverlayStatus>,
)> {
    // ── Production overlay start ────────────────────
    // The app owns production overlay start: the GUI live path creates the
    // overlay through the display-backend seam, whatever backend the
    // persisted setting selects. Degradation: a handle with a
    // headless_noop / failed status keeps the loop running; an unresolvable
    // game window logs the reason and runs without a surface; only a hard
    // start error stops the loop.
    //
    // The started handle's `OverlayStatus` is published as soon as it
    // exists and re-published whenever it changes, so the Debug
    // screen's "Overlay:" field reports the real backend state instead of
    // staying at the static default.
    match start_live_overlay(env.config_root, live_overlay_target(runtime)) {
        Ok(StartedLiveOverlay::Created(handle)) => {
            let handle = *handle;
            log_overlay_status_change(handle.status(), published_overlay_status, env.event_tx);
            Some((Some(OverlayPresenter::new(handle)), None))
        }
        Ok(StartedLiveOverlay::WindowUnavailable { reason }) => {
            // No surface exists and none may be claimed: every published state
            // carries the synthesized no-surface status whose limitation is
            // the loop's own reason.
            let status = overlay_status_without_surface(&reason);
            log_overlay_status_change(&status, published_overlay_status, env.event_tx);
            record_state_change(
                env.diagnostics_context,
                0,
                DiagnosticsOutcome::failed(reason.clone()),
                || "no overlay surface: presentation is not submitted".to_string(),
            );
            Some((None, Some(status)))
        }
        Err(error) => {
            record_state_change(
                env.diagnostics_context,
                0,
                DiagnosticsOutcome::failed(format!("overlay start failed: {error}")),
                || "overlay start".to_string(),
            );
            finish_diagnostics(
                diagnostics,
                env.diagnostics_context,
                "overlay start failed",
                None,
            );
            env.lifecycle.set(RuntimeLifecycle::Failed);
            env.event_tx.send(RuntimeEvent::Error {
                message: format!("overlay start failed: {error}"),
            });
            env.event_tx.send(RuntimeEvent::Stopped {
                reason: "overlay start failed".to_string(),
            });
            None
        }
    }
}

/// The settings hot-reload seam of the live loop.
#[cfg(feature = "real-runtime")]
struct SettingsReloadSeam {
    signal: d4lc_runtime::RuntimeReloadSignal,
    /// The configuration revision the running pair was prepared under.
    applied_revision: u64,
    /// The re-prepared engine slot: a settings reload re-runs the
    /// once-per-start detection, and an `auto` start may legitimately
    /// resolve to another engine (for example the game window appearing).
    /// The reload path swaps the runtime without stopping the loop, so the
    /// new resolution is staged here and published on `Applied`.
    staged_engine: Mutex<Option<crate::display_backend::CaptureEngineResolution>>,
}

#[cfg(feature = "real-runtime")]
impl SettingsReloadSeam {
    fn new(signal: d4lc_runtime::RuntimeReloadSignal) -> Self {
        Self {
            signal,
            applied_revision: 1,
            staged_engine: Mutex::new(None),
        }
    }

    /// Settings hot-reload seam: drain a pending reload request before
    /// evaluating the next frame. Re-preparation runs here on the loop
    /// thread so the UI thread only ever sets the shared flag. The loop never
    /// stops on a reload failure. An applied reload is a new configuration
    /// revision: the coordinator invalidates what the old configuration
    /// analyzed and discards the replaced worker's late results
    /// (`ConfigChanged`). The re-prepare closure stages the fresh engine
    /// resolution in the slot; the loop publishes it when the swap is applied.
    fn drain(
        &mut self,
        env: &LoopEnv<'_>,
        runtime: &mut d4lc_runtime::PreparedRuntime,
        session: &mut d4lc_runtime::RuntimeSession,
        game_window: &mut GameWindowWatch,
        controls: &live_state::ControlInbox,
        last_processed_frame: u64,
    ) {
        let event_tx = env.event_tx;
        let diagnostics_context = env.diagnostics_context;
        // The running capture session carries over into the re-preparation:
        // on the Wayland engine a new portal session would conflict with the
        // running one. Taken every tick (a shared handle, nothing on X11) so
        // a request that lands between a peek and the drain cannot miss it.
        let reuse = crate::runtime_prepare::ReusableCapture::from_runtime(runtime);
        let report_reload_progress = |step: crate::runtime_prepare::PrepareProgress| {
            let detail = step.describe();
            eprintln!("[d4lc] settings reload: {detail}");
            event_tx.send(RuntimeEvent::LogMessage {
                timestamp: crate::clock::hh_mm_now(),
                message: format!("settings reload: {detail}"),
            });
        };
        let staged_engine = &self.staged_engine;
        let prepare_replacement = || {
            crate::runtime_prepare::prepare_app_runtime_with_progress(
                env.config_root,
                None,
                None,
                None,
                None,
                None,
                reuse,
                &report_reload_progress,
            )
            .map(|(rt, sess, resolution)| {
                *staged_engine
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(resolution);
                (rt, sess)
            })
            .map_err(|e| e.to_string())
        };
        match apply_settings_reload(
            &self.signal,
            runtime,
            session,
            diagnostics_context,
            &mut self.applied_revision,
            prepare_replacement,
        ) {
            d4lc_runtime::ReloadOutcome::Idle => {}
            d4lc_runtime::ReloadOutcome::Applied => {
                // A re-prepared runtime may have discovered another game
                // window: the observer follows the captured one.
                game_window.follow(runtime, controls.clone());
                // A re-prepared runtime re-runs the once-per-start engine
                // detection: publish the new resolution so the Debug row
                // never claims a stale engine.
                if let Some(resolution) = self
                    .staged_engine
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    publish_capture_engine(env.app_state, runtime, &resolution);
                }
                let applied_config_revision = self.applied_revision;
                let detail = format!("runtime re-prepared: preset '{}'", runtime.preset_name());
                record_state_change(
                    diagnostics_context,
                    last_processed_frame,
                    DiagnosticsOutcome::Ok,
                    || {
                        format!(
                            "applied configuration revision {applied_config_revision}: {detail}"
                        )
                    },
                );
                event_tx.send(RuntimeEvent::SettingsReloaded {
                    detail: detail.clone(),
                    revision: applied_config_revision,
                });
                event_tx.send(RuntimeEvent::LogMessage {
                    timestamp: crate::clock::hh_mm_now(),
                    message: format!("settings reloaded: {detail}"),
                });
            }
            d4lc_runtime::ReloadOutcome::Failed { message } => {
                record_state_change(
                    diagnostics_context,
                    last_processed_frame,
                    DiagnosticsOutcome::failed(format!("settings reload failed: {message}")),
                    || "settings hot reload".to_string(),
                );
                event_tx.send(RuntimeEvent::SettingsReloadFailed {
                    message: message.clone(),
                });
                event_tx.send(RuntimeEvent::LogMessage {
                    timestamp: crate::clock::hh_mm_now(),
                    message: format!("settings reload failed: {message}"),
                });
            }
        }
    }
}

/// The per-tick inputs read from the app state in one lock pass: the debug
/// capture lock (the app-owned bit the Ctrl+F11 hotkey flips), the pacing
/// floor from the upstream delay settings the loop applies live every tick,
/// and the overlay menu settings.
#[cfg(feature = "real-runtime")]
fn read_tick_inputs(app_state: &Mutex<AppState>) -> (bool, Duration, (bool, u32, u32, bool)) {
    let state = app_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let settings = &state.shell.settings;
    (
        state.runtime_status.debug_lock_screencapture,
        Duration::from_millis(u64::from(settings.screen_capture_delay))
            + Duration::from_millis(u64::from(settings.overlay_update_delay)),
        (
            settings.show_overlay_icon,
            settings.overlay_icon_pos_x,
            settings.overlay_icon_pos_y,
            settings.overlay_menu_locked,
        ),
    )
}

/// Overlay status log and menu-lock mirror.
///
/// The published state already carries the handle's post-update status: a
/// draw failure of this very frame is in the snapshot the UI projected. Here
/// the loop only logs a changed signature once, and a menu-lock flip is
/// mirrored into the app-side overlay gate exactly like upstream's
/// ToggleOverlayMessage (lock = overlay on, unlock = off) and into the
/// coordinator at once: an unlocked menu hides the item markers before the
/// next frame (OverlayHandler.cs:131).
#[cfg(feature = "real-runtime")]
fn mirror_overlay_menu_lock(
    env: &LoopEnv<'_>,
    core: &LiveLoopCore<StartedOverlay>,
    session: &mut d4lc_runtime::RuntimeSession,
    published_overlay_status: &mut Option<OverlayStatus>,
    last_menu_locked: &mut Option<bool>,
) {
    let menu_locked = match core.presenter() {
        Some(presenter) => {
            log_overlay_status_change(presenter.status(), published_overlay_status, env.event_tx);
            Some(presenter.status().menu_locked)
        }
        None => None,
    };
    if let Some(menu_locked) = menu_locked {
        if *last_menu_locked != Some(menu_locked) {
            let first_observation = last_menu_locked.is_none();
            let mut state = env
                .app_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match reconcile_overlay_gate_with_menu_lock(
                menu_locked,
                last_menu_locked,
                &mut state,
                env.config_root,
            ) {
                Ok(true) => {
                    env.event_tx.send(RuntimeEvent::LogMessage {
                        timestamp: crate::clock::hh_mm_now(),
                        message: if menu_locked {
                            "overlay menu locked: overlay enabled".to_string()
                        } else {
                            "overlay menu unlocked: overlay disabled".to_string()
                        },
                    });
                }
                Ok(false) => {}
                Err(error) => {
                    eprintln!("[d4lc] overlay gate reconcile failed: {error}");
                }
            }
            drop(state);
            if !first_observation {
                session.coordinator_mut().set_overlay_enabled(menu_locked);
            }
        }
    }
}

/// Pace the next tick; returns how long the loop waited.
///
/// The fixed sleep after the work is gone. On the Wayland portal engine the
/// loop wakes the moment the capture thread delivers a new frame (bounded by
/// one target interval so controls, retries and the reload seam stay live);
/// on X11 it captures on demand and sleeps until the next interval boundary.
/// A step that overran the interval does not wait at all, and the next tick
/// processes only the newest frame — the frame slot is latest-wins.
///
/// Upstream delay settings: ScreenCaptureDelay is the pause between captures
/// (ScreenCaptureHandler.cs:266) and OverlayUpdateDelay the pause after each
/// processing pass (ScreenProcessHandler.cs:148). The loop reads both per
/// tick and raises the tick deadline to their sum when it exceeds the
/// display pacing. On the Wayland engine a delivered frame wakes the wait
/// early, but never before the delay floor (see `wait_for_paced_frame`).
#[cfg(feature = "real-runtime")]
fn wait_for_next_tick(
    runtime: &d4lc_runtime::PreparedRuntime,
    tick_start: std::time::Instant,
    delay_floor: Duration,
    last_seen_frame_generation: &mut u64,
) -> Duration {
    let pacing = runtime.pacing();
    let interval = delay_paced_interval(pacing.interval, delay_floor);
    let deadline = tick_start.checked_add(interval);
    let wait_started = std::time::Instant::now();
    match runtime.frame_signal() {
        Some(signal) => {
            if let Some(generation) = wait_for_paced_frame(
                signal,
                *last_seen_frame_generation,
                tick_start.checked_add(delay_floor),
                deadline,
            ) {
                *last_seen_frame_generation = generation;
            }
        }
        None => thread::sleep(remaining_wait(deadline, std::time::Instant::now())),
    }
    wait_started.elapsed()
}

/// Record the per-tick timing sample of the step that just ran.
#[cfg(feature = "real-runtime")]
fn record_loop_tick(
    core: &LiveLoopCore<StartedOverlay>,
    step: &LiveStep,
    waited: Duration,
    tick_start: std::time::Instant,
    previous_tick_start: std::time::Instant,
) {
    core.loop_timings()
        .record(d4lc_runtime::diagnostics::LoopTickSample {
            capture_us: step.stage_us.capture_us,
            detect_us: step.stage_us.detect_us,
            detect_crop_us: step.stage_us.detect_crop_us,
            detect_gray_us: step.stage_us.detect_gray_us,
            detect_integral_us: step.stage_us.detect_integral_us,
            detect_scan_us: step.stage_us.detect_scan_us,
            detect_scan_threads: step
                .stage_us
                .detect_scan_threads
                .and_then(|threads| u64::try_from(threads).ok()),
            coordinator_us: step.stage_us.coordinator_us,
            present_us: step.present_us,
            present_raster_us: step.present_timings.raster_us,
            present_commit_us: step.present_timings.commit_us,
            sleep_us: d4lc_overlay_x11::OverlayDrawTimings::micros(waited),
            period_us: d4lc_overlay_x11::OverlayDrawTimings::micros(
                tick_start.duration_since(previous_tick_start),
            ),
        });
}

/// Stop: hide first, then release the threads.
///
/// The overlay is cleared and the coordinator stops accepting anything
/// before the session (and its OCR worker, which may sit inside a native
/// OCR call) is released. Until that release returned, the lifecycle stays
/// `Stopping`.
#[cfg(feature = "real-runtime")]
fn finish_live_loop(
    env: &LoopEnv<'_>,
    mut core: LiveLoopCore<StartedOverlay>,
    mut session: d4lc_runtime::RuntimeSession,
    runtime: d4lc_runtime::PreparedRuntime,
    diagnostics: &mut crate::diagnostics::AppDiagnostics,
    last_processed_frame: u64,
    stop_failure: Option<String>,
) {
    core.stop(&mut session, last_processed_frame);
    env.lifecycle.begin_stop();
    let loop_timings_line = core.loop_timings().percentiles().render();
    drop(core);
    let failed = stop_failure.is_some();
    match &stop_failure {
        Some(message) => record_state_change(
            env.diagnostics_context,
            last_processed_frame,
            DiagnosticsOutcome::failed(message.clone()),
            || "live loop stopped on a fatal pipeline error".to_string(),
        ),
        None => record_state_change(
            env.diagnostics_context,
            last_processed_frame,
            DiagnosticsOutcome::Ok,
            || "shutdown signalled".to_string(),
        ),
    }
    session.coordinator_mut().finish(failed);
    drop(session);
    drop(runtime);
    finish_diagnostics(
        diagnostics,
        env.diagnostics_context,
        stop_failure.as_deref().unwrap_or("shutdown signalled"),
        Some(loop_timings_line),
    );
    env.lifecycle.set(if failed {
        RuntimeLifecycle::Failed
    } else {
        RuntimeLifecycle::Stopped
    });
    env.event_tx.send(RuntimeEvent::Stopped {
        reason: "shutdown signalled".to_string(),
    });
}

/// Drain a pending settings reload: on success the freshly prepared pair is
/// swapped in and becomes the next applied configuration revision, so the
/// coordinator invalidates what the old configuration analyzed and discards
/// every OCR result planned under it.
#[cfg(feature = "real-runtime")]
pub(crate) fn apply_settings_reload(
    signal: &d4lc_runtime::RuntimeReloadSignal,
    runtime: &mut d4lc_runtime::PreparedRuntime,
    session: &mut d4lc_runtime::RuntimeSession,
    diagnostics: &d4lc_runtime::DiagnosticsContext,
    applied_config_revision: &mut u64,
    prepare: impl FnOnce() -> Result<
        (d4lc_runtime::PreparedRuntime, d4lc_runtime::RuntimeSession),
        String,
    >,
) -> d4lc_runtime::ReloadOutcome {
    let outcome = d4lc_runtime::apply_pending_reload(signal, runtime, session, prepare);
    if outcome == d4lc_runtime::ReloadOutcome::Applied {
        session.wake_on_ocr_results(runtime.frame_signal());
        *applied_config_revision = applied_config_revision.saturating_add(1);
        diagnostics.set_config_revision(d4lc_runtime::Revision::Applied(*applied_config_revision));
    }
    outcome
}

/// The live loop's game-window observer and the capability report it last
/// published.
#[cfg(feature = "real-runtime")]
struct GameWindowWatch {
    observer: Option<crate::game_window_observer::GameWindowObserver>,
    fixed: Option<crate::game_window_observer::GameWindowObservation>,
    published: Option<crate::game_window_observer::GameWindowObservation>,
}

#[cfg(feature = "real-runtime")]
impl GameWindowWatch {
    fn start(runtime: &d4lc_runtime::PreparedRuntime, controls: live_state::ControlInbox) -> Self {
        use crate::game_window_observer::{self as observer, GameWindowObservation};
        let (observer, fixed) = if runtime.captures_wayland_stream() {
            // A Wayland capture stream has no X11 window id: the KDE
            // window-management source identifies and observes the game
            // window on its own connection. Its Err is already the fixed
            // capability report (unsupported compositor/build, or a start
            // failure naming the cause).
            match observer::start_wayland(controls) {
                Ok(observer) => (Some(observer), None),
                Err(fixed) => (None, Some(fixed)),
            }
        } else {
            match runtime.game_window_id() {
                Some(window) => match observer::start_x11(window, controls) {
                    Ok(observer) => (Some(observer), None),
                    Err(reason) => (None, Some(GameWindowObservation::Failed { reason })),
                },
                None => (
                    None,
                    Some(GameWindowObservation::Unsupported {
                        reason: observer::unsupported_reason(),
                    }),
                ),
            }
        };
        Self {
            observer,
            fixed,
            published: None,
        }
    }

    /// Restart the observer when the runtime captures another window.
    fn follow(
        &mut self,
        runtime: &d4lc_runtime::PreparedRuntime,
        controls: live_state::ControlInbox,
    ) {
        let current = self
            .observer
            .as_ref()
            .map(crate::game_window_observer::GameWindowObserver::window);
        // A Wayland stream's window identity lives in the observer's own
        // connection (`game_window_id` is None there): a re-prepare keeps
        // the running source instead of restarting it on every reload.
        let unchanged = if runtime.captures_wayland_stream() {
            self.observer.is_some()
        } else {
            current == runtime.game_window_id()
        };
        if unchanged && self.fixed.is_none() {
            return;
        }
        let published = self.published.take();
        *self = Self::start(runtime, controls);
        self.published = published;
    }

    fn observation(&self) -> crate::game_window_observer::GameWindowObservation {
        match (&self.observer, &self.fixed) {
            (Some(observer), _) => observer.observation(),
            (None, Some(fixed)) => fixed.clone(),
            (None, None) => crate::game_window_observer::GameWindowObservation::Failed {
                reason: "no observer".to_string(),
            },
        }
    }

    /// Send the capability report when it changed.
    fn publish(&mut self, event_tx: &RuntimeEventSink) {
        let observation = self.observation();
        if self.published.as_ref() == Some(&observation) {
            return;
        }
        let text = observation.text();
        event_tx.send(RuntimeEvent::GameWindowObservation { text: text.clone() });
        event_tx.send(RuntimeEvent::LogMessage {
            timestamp: crate::clock::hh_mm_now(),
            message: format!("game window: {text}"),
        });
        self.published = Some(observation);
    }
}

/// Convenience: start a live loop with a default frame interval (~60 FPS).
#[cfg(not(feature = "real-runtime"))]
pub fn start_live_loop_default(
    app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    links: LiveLoopLinks,
) -> LiveLoopHandle {
    start_live_loop_with_links(app_state, event_tx, links, Duration::from_micros(16_667))
}

/// Start the live loop over `links`. `config_root` is forwarded for real
/// pipeline preparation, `reload_signal` keeps the settings hot-reload seam
/// connected for the new loop.
#[cfg(feature = "real-runtime")]
pub fn start_live_loop_default(
    app_state: Arc<Mutex<AppState>>,
    event_tx: RuntimeEventSink,
    links: LiveLoopLinks,
    config_root: PathBuf,
    reload_signal: d4lc_runtime::RuntimeReloadSignal,
) -> LiveLoopHandle {
    start_live_loop_with_links(app_state, event_tx, links, config_root, reload_signal)
}

/// The Wayland stream's live rate line for the Capture engine row; `None` for engines without a ScreenCast stream.
#[cfg(feature = "wayland-portal")]
pub(super) fn capture_stream_line(runtime: &d4lc_runtime::PreparedRuntime) -> Option<String> {
    let source = runtime.shared_wayland_capture_source()?;
    Some(match source.stream_rate_status() {
        Ok(status) => status.render(),
        Err(error) => format!("stream rates unavailable: {error}"),
    })
}

#[cfg(not(feature = "wayland-portal"))]
pub(super) fn capture_stream_line(_runtime: &d4lc_runtime::PreparedRuntime) -> Option<String> {
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::runtime_bridge::debug_commands::OverlayLoopOutcome;
    #[cfg(not(feature = "real-runtime"))]
    use crate::runtime_bridge::live_state::{LiveStateMailbox, RulesPublisher};
    #[cfg(feature = "real-runtime")]
    use crate::runtime_bridge::test_support::empty_config_root;
    use crate::runtime_bridge::test_support::fresh_controller;
    #[cfg(not(feature = "real-runtime"))]
    use crate::runtime_bridge::test_support::{
        rules_state_with_descriptors, sample_overlay_descriptor_state, start_test_live_loop,
    };

    /// Sleep-until pacing — a 10 ms step of a
    /// 16.7 ms interval waits the remaining ~6.7 ms, an overrunning 20 ms
    /// step waits zero (no catch-up bursts).
    #[cfg(feature = "real-runtime")]
    #[test]
    fn remaining_wait_sleeps_until_the_deadline_and_skips_on_overrun() {
        let tick_start = std::time::Instant::now();
        let deadline = tick_start.checked_add(Duration::from_micros(16_667));

        let after_10ms = tick_start + Duration::from_millis(10);
        let wait = remaining_wait(deadline, after_10ms);
        assert!(
            wait > Duration::from_millis(6) && wait < Duration::from_millis(7),
            "the wait must be the remaining interval, got {wait:?}"
        );

        let after_20ms = tick_start + Duration::from_millis(20);
        assert_eq!(
            remaining_wait(deadline, after_20ms),
            Duration::ZERO,
            "an overrunning step must not wait"
        );
        assert_eq!(remaining_wait(None, tick_start), Duration::ZERO);
    }

    /// A frame delivered right after the tick started is processed at the
    /// delay floor, not at once; a frame after the floor wakes the wait
    /// immediately; without a frame the wait ends at the deadline.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn frame_signalled_wait_holds_the_delay_floor() {
        let signal = d4lc_capture::FrameSignal::new();
        let start = std::time::Instant::now();
        let seen = signal.generation();
        let fresh = signal.bump();
        let woke = wait_for_paced_frame(
            &signal,
            seen,
            start.checked_add(Duration::from_millis(40)),
            start.checked_add(Duration::from_millis(400)),
        );
        let waited = start.elapsed();
        assert_eq!(woke, Some(fresh));
        assert!(
            waited >= Duration::from_millis(40) && waited < Duration::from_millis(300),
            "an early frame waits for the floor: {waited:?}"
        );

        let bumper = signal.clone();
        let later = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            bumper.bump()
        });
        let start = std::time::Instant::now();
        let woke = wait_for_paced_frame(
            &signal,
            fresh,
            start.checked_add(Duration::from_millis(10)),
            start.checked_add(Duration::from_millis(2_000)),
        );
        let waited = start.elapsed();
        assert_eq!(woke, later.join().ok());
        assert!(
            waited < Duration::from_millis(1_000),
            "a frame after the floor wakes the wait: {waited:?}"
        );

        let start = std::time::Instant::now();
        let none = wait_for_paced_frame(
            &signal,
            signal.generation(),
            start.checked_add(Duration::from_millis(5)),
            start.checked_add(Duration::from_millis(30)),
        );
        assert_eq!(none, None);
        assert!(start.elapsed() >= Duration::from_millis(30));
    }

    /// Named decision point: the upstream delay settings form the
    /// tick-period floor — the display pacing applies only when it is the
    /// larger of the two.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn delay_settings_raise_the_tick_period_over_the_display_pacing() {
        // Defaults (50 + 5 ms) sit above the 60 Hz display pacing.
        assert_eq!(
            delay_paced_interval(Duration::from_micros(16_667), Duration::from_millis(55)),
            Duration::from_millis(55)
        );
        // A display rate slower than the delays stays authoritative.
        assert_eq!(
            delay_paced_interval(Duration::from_millis(100), Duration::from_millis(55)),
            Duration::from_millis(100)
        );
        // Zero delays change nothing.
        assert_eq!(
            delay_paced_interval(Duration::from_micros(16_667), Duration::ZERO),
            Duration::from_micros(16_667)
        );
    }

    // -------------------------------------------------------------------
    // Live-loop start/stop lifecycle tests
    // -------------------------------------------------------------------

    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn real_runtime_seam_verification_is_explicitly_skipped_without_feature() {
        eprintln!(
            "skipping real-runtime seam verification: build without --features real-runtime; \
             this target only exercises the structured stub loop"
        );
    }

    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn live_loop_starts_and_stops_without_blocking() {
        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, event_rx) = RuntimeEventSink::new();

        let handle = start_test_live_loop(app_state, event_tx, Duration::from_millis(2));
        let links = handle.links().clone();

        // Should receive a Started event.
        let started = event_rx.recv_timeout(Duration::from_millis(500));
        assert!(matches!(started, Ok(RuntimeEvent::Started)));

        // The per-frame state arrives through the mailbox, not the channel.
        let live = wait_for_live_state(&links.mailbox);
        assert_eq!(live.epoch, handle.epoch());

        // Stop the loop: returns at once, fences the mailbox.
        let epoch = handle.epoch();
        handle.stop();
        assert!(!links.mailbox.is_open(epoch));

        // Should receive Stopped event.
        let stopped = event_rx.recv_timeout(Duration::from_millis(500));
        assert!(
            matches!(stopped, Ok(RuntimeEvent::Stopped { .. })),
            "expected Stopped event"
        );
    }

    /// Diagnostics wiring contract: the real live loop owns the
    /// bounded app sinks — it opens them at start, records its state
    /// transitions, and writes the aggregate record when it stops. This test
    /// runs the real loop with a temp diagnostics directory; it asserts the
    /// durable record, not a specific capture outcome (the environment decides
    /// whether preparation or the overlay surface succeeds).
    #[cfg(feature = "real-runtime")]
    #[test]
    fn real_live_loop_writes_a_bounded_diagnostics_record_and_aggregate() {
        let config_root = empty_config_root("diagnostics-wiring-root");
        let diagnostics_dir = empty_config_root("diagnostics-wiring-sink");
        std::env::set_var(
            crate::diagnostics::DIAGNOSTICS_DIR_ENV_VAR,
            diagnostics_dir.as_os_str(),
        );
        std::env::remove_var(crate::diagnostics::DIAGNOSTICS_DETAIL_ENV_VAR);

        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, event_rx) = RuntimeEventSink::new();
        let handle = start_live_loop(
            app_state,
            event_tx,
            config_root.to_path_buf(),
            d4lc_runtime::RuntimeReloadSignal::new(),
        );
        let first = event_rx.recv_timeout(Duration::from_millis(2_000));
        assert!(matches!(first, Ok(RuntimeEvent::Preparing { .. })));
        // Let the loop reach its first frames (or its preparation error).
        std::thread::sleep(Duration::from_millis(200));
        handle.stop();

        let entries: Vec<String> = std::fs::read_dir(&diagnostics_dir)
            .unwrap_or_else(|error| panic!("diagnostics dir must exist: {error}"))
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("diagnostics-") && name.ends_with(".jsonl"))
            .collect();
        assert!(
            !entries.is_empty(),
            "the loop must create its bounded log file, found: {entries:?}"
        );

        let mut content = String::new();
        for name in &entries {
            content.push_str(
                &std::fs::read_to_string(diagnostics_dir.join(name))
                    .unwrap_or_else(|error| panic!("read {name}: {error}")),
            );
        }
        assert!(
            content.contains("\"record\":\"event\""),
            "the loop must record its state transitions: {content}"
        );
        assert!(
            content.contains("\"phase\":\"state_change\""),
            "the loop start must be recorded as a state transition"
        );
        assert!(
            content.contains("\"record\":\"aggregate\""),
            "the loop stop must write the aggregate statistics record"
        );

        // No detail artifacts with the default verbosity, and the ring/file
        // bounds stay the production ones.
        assert!(
            !diagnostics_dir
                .join(crate::diagnostics::DETAIL_DIR_NAME)
                .exists(),
            "detail diagnostics are opt-in and must write nothing by default"
        );
        std::env::remove_var(crate::diagnostics::DIAGNOSTICS_DIR_ENV_VAR);
        let _ = std::fs::remove_dir_all(&config_root);
        let _ = std::fs::remove_dir_all(&diagnostics_dir);
    }

    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn live_loop_reads_updated_app_state() {
        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, event_rx) = RuntimeEventSink::new();

        // Set overlay descriptor state before starting.
        {
            let mut state = app_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.overlay_descriptor_state = Some(sample_overlay_descriptor_state());
        }

        let handle =
            start_test_live_loop(Arc::clone(&app_state), event_tx, Duration::from_millis(1));

        // Drain Started.
        let _ = event_rx.recv_timeout(Duration::from_millis(200));

        // Read a couple of debug events.
        let _ = event_rx.recv_timeout(Duration::from_millis(200));
        let _ = event_rx.recv_timeout(Duration::from_millis(200));

        // Now update app state mid-loop.
        {
            let mut state = app_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.overlay_descriptor_state = None;
        }

        // Drain more events — loop should not crash on state mutation.
        let _ = event_rx.recv_timeout(Duration::from_millis(200));

        handle.stop();

        // Drain Stopped.
        let _ = event_rx.recv_timeout(Duration::from_millis(200));
    }

    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn live_loop_handle_is_running_tracks_state() {
        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = RuntimeEventSink::new();

        let handle = start_test_live_loop(app_state, event_tx, Duration::from_millis(100));
        assert!(handle.is_running());

        handle.stop();
        // After stop, the thread is joined (taken), so is_running is false.
        // Note: the handle is consumed by stop(), so we test this before stop.
    }

    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn live_loop_drop_stops_thread() {
        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, event_rx) = RuntimeEventSink::new();

        let handle = start_test_live_loop(app_state, event_tx, Duration::from_millis(1));

        // Drain Started.
        let _ = event_rx.recv_timeout(Duration::from_millis(200));

        // Drop the handle — this should signal shutdown and join the thread.
        drop(handle);

        // Drain all remaining events until Stopped or channel closed. Log
        // lines may have been queued before the shutdown took effect.
        loop {
            match event_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(RuntimeEvent::Stopped { .. }) => break, // expected terminal event
                Ok(_) => continue,                         // any other queued event
                Err(_) => break,                           // channel closed, acceptable on drop
            }
        }
    }

    /// Poll a mailbox (bounded) until the loop published a state.
    #[cfg(not(feature = "real-runtime"))]
    fn wait_for_live_state(mailbox: &LiveStateMailbox) -> LiveState {
        for _ in 0..500 {
            if let Some(state) = mailbox.take() {
                return state;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the live loop published no state");
    }

    /// The loop evaluates the immutable rules snapshot the UI published —
    /// not `AppState` — and publishes the evaluation through the mailbox.
    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn live_loop_evaluates_the_published_rules_snapshot() {
        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = RuntimeEventSink::new();
        let links = LiveLoopLinks::new();
        let mut publisher = RulesPublisher::with_measure(
            links.rules.clone(),
            Arc::new(d4lc_core::text_measure::FixedTextMeasure::HALF_EM),
        );
        assert_eq!(publisher.observe(&rules_state_with_descriptors()), Some(1));

        let handle = start_live_loop_with_links(
            Arc::clone(&app_state),
            event_tx,
            links.clone(),
            Duration::from_millis(1),
        );
        let state = wait_for_live_state(&links.mailbox);
        assert_eq!(state.presentation.rules_revision, Some(1));
        assert!(
            state.debug.descriptor_count > 0,
            "the rules snapshot's descriptors must reach the loop"
        );
        assert_eq!(
            state.debug.descriptor_count,
            state.presentation.descriptors.len()
        );

        // A rules change is a new revision the running loop picks up: the
        // mutation bumps the revision, as every action does.
        let mut changed = rules_state_with_descriptors();
        changed.shell.settings.collapsed_paragonboard_enabled = true;
        changed.bump_rules_input_revision();
        assert_eq!(publisher.observe(&changed), Some(2));
        assert_eq!(
            publisher.observe(&changed),
            None,
            "unchanged: no new revision"
        );
        let mut revision = None;
        for _ in 0..500 {
            if let Some(state) = links.mailbox.take() {
                revision = state.presentation.rules_revision;
                if revision == Some(2) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(revision, Some(2));
        handle.stop();
    }

    // -------------------------------------------------------------------
    // RuntimeController-style tests (start/stop/drop patterns)
    // -------------------------------------------------------------------

    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn runtime_stop_is_idempotent() {
        // Test that dropping a handle stops the loop, and dropping a second
        // handle (or a moved handle) is safe.
        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx, event_rx) = RuntimeEventSink::new();

        let handle = start_test_live_loop(app_state, event_tx, Duration::from_millis(1));

        // Drain Started.
        let _ = event_rx.recv_timeout(Duration::from_millis(200));

        // Call stop explicitly — this consumes the handle.
        handle.stop();

        // Drain all remaining events; should get Stopped eventually.
        let mut found_stopped = false;
        loop {
            match event_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(RuntimeEvent::Stopped { .. }) => {
                    found_stopped = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(found_stopped, "expected Stopped event after handle.stop()");

        // The handle was consumed by stop(), so there is no "second stop"
        // to test on the same handle.  The test verifies that calling
        // stop() once shuts down cleanly without panic.
    }

    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn runtime_controller_start_called_twice_does_not_spawn_duplicate() {
        // This test verifies that calling start on an already-started live
        // loop is safe.  The current architecture creates a new handle per
        // start_live_loop call; starting twice means dropping an old handle
        // (which shuts down the old loop) and creating a new one.
        //
        // The test verifies that dropping + re-starting works cleanly.
        let app_state = Arc::new(std::sync::Mutex::new(AppState::default_startup()));
        let (event_tx1, event_rx1) = RuntimeEventSink::new();

        let handle1 =
            start_test_live_loop(Arc::clone(&app_state), event_tx1, Duration::from_millis(1));

        // Drain Started from loop 1.
        let _ = event_rx1.recv_timeout(Duration::from_millis(200));

        // Drop handle1 to stop loop 1.
        drop(handle1);

        // Start loop 2.
        let (event_tx2, _event_rx2) = RuntimeEventSink::new();
        let handle2 = start_test_live_loop(app_state, event_tx2, Duration::from_millis(1));

        // Wait briefly — should not panic (no duplicate thread error).
        thread::sleep(Duration::from_millis(50));

        // Clean up loop 2.
        drop(handle2);
    }

    /// Stopping never waits for the loop thread — not even when the
    /// thread is stuck in a call it cannot leave (a native OCR call). The
    /// lifecycle stays `Stopping` until the thread really ended.
    #[test]
    fn stop_returns_at_once_and_stays_stopping_until_the_thread_ends() {
        use std::sync::atomic::AtomicBool;
        let links = LiveLoopLinks::new();
        let epoch = links.mailbox.open_epoch();
        let lifecycle = LifecycleCell::default();
        lifecycle.set(RuntimeLifecycle::Running);
        let release = Arc::new(AtomicBool::new(false));
        let thread_release = Arc::clone(&release);
        let thread = std::thread::spawn(move || {
            while !thread_release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let handle = LiveLoopHandle {
            shutdown: Arc::new(AtomicBool::new(false)),
            thread: Some(thread),
            lifecycle: lifecycle.clone(),
            epoch,
            links: links.clone(),
        };
        assert!(handle.is_running());

        let started = std::time::Instant::now();
        handle.stop();
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "stop blocked for {:?}",
            started.elapsed()
        );
        assert!(
            !links.mailbox.is_open(epoch),
            "publications are fenced at once"
        );
        assert_eq!(lifecycle.get(), RuntimeLifecycle::Stopping);

        release.store(true, Ordering::SeqCst);
        for _ in 0..1000 {
            if lifecycle.get() == RuntimeLifecycle::Stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(lifecycle.get(), RuntimeLifecycle::Stopped);
    }

    /// A loop thread that ended or panicked is never reported as
    /// running, and the controller replaces it instead of calling it
    /// `AlreadyRunning`.
    #[test]
    fn a_dead_loop_thread_is_not_running_and_is_replaced() {
        use std::sync::atomic::AtomicBool;
        let links = LiveLoopLinks::new();
        let epoch = links.mailbox.open_epoch();
        let lifecycle = LifecycleCell::default();
        lifecycle.set(RuntimeLifecycle::Running);
        let thread = std::thread::Builder::new()
            .name("d4lc-test-panicking-loop".to_string())
            .spawn(|| panic!("simulated live-loop panic"))
            .unwrap_or_else(|error| panic!("spawn: {error}"));
        let handle = LiveLoopHandle {
            shutdown: Arc::new(AtomicBool::new(false)),
            thread: Some(thread),
            lifecycle,
            epoch,
            links,
        };
        for _ in 0..1000 {
            if !handle.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!handle.is_running());
        assert_eq!(handle.lifecycle(), RuntimeLifecycle::Failed);

        let controller = fresh_controller();
        controller.install_live_handle(handle);
        assert!(!controller.is_live_loop_running());
        assert_eq!(
            controller.live_loop_lifecycle(),
            Some(RuntimeLifecycle::Failed)
        );
        assert_eq!(
            controller.ensure_live_loop_running(),
            OverlayLoopOutcome::Started,
            "a dead handle must not count as running"
        );
        let _ = controller.ensure_live_loop_stopped();
    }
}
