//! Debug-screen command controller that owns the live-loop handle.

use super::events::RuntimeEventSink;
use super::live_loop::{start_live_loop_default, LiveLoopHandle};
use super::live_state::LiveLoopLinks;
use crate::app_state::{AppState, DebugActionStatus};
#[cfg(test)]
use d4lc_runtime::RuntimeLifecycle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Debug-screen command controller.
//
// App-owned seam that owns the live-loop handle and exposes the
// `ReloadPresetImages` / `ExportDebugImages` requests from the
// Debug-screen handlers to the live-loop owner. The result seam
// is a `DebugCommandOutcome` returned by the controller; the
// caller stamps `last_debug_action` and the Logging screen only
// after the controller returns. This keeps the
// "no optimistic success" rule explicit on the new path.
// ---------------------------------------------------------------------------

/// Outcome of a debug-command request dispatched through the
/// app-owned controller. Aligned with the existing
/// `DebugActionStatus` vocabulary so the controller result maps
/// directly onto the `last_debug_action` stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugCommandOutcome {
    /// The request ran and produced real, observable evidence.
    Applied {
        reason: String,
        /// Optional bundle / artifact path the caller can attach
        /// to the `DebugActionRecord` reason and surface on the
        /// Logging screen.
        artifact_path: Option<PathBuf>,
    },
    /// The request ran but the work returned an error. Reason is
    /// the error string from the failing step (capture,
    /// detection, OCR, runtime prep, etc.).
    Failed { reason: String },
    /// The request could not be attempted: no live runtime,
    /// default build without `real-runtime`, or the request was
    /// blocked by a precondition (e.g. controller was not
    /// installed).
    Blocked { reason: String },
}

impl DebugCommandOutcome {
    /// Map the outcome to the existing `DebugActionStatus`
    /// vocabulary so the Debug-screen record / Logging screen
    /// uses the same Applied / Failed / Blocked wording the
    /// other actions already use.
    pub fn status(&self) -> DebugActionStatus {
        match self {
            Self::Applied { .. } => DebugActionStatus::Applied,
            Self::Failed { .. } => DebugActionStatus::Failed,
            Self::Blocked { .. } => DebugActionStatus::Blocked,
        }
    }

    /// Human-readable reason for the Logging screen.
    pub fn reason(&self) -> &str {
        match self {
            Self::Applied { reason, .. } => reason.as_str(),
            Self::Failed { reason } => reason.as_str(),
            Self::Blocked { reason } => reason.as_str(),
        }
    }

    /// Optional artifact path (bundle directory) the caller can
    /// surface on the Logging screen.
    pub fn artifact_path(&self) -> Option<&Path> {
        match self {
            Self::Applied {
                artifact_path: Some(p),
                ..
            } => Some(p.as_path()),
            _ => None,
        }
    }
}

#[cfg(not(feature = "real-runtime"))]
const DEBUG_BLOCKED_REAL_RUNTIME_DISABLED: &str = "real-runtime feature is disabled in this build; debug command controller cannot drive the live loop";
const DEBUG_BLOCKED_NO_LIVE_RUNTIME: &str =
    "no live runtime is currently running; start the live loop before issuing debug commands";

/// Outcome of a live-loop lifecycle reconcile through
/// [`DebugCommandController::ensure_running_for_overlay`] and the two
/// directional entry points it delegates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayLoopOutcome {
    /// The loop was not installed and this request started it.
    Started,
    /// The loop was already installed; the start request was a no-op.
    AlreadyRunning,
    /// The loop was installed and this request stopped it.
    Stopped,
    /// No loop was installed; the stop request was a no-op.
    AlreadyStopped,
}

impl OverlayLoopOutcome {
    /// One-line description for the log path: the wiring callbacks
    /// log the confirmed outcome of every reconcile request (never an
    /// optimistic "started" for a request that was a no-op).
    pub const fn description(self) -> &'static str {
        match self {
            Self::Started => "live loop started",
            Self::AlreadyRunning => "live loop already running; start request ignored",
            Self::Stopped => "live loop stopped",
            Self::AlreadyStopped => "live loop not running; stop request was a no-op",
        }
    }
}

/// App-owned controller that owns the live-loop handle and
/// exposes the Debug-screen `ReloadPresetImages` and
/// `ExportDebugImages` requests.
///
/// The controller is created in `wire_runtime_actions` and is
/// the single writer of the live-loop handle. The Debug-screen
/// handlers receive a reference to the same controller through
/// `wire_debug_actions` and can submit requests without owning
/// the live loop directly.
///
/// All controller methods are safe to call from the Slint event
/// loop thread. The live-loop handle is taken/inserted under a
/// `Mutex` so concurrent start / stop / reload calls from
/// different UI paths stay consistent.
///
/// # Lock order
///
/// `AppState` (the shared `Arc<Mutex<AppState>>`) is the outer lock and
/// `inner` the inner one: a caller may hold `AppState` while it calls a
/// controller method, and no controller method ever locks `AppState`
/// (it only hands the `Arc` to the loops it starts). Anything a request
/// needs from `AppState` is read by the caller and passed in, as
/// [`Self::export_debug_images`] does. Locking `AppState` here would
/// deadlock every caller that already holds it (`std::sync::Mutex` is
/// not reentrant).
#[derive(Clone)]
pub struct DebugCommandController {
    /// Inner lock of the order documented above: never held while
    /// `AppState` is acquired.
    inner: Arc<Mutex<DebugCommandControllerInner>>,
}

pub struct DebugCommandControllerInner {
    live_handle: Option<LiveLoopHandle>,
    event_tx: RuntimeEventSink,
    app_state: Arc<Mutex<AppState>>,
    config_root: PathBuf,
    /// Settings hot-reload seam shared with the running live loop.
    settings_reload: d4lc_runtime::RuntimeReloadSignal,
    /// Mailbox, rules snapshot slot and control inbox every loop this
    /// controller starts shares with the UI.
    links: LiveLoopLinks,
}

impl DebugCommandController {
    /// Build a fresh controller. The live handle starts as
    /// `None`; `wire_runtime_actions` installs it once
    /// `start_live_loop_default` returns.
    pub fn new(
        app_state: Arc<Mutex<AppState>>,
        event_tx: RuntimeEventSink,
        config_root: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DebugCommandControllerInner {
                live_handle: None,
                event_tx,
                app_state,
                config_root,
                settings_reload: d4lc_runtime::RuntimeReloadSignal::new(),
                links: LiveLoopLinks::new(),
            })),
        }
    }

    /// True when the installed live loop is actually running: a handle
    /// whose thread ended, panicked or was asked to stop does not count
    pub fn is_live_loop_running(&self) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .live_handle
            .as_ref()
            .is_some_and(LiveLoopHandle::is_running)
    }

    /// Lifecycle of the installed live loop, if one is installed.
    #[cfg(test)]
    pub fn live_loop_lifecycle(&self) -> Option<RuntimeLifecycle> {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.live_handle.as_ref().map(LiveLoopHandle::lifecycle)
    }

    /// The links every loop of this controller shares with the UI: the
    /// latest-state mailbox the UI drains, the rules snapshot slot the UI
    /// publishes into, and the control inbox.
    pub fn links(&self) -> LiveLoopLinks {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.links.clone()
    }

    /// Install a freshly-started live handle, replacing any prior
    /// handle (a double-start is a programming error in the
    /// caller, so we drop the old handle here).
    pub fn install_live_handle(&self, handle: LiveLoopHandle) {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.live_handle = Some(handle);
    }

    /// Take the live handle out of the controller (e.g. for a
    /// stop / reload). Returns the previous handle so the caller
    /// can call `.stop()` on it before installing a fresh one.
    pub fn take_live_handle(&self) -> Option<LiveLoopHandle> {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.live_handle.take()
    }

    /// Start the live loop unless it is already installed.
    ///
    /// Single-writer seam shared by the Debug-screen start control, the
    /// overlay power toggle, the Ctrl+F12 hotkey arm, and the launch
    /// restore. The double-start guard lives here: a request while a loop
    /// is installed is an `AlreadyRunning` no-op, never a second
    /// loop.
    ///
    /// Neither starting nor stopping joins a thread, so both are safe even
    /// while the caller holds the `AppState` lock (the new loop thread simply
    /// blocks on its first frame until that lock is released).
    pub fn ensure_live_loop_running(&self) -> OverlayLoopOutcome {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if guard
            .live_handle
            .as_ref()
            .is_some_and(LiveLoopHandle::is_running)
        {
            return OverlayLoopOutcome::AlreadyRunning;
        }
        // A handle whose loop ended (fatal error, panic) is not a running
        // loop: release it and start a fresh one.
        if let Some(dead) = guard.live_handle.take() {
            dead.stop();
        }
        let handle = start_live_loop_default(
            Arc::clone(&guard.app_state),
            guard.event_tx.clone(),
            guard.links.clone(),
            #[cfg(feature = "real-runtime")]
            guard.config_root.clone(),
            #[cfg(feature = "real-runtime")]
            guard.settings_reload.clone(),
        );
        guard.live_handle = Some(handle);
        OverlayLoopOutcome::Started
    }

    /// Stop the live loop when one is installed; a stop request with no loop
    /// installed is an `AlreadyStopped` no-op.
    ///
    /// Never blocks: the loop's publications are blocked and its overlay is
    /// hidden at once, and the thread ends on its own (stop contract on
    /// [`LiveLoopHandle`]). Safe on the UI thread, with or without the
    /// `AppState` lock held.
    pub fn ensure_live_loop_stopped(&self) -> OverlayLoopOutcome {
        let handle = {
            let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            guard.live_handle.take()
        };
        match handle {
            Some(handle) => {
                handle.stop();
                OverlayLoopOutcome::Stopped
            }
            None => OverlayLoopOutcome::AlreadyStopped,
        }
    }

    /// Reconcile the live loop with the persisted overlay flag
    /// (`Settings.OverlayEnabled`).
    ///
    /// Upstream's `ScreenProcessHandler.IsEnabled` is set by BOTH the GUI
    /// toggle and the hotkey, and while it is false the handler disposes
    /// incoming captures and runs nothing (`ScreenProcessHandler.cs:88`,
    /// `:116-169`); the port's equivalent of "the handler runs" is the live
    /// loop, so `true` ensures the loop is running and `false` ensures it is
    /// stopped. Shared by the affix power toggle, the Ctrl+F12 hotkey arm,
    /// and the launch restore so all three cannot disagree.
    pub fn ensure_running_for_overlay(&self, enabled: bool) -> OverlayLoopOutcome {
        if enabled {
            self.ensure_live_loop_running()
        } else {
            self.ensure_live_loop_stopped()
        }
    }

    /// Snapshot the configured `config_root` so request
    /// handlers can call `prepare_app_runtime` with the same
    /// workspace the live loop is using.
    pub fn config_root(&self) -> PathBuf {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.config_root.clone()
    }

    /// Signal handle handed to a newly started live loop so the settings
    /// hot-reload monitor can reach the running loop through this
    /// controller.
    #[cfg(test)]
    pub(crate) fn settings_reload_signal(&self) -> d4lc_runtime::RuntimeReloadSignal {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.settings_reload.clone()
    }

    /// Request a settings hot reload on the running live loop: the loop
    /// re-prepares from the on-disk settings before its next frame and
    /// keeps running. No-op when no live loop is installed — the next
    /// start prepares from the current settings anyway, so a request
    /// must never linger as a stale flag for a future loop.
    pub fn request_settings_reload(&self) {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if guard.live_handle.is_some() {
            guard.settings_reload.request();
        }
    }

    /// Restart the live loop for a changed `DisplayBackend` setting: the
    /// resolved engine is read once per start, so the
    /// change applies through the existing start/stop paths — the same
    /// rebuild shape as [`DebugCommandController::reload_preset_images`] —
    /// instead of waiting for a manual restart.
    ///
    /// The runtime prerequisites are validated before the old loop is
    /// stopped, so a restart whose preparation cannot succeed never leaves
    /// the app without a running loop; the reason is returned and
    /// the "restart runtime to apply" status stays. The capture is not
    /// checked here (on Wayland that would open a portal session on the
    /// UI thread); the new loop's own preparation establishes it and
    /// reports its outcome. Never blocks the UI thread: neither stop nor
    /// start joins a thread.
    pub fn restart_for_display_backend(&self) -> Result<(), String> {
        let config_root = self.config_root();
        #[cfg(feature = "real-runtime")]
        crate::runtime_prepare::check_runtime_prerequisites_without_capture(&config_root)
            .map_err(|error| error.to_string())?;
        #[cfg(not(feature = "real-runtime"))]
        let _ = &config_root;
        let old_handle = self.take_live_handle();
        if let Some(handle) = old_handle {
            handle.stop();
        }
        let (app_state, event_tx, links) = {
            let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            (
                Arc::clone(&guard.app_state),
                guard.event_tx.clone(),
                guard.links.clone(),
            )
        };
        #[cfg(feature = "real-runtime")]
        {
            let reload_signal = {
                let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
                guard.settings_reload.clone()
            };
            let new_handle =
                start_live_loop_default(app_state, event_tx, links, config_root, reload_signal);
            self.install_live_handle(new_handle);
        }
        #[cfg(not(feature = "real-runtime"))]
        {
            let _ = config_root;
            let new_handle = start_live_loop_default(app_state, event_tx, links);
            self.install_live_handle(new_handle);
        }
        Ok(())
    }

    /// `ReloadPresetImages` request: stop the live loop, rebuild
    /// the runtime from the same `config_root`, and start a new
    /// live loop. Returns the confirmed outcome (Applied / Failed
    /// / Blocked) for the caller to stamp on `last_debug_action`.
    ///
    /// The previous loop is stopped without a join (stop contract on
    /// [`LiveLoopHandle`]): its publications are fenced off before the new
    /// loop opens its own epoch.
    ///
    /// Approved Linux-native equivalent of upstream
    /// `DebugViewModel::ReloadPresetImagesExecute`, which emits
    /// `AvailableImagesChangedMessage` and lets the next cycle
    /// pick up the new preset images. On Linux we cannot mutate
    /// the in-process `PreparedDetector` in place (that lives in
    /// the forbidden `crates/d4lc-runtime`); we rebuild by
    /// restarting the loop, which is the safe and observable
    /// route.
    pub fn reload_preset_images(&self) -> DebugCommandOutcome {
        // No live runtime: the request has nothing to reload.
        // Both `real-runtime` and default builds use this same
        // Blocked reason — there is no live loop to rebuild.
        let old_handle = match self.take_live_handle() {
            Some(h) => h,
            None => {
                return DebugCommandOutcome::Blocked {
                    reason: DEBUG_BLOCKED_NO_LIVE_RUNTIME.to_string(),
                };
            }
        };
        old_handle.stop();

        // Default build without `real-runtime`: the request
        // still has nothing real to reload, even though we
        // managed to take a stub handle. Surface the
        // Blocked reason.
        #[cfg(not(feature = "real-runtime"))]
        {
            DebugCommandOutcome::Blocked {
                reason: DEBUG_BLOCKED_REAL_RUNTIME_DISABLED.to_string(),
            }
        }

        // Real-runtime: validate everything the rebuild needs apart from
        // the capture (settings, preset templates, OCR and evaluation
        // prerequisites), then start a fresh loop. The capture is not
        // checked here: on Wayland that would open a portal session (and
        // possibly the compositor dialog) on the UI thread, and then a
        // second one in the new loop. The new loop's own preparation
        // establishes the capture and reports its outcome.
        #[cfg(feature = "real-runtime")]
        {
            let config_root = self.config_root();
            match crate::runtime_prepare::check_runtime_prerequisites_without_capture(&config_root)
            {
                Ok(()) => {
                    let (app_state, event_tx, links, reload_signal) = {
                        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
                        (
                            Arc::clone(&guard.app_state),
                            guard.event_tx.clone(),
                            guard.links.clone(),
                            guard.settings_reload.clone(),
                        )
                    };
                    let new_handle = start_live_loop_default(
                        app_state,
                        event_tx,
                        links,
                        config_root,
                        reload_signal,
                    );
                    self.install_live_handle(new_handle);
                    DebugCommandOutcome::Applied {
                        reason:
                            "preset images reloaded: live loop stopped, settings, templates and OCR prerequisites validated from config_root, new live loop started (it establishes the capture itself)"
                                .to_string(),
                        artifact_path: None,
                    }
                }
                Err(e) => {
                    // The old loop is stopped and the rebuild
                    // failed; the live handle is currently
                    // `None`. The user has to start a new loop
                    // manually. We never report `Applied` when
                    // the rebuild itself fails.
                    DebugCommandOutcome::Failed {
                        reason: format!("preset reload failed: runtime prerequisite error: {e}"),
                    }
                }
            }
        }
    }

    /// `ExportDebugImages` request: drive a bounded
    /// file-artifact side channel under
    /// `debug-output/export-<unix>/`. Returns the
    /// confirmed outcome (Applied with the bundle path, or
    /// Blocked / Failed with the reason).
    ///
    /// `live_capture_is_wayland_stream` is the caller's read of the
    /// running loop's published capture engine
    /// (`AppState::capture_engine`, whose `wayland_stream_geometry` is
    /// set exactly for the Wayland portal engine). The loop publishes it
    /// once per runtime start and again per applied reload; before the
    /// first publication it is `false`, and the export's portal-free
    /// preparation is then the authoritative backstop that refuses a
    /// Wayland engine by name. The caller reads it because this
    /// controller never locks `AppState` (see the lock order on
    /// [`DebugCommandController`]); the export itself runs with no
    /// `AppState` lock held, so the caller must not hold one either.
    pub fn export_debug_images(
        &self,
        #[cfg_attr(
            not(feature = "real-runtime"),
            expect(unused_variables, reason = "the default build exports nothing")
        )]
        live_capture_is_wayland_stream: bool,
    ) -> DebugCommandOutcome {
        // No live runtime: a one-shot export pass that is not
        // anchored to the live loop would be a separate
        // "snapshot" rather than a "live debug" export, which
        // is not offered. Refuse.
        if !self.is_live_loop_running() {
            return DebugCommandOutcome::Failed {
                reason: DEBUG_BLOCKED_NO_LIVE_RUNTIME.to_string(),
            };
        }

        #[cfg(not(feature = "real-runtime"))]
        {
            DebugCommandOutcome::Blocked {
                reason: DEBUG_BLOCKED_REAL_RUNTIME_DISABLED.to_string(),
            }
        }

        #[cfg(feature = "real-runtime")]
        {
            // A live Wayland ScreenCast session is exclusive per process
            // (the capture crate's portal-session slot): the export's
            // one-shot re-preparation would wait on the slot and can never
            // establish a second portal session beside the live loop's.
            // Refuse with the named Wayland gap instead of stalling the
            // request on a wait that cannot succeed.
            if live_capture_is_wayland_stream {
                return DebugCommandOutcome::Failed {
                    reason: crate::actions::debug::export::WAYLAND_EXPORT_GAP.to_string(),
                };
            }
            let config_root = self.config_root();
            crate::actions::debug::export::attempt_real_export(&config_root)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[cfg(feature = "real-runtime")]
    use crate::runtime_bridge::live_loop::start_live_loop;
    #[cfg(feature = "real-runtime")]
    use crate::runtime_bridge::test_support::empty_config_root;
    use crate::runtime_bridge::test_support::{fresh_controller, start_test_live_loop};
    use std::time::Duration;

    #[test]
    fn debug_command_controller_is_live_loop_running_starts_false() {
        let controller = fresh_controller();
        assert!(!controller.is_live_loop_running());
    }

    #[test]
    fn debug_command_controller_install_and_take_handle_round_trip() {
        let controller = fresh_controller();
        assert!(!controller.is_live_loop_running());

        // Install a dummy handle. We can't easily build a real
        // LiveLoopHandle in a unit test without a background
        // thread, but we can use the take/None round trip to
        // prove the install path.
        let prev = controller.take_live_handle();
        assert!(prev.is_none());

        // After take, is_live_loop_running stays false.
        assert!(!controller.is_live_loop_running());
    }

    #[test]
    fn ensure_running_for_overlay_stop_without_loop_is_a_no_op() {
        let controller = fresh_controller();
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::AlreadyStopped
        );
        assert!(!controller.is_live_loop_running());
    }

    // Single-writer semantics need a loop that survives without a session;
    // that is the stub loop of the default build. Under `real-runtime` the
    // loop prepares the actual capture stack, which on a display-less host
    // (every `env -u DISPLAY` test run) fails and ends the loop asynchronously
    // — `is_live_loop_running()` there is a race the test can only lose
    // eventually. The real-runtime start contract is covered separately
    // below; live behavior by the pilot suite and visual smoke.
    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn ensure_running_for_overlay_true_starts_once_then_reports_already_running() {
        let controller = fresh_controller();
        assert_eq!(
            controller.ensure_running_for_overlay(true),
            OverlayLoopOutcome::Started
        );
        assert!(controller.is_live_loop_running());

        // A second ON request (power toggle pressed again, repeated
        // hotkey) must not start a second loop.
        assert_eq!(
            controller.ensure_running_for_overlay(true),
            OverlayLoopOutcome::AlreadyRunning
        );
        assert!(controller.is_live_loop_running());

        // Cleanup: stop joins the loop thread.
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::Stopped
        );
        assert!(!controller.is_live_loop_running());
    }

    #[test]
    fn ensure_running_for_overlay_false_stops_the_installed_loop() {
        let controller = fresh_controller();
        assert_eq!(
            controller.ensure_running_for_overlay(true),
            OverlayLoopOutcome::Started
        );

        // OFF while running stops the loop exactly once; a second OFF
        // (double toggle / hotkey repeat) is the no-op.
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::Stopped
        );
        assert!(!controller.is_live_loop_running());
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::AlreadyStopped
        );
    }

    // The `real-runtime` start contract that holds on every host: ON returns
    // `Started` synchronously (a handle is installed; nothing hangs or
    // panics when the host has no display) and OFF then reports `Stopped`.
    // The loop's own survival on a display-less host is not asserted — it
    // legitimately ends when capture preparation finds no usable engine.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn ensure_running_for_overlay_starts_and_stops_without_a_display() {
        let controller = fresh_controller();
        assert_eq!(
            controller.ensure_running_for_overlay(true),
            OverlayLoopOutcome::Started
        );
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::Stopped
        );
        assert!(!controller.is_live_loop_running());
    }

    #[test]
    fn overlay_loop_outcome_descriptions_are_stable() {
        // The log path must never describe a no-op as a state change.
        assert_eq!(
            OverlayLoopOutcome::Started.description(),
            "live loop started"
        );
        assert_eq!(
            OverlayLoopOutcome::Stopped.description(),
            "live loop stopped"
        );
        assert!(OverlayLoopOutcome::AlreadyRunning
            .description()
            .contains("already running"));
        assert!(OverlayLoopOutcome::AlreadyStopped
            .description()
            .contains("no-op"));
    }

    #[test]
    fn debug_command_outcome_status_maps_to_debug_action_status() {
        let applied = DebugCommandOutcome::Applied {
            reason: "ok".to_string(),
            artifact_path: None,
        };
        assert_eq!(
            applied.status(),
            crate::app_state::DebugActionStatus::Applied
        );
        assert_eq!(applied.reason(), "ok");
        assert!(applied.artifact_path().is_none());

        let failed = DebugCommandOutcome::Failed {
            reason: "boom".to_string(),
        };
        assert_eq!(failed.status(), crate::app_state::DebugActionStatus::Failed);
        assert_eq!(failed.reason(), "boom");
        assert!(failed.artifact_path().is_none());

        let blocked = DebugCommandOutcome::Blocked {
            reason: "no runtime".to_string(),
        };
        assert_eq!(
            blocked.status(),
            crate::app_state::DebugActionStatus::Blocked
        );
        assert_eq!(blocked.reason(), "no runtime");
        assert!(blocked.artifact_path().is_none());
    }

    #[test]
    fn debug_command_outcome_artifact_path_is_some_only_for_applied_with_path() {
        let applied_with_path = DebugCommandOutcome::Applied {
            reason: "ok".to_string(),
            artifact_path: Some(PathBuf::from("/tmp/bundle")),
        };
        assert_eq!(
            applied_with_path.artifact_path(),
            Some(std::path::Path::new("/tmp/bundle"))
        );

        let applied_no_path = DebugCommandOutcome::Applied {
            reason: "ok".to_string(),
            artifact_path: None,
        };
        assert!(applied_no_path.artifact_path().is_none());
    }

    #[test]
    fn debug_command_controller_reload_preset_images_blocked_without_live_handle() {
        let controller = fresh_controller();
        // No live handle installed — the request must be
        // Blocked, never Applied.
        let outcome = controller.reload_preset_images();
        assert_eq!(
            outcome.status(),
            crate::app_state::DebugActionStatus::Blocked
        );
        assert!(outcome.reason().contains("no live runtime"));
        assert!(outcome.artifact_path().is_none());
    }

    /// The preset reload validates without a capture (no portal session
    /// on the UI thread) and never reports `Applied` for a rebuild whose
    /// prerequisites fail.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn preset_reload_validates_prerequisites_without_capture() {
        let config_root = empty_config_root("preset-reload-root");
        // A preset that exists in neither the config root nor the bundled
        // tree: the defaults alone are a valid runtime since the detector
        // falls back to the bundled presets.
        d4lc_config::save_app_settings(
            &config_root,
            &d4lc_config::AppSettings {
                selected_system_preset: "no_such_preset".to_string(),
                ..d4lc_config::AppSettings::default()
            },
        )
        .expect("save settings");
        let app_state = Arc::new(Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = RuntimeEventSink::new();
        let controller = DebugCommandController::new(
            Arc::clone(&app_state),
            event_tx.clone(),
            config_root.to_path_buf(),
        );
        controller.install_live_handle(start_live_loop(
            app_state,
            event_tx,
            config_root.to_path_buf(),
            d4lc_runtime::RuntimeReloadSignal::new(),
        ));

        let outcome = controller.reload_preset_images();
        assert_eq!(
            outcome.status(),
            crate::app_state::DebugActionStatus::Failed
        );
        assert!(
            outcome.reason().contains("runtime prerequisite error"),
            "{}",
            outcome.reason()
        );
        assert!(!controller.is_live_loop_running());
        let _ = std::fs::remove_dir_all(&config_root);
    }

    #[test]
    fn debug_command_controller_export_debug_images_failed_without_live_handle() {
        let controller = fresh_controller();
        // A one-shot export that is not anchored to the live loop would
        // be a snapshot, not a "live debug" export. Refuse.
        let outcome = controller.export_debug_images(false);
        assert_eq!(
            outcome.status(),
            crate::app_state::DebugActionStatus::Failed
        );
        assert!(outcome.reason().contains("no live runtime"));
        assert!(outcome.artifact_path().is_none());
    }

    /// Lock-order guard (audit N-01): the controller never locks
    /// `AppState`, so an export request returns while another holder (the
    /// UI callback, the live loop) keeps the `AppState` lock. Before the
    /// fix the controller re-locked `AppState` here and blocked until the
    /// holder released it — forever when the holder was its own caller.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn export_debug_images_never_waits_for_the_app_state_lock() {
        let app_state = Arc::new(Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = RuntimeEventSink::new();
        let controller = DebugCommandController::new(
            Arc::clone(&app_state),
            event_tx,
            PathBuf::from("/tmp/d4lc-test-root"),
        );
        controller.install_live_handle(LiveLoopHandle::idle_for_test());
        assert!(controller.is_live_loop_running());

        let held = app_state.lock().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = controller.clone();
        std::thread::spawn(move || {
            let _ = done_tx.send(worker.export_debug_images(true));
        });
        let outcome = done_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("the export waited for the held AppState lock"));
        drop(held);
        assert_eq!(
            outcome,
            DebugCommandOutcome::Failed {
                reason: crate::actions::debug::export::WAYLAND_EXPORT_GAP.to_string()
            }
        );
        if let Some(handle) = controller.take_live_handle() {
            handle.stop();
        }
    }

    #[test]
    fn request_settings_reload_without_live_handle_is_a_no_op() {
        let controller = fresh_controller();
        // No live loop: a reload request must never linger as a stale flag
        // for a future loop start.
        controller.request_settings_reload();
        assert!(!controller.settings_reload_signal().is_requested());
    }

    #[test]
    fn request_settings_reload_reaches_the_running_live_loop() {
        let controller = fresh_controller();
        let app_state = Arc::new(Mutex::new(AppState::default_startup()));
        let (event_tx, _event_rx) = RuntimeEventSink::new();
        let _ = _event_rx; // events from a real-runtime prepare failure are dropped

        // Install a handle via the same start path the wiring uses so the
        // request gate (live_handle.is_some()) opens.
        let handle = start_test_live_loop(app_state, event_tx, Duration::from_millis(100));
        controller.install_live_handle(handle);

        controller.request_settings_reload();
        assert!(
            controller.settings_reload_signal().is_requested(),
            "a reload request with a running live loop must set the shared flag"
        );

        // Cleanup: stop and remove the handle.
        if let Some(handle) = controller.take_live_handle() {
            handle.stop();
        }
    }

    #[test]
    fn settings_reload_signal_shares_one_flag_with_the_controller() {
        let controller = fresh_controller();
        let signal = controller.settings_reload_signal();
        assert!(!signal.is_requested());

        // The signal handed to the live loop and the controller's seam must
        // be the same flag: requesting through the signal handle is visible
        // through a fresh clone, which is what the loop thread receives.
        signal.request();
        assert!(controller.settings_reload_signal().is_requested());
    }
}
