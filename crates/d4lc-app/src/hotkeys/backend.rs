//! Registration report types, backend start-up, and the backend thread handle.

use std::sync::Arc;

use super::wayland::{is_wayland_session, probe_wayland_portal, start_wayland_portal_backend};
use super::x11::{
    build_event_lookup_table, run_event_loop, try_register_x11, HOTKEY_EVENT_POLL_INTERVAL,
};

/// Per-binding registration outcome.
///
/// The variant set is deliberately explicit about *why* a
/// binding is not `Registered` so the UI / log can surface
/// actionable errors instead of silently dropping the hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyRegistrationStatus {
    /// `enabled == false` in config. No attempt was made.
    /// This is the expected state for fresh installs (the
    /// defaults are all disabled).
    Disabled,
    /// X11 was not reachable (DISPLAY missing, Wayland session,
    /// connection failed). Recorded per binding so the UI can
    /// see which bindings were *attempted*.
    Unavailable { reason: String },
    /// Binding string did not parse. Distinct from X11 errors
    /// — the user can fix this in the HotkeyConfig dialog.
    ParseError { detail: String },
    /// X11 returned `BadAccess` for the grab. Most often
    /// another client owns the key combination (competing
    /// global-hotkey daemon, WM shortcut, etc.).
    Conflict { detail: String },
    /// The GlobalShortcuts portal bound the shortcut. The bind
    /// response is portal evidence that the compositor accepted the
    /// binding; the *first* registration went through the
    /// compositor's consent dialog. Compositor-side edits (GNOME
    /// Settings) are not observed by the app.
    Registered,
    /// The user refused the compositor consent dialog / cancelled
    /// the portal bind request: the shortcut was **not** bound.
    /// Reported per binding instead of a fake success.
    Refused { detail: String },
    /// The portal bind request succeeded but its response did not
    /// include this shortcut, so the binding is unconfirmed — the
    /// app does not claim it is active.
    Unconfirmed { detail: String },
}

/// One row in the per-binding registration report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyRegistration {
    /// Stable action identifier (`"SwitchPreset"`, …) copied
    /// verbatim from `d4lc_config::HotkeyBinding::name`.
    pub name: String,
    /// The user-facing binding string the result refers to
    /// (preserved verbatim for diagnostics even if it failed
    /// to parse).
    pub binding: String,
    pub status: HotkeyRegistrationStatus,
}

/// Backend capability. Mirrors the `WindowLifecycleCapabilities`
/// pattern in `app_state.rs` so the rest of the app can
/// introspect what the current session can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyBackendCapability {
    /// X11 connection was established and the registration
    /// loop ran. Some (or all) bindings may still be
    /// `Conflict` / `ParseError` — that is reported per row.
    X11Available,
    /// No X11 session is reachable. The `registrations` list
    /// will be empty or filled with `Unavailable` rows.
    X11Unavailable,
    /// Wayland session with a reachable GlobalShortcuts portal
    /// backend: `registrations` carries the per-binding portal
    /// outcome (`Registered` / `Refused` / `Unconfirmed` /
    /// `Unavailable` / `Disabled`).
    WaylandPortalAvailable,
    /// Wayland session without portal-backed hotkeys. The
    /// `unavailable_reason` names the missing piece: no
    /// GlobalShortcuts portal backend (sway / vanilla wlroots),
    /// no wayland-hotkeys build feature, or no enabled binding
    /// to register (the portal is not probed in that case).
    WaylandPortalUnavailable,
    /// `init_hotkeys()` has not run yet (e.g. when a test
    /// constructs `HotkeyBackendStatus::default` without going
    /// through startup).
    NotInitialized,
}

/// Top-level backend result. Returned by `init_hotkeys()` and
/// stored on `AppState::hotkey_backend`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBackendStatus {
    pub capability: HotkeyBackendCapability,
    /// Short human-readable reason for `X11Unavailable` or
    /// `WaylandPortalUnavailable`. `None` for `X11Available`,
    /// `WaylandPortalAvailable`, and `NotInitialized`.
    pub unavailable_reason: Option<String>,
    /// Per-binding registration rows in the same order as the
    /// `d4lc_config::HotkeyConfig::bindings` list.
    pub registrations: Vec<HotkeyRegistration>,
}

impl Default for HotkeyBackendStatus {
    fn default() -> Self {
        Self {
            capability: HotkeyBackendCapability::NotInitialized,
            unavailable_reason: None,
            registrations: Vec::new(),
        }
    }
}

/// One configured binding to feed into the
/// registration loop and the persistent event loop. Decouples
/// the hotkey module from `d4lc_config` so the parser /
/// registration path is testable without the schema crate.
///
/// `wiring::wire_hotkey_actions` builds the list from the persisted
/// `d4lc_config::AppSettings::hotkey_config.bindings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyRequest {
    pub name: String,
    pub binding: String,
    pub enabled: bool,
}

/// Probe the global hotkey backend for `requests` and report what each
/// binding can do in this session.
pub fn init_hotkeys_from(requests: &[HotkeyRequest]) -> HotkeyBackendStatus {
    if is_wayland_session() {
        // Registration probe only: report the structured portal
        // outcome without starting the persistent event loop,
        // mirroring the X11 probe contract.
        return probe_wayland_portal(requests).0;
    }
    match try_register_x11(requests) {
        Ok((capability, unavailable_reason, registrations)) => HotkeyBackendStatus {
            capability,
            unavailable_reason,
            registrations,
        },
        Err(reason) => {
            // `Err` from `try_register_x11` is currently unused
            // — every failure is signalled via the `(capability,
            // reason, registrations)` tuple. The branch is here
            // so a future transport-level failure (e.g. a panic
            // guard) can be surfaced without changing the
            // signature.
            build_unavailable_status(requests, reason)
        }
    }
}

fn build_unavailable_status(requests: &[HotkeyRequest], reason: String) -> HotkeyBackendStatus {
    let registrations = requests
        .iter()
        .map(|r| HotkeyRegistration {
            name: r.name.clone(),
            binding: r.binding.clone(),
            status: HotkeyRegistrationStatus::Unavailable {
                reason: reason.clone(),
            },
        })
        .collect();
    HotkeyBackendStatus {
        capability: HotkeyBackendCapability::X11Unavailable,
        unavailable_reason: Some(reason),
        registrations,
    }
}

/// One runtime hotkey event delivered from the backend thread
/// to the UI thread via `mpsc::channel`.
///
/// Carries the stable action name from the original
/// `d4lc_config::HotkeyBinding` (e.g. `"SwitchPreset"`,
/// `"TakeScreenshot"`) so the dispatcher can route to the
/// matching existing UI action handler. The event itself is
/// plain data; the heavy lifting happens on the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyEvent {
    /// Stable action identifier (matches the `HotkeyRequest::name`
    /// that was registered on the backend side).
    pub name: String,
}

impl HotkeyEvent {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Upper bound on how long [`HotkeyBackendHandle::stop`] may
/// block its caller (the UI thread) waiting for the backend
/// thread to finish.
///
/// A healthy stop returns within one
/// [`HOTKEY_EVENT_POLL_INTERVAL`] (the loop notices the flag on
/// its next poll tick) plus the best-effort ungrab pass. The
/// bound only bites when the thread is stuck outside the poll
/// loop — e.g. inside the blocking `check()` round trips of the
/// re-grab phase, inside `RustConnection::connect`, or inside
/// the Wayland portal's activation stream — and the loop cannot
/// be interrupted from here.
pub(super) const HOTKEY_STOP_JOIN_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(500);

/// Granularity of [`HotkeyBackendHandle::stop`]'s bounded wait:
/// fine enough to reap a thread that finishes right after a
/// check, coarse enough not to spin on the caller's thread.
const HOTKEY_STOP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// Result of one bounded wait for the backend thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopOutcome {
    /// The thread finished within the bound and was joined: its
    /// `RustConnection` — and with it the X11 grabs — is gone by
    /// the time this returns.
    Joined,
    /// The bound expired with the thread still running. The
    /// handle is detached (dropping the `JoinHandle` keeps the
    /// thread running): it exits on its own once it observes the
    /// shutdown flag, and the caller must not wait any further.
    TimedOut,
}

/// Handle to the live X11 hotkey backend thread.
///
/// Mirrors the `LiveLoopHandle` shape in `runtime_bridge.rs`:
/// a `shutdown` flag that the UI thread can flip to ask the
/// backend thread to exit, plus the `JoinHandle` so the UI
/// thread can wait for clean teardown. The `Drop` impl is a
/// best-effort safety net — explicit `stop()` is the
/// recommended path.
///
/// # Bounded stop
///
/// `stop()` is the seam hotkey re-registration runs on the UI
/// thread (`actions::settings::restart_hotkey_backend`, the
/// confirm path that released and re-took the grabs), so it must
/// never wait for an event that may never arrive. Two mechanisms
/// keep it bounded:
///
/// 1. the X11 event loop polls the connection instead of
///    blocking in `wait_for_event()`, so it observes the flag on
///    an idle server within one [`HOTKEY_EVENT_POLL_INTERVAL`];
/// 2. `stop()` waits at most [`HOTKEY_STOP_JOIN_TIMEOUT`] for
///    the thread to finish and detaches it (with a log) when the
///    bound expires, instead of blocking the caller.
pub struct HotkeyBackendHandle {
    /// Shared shutdown signal flipped by `stop()` / `Drop`.
    pub(crate) shutdown: Arc<std::sync::atomic::AtomicBool>,
    /// Background thread that owns the `RustConnection` and the
    /// poll loop. Joined on `stop()`, signalled but not joined on
    /// `Drop`.
    pub(crate) thread: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyBackendHandle {
    /// Signal the backend thread to shut down and wait — bounded —
    /// for it to finish. Idempotent.
    pub fn stop(mut self) {
        if self.stop_with_timeout(HOTKEY_STOP_JOIN_TIMEOUT) == StopOutcome::TimedOut {
            eprintln!(
                "[d4lc] hotkey: backend thread did not finish within {}ms of the shutdown \
                 signal; detaching it so the caller keeps running. The X11 loop notices the \
                 flag on its next poll tick (at most {}ms); the portal loop does so on its next \
                 activation. The connection and the grabs are released when the thread exits.",
                HOTKEY_STOP_JOIN_TIMEOUT.as_millis(),
                HOTKEY_EVENT_POLL_INTERVAL.as_millis()
            );
        }
    }

    /// [`Self::stop`] with an explicit bound, so the expired-bound
    /// branch is testable without waiting for the production
    /// [`HOTKEY_STOP_JOIN_TIMEOUT`].
    ///
    /// Returns [`StopOutcome::Joined`] once the thread has really
    /// finished (or when there is no thread to wait for);
    /// [`StopOutcome::TimedOut`] when the caller must stop waiting
    /// and leave the thread to exit on its own.
    fn stop_with_timeout(&mut self, bound: std::time::Duration) -> StopOutcome {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let Some(handle) = self.thread.take() else {
            return StopOutcome::Joined;
        };
        let deadline = std::time::Instant::now() + bound;
        while !handle.is_finished() {
            let now = std::time::Instant::now();
            if now >= deadline {
                // `handle` is dropped here, which detaches the
                // thread rather than joining it. The thread is not
                // interrupted: it still owns the connection and
                // exits on its own once it observes the flag.
                return StopOutcome::TimedOut;
            }
            std::thread::sleep(HOTKEY_STOP_POLL_INTERVAL.min(deadline - now));
        }
        // `is_finished()` already observed the thread's exit, so
        // this join cannot block — it only reaps the thread. A
        // panic in the thread body surfaces here instead of being
        // swallowed.
        if handle.join().is_err() {
            eprintln!(
                "[d4lc] hotkey: backend thread panicked; its X11 connection (and the grabs \
                 registered on it) was dropped with the thread"
            );
        }
        StopOutcome::Joined
    }
}

impl Drop for HotkeyBackendHandle {
    fn drop(&mut self) {
        // Mirror the `LiveLoopHandle::drop` pattern: signal
        // shutdown so the thread can exit, do not block on
        // join (the app may be tearing down and blocking the
        // UI thread on a join is the wrong choice). The
        // thread will see the flag on its next poll tick (or,
        // on the portal path, its next activation) and exit;
        // the `RustConnection` is dropped with the thread,
        // which is the OS-level ungrab signal X11
        // understands.
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Start the persistent hotkey backend for the supplied
/// binding list.
///
/// Returns:
///
/// - `(status, Some(handle), Some(event_rx))` — backend thread
///   is running, the caller must drive `event_rx` (typically
///   from a Slint `Timer`) and stash `handle` somewhere that
///   lives for the full app lifetime (e.g. `Box::leak`).
/// - `(status, None, None)` — X11 (or the x11rb feature) is
///   unavailable; no thread is started, the status already
///   reports the reason. Safe to call with no further wiring.
///
/// **No dispatch logic lives here.** The persistent thread
/// just translates X11 `KeyPress` events into
/// `HotkeyEvent::Action { name }` values on the channel; the
/// UI-side dispatcher (see `dispatch_hotkey_event` below) is
/// what maps an action name to an existing action handler.
///
/// `config_settings` is the loaded `d4lc_config::AppSettings`
/// the `wiring` layer hands in. The function projects
/// `config_settings.hotkey_config.bindings` into a
/// `Vec<HotkeyRequest>` via the public
/// `requests_from_settings` helper so callers do not need to
/// depend on the schema crate.
pub fn start_hotkey_backend(
    config_settings: &d4lc_config::AppSettings,
) -> (
    HotkeyBackendStatus,
    Option<HotkeyBackendHandle>,
    Option<std::sync::mpsc::Receiver<HotkeyEvent>>,
) {
    let requests = requests_from_settings(config_settings);

    // Wayland session: the GlobalShortcuts portal replaces the X11
    // grab path entirely. The portal bind runs on this
    // thread before the Slint event loop starts, so the compositor
    // consent dialog (first registration only) is answered before
    // the UI spins up; the Activated event loop then runs on the
    // dedicated backend thread like the X11 loop does.
    if is_wayland_session() {
        return start_wayland_portal_backend(&requests);
    }

    // Re-use the registration probe to decide capability
    // and per-binding outcome. When everything is unavailable
    // (no DISPLAY, Wayland session, no x11rb feature, or no
    // enabled binding), we skip the thread entirely so the
    // main thread does not have to drain a permanently-empty
    // channel.
    let status = init_hotkeys_from(&requests);

    let has_registered = status
        .registrations
        .iter()
        .any(|r| matches!(r.status, HotkeyRegistrationStatus::Registered));

    if status.capability != HotkeyBackendCapability::X11Available || !has_registered {
        return (status, None, None);
    }

    // Build the channel + shutdown signal + thread.
    let (tx, rx) = std::sync::mpsc::channel::<HotkeyEvent>();
    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);

    // Build the per-binding lookup table the thread will use
    // to translate a (keycode, modifier-mask) match into an
    // action name. The same lookup is used in the registration
    // probe — we rebuild it here rather than threading it
    // through the status type so the public surface stays
    // pure data.
    let table = Arc::new(build_event_lookup_table(&requests));

    let thread = std::thread::Builder::new()
        .name("d4lc-hotkey-event-loop".to_string())
        .spawn(move || {
            run_event_loop(table, tx, shutdown_clone);
        })
        .ok();

    let handle = thread.map(|t| HotkeyBackendHandle {
        shutdown,
        thread: Some(t),
    });

    (status, handle, Some(rx))
}

/// Project a `d4lc_config::AppSettings` into the
/// `Vec<HotkeyRequest>` the registration + event loop consume.
///
/// Pure data projection — no IO, no env-var checks. The order
/// matches the order in `settings.hotkey_config.bindings`
/// (preserved by `HotkeyConfig::default_bindings()` /
/// `upsert`).
pub fn requests_from_settings(config_settings: &d4lc_config::AppSettings) -> Vec<HotkeyRequest> {
    config_settings
        .hotkey_config
        .bindings
        .iter()
        .map(|b| HotkeyRequest {
            name: b.name.clone(),
            binding: b.binding.clone(),
            enabled: b.enabled,
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::hotkeys::test_support::{EnvGuard, ENV_LOCK};
    #[cfg(feature = "x11rb-backend")]
    use crate::hotkeys::x11::poll_until_event_or_shutdown;

    // -- init_hotkeys / unavailable paths (no DISPLAY / Wayland / no-x11rb) --

    /// Probe the seven upstream default bindings, all disabled.
    fn init_hotkeys() -> HotkeyBackendStatus {
        let requests: Vec<HotkeyRequest> = [
            ("SwitchPreset", "Ctrl+F5"),
            ("SwitchOverlay", "Ctrl+F6"),
            ("TakeScreenshot", "Ctrl+F10"),
            ("ToggleController", "Ctrl+F9"),
            ("ToggleOverlay", "Ctrl+F12"),
            ("ToggleDebugLockScreencapture", "Ctrl+F11"),
            ("ToggleTuningPrismHints", "Ctrl+F7"),
        ]
        .into_iter()
        .map(|(name, binding)| HotkeyRequest {
            name: name.to_string(),
            binding: binding.to_string(),
            enabled: false,
        })
        .collect();
        init_hotkeys_from(&requests)
    }

    #[test]
    fn init_hotkeys_reports_unavailable_when_display_unset() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::capture();
        std::env::remove_var("DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");
        let status = init_hotkeys();
        assert_eq!(status.capability, HotkeyBackendCapability::X11Unavailable);
        assert!(status
            .unavailable_reason
            .as_deref()
            .unwrap_or("")
            .contains("DISPLAY"));
        // 7 default rows, all either Disabled (because we built
        // the default-disabled list) or Unavailable. With
        // the default 7 disabled, every row is `Disabled`.
        assert_eq!(status.registrations.len(), 7);
        for row in &status.registrations {
            assert_eq!(row.status, HotkeyRegistrationStatus::Disabled);
        }
    }

    #[test]
    fn init_hotkeys_reports_wayland_session() {
        // The blanket `X11Unavailable` gate is upgraded
        // to the structured Wayland states. A Wayland session routes
        // to the GlobalShortcuts portal path even when `DISPLAY`
        // would make X11 reachable; with the default all-disabled
        // bindings nothing is bindable, so the portal is
        // reported as "not probed" — hermetically, without touching
        // the session bus.
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::capture();
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
        std::env::set_var("WAYLAND_DISPLAY", "wayland-test");
        std::env::set_var("DISPLAY", ":0");
        let status = init_hotkeys();
        assert_eq!(
            status.capability,
            HotkeyBackendCapability::WaylandPortalUnavailable
        );
        assert_ne!(status.capability, HotkeyBackendCapability::X11Unavailable);
        let reason = status.unavailable_reason.as_deref().unwrap_or_default();
        assert!(
            reason.contains("not probed") && reason.contains("GlobalShortcuts"),
            "unavailable_reason must name the not-probed portal state: {reason}"
        );
        assert_eq!(status.registrations.len(), 7);
        for row in &status.registrations {
            assert_eq!(row.status, HotkeyRegistrationStatus::Disabled);
        }
    }

    #[test]
    fn init_hotkeys_from_records_per_binding_outcomes() {
        // Pure-status test: no X server connection attempted
        // because every binding here is parseable but enabled
        // only when an X server is reachable. We feed the
        // exact requests we want to inspect, then mark them
        // all `enabled = false` so the parser / status
        // machinery still runs but no grab is sent.
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::capture();
        std::env::remove_var("DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");
        let requests = vec![
            HotkeyRequest {
                name: "Good".into(),
                binding: "Ctrl+F5".into(),
                enabled: false,
            },
            HotkeyRequest {
                name: "Bad".into(),
                binding: "Ctrl+".into(),
                enabled: true,
            },
            HotkeyRequest {
                name: "Enabled".into(),
                binding: "Ctrl+F12".into(),
                enabled: true,
            },
        ];
        let status = init_hotkeys_from(&requests);
        assert_eq!(status.capability, HotkeyBackendCapability::X11Unavailable);
        let rows: Vec<&HotkeyRegistration> = status.registrations.iter().collect();
        assert_eq!(rows.len(), 3);
        // Disabled bindings stay Disabled even when X11 is
        // unreachable (the parser did not run on them).
        assert_eq!(rows[0].status, HotkeyRegistrationStatus::Disabled);
        // The parse-error binding is recorded as Unavailable
        // here because the no-DISPLAY path short-circuits
        // before parsing. That is the expected,
        // outcome: we never claimed the binding is registered.
        assert_eq!(
            rows[1].status,
            HotkeyRegistrationStatus::Unavailable {
                reason: rows[1].status.unavailable_reason().unwrap_or_default()
            }
        );
        assert_eq!(
            rows[2].status,
            HotkeyRegistrationStatus::Unavailable {
                reason: rows[2].status.unavailable_reason().unwrap_or_default()
            }
        );
    }

    // -- Helper for tests ---------------------------------------------------

    impl HotkeyRegistrationStatus {
        /// Extract the `reason` payload from any variant that
        /// carries one. Test-only convenience — avoids having
        /// to write a giant `match` in each assertion.
        fn unavailable_reason(&self) -> Option<String> {
            match self {
                Self::Unavailable { reason } => Some(reason.clone()),
                Self::ParseError { detail } => Some(detail.clone()),
                Self::Conflict { detail } => Some(detail.clone()),
                _ => None,
            }
        }
    }

    // -- Backend status shape ----------------------------------------------

    #[test]
    fn default_status_is_not_initialized_with_no_registrations() {
        let s = HotkeyBackendStatus::default();
        assert_eq!(s.capability, HotkeyBackendCapability::NotInitialized);
        assert!(s.unavailable_reason.is_none());
        assert!(s.registrations.is_empty());
    }

    #[test]
    fn init_hotkeys_reports_per_binding_row_for_tuning_prism_binding() {
        // Conflict-reporting hook: the seventh binding participates
        // in the same per-binding registration report
        // (`Registered` / `Conflict` / `ParseError` / `Disabled` /
        // `Unavailable`) as the other six. Without an X server the
        // enabled binding still gets its own row.
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::capture();
        std::env::remove_var("DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");
        let mut settings = d4lc_config::AppSettings::default();
        settings
            .hotkey_config
            .find_by_name_mut("ToggleTuningPrismHints")
            .expect("default hotkey config should include ToggleTuningPrismHints")
            .enabled = true;
        let status = init_hotkeys_from(&requests_from_settings(&settings));
        assert_eq!(status.capability, HotkeyBackendCapability::X11Unavailable);
        let prism_row = status
            .registrations
            .iter()
            .find(|row| row.name == "ToggleTuningPrismHints")
            .expect("ToggleTuningPrismHints must have its own registration row");
        assert_eq!(prism_row.binding, "Ctrl+F7");
        assert!(matches!(
            prism_row.status,
            HotkeyRegistrationStatus::Unavailable { .. }
        ));
    }

    // -- requests_from_settings projection --------------------------

    #[test]
    fn requests_from_settings_projects_all_seven_bindings() {
        let settings = d4lc_config::AppSettings::default();
        let requests = requests_from_settings(&settings);
        // The 7 upstream defaults are always present in
        // `HotkeyConfig::default()`.
        assert_eq!(requests.len(), 7);
        let names: Vec<&str> = requests.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"SwitchPreset"));
        assert!(names.contains(&"SwitchOverlay"));
        assert!(names.contains(&"TakeScreenshot"));
        assert!(names.contains(&"ToggleController"));
        assert!(names.contains(&"ToggleOverlay"));
        assert!(names.contains(&"ToggleDebugLockScreencapture"));
        assert!(names.contains(&"ToggleTuningPrismHints"));
        // Defaults are disabled — registration + event loop
        // should not pick them up.
        for r in &requests {
            assert!(!r.enabled, "default binding {} should be disabled", r.name);
        }
        // The new binding keeps the upstream Ctrl+F7 default.
        let prism = requests
            .iter()
            .find(|r| r.name == "ToggleTuningPrismHints")
            .expect("ToggleTuningPrismHints request must exist");
        assert_eq!(prism.binding, "Ctrl+F7");
    }

    // -- start_hotkey_backend availability ---------------------------

    #[test]
    fn start_hotkey_backend_returns_no_thread_when_display_unset() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::capture();
        std::env::remove_var("DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");

        let settings = d4lc_config::AppSettings::default();
        let (status, handle, rx) = start_hotkey_backend(&settings);
        assert_eq!(status.capability, HotkeyBackendCapability::X11Unavailable);
        assert!(
            handle.is_none(),
            "no thread must be started when X11 is unavailable"
        );
        assert!(
            rx.is_none(),
            "no receiver must be returned when X11 is unavailable"
        );
    }

    #[test]
    fn start_hotkey_backend_returns_no_thread_when_all_bindings_disabled() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::capture();
        std::env::remove_var("DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");

        // Even if X11 were reachable, no enabled binding
        // means no thread (the registration probe would
        // produce zero `Registered` rows).
        let settings = d4lc_config::AppSettings::default();
        let (status, handle, rx) = start_hotkey_backend(&settings);
        // On a DISPLAY-less test runner, capability is
        // X11Unavailable and the handle is None.
        assert!(handle.is_none());
        assert!(rx.is_none());
        assert_eq!(status.registrations.len(), 7);
        for row in &status.registrations {
            assert_eq!(row.status, HotkeyRegistrationStatus::Disabled);
        }
    }

    // -- HotkeyEvent + HotkeyBackendHandle shape ---------------------

    #[test]
    fn hotkey_event_name_round_trips() {
        let e = HotkeyEvent::new("TakeScreenshot");
        assert_eq!(e.name, "TakeScreenshot");
    }

    #[test]
    fn hotkey_backend_handle_drop_signals_shutdown() {
        use std::sync::atomic::AtomicBool;
        let flag = Arc::new(AtomicBool::new(false));
        let handle = HotkeyBackendHandle {
            shutdown: Arc::clone(&flag),
            thread: None, // no thread -> drop only flips the flag
        };
        drop(handle);
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    // -- Bounded, wakeable stop -------------------------------------------
    //
    // Saving hotkey settings re-registers the grabs from the confirm
    // handler (`actions::settings::restart_hotkey_backend`), which stops
    // the running backend on the UI thread. The stop must therefore be
    // bounded and must not need an X event to make progress; otherwise an
    // idle X session freezes the app until the next key press.

    /// The event loop's waiting step hands control back as soon as the
    /// shutdown flag is set, with no X connection and no event: the flag
    /// wins over a pending event, an idle connection observes a flag set
    /// from another thread within the poll cadence, and a real event is
    /// passed through unchanged.
    #[cfg(feature = "x11rb-backend")]
    #[test]
    fn event_loop_observes_shutdown_on_an_idle_connection() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        // (1) Flag already set: no poll happens at all, so nothing can
        //     delay the exit — not even a pending event.
        let shutdown = AtomicBool::new(true);
        let polls = AtomicUsize::new(0);
        let ready = poll_until_event_or_shutdown(&shutdown, || {
            polls.fetch_add(1, Ordering::SeqCst);
            Ok::<Option<u8>, x11rb::errors::ConnectionError>(Some(1))
        });
        assert_eq!(
            ready.expect("flag-checked step"),
            None,
            "the shutdown flag must win over a pending event"
        );
        assert_eq!(
            polls.load(Ordering::SeqCst),
            0,
            "the flag is checked before every poll"
        );

        // (2) Stop under no events: the connection is idle (the poll
        //     never yields an event) and the flag is set from another
        //     thread — the step returns within the poll cadence, which is
        //     what bounds `stop()` on an idle X session.
        let shutdown = Arc::new(AtomicBool::new(false));
        let setter = {
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                std::thread::sleep(HOTKEY_EVENT_POLL_INTERVAL / 2);
                shutdown.store(true, Ordering::SeqCst);
            })
        };
        let started = std::time::Instant::now();
        let ready = poll_until_event_or_shutdown(&shutdown, || {
            Ok::<Option<u8>, x11rb::errors::ConnectionError>(None)
        });
        let elapsed = started.elapsed();
        setter.join().expect("flag setter thread");
        assert_eq!(
            ready.expect("idle-connection step"),
            None,
            "an idle connection must still observe the flag"
        );
        assert!(
            elapsed < HOTKEY_STOP_JOIN_TIMEOUT,
            "the flag must be observed at the poll cadence \
             ({HOTKEY_EVENT_POLL_INTERVAL:?}), not at the join bound \
             ({HOTKEY_STOP_JOIN_TIMEOUT:?}): {elapsed:?}"
        );

        // (3) A real event is handed back unchanged while the flag is
        //     clear.
        let shutdown = AtomicBool::new(false);
        let ready = poll_until_event_or_shutdown(&shutdown, || {
            Ok::<Option<u8>, x11rb::errors::ConnectionError>(Some(7))
        });
        assert_eq!(ready.expect("event step"), Some(7));
    }

    /// The re-registration seam: `stop()` on an idle backend (no X event
    /// is ever delivered) returns within the poll cadence and leaves the
    /// thread actually joined, so the UI thread cannot sit in a join
    /// waiting for the next key press.
    #[cfg(feature = "x11rb-backend")]
    #[test]
    fn hotkey_backend_stop_returns_on_an_idle_backend() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let shutdown = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let thread = {
            let shutdown = Arc::clone(&shutdown);
            let exited = Arc::clone(&exited);
            std::thread::spawn(move || {
                // The production waiting step on an idle connection:
                // exactly the loop shape the live X11 backend runs while
                // no key is pressed.
                let _ = poll_until_event_or_shutdown::<()>(&shutdown, || {
                    Ok::<Option<()>, x11rb::errors::ConnectionError>(None)
                });
                exited.store(true, Ordering::SeqCst);
            })
        };
        let handle = HotkeyBackendHandle {
            shutdown,
            thread: Some(thread),
        };

        let started = std::time::Instant::now();
        handle.stop();
        let elapsed = started.elapsed();

        assert!(
            exited.load(Ordering::SeqCst),
            "stop() must return with the backend thread joined"
        );
        assert!(
            elapsed < HOTKEY_STOP_JOIN_TIMEOUT,
            "an idle backend must stop at the poll cadence \
             ({HOTKEY_EVENT_POLL_INTERVAL:?}), not at the join bound \
             ({HOTKEY_STOP_JOIN_TIMEOUT:?}): {elapsed:?}"
        );
    }

    /// The expired-bound path: a backend thread that ignores the shutdown
    /// flag (the shape of the portal loop blocked in its activation
    /// stream, or of the X11 re-grab phase blocked on the server) must
    /// not hold the caller — the wait is cut at the bound and the thread
    /// is detached instead of joined.
    #[test]
    fn hotkey_backend_stop_detaches_a_thread_that_ignores_shutdown() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let bound = std::time::Duration::from_millis(50);
        let shutdown = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let thread = {
            let exited = Arc::clone(&exited);
            std::thread::spawn(move || {
                // Ignores `shutdown` entirely: only the test's release
                // ends it (a thread stuck in a blocking call the flag
                // cannot interrupt).
                let _ = release_rx.recv();
                exited.store(true, Ordering::SeqCst);
            })
        };
        let mut handle = HotkeyBackendHandle {
            shutdown: Arc::clone(&shutdown),
            thread: Some(thread),
        };

        let started = std::time::Instant::now();
        let outcome = handle.stop_with_timeout(bound);
        let elapsed = started.elapsed();

        assert_eq!(outcome, StopOutcome::TimedOut);
        assert!(
            elapsed >= bound && elapsed < std::time::Duration::from_secs(1),
            "the bound must expire before the caller is released, and must actually \
             bound the wait: {elapsed:?}"
        );
        assert!(
            shutdown.load(Ordering::SeqCst),
            "the shutdown flag is set even when the bound expires"
        );
        assert!(
            handle.thread.is_none(),
            "a timed-out thread is detached, not kept for a later join"
        );

        // Detaching is the production contract, but the test must not
        // leak a live thread: release it and confirm it ends.
        drop(release_tx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !exited.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "the released stub thread must exit"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
