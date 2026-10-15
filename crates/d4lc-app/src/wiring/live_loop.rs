//! Late-bound live-loop controller slot and the runtime-bridge wiring
//! (start/stop live loop, event drain, overlay restore).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;

use crate::actions;
use crate::app_state::AppState;
use crate::runtime_bridge;
use crate::slint_generated::AppWindow;

use super::lock_state;

/// Process-wide live-loop controller published by the composition root
/// (`main.rs`) right after [`wire_runtime_actions`] returns.
///
/// The affix power toggle callback is registered before the runtime wiring
/// runs, and the hotkey drain timer never takes the controller as a
/// parameter, so both read the single-writer controller from here at event
/// time. Same late-bound shape as
/// `actions::settings::current_hotkey_receiver`: the composition root is the
/// only publisher, every consumer just clones the handle. A composition that
/// never publishes one (a headless wiring harness) keeps the flag-only
/// behavior and logs the missing seam instead of claiming a state change.
static LIVE_LOOP_CONTROLLER: Mutex<Option<runtime_bridge::DebugCommandController>> =
    Mutex::new(None);

/// Publish (or replace) the app's single live-loop controller.
///
/// Replacing drops the previous controller, which drops its `LiveLoopHandle`
/// and therefore stops any loop it still owned — a fresh wiring pass can
/// never leave a stale loop running.
pub fn install_live_loop_controller(controller: runtime_bridge::DebugCommandController) {
    let mut slot = match LIVE_LOOP_CONTROLLER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *slot = Some(controller);
}

/// The controller published by [`install_live_loop_controller`], if the
/// composition root has run.
fn live_loop_controller() -> Option<runtime_bridge::DebugCommandController> {
    let slot = match LIVE_LOOP_CONTROLLER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    slot.clone()
}

/// Reconcile the live loop with a persisted overlay flag through the
/// app-owned controller.
///
/// This is the one place the overlay flag (power toggle, Ctrl+F12 hotkey,
/// launch restore) reaches the live-loop lifecycle, so the three paths
/// cannot disagree about the running state. The confirmed outcome is logged:
/// a no-op request never claims a state change.
///
/// Switching the overlay off keeps a running loop: the flag reaches it
/// through the rules snapshot, and the switched-off loop captures nothing,
/// like upstream's capture and process handlers that only flip `IsEnabled`
/// (`ScreenCaptureHandler.cs:91-96`, `ScreenProcessHandler.cs:157-162`).
/// Switching it back on is then immediate instead of re-preparing the
/// templates and the OCR worker. Switching on starts a loop only when none
/// is running.
pub(super) fn apply_overlay_loop_request(enabled: bool, source: &str) {
    match live_loop_controller() {
        Some(controller) if !enabled && controller.is_live_loop_running() => {
            eprintln!(
                "[d4lc] overlay toggle ({source}): overlay off; the live loop idles without capturing"
            );
        }
        Some(controller) => {
            let outcome = controller.ensure_running_for_overlay(enabled);
            eprintln!("[d4lc] overlay toggle ({source}): {}", outcome.description());
        }
        None => eprintln!(
            "[d4lc] overlay toggle ({source}): live-loop controller not installed; only the persisted flag changed"
        ),
    }
}

/// Restore the persisted overlay state at launch.
///
/// The port persists the overlay on/off flag with the rest of the overlay
/// config, and upstream's `ScreenProcessHandler` runs the full
/// capture/detect/OCR/overlay pipeline whenever `IsEnabled` is true
/// (`ScreenProcessHandler.cs:88,116-169`); restoring the flag at launch
/// therefore keeps the overlay live after a restart instead of making the
/// user re-toggle the power button every session. Returns the confirmed
/// lifecycle outcome when the persisted flag is on, `None` when it is off
/// (nothing to restore).
///
/// Caller contract: must not hold the `AppState` lock across the call (see
/// [`apply_overlay_loop_request`]).
pub fn restore_overlay_live_loop(
    app_state: &Arc<Mutex<AppState>>,
    controller: &runtime_bridge::DebugCommandController,
) -> Option<runtime_bridge::OverlayLoopOutcome> {
    let enabled = lock_state(app_state).shell.settings.overlay_enabled;
    if !enabled {
        return None;
    }
    let outcome = controller.ensure_running_for_overlay(true);
    eprintln!("[d4lc] overlay launch restore: {}", outcome.description());
    Some(outcome)
}

/// Wire runtime bridge callbacks (start/stop live loop + event drain).
///
/// Registers:
/// - `start-live-loop` — creates channels, starts live loop, installs the
///   handle on the app-owned `DebugCommandController`
/// - `stop-live-loop` — fences and hides the loop, signals shutdown and
///   clears the handle via the same controller, without blocking
/// - A periodic timer that drains RuntimeEvents into AppState + Slint properties
///
/// Returns the `DebugCommandController` so the Debug-screen wiring can
/// share the live-loop ownership. The controller is also leaked for the
/// app lifetime so any later caller can clone it without taking the
/// return value; the composition root publishes it through
/// [`install_live_loop_controller`] so callbacks registered before this
/// function (the affix power toggle) and callbacks that do not take the
/// controller (the hotkey drain timer) reach the same single writer.
pub fn wire_runtime_actions(
    app: &AppWindow,
    app_state: Arc<Mutex<AppState>>,
    config_root: PathBuf,
) -> runtime_bridge::DebugCommandController {
    let app_weak = app.as_weak();

    // Create the runtime notification channels: bounded control (lifecycle,
    // errors, reload outcomes, capability reports) and bounded log lines
    // Analysis results do not travel here at all — they are the
    // latest-state mailbox the same drain tick reads below.
    let (event_tx, event_rx) = runtime_bridge::RuntimeEventSink::new();

    // App-owned controller. All access to the live-loop handle
    // goes through this controller so the Debug-screen
    // `ReloadPresetImages` / `ExportDebugImages` requests and
    // the start / stop callbacks share one writer.
    let controller = runtime_bridge::DebugCommandController::new(
        Arc::clone(&app_state),
        event_tx.clone(),
        config_root.clone(),
    );

    let event_rx: Arc<std::sync::Mutex<runtime_bridge::RuntimeEventReceiver>> =
        Arc::new(std::sync::Mutex::new(event_rx));

    // ── Start live loop ──────────────────────────────────────────
    // The Debug-screen start control is the manual dev control for the
    // same loop the overlay power toggle drives; the double-start guard
    // and the confirmed outcome live in the controller seam.
    let controller_clone = controller.clone();
    app.on_start_live_loop(move || {
        let outcome = controller_clone.ensure_live_loop_running();
        eprintln!("[d4lc] runtime: {}", outcome.description());
    });

    // ── Stop live loop ───────────────────────────────────────────
    let controller_clone2 = controller.clone();
    app.on_stop_live_loop(move || {
        // Never blocks the UI thread: the loop's publications are fenced
        // and its overlay hidden at once; its thread ends on its own.
        let outcome = controller_clone2.ensure_live_loop_stopped();
        eprintln!("[d4lc] runtime: {}", outcome.description());
    });

    // ── Periodic timer — drain events into UI ────────────────────
    let state_clone2 = Arc::clone(&app_state);
    let rx_clone = Arc::clone(&event_rx);

    // Settings hot-reload observation rides the same 50 ms tick: the drain
    // timer is the existing live-apply seam. In-app settings edits persist
    // through d4lc-config and out-of-app edits land in the same file, so
    // one stamped-file observation covers both; a preparation-relevant
    // change requests a hot reload on the running loop, a broken file
    // surfaces as a non-fatal status.
    let mut reload_monitor =
        runtime_bridge::settings_reload::SettingsReloadMonitor::new(&config_root);
    let reload_controller = controller.clone();

    // The runtime side evaluates rules from an immutable snapshot the UI
    // publishes on change (a new rules revision), and hands the accepted
    // presentation back through the bounded latest-state mailbox. The same
    // tick drains both: the UI only projects what the loop accepted.
    let links = controller.links();
    // The paragon panels are sized from the overlay font: resolve it now,
    // off the UI and live-loop threads.
    d4lc_overlay_x11::text::preload_system_face();
    let mut rules_publisher = runtime_bridge::RulesPublisher::new(links.rules.clone());
    let live_mailbox = links.mailbox.clone();
    let mut drain = actions::runtime::RuntimeDrain::new();

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(50),
        move || {
            if let Some(app) = app_weak.upgrade() {
                let mut state = match state_clone2.lock() {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };
                runtime_bridge::settings_reload::observe_settings_change(
                    &mut reload_monitor,
                    &reload_controller,
                    &mut state,
                );
                rules_publisher.observe(&state);
                let events = match rx_clone.lock() {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };
                actions::runtime::drain_runtime_events(
                    &app,
                    &mut state,
                    &events,
                    &live_mailbox,
                    &mut drain,
                );
            }
        },
    );

    // The timer, event_tx, and controller must live for the
    // entire app lifetime. Box::leak prevents them from being
    // dropped when this function returns.
    Box::leak(Box::new(timer));
    Box::leak(Box::new(event_tx));
    Box::leak(Box::new(controller.clone()));
    controller
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::runtime_bridge::{DebugCommandController, OverlayLoopOutcome};

    /// Controller over a caller-supplied state, mirroring the runtime
    /// wiring's construction (no AppWindow needed for the lifecycle seam).
    fn controller_for(app_state: Arc<Mutex<AppState>>) -> DebugCommandController {
        let (event_tx, _event_rx) = runtime_bridge::RuntimeEventSink::new();
        DebugCommandController::new(app_state, event_tx, PathBuf::from("/tmp/d4lc-test-root"))
    }

    // See debug_commands::ensure_running_for_overlay_true_starts_once for
    // why the loop-liveness assertion runs only in the default build: with
    // `real-runtime` on a display-less test host the loop ends on its own
    // once capture preparation finds no usable engine.
    #[cfg(not(feature = "real-runtime"))]
    #[test]
    fn restore_overlay_live_loop_starts_when_the_persisted_flag_is_on() {
        let mut state = AppState::default_startup();
        state.shell.settings.overlay_enabled = true;
        let app_state = Arc::new(Mutex::new(state));
        let controller = controller_for(Arc::clone(&app_state));

        let outcome = restore_overlay_live_loop(&app_state, &controller);

        assert_eq!(outcome, Some(OverlayLoopOutcome::Started));
        assert!(controller.is_live_loop_running());

        // Cleanup: stop joins the loop thread.
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::Stopped
        );
    }

    // The synchronous restore contract that holds on every host: a
    // persisted overlay flag starts a loop and reports `Started` even when
    // the host has no display.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn restore_overlay_live_loop_reports_started_without_a_display() {
        let mut state = AppState::default_startup();
        state.shell.settings.overlay_enabled = true;
        let app_state = Arc::new(Mutex::new(state));
        let controller = controller_for(Arc::clone(&app_state));

        assert_eq!(
            restore_overlay_live_loop(&app_state, &controller),
            Some(OverlayLoopOutcome::Started)
        );
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::Stopped
        );
    }

    #[test]
    fn restore_overlay_live_loop_does_nothing_when_the_persisted_flag_is_off() {
        let app_state = Arc::new(Mutex::new(AppState::default_startup()));
        assert!(
            !app_state
                .lock()
                .expect("app state lock")
                .shell
                .settings
                .overlay_enabled
        );
        let controller = controller_for(Arc::clone(&app_state));

        assert_eq!(restore_overlay_live_loop(&app_state, &controller), None);
        assert!(!controller.is_live_loop_running());
    }

    // Same-tier split as the restore test above: the slot-sharing contract
    // is asserted through the synchronous outcomes everywhere; the running
    // assertion only in the default build's stub loop.
    #[test]
    fn installed_live_loop_controller_is_the_single_writer_slot() {
        let app_state = Arc::new(Mutex::new(AppState::default_startup()));
        let controller = controller_for(app_state);
        install_live_loop_controller(controller.clone());

        let from_slot = live_loop_controller().expect("slot must hold the installed controller");
        assert!(!from_slot.is_live_loop_running());

        // The slot hands out the same controller (shared inner), so a
        // start through the slot is visible on the installed handle.
        assert_eq!(
            from_slot.ensure_running_for_overlay(true),
            OverlayLoopOutcome::Started
        );
        #[cfg(not(feature = "real-runtime"))]
        assert!(controller.is_live_loop_running());
        assert_eq!(
            controller.ensure_running_for_overlay(false),
            OverlayLoopOutcome::Stopped
        );
    }
}
