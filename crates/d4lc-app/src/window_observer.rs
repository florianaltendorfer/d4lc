//! X11 window-state observer: hide the main window to the tray on minimize.
//!
//! ## Upstream evidence
//!
//! `MainWindowViewModel.cs:328-350` — `WindowStateChangedExecute`:
//!
//! - `if (!_settingsManager.Settings.MinimizeToTray) return;` — the
//!   handler is gated on the `MinimizeToTray` setting.
//! - `case WindowState.Minimized:` — when the main window becomes
//!   minimized it is hidden (`Visibility.Collapsed`) so the companion
//!   lives on in the tray, **unless** `_releaseManager.UpdateAvailable`
//!   is set, in which case the window is restored to `WindowState.Normal`
//!   instead (a pending update must stay visible).
//! - Every other `WindowState` (Normal / Maximized) is a no-op.
//!
//! `MainWindow.xaml:28-29` binds this command to the WPF `StateChanged`
//! event, which fires on every window-state transition.
//!
//! ## Why a poller
//!
//! Slint 1.16 exposes neither a window-state-changed event nor a
//! programmatic minimize request on its Rust API, and winit 0.30 does not
//! forward X11 `WM_STATE` transitions to application code. The
//! Linux adaptation is to **poll the ICCCM `WM_STATE` property** of our
//! own top-level window through the `x11rb-backend` transport on a
//! dedicated low-frequency Slint timer (250 ms, the same `Box::leak`ed
//! `slint::Timer` pattern as `wire_runtime_actions` / `wire_hotkey_actions`).
//! The window manager updates `WM_STATE` for every state transition, so
//! the poller observes the same facts the WPF `StateChanged` event
//! delivered upstream.
//!
//! ## Behavior contract
//!
//! - The tracker starts with an `Unknown` baseline. The first observed
//!   iconic state still counts as a minimize transition: this makes the
//!   `LaunchMinimized=true` startup path behave like upstream, where
//!   `ApplicationLoadedExecute` sets `WindowState.Minimized` and that
//!   *event* drives the hide-to-tray handler (`MainWindowViewModel.cs:116-126`
//!   + `MainWindow.xaml.cs` StateChanged binding).
//! - On `Normal → Iconic` (or `Unknown → Iconic`) the shared
//!   `actions::handle_window_state_changed` decision table runs with
//!   `minimize_to_tray` from live settings and the current capabilities.
//!   Only the `HideToTray` outcome hides the window (via Slint's
//!   `ComponentHandle::hide`, the same mechanism the close-to-tray path
//!   uses through `CloseRequestResponse::HideWindow`).
//!   The window can be restored from the tray through the existing
//!   `tray::set_show_window_hook` show path.
//! - On `Iconic/Hidden → Normal` the handler runs with `is_minimized =
//!   false`, which resets `last_window_state_action` to `NoOp` and keeps
//!   the lifecycle projection consistent after a restore.
//! - Transitions are edge-triggered: repeated iconic readings while the
//!   window stays minimized are `Unchanged` (idempotent), and repeated
//!   handler invocations cannot stack.
//! - `update_pending` comes from `WindowLifecycleState::update_pending`,
//!   which the check-only release check sets when a newer release is
//!   known (`crate::update_check`). Upstream reads the same fact from
//!   `_releaseManager.UpdateAvailable` to keep a pending update visible
//!   instead of hiding the window to the tray
//!   (`MainWindowViewModel.cs:328-350`).
//!
//! ## Capability reporting
//!
//! `WindowLifecycleCapabilities::window_minimize_observer_active` is
//! `true` only while the `WM_STATE` reader is running against the live
//! window. Every failure path (window dropped, backend not X11, X11
//! connect/atom failure, winit access failure) appends a
//! `capability_notes` entry and leaves the flag `false`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::winit_030::WinitWindowAccessor as _;

use crate::app_state::AppState;

// ---------------------------------------------------------------------------
// Pure state machine — no platform code, fully unit-testable.
// ---------------------------------------------------------------------------

/// ICCCM `WM_STATE` values we distinguish. `WithdrawnState` is reported
/// for unmanaged/unmapped windows; after a hide-to-tray the window can
/// drop back to withdrawn, which is a *post-minimize* state and must not
/// re-trigger the hide path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmStateValue {
    /// `WithdrawnState` (0).
    Withdrawn,
    /// `NormalState` (1).
    Normal,
    /// `IconicState` (3).
    Iconic,
}

impl WmStateValue {
    /// Parse the first `WM_STATE` property word (native-endian u32 as
    /// returned by `x11rb` for a format-32 property on the little-endian
    /// connection d4lc negotiates).
    pub fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Withdrawn),
            1 => Some(Self::Normal),
            3 => Some(Self::Iconic),
            _ => None,
        }
    }
}

/// Last observed window state inside the tracker. `Unknown` is the
/// pre-first-reading baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObservedWindowState {
    #[default]
    Unknown,
    Normal,
    Iconic,
    /// Withdrawn/unmapped. Reached after hide-to-tray (or before the
    /// window manager maps the window); never an action trigger itself.
    Hidden,
}

/// What one `WM_STATE` reading means relative to the previous reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStateObservation {
    /// Same state as before, or a state change that carries no action.
    Unchanged,
    /// The window just became iconified (minimized). Edge-triggered:
    /// fired only on the transition into `Iconic`.
    Minimized,
    /// The window came back to the normal state from iconic/hidden
    /// (restore from taskbar or tray). Edge-triggered.
    Restored,
}

/// Edge-triggered `WM_STATE` transition tracker. Mirrors the WPF
/// `StateChanged` event: it fires exactly once per state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MinimizeTransitionTracker {
    last: ObservedWindowState,
}

impl MinimizeTransitionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one reading. `None` (missing/unparseable `WM_STATE`, failed
    /// read) keeps the last state — a transient read failure must never
    /// fabricate a transition.
    pub fn observe(&mut self, reading: Option<WmStateValue>) -> WindowStateObservation {
        let next = match reading {
            Some(WmStateValue::Normal) => ObservedWindowState::Normal,
            Some(WmStateValue::Iconic) => ObservedWindowState::Iconic,
            Some(WmStateValue::Withdrawn) => ObservedWindowState::Hidden,
            // No usable reading: no transition, state unchanged.
            None => return WindowStateObservation::Unchanged,
        };

        if next == self.last {
            return WindowStateObservation::Unchanged;
        }

        let previous = self.last;
        self.last = next;

        match (previous, next) {
            // Into iconic from any baseline (including the startup
            // `Unknown` — see module docs for the LaunchMinimized case).
            (_, ObservedWindowState::Iconic) => WindowStateObservation::Minimized,
            // Back to normal from a minimized/hidden baseline.
            (
                ObservedWindowState::Iconic | ObservedWindowState::Hidden,
                ObservedWindowState::Normal,
            ) => WindowStateObservation::Restored,
            // Withdrawn/hide and first-contact baseline changes carry no
            // action of their own.
            _ => WindowStateObservation::Unchanged,
        }
    }

    /// Current tracked state (test/observability helper).
    #[cfg(test)]
    pub fn last(&self) -> ObservedWindowState {
        self.last
    }
}

/// Poll interval for the live `WM_STATE` reader. Low-frequency on
/// purpose: 250 ms is well under human-perceivable hide latency for a
/// minimize action and negligible next to the 50 ms hotkey/runtime
/// drain timers.
pub const OBSERVER_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Decide whether the observer should hide the window for this action.
///
/// Only `HideToTray` performs a platform hide; every other outcome
/// (`NoOp`, `MinimizeOnly`, `MinimizeToTrayRequestedNoTray`) is already
/// recorded by `handle_window_state_changed` and must stay non-mutating.
pub fn should_hide_window(action: crate::app_state::WindowLifecycleStateAction) -> bool {
    action == crate::app_state::WindowLifecycleStateAction::HideToTray
}

// ---------------------------------------------------------------------------
// Live X11 reader + Slint wiring (feature-gated like the X11 minimize helpers).
// ---------------------------------------------------------------------------

/// Reads the ICCCM `WM_STATE` property of one fixed X11 window over a
/// dedicated connection. Owned by the observer timer closure, so it lives
/// and dies with the event-loop thread — no extra thread, no locking.
#[cfg(feature = "x11rb-backend")]
pub struct WmStateReader {
    conn: x11rb::rust_connection::RustConnection,
    window_id: u32,
    wm_state_atom: u32,
}

#[cfg(feature = "x11rb-backend")]
impl WmStateReader {
    /// Open a reader for `target` (screen index + window id from the raw
    /// window/display handles).
    pub fn connect(screen_num: usize, window_id: u32) -> Result<Self, String> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;

        let (conn, _screen) = x11rb::rust_connection::RustConnection::connect(None)
            .map_err(|error| format!("X11 connect: {error}"))?;
        if screen_num >= conn.setup().roots.len() {
            return Err(format!(
                "X11 screen {screen_num} is out of range for the active display"
            ));
        }

        let wm_state_atom = conn
            .intern_atom(false, b"WM_STATE")
            .map_err(|error| format!("intern_atom WM_STATE: {error}"))?
            .reply()
            .map_err(|error| format!("intern_atom WM_STATE reply: {error}"))?
            .atom;

        Ok(Self {
            conn,
            window_id,
            wm_state_atom,
        })
    }

    /// Read the current `WM_STATE`. `Ok(None)` = property missing or an
    /// unparseable state word (window not yet managed / unmanaged).
    pub fn read(&self) -> Result<Option<WmStateValue>, String> {
        use x11rb::protocol::xproto::ConnectionExt as _;

        let reply = self
            .conn
            .get_property(
                false,
                self.window_id,
                self.wm_state_atom,
                self.wm_state_atom,
                0,
                2,
            )
            .map_err(|error| format!("get_property WM_STATE: {error}"))?
            .reply()
            .map_err(|error| format!("get_property WM_STATE reply: {error}"))?;

        if reply.value.is_empty() {
            return Ok(None);
        }

        let raw = u32::from_le_bytes([
            reply.value[0],
            reply.value.get(1).copied().unwrap_or(0),
            reply.value.get(2).copied().unwrap_or(0),
            reply.value.get(3).copied().unwrap_or(0),
        ]);

        Ok(WmStateValue::from_raw(raw))
    }
}

#[cfg(feature = "x11rb-backend")]
fn build_wm_state_reader(
    winit_window: &slint::winit_030::winit::window::Window,
) -> Result<WmStateReader, String> {
    use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};

    let raw_window_handle = winit_window
        .window_handle()
        .map_err(|error| format!("window_handle: {error}"))?
        .as_raw();
    let raw_display_handle = winit_window
        .display_handle()
        .map_err(|error| format!("display_handle: {error}"))?
        .as_raw();
    let target =
        crate::actions::x11_window_target_from_raw_handles(raw_window_handle, raw_display_handle)?;

    WmStateReader::connect(target.screen_num, target.window_id)
}

/// Mark the minimize observer as active in the shared capability state.
#[cfg(feature = "x11rb-backend")]
fn mark_observer_active(state: &Arc<Mutex<AppState>>) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state
        .window_lifecycle
        .capabilities
        .window_minimize_observer_active = true;
    state.window_lifecycle.capability_notes.push(
        "X11 minimize observer active: polling WM_STATE on the live window (250ms).".to_string(),
    );
}

/// Record an observer failure and leave the capability flag off.
fn record_observer_failure(state: &Arc<Mutex<AppState>>, note: String) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.window_lifecycle.capability_notes.push(note);
}

/// Wire the window-state observer to the live Slint window.
///
/// Called from `main` after `handle_app_loaded` so the observer races the
/// startup minimize the same way the WPF `StateChanged` event did
/// upstream. The function never blocks: it schedules an async task that
/// awaits the live winit window and starts the timer, and every failure
/// is recorded in `WindowLifecycleState.capability_notes`.
pub fn wire_window_state_observer<H>(window: slint::Weak<H>, state: Arc<Mutex<AppState>>)
where
    H: slint::ComponentHandle + 'static,
{
    let window_weak = window;
    let state_for_spawn_error = Arc::clone(&state);
    let spawn_result = slint::spawn_local(async move {
        let Some(app) = window_weak.upgrade() else {
            record_observer_failure(
                &state,
                "Window-state observer not started: the Slint window was dropped before the observer task could run.".to_string(),
            );
            return;
        };

        let ui_window = app.window();
        let winit_window = match ui_window.winit_window().await {
            Ok(winit_window) => winit_window,
            Err(error) => {
                record_observer_failure(
                    &state,
                    format!(
                        "Window-state observer not started: d4lc could not access the live backend window handle: {error}."
                    ),
                );
                return;
            }
        };

        #[cfg(feature = "x11rb-backend")]
        {
            let reader = match build_wm_state_reader(winit_window.as_ref()) {
                Ok(reader) => reader,
                Err(error) => {
                    record_observer_failure(
                        &state,
                        format!(
                            "Window-state observer not started: d4lc could not open the WM_STATE reader for the live window: {error}."
                        ),
                    );
                    return;
                }
            };

            // Wayland / non-X11 backends never reach here: the raw
            // handle classifier errors out first with a recorded note.
            mark_observer_active(&state);
            eprintln!(
                "[d4lc] window observer: polling WM_STATE for window {:#x} every {}ms",
                reader.window_id,
                OBSERVER_POLL_INTERVAL.as_millis()
            );

            start_observer_timer(app.as_weak(), state, reader);
        }

        #[cfg(not(feature = "x11rb-backend"))]
        {
            let _ = (app, winit_window);
            record_observer_failure(
                &state,
                "Window-state observer not started: d4lc was built without the x11rb-backend WM_STATE transport.".to_string(),
            );
        }
    });

    if let Err(error) = spawn_result {
        record_observer_failure(
            &state_for_spawn_error,
            format!(
                "Window-state observer not started: d4lc could not schedule the observer task: {error}."
            ),
        );
    }
}

/// The timer body. Split out so the feature-gated wiring stays readable.
#[cfg(feature = "x11rb-backend")]
fn start_observer_timer<H>(app: slint::Weak<H>, state: Arc<Mutex<AppState>>, reader: WmStateReader)
where
    H: slint::ComponentHandle + 'static,
{
    let mut tracker = MinimizeTransitionTracker::new();
    // One-shot failure latch: when the X11 connection dies the timer
    // keeps its (cheap) tick but stops touching X andAppState, and the
    // gap is recorded exactly once.
    let mut reported_read_failure = false;

    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, OBSERVER_POLL_INTERVAL, move || {
        if reported_read_failure {
            return;
        }

        let reading = match reader.read() {
            Ok(reading) => reading,
            Err(error) => {
                reported_read_failure = true;
                record_observer_failure(
                    &state,
                    format!(
                        "X11 minimize observer stopped: WM_STATE read failed ({error}); the window will no longer auto-hide on minimize."
                    ),
                );
                return;
            }
        };

        match tracker.observe(reading) {
            WindowStateObservation::Unchanged => {}
            WindowStateObservation::Minimized => {
                // Upstream gates the whole handler on MinimizeToTray and
                // `UpdateAvailable` (WindowStateChangedExecute). The live
                // check-only release result feeds the same guard here, so a
                // known update keeps the window visible.
                let action = {
                    let mut state = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let update_pending = state.window_lifecycle.update_pending;
                    crate::actions::handle_window_state_changed(&mut state, true, update_pending)
                };

                if should_hide_window(action) {
                    if let Some(window) = app.upgrade() {
                        if let Err(error) = window.hide() {
                            eprintln!(
                                "[d4lc] window observer: minimized but hide() failed: {error}"
                            );
                        } else {
                            eprintln!(
                                "[d4lc] window observer: minimized → hidden to tray (action={})",
                                action.description()
                            );
                        }
                    }
                }
            }
            WindowStateObservation::Restored => {
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                crate::actions::handle_window_state_changed(&mut state, false, false);
            }
        }
    });

    // Same app-lifetime escape hatch as `wire_runtime_actions` /
    // `wire_hotkey_actions`: the observer runs for the full session.
    Box::leak(Box::new(timer));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::WindowLifecycleCapabilities;

    // -------------------------------------------------------------------
    // WmStateValue::from_raw
    // -------------------------------------------------------------------

    #[test]
    fn wm_state_raw_values_map_to_icccm_states() {
        assert_eq!(WmStateValue::from_raw(0), Some(WmStateValue::Withdrawn));
        assert_eq!(WmStateValue::from_raw(1), Some(WmStateValue::Normal));
        assert_eq!(WmStateValue::from_raw(3), Some(WmStateValue::Iconic));
        assert_eq!(
            WmStateValue::from_raw(2),
            None,
            "states 2+ are not ICCCM states d4lc tracks"
        );
        assert_eq!(WmStateValue::from_raw(99), None);
    }

    // -------------------------------------------------------------------
    // Transition tracker — the WPF `StateChanged` analog
    // -------------------------------------------------------------------

    #[test]
    fn tracker_starts_unknown_and_ignores_missing_readings() {
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(tracker.last(), ObservedWindowState::Unknown);
        assert_eq!(tracker.observe(None), WindowStateObservation::Unchanged);
        assert_eq!(tracker.last(), ObservedWindowState::Unknown);
    }

    #[test]
    fn tracker_unknown_to_normal_is_baseline_not_restore() {
        // The window maps normal at startup; that is not a "restore".
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Unchanged
        );
        assert_eq!(tracker.last(), ObservedWindowState::Normal);
    }

    #[test]
    fn tracker_normal_to_iconic_fires_minimized_exactly_once() {
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Unchanged
        );

        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Minimized
        );
        // Idempotency: the user keeping the window minimized must not
        // re-fire the hide path on every 250ms poll.
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Unchanged
        );
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Unchanged
        );
    }

    #[test]
    fn tracker_unknown_to_iconic_fires_minimized_for_launch_minimized_startup() {
        // Upstream `ApplicationLoadedExecute` sets WindowState.Minimized
        // and the StateChanged event then drives hide-to-tray. The
        // observer starts before the WM settles the startup minimize, so
        // the first reading can be iconic — it must fire.
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Minimized
        );
    }

    #[test]
    fn tracker_iconic_to_normal_fires_restored() {
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Minimized
        );
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Restored
        );
        // Staying restored stays quiet.
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Unchanged
        );
    }

    #[test]
    fn tracker_withdrawn_after_iconic_does_not_refire_and_restores_later() {
        // hide-to-tray can withdraw the window; that must not re-trigger
        // a minimize action, and a later show must restore cleanly.
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Minimized
        );
        assert_eq!(
            tracker.observe(Some(WmStateValue::Withdrawn)),
            WindowStateObservation::Unchanged
        );
        assert_eq!(tracker.last(), ObservedWindowState::Hidden);
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Restored
        );
    }

    #[test]
    fn tracker_transient_read_failure_keeps_state() {
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Unchanged
        );
        // A single dropped reading must not desync the tracker.
        assert_eq!(tracker.observe(None), WindowStateObservation::Unchanged);
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Minimized,
            "a normal→(dropped)→iconic sequence must still fire the minimize"
        );
    }

    #[test]
    fn tracker_full_tray_cycle_is_stable() {
        // minimize → hide(withdraw) → restore-from-tray(normal) →
        // minimize again: the second minimize must fire again.
        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Unchanged
        );
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Minimized
        );
        assert_eq!(
            tracker.observe(Some(WmStateValue::Withdrawn)),
            WindowStateObservation::Unchanged
        );
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Restored
        );
        assert_eq!(
            tracker.observe(Some(WmStateValue::Iconic)),
            WindowStateObservation::Minimized,
            "the second minimize after a full tray cycle must fire again"
        );
    }

    // -------------------------------------------------------------------
    // Hide gating — mirrors the upstream decision table
    // -------------------------------------------------------------------

    #[test]
    fn only_hide_to_tray_action_hides_the_window() {
        use crate::actions::decide_window_state_change;
        use crate::app_state::WindowLifecycleStateAction;

        let tray_caps = WindowLifecycleCapabilities {
            tray_available: true,
            window_hide_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };

        let action = decide_window_state_change(true, true, tray_caps, false);
        assert_eq!(action, WindowLifecycleStateAction::HideToTray);
        assert!(should_hide_window(action));

        // Setting off: record-only, never hide (upstream early-returns).
        let action = decide_window_state_change(true, false, tray_caps, false);
        assert!(!should_hide_window(action));

        // Tray unavailable: no-hide fallback.
        let action = decide_window_state_change(
            true,
            true,
            WindowLifecycleCapabilities::SLINT_1X_LINUX,
            false,
        );
        assert_eq!(
            action,
            WindowLifecycleStateAction::MinimizeToTrayRequestedNoTray
        );
        assert!(!should_hide_window(action));

        // Update pending: upstream restores instead of hiding.
        let action = decide_window_state_change(true, true, tray_caps, true);
        assert_eq!(action, WindowLifecycleStateAction::NoOp);
        assert!(!should_hide_window(action));

        // Restore event: never hides.
        let action = decide_window_state_change(false, true, tray_caps, false);
        assert_eq!(action, WindowLifecycleStateAction::NoOp);
        assert!(!should_hide_window(action));
    }
}

// ---------------------------------------------------------------------------
// Opt-in real X11 session test
//
// Drives the production seam end-to-end against a real window manager:
// a throwaway top-level window is created and mapped, the
// `WM_CHANGE_STATE(IconicState)` minimize transport from
// `actions::send_x11_minimize_request` iconifies
// it, and the production `WmStateReader` + `MinimizeTransitionTracker`
// must observe Minimized/Restored through real `WM_STATE` readback.
//
// Run with:
//   cargo test -p d4lc-app window_observer_real_session -- --ignored --nocapture
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "x11rb-backend"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod real_session_tests {
    use super::*;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, EventMask, WindowClass};
    use x11rb::rust_connection::RustConnection;

    const POLL_STEP: Duration = Duration::from_millis(100);
    const MANAGE_TIMEOUT: Duration = Duration::from_secs(10);
    const TRANSITION_TIMEOUT: Duration = Duration::from_secs(5);

    fn poll_until<F>(timeout: Duration, mut predicate: F) -> Result<(), String>
    where
        F: FnMut() -> Result<bool, String>,
    {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if predicate()? {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("condition not reached within {timeout:?}"));
            }
            std::thread::sleep(POLL_STEP);
        }
    }

    fn send_state_request(
        conn: &RustConnection,
        root: u32,
        target_window: u32,
        wm_change_state_atom: u32,
        state: u32,
        label: &str,
    ) -> Result<(), String> {
        let event = x11rb::protocol::xproto::ClientMessageEvent::new(
            32,
            target_window,
            wm_change_state_atom,
            [state, 0, 0, 0, 0],
        );
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .map_err(|error| format!("send_event {label}: {error}"))?;
        conn.flush()
            .map_err(|error| format!("flush {label}: {error}"))
    }

    /// Real-session proof: minimize + restore a real managed window and
    /// observe both transitions through the production reader/tracker.
    #[test]
    #[ignore = "real X11 session: opt-in; run with `cargo test -p d4lc-app window_observer_real_session -- --ignored --nocapture`"]
    fn window_observer_real_session_minimize_and_restore() {
        if std::env::var_os("DISPLAY").is_none() {
            panic!("the real-session test requires a real X11 DISPLAY");
        }
        println!(
            "session: XDG_SESSION_TYPE={:?} DISPLAY={:?}",
            std::env::var("XDG_SESSION_TYPE").ok(),
            std::env::var("DISPLAY").ok()
        );

        let (conn, screen_num) = RustConnection::connect(None).expect("X11 connect");
        let root = conn.setup().roots[screen_num].root;
        let wm_change_state_atom = conn
            .intern_atom(false, b"WM_CHANGE_STATE")
            .expect("send intern_atom WM_CHANGE_STATE")
            .reply()
            .expect("intern_atom WM_CHANGE_STATE reply")
            .atom;

        // Throwaway managed top-level window on the real desktop.
        let window_id = conn.generate_id().expect("generate_id");
        conn.create_window(
            0,
            window_id,
            root,
            0,
            0,
            200,
            150,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &x11rb::protocol::xproto::CreateWindowAux::new(),
        )
        .expect("send create_window");
        conn.map_window(window_id).expect("send map_window");
        conn.flush().expect("flush after map");
        println!("probe window {window_id:#x} created and mapped on root {root:#x}");

        let reader = WmStateReader::connect(screen_num, window_id)
            .expect("production WmStateReader::connect");

        // 1. The window manager must manage the window (WM_STATE appears
        //    as NormalState) before any transition can be observed.
        poll_until(MANAGE_TIMEOUT, || {
            Ok(reader.read()? == Some(WmStateValue::Normal))
        })
        .expect("WM never managed the probe window (no NormalState WM_STATE)");
        println!("managed: WM_STATE=NormalState (1)");

        let mut tracker = MinimizeTransitionTracker::new();
        assert_eq!(
            tracker.observe(reader.read().expect("baseline read")),
            WindowStateObservation::Unchanged,
            "the pre-minimize baseline must not fire"
        );

        // 2. Real minimize through the `WM_CHANGE_STATE` transport; the
        //    observer must see Minimized via WM_STATE.
        send_state_request(
            &conn,
            root,
            window_id,
            wm_change_state_atom,
            crate::actions::X11_ICONIC_STATE,
            "minimize",
        )
        .expect("send WM_CHANGE_STATE IconicState");
        println!("sent WM_CHANGE_STATE(IconicState=3) to window {window_id:#x}");

        poll_until(TRANSITION_TIMEOUT, || {
            Ok(matches!(
                reader.read()?,
                Some(WmStateValue::Iconic) | Some(WmStateValue::Withdrawn)
            ))
        })
        .expect("window did not become iconic after WM_CHANGE_STATE(IconicState)");
        let iconic_reading = reader.read().expect("iconic read");
        println!("minimized: WM_STATE={iconic_reading:?}");
        assert_eq!(
            tracker.observe(iconic_reading),
            WindowStateObservation::Minimized,
            "real minimize transition must fire exactly one Minimized observation"
        );
        // Idempotency at the real-WM level: extra iconic readings are quiet.
        assert_eq!(
            tracker.observe(reader.read().expect("repeat iconic read")),
            WindowStateObservation::Unchanged
        );

        // 3. Real restore: mapping an iconic top-level window is the
        //    ICCCM deiconify request (4.1.4) — the same transition a
        //    taskbar/tray restore performs; the observer must see
        //    Restored so the lifecycle projection stays consistent.
        conn.map_window(window_id)
            .expect("send map_window deiconify");
        conn.flush().expect("flush deiconify");
        println!("sent XMapWindow (deiconify) to window {window_id:#x}");

        poll_until(TRANSITION_TIMEOUT, || {
            Ok(reader.read()? == Some(WmStateValue::Normal))
        })
        .expect("window did not return to NormalState after deiconify request");
        println!("restored: WM_STATE=NormalState (1)");
        assert_eq!(
            tracker.observe(Some(WmStateValue::Normal)),
            WindowStateObservation::Restored,
            "real restore transition must fire exactly one Restored observation"
        );

        // Cleanup: destroy the probe window.
        conn.destroy_window(window_id).expect("send destroy_window");
        conn.flush().expect("flush after destroy");
        println!(
            "real-session PASS: production WM_STATE reader + tracker observed a real minimize/restore cycle on window {window_id:#x}"
        );
    }
}
