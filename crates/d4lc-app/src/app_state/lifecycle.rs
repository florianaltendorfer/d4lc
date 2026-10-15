use d4lc_core::Rect;

// ---------------------------------------------------------------------------
// Window lifecycle projection
//
// Pure-Rust record of the window lifecycle policy decisions and of any
// capability gap that kept a platform action from running. The platform
// actions themselves (minimize, hide-to-tray, tray show/exit) run in
// `actions` and `tray`.
// ---------------------------------------------------------------------------

/// Outcome of `handle_window_closing` settings persistence.
///
/// Used by the lifecycle state to report whether the
/// `SettingsPanelState` was successfully written through the
/// `d4lc-config` pipeline at close time. Errors are recorded, not
/// swallowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PersistenceOutcome {
    /// `load_app_settings` returned `Ok` and the updated settings
    /// were written back to disk. Default baseline before the close
    /// handler has run.
    #[default]
    Persisted,
    /// `load_app_settings` returned an error; the on-disk file is
    /// untouched.
    LoadFailed,
    /// `save_app_settings` returned an error; the in-memory state
    /// was updated but not written to disk.
    SaveFailed,
}

/// Capability flags describing what the current platform session
/// can do. Mirrored from `actions::WindowLifecycleCapabilities` so
/// `AppState` does not have to depend on the actions module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WindowLifecycleCapabilities {
    /// Whether a tray backend is wired and reachable in the current
    /// session.
    pub tray_available: bool,
    /// Whether the current session proved that a startup minimize can
    /// be issued through the active native window backend.
    pub window_minimize_supported: bool,
    /// Whether the platform windowing layer exposes a programmatic
    /// hide API to Rust. `true` on X11 (Slint `hide()` /
    /// `CloseRequestResponse::HideWindow`), `false` on Wayland.
    pub window_hide_supported: bool,
    /// Whether the backend exposes reliable live main-window
    /// position + size values for persistence/restore.
    pub window_geometry_supported: bool,
    /// Whether the live X11 `WM_STATE` minimize observer
    /// is running against the current window.
    /// `true` only while the observer timer is actually polling;
    /// every startup failure path leaves it `false` and records a
    /// capability note.
    pub window_minimize_observer_active: bool,
}

impl WindowLifecycleCapabilities {
    /// No native window capability at all.
    #[cfg(test)]
    pub const SLINT_1X_LINUX: Self = Self {
        tray_available: false,
        window_minimize_supported: false,
        window_hide_supported: false,
        window_geometry_supported: false,
        window_minimize_observer_active: false,
    };
}

/// What `handle_app_loaded` should do for the main window at startup.
///
/// Mirrors upstream `MainWindowViewModel.cs:116-126` (the
/// `LaunchMinimized` branch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WindowLifecycleLaunchAction {
    /// Show the window normally.
    #[default]
    Normal,
    /// Start the window minimized. The decision is recorded; the
    /// runtime startup path attempts a real native minimize and
    /// records any capability gap in `capability_notes`.
    Minimized,
}

/// What `handle_window_closing` should do when the user closes the
/// main window.
///
/// Upstream collapses this to a single `WindowClosingExecute` that
/// always persists and exits. On Linux with a real tray backend the
/// "minimize to tray on close" path is the close-veto analog: the
/// window is hidden instead of destroyed. When the tray backend is
/// not available the close falls through to a normal close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WindowLifecycleCloseAction {
    /// Close and exit (default when `minimize_to_tray` is `false`).
    #[default]
    Close,
    /// Veto the close and hide the window to the tray. Requires
    /// `minimize_to_tray == true` and `tray_available == true`.
    MinimizeToTray,
    /// `minimize_to_tray` is on but no tray backend is available —
    /// fall back to a normal close and report the capability gap.
    KeepVisibleNoTray,
}

impl WindowLifecycleCloseAction {
    pub const fn description(self) -> &'static str {
        match self {
            Self::Close => "close-and-exit",
            Self::MinimizeToTray => "minimize-to-tray",
            Self::KeepVisibleNoTray => {
                "close-and-exit (minimize-to-tray requested but tray unavailable)"
            }
        }
    }
}

/// What `handle_window_state_changed` should do on a state change.
///
/// Upstream (`MainWindowViewModel.cs:323-345`) collapses to: if
/// `MinimizeToTray` is on, hide the window on minimize unless an
/// update is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WindowLifecycleStateAction {
    /// No-op — used when the new state is restored/normal, or when
    /// the request is gated by `update_pending`.
    #[default]
    NoOp,
    /// Minimize only — tray unavailable, or `minimize_to_tray` off.
    MinimizeOnly,
    /// Hide to tray (X11/Wayland `_NET_WM_STATE_HIDDEN`).
    /// Requires `minimize_to_tray == true` and `tray_available == true`.
    HideToTray,
    /// `minimize_to_tray` is on but the tray is not wired up —
    /// record the gap and report a no-op. Fallback.
    MinimizeToTrayRequestedNoTray,
}

impl WindowLifecycleStateAction {
    pub const fn description(self) -> &'static str {
        match self {
            Self::NoOp => "no-op",
            Self::MinimizeOnly => "minimize-only",
            Self::HideToTray => "hide-to-tray",
            Self::MinimizeToTrayRequestedNoTray => {
                "minimize-to-tray-requested-but-tray-unavailable"
            }
        }
    }
}

/// What `handle_tray_open` recorded (most recent invocation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowLifecycleTrayOpenAction {
    /// Show / restore the main window.
    OpenOrRestore,
}

/// What `handle_tray_exit` recorded (most recent invocation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowLifecycleTrayExitAction {
    /// Exit the application.
    Exit,
}

/// Projection of the window-lifecycle policy into `AppState`.
///
/// All fields are public so the actions module can update them
/// directly. The struct is plain data — no IO, no platform calls.
/// When a policy decision cannot be performed
/// because the platform lacks the API, the action is still recorded
/// (so the rest of the shell can observe what was *requested*) and
/// a clear `capability_notes` entry explains the gap.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowLifecycleState {
    /// Platform capability snapshot taken at the time this state
    /// was last refreshed.
    pub capabilities: WindowLifecycleCapabilities,
    /// Last launch decision, populated by `AppState::from_shell` so
    /// the launch action is known as soon as settings are loaded.
    pub last_launch_action: WindowLifecycleLaunchAction,
    /// Last close decision, populated by `handle_window_closing`.
    pub last_close_action: WindowLifecycleCloseAction,
    /// Last window-state change decision, populated by
    /// `handle_window_state_changed`.
    pub last_window_state_action: WindowLifecycleStateAction,
    /// Last tray open action, populated by `handle_tray_open`.
    pub last_tray_open_action: Option<WindowLifecycleTrayOpenAction>,
    /// Last tray exit action, populated by `handle_tray_exit`.
    pub last_tray_exit_action: Option<WindowLifecycleTrayExitAction>,
    /// Human-readable notes about platform capability gaps
    /// observed during the most recent lifecycle events. Each note
    /// is appended (not replaced) so the projection preserves the
    /// full gap history for the current session.
    pub capability_notes: Vec<String>,
    /// Outcome of the close-time settings persistence.
    pub persistence_outcome: PersistenceOutcome,
    /// Convenience flag — `true` iff
    /// `persistence_outcome == PersistenceOutcome::Persisted`.
    pub persisted_at_close: bool,
    /// Last known persisted main-window geometry snapshot.
    pub window_geometry: Option<Rect>,
    /// Live check-only update result: `true` once the background release
    /// check found a strictly newer release.
    ///
    /// Upstream keeps the same fact on `ReleaseManager.UpdateAvailable`
    /// and reads it in two places: the minimized-state handler skips the
    /// hide-to-tray while an update is pending
    /// (`MainWindowViewModel.cs:328-350`, `WindowStateChangedExecute`)
    /// and the launch-minimized decision refuses to start minimized
    /// (`MainWindowViewModel.cs:120-123`). Per session only — upstream
    /// never persists it (`ReleaseManager.cs:20-38`), so neither do we.
    pub update_pending: bool,
}

impl Default for WindowLifecycleState {
    fn default() -> Self {
        Self {
            capabilities: WindowLifecycleCapabilities::default(),
            last_launch_action: WindowLifecycleLaunchAction::Normal,
            last_close_action: WindowLifecycleCloseAction::Close,
            last_window_state_action: WindowLifecycleStateAction::NoOp,
            last_tray_open_action: None,
            last_tray_exit_action: None,
            capability_notes: Vec::new(),
            persistence_outcome: PersistenceOutcome::Persisted,
            persisted_at_close: false,
            window_geometry: None,
            update_pending: false,
        }
    }
}
