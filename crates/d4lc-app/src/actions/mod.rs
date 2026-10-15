//! App-shell action handlers.
//!
//! Each handler mirrors an upstream WPF `RelayCommand` execute method.
//! Actions delegate to core-crate APIs and never contain capture, OCR,
//! parser, rules, import, or overlay business logic.
//!
//! ## Upstream evidence
//!
//! - `MainWindowViewModel.cs:116-126` — `ApplicationLoadedExecute`
//! - `MainWindowViewModel.cs:218-228` — `LaunchGitHubExecute`
//! - `MainWindowViewModel.cs:230-240` — `LaunchGitHubWikiExecute`
//! - `MainWindowViewModel.cs:242-252` — `LaunchKofiExecute`
//! - `MainWindowViewModel.cs:254-262` — `NotifyIconDoubleClickExecute`
//! - `MainWindowViewModel.cs:264-272` — `NotifyIconOpenExecute`
//! - `MainWindowViewModel.cs:274-280` — `NotifyIconExitExecute`
//! - `MainWindowViewModel.cs:319-321` — `WindowClosingExecute`
//! - `MainWindowViewModel.cs:323-345` — `WindowStateChangedExecute`

pub mod affix;
pub mod affix_cmds;
pub mod affix_dialogs;
pub mod build_download;
pub mod build_import_dialogs;
pub mod debug;
pub mod dialogs;
pub mod hotkeys_dialogs;
pub mod logging;
pub mod runtime;
pub mod settings;
pub mod trade;
pub mod trade_dialogs;

use std::path::Path;
use std::process;
use std::sync::{Arc, Mutex};

use d4lc_config::{load_app_settings, save_app_settings};
use d4lc_core::Rect;
#[cfg(feature = "x11rb-backend")]
use raw_window_handle::{HasDisplayHandle as _, RawDisplayHandle};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor as _;
use slint::{CloseRequestResponse, PhysicalPosition, PhysicalSize};

use crate::app_state::{
    AppState, PersistenceOutcome, WindowLifecycleCapabilities, WindowLifecycleCloseAction,
    WindowLifecycleLaunchAction, WindowLifecycleStateAction, WindowLifecycleTrayExitAction,
    WindowLifecycleTrayOpenAction,
};

/// Version string logged on startup, mirroring upstream
/// `Assembly.GetExecutingAssembly().GetName().Version`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Title-bar link targets. The three upstream links keep the targets of
/// upstream's window commands (`MainWindow.xaml:33-51`,
/// `MainWindowViewModel.cs:218-252`); `D4lcRepository` is this port's own
/// repository, taken from the workspace `repository` metadata that the
/// update check also uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBarLink {
    D4lcRepository,
    UpstreamRepository,
    UpstreamWiki,
    UpstreamKofi,
}

impl TitleBarLink {
    pub const fn url(self) -> &'static str {
        match self {
            Self::D4lcRepository => env!("CARGO_PKG_REPOSITORY"),
            Self::UpstreamRepository => "https://github.com/josdemmers/Diablo4Companion",
            Self::UpstreamWiki => "https://github.com/josdemmers/Diablo4Companion/wiki",
            Self::UpstreamKofi => "https://ko-fi.com/josdemmers",
        }
    }
}

// ---------------------------------------------------------------------------
// Window lifecycle policy
//
// Pure headless policy logic, no platform API. The launch / close /
// minimize-to-tray decisions are derived from the
// `SettingsPanelState::{launch_minimized, minimize_to_tray}` flags and a
// runtime `tray_available` capability signal, and recorded into
// `AppState::window_lifecycle` so the rest of the shell can inspect them.
//
// The action / state enums live in `app_state` (so the projection is
// self-contained) and are re-aliased here under their short names for
// readability in the policy code.
// ---------------------------------------------------------------------------

/// Short alias for the launch action enum (see `app_state`).
pub type LaunchAction = WindowLifecycleLaunchAction;
/// Short alias for the close action enum (see `app_state`).
pub type CloseAction = WindowLifecycleCloseAction;
/// Short alias for the window-state action enum (see `app_state`).
pub type WindowStateAction = WindowLifecycleStateAction;
/// Short alias for the tray-open action enum (see `app_state`).
pub type TrayOpenAction = WindowLifecycleTrayOpenAction;
/// Short alias for the tray-exit action enum (see `app_state`).
pub type TrayExitAction = WindowLifecycleTrayExitAction;

#[cfg(feature = "x11rb-backend")]
pub const X11_ICONIC_STATE: u32 = 3;

#[cfg(feature = "x11rb-backend")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct X11WindowTarget {
    pub(crate) screen_num: usize,
    pub(crate) window_id: u32,
}

/// Build the X11 client message that requests an IconicState/minimized
/// transition for an already-known X11 window id.
///
/// This is the same `WM_CHANGE_STATE` request `winit` sends on X11.
#[cfg(feature = "x11rb-backend")]
pub fn build_x11_minimize_request(
    target_window: u32,
    wm_change_state_atom: u32,
) -> x11rb::protocol::xproto::ClientMessageEvent {
    x11rb::protocol::xproto::ClientMessageEvent::new(
        32,
        target_window,
        wm_change_state_atom,
        [X11_ICONIC_STATE, 0, 0, 0, 0],
    )
}

/// Send the X11 minimize request for an already-known top-level window id.
///
/// This mirrors the X11 transport path used by `winit`: send
/// `WM_CHANGE_STATE(IconicState)` to the root window with the standard
/// substructure masks, then flush the connection.
#[cfg(feature = "x11rb-backend")]
pub fn send_x11_minimize_request<C: x11rb::connection::Connection>(
    conn: &C,
    root_window: u32,
    target_window: u32,
    wm_change_state_atom: u32,
) -> Result<(), x11rb::errors::ConnectionError> {
    use x11rb::protocol::xproto::{ConnectionExt as _, EventMask};

    conn.send_event(
        false,
        root_window,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        build_x11_minimize_request(target_window, wm_change_state_atom),
    )?;
    conn.flush()
}

#[cfg(feature = "x11rb-backend")]
pub(crate) fn x11_window_target_from_raw_handles(
    raw_window_handle: RawWindowHandle,
    raw_display_handle: RawDisplayHandle,
) -> Result<X11WindowTarget, String> {
    let window_id = match raw_window_handle {
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).map_err(|_| {
            format!(
                "xlib window id {:#x} does not fit into an X11 window id",
                handle.window
            )
        })?,
        other => {
            return Err(format!(
                "native window handle is not X11-compatible: {other:?}"
            ))
        }
    };

    let screen_num = match raw_display_handle {
        RawDisplayHandle::Xcb(handle) => usize::try_from(handle.screen)
            .map_err(|_| format!("xcb screen index {} is negative", handle.screen))?,
        RawDisplayHandle::Xlib(handle) => usize::try_from(handle.screen)
            .map_err(|_| format!("xlib screen index {} is negative", handle.screen))?,
        other => {
            return Err(format!(
                "native display handle is not X11-compatible: {other:?}"
            ))
        }
    };

    Ok(X11WindowTarget {
        screen_num,
        window_id,
    })
}

#[cfg(feature = "x11rb-backend")]
fn minimize_live_x11_window(
    window: &slint::winit_030::winit::window::Window,
) -> Result<u32, String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;

    let raw_window_handle = window
        .window_handle()
        .map_err(|error| format!("window_handle: {error}"))?
        .as_raw();
    let raw_display_handle = window
        .display_handle()
        .map_err(|error| format!("display_handle: {error}"))?
        .as_raw();
    let target = x11_window_target_from_raw_handles(raw_window_handle, raw_display_handle)?;

    let (conn, _connected_screen_num) = x11rb::rust_connection::RustConnection::connect(None)
        .map_err(|error| format!("X11 connect: {error}"))?;

    let Some(screen) = conn.setup().roots.get(target.screen_num) else {
        return Err(format!(
            "X11 screen {} is out of range for the active display",
            target.screen_num
        ));
    };

    let wm_change_state_atom = conn
        .intern_atom(false, b"WM_CHANGE_STATE")
        .map_err(|error| format!("intern_atom WM_CHANGE_STATE: {error}"))?
        .reply()
        .map_err(|error| format!("intern_atom WM_CHANGE_STATE reply: {error}"))?
        .atom;

    send_x11_minimize_request(&conn, screen.root, target.window_id, wm_change_state_atom)
        .map_err(|error| format!("send_x11_minimize_request: {error}"))?;

    Ok(target.window_id)
}

fn append_launch_minimize_note(state: &Arc<Mutex<AppState>>, note: String) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state
        .window_lifecycle
        .capabilities
        .window_minimize_supported = false;
    state.window_lifecycle.capability_notes.push(note);
}

fn mark_launch_minimize_supported(state: &Arc<Mutex<AppState>>) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state
        .window_lifecycle
        .capabilities
        .window_minimize_supported = true;
}

fn append_window_lifecycle_note(state: &Arc<Mutex<AppState>>, note: String) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.window_lifecycle.capability_notes.push(note);
}

fn set_live_window_capabilities(
    state: &Arc<Mutex<AppState>>,
    window_hide_supported: bool,
    window_geometry_supported: bool,
) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.window_lifecycle.capabilities.window_hide_supported = window_hide_supported;
    state
        .window_lifecycle
        .capabilities
        .window_geometry_supported = window_geometry_supported;
}

fn live_window_support_from_raw_handle(
    window: &slint::winit_030::winit::window::Window,
) -> Result<(bool, bool), String> {
    let raw_window_handle = window
        .window_handle()
        .map_err(|error| format!("window_handle: {error}"))?
        .as_raw();

    match raw_window_handle {
        RawWindowHandle::Xcb(_) | RawWindowHandle::Xlib(_) => Ok((true, true)),
        RawWindowHandle::Wayland(_) => Ok((false, false)),
        other => Err(format!(
            "native window handle does not expose a supported desktop backend for tray-hide/geometry persistence: {other:?}"
        )),
    }
}

fn apply_window_geometry(window: &slint::Window, geometry: Rect) {
    window.set_size(PhysicalSize::new(geometry.width, geometry.height));
    window.set_position(PhysicalPosition::new(geometry.x, geometry.y));
}

fn capture_window_geometry(
    window: &slint::Window,
    geometry_supported: bool,
) -> Result<Rect, String> {
    if !geometry_supported {
        return Err(
            "the current backend does not expose reliable main-window geometry".to_string(),
        );
    }

    let size = window.size();
    if size.width == 0 || size.height == 0 {
        return Err("the live main window reported a zero-sized geometry snapshot".to_string());
    }

    let position = window.position();
    Ok(Rect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

/// Write the in-memory settings and window geometry through
/// `Config/Settings.json`. A failure carries the config error text.
fn persist_state_to_config(
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), (PersistenceOutcome, String)> {
    let mut config =
        load_app_settings(config_root).map_err(|error| (PersistenceOutcome::LoadFailed, error))?;
    state.shell.settings.apply_to_config(&mut config);
    config.window_geometry = state.window_lifecycle.window_geometry;
    save_app_settings(config_root, &config).map_err(|error| (PersistenceOutcome::SaveFailed, error))
}

fn record_persistence_outcome(state: &mut AppState, outcome: PersistenceOutcome) {
    state.window_lifecycle.persistence_outcome = outcome;
    state.window_lifecycle.persisted_at_close = outcome == PersistenceOutcome::Persisted;
}

fn record_close_action(state: &mut AppState) -> CloseAction {
    let caps = state.window_lifecycle.capabilities;
    let close_action = decide_close_action(state.shell.settings.minimize_to_tray, caps);
    state.window_lifecycle.last_close_action = close_action;

    if close_action == CloseAction::KeepVisibleNoTray {
        state
            .window_lifecycle
            .capability_notes
            .push(format!(
                "minimize_to_tray=true at close but tray hide is unavailable (tray_available={}, window_hide_supported={}, close_action={}); falling back to a normal close.",
                caps.tray_available,
                caps.window_hide_supported,
                close_action.description()
            ));
    }

    close_action
}

fn persist_close_state(state: &mut AppState, config_root: &Path, log_context: &str) {
    match persist_state_to_config(state, config_root) {
        Ok(()) => record_persistence_outcome(state, PersistenceOutcome::Persisted),
        Err((outcome, detail)) => {
            record_persistence_outcome(state, outcome);
            let message = format!("{log_context} persistence failed ({outcome:?}): {detail}");
            eprintln!("[d4lc] {message}");
            state.append_log_message(message, crate::clock::hh_mm_now());
        }
    }
}

#[cfg(not(test))]
fn exit_process(code: i32) -> ! {
    process::exit(code)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessExit(pub i32);

#[cfg(test)]
fn exit_process(code: i32) -> ! {
    std::panic::panic_any(ProcessExit(code))
}

// ---------------------------------------------------------------------------
// Close-to-tray restore geometry
//
// The close-to-tray hide is non-destroying on X11 (pinned Slint 1.16.1
// behavior: `WindowEvent::CloseRequested` → `WindowInner::request_close`
// → `hide()` → the winit adapter's X11 branch only unmaps —
// `winitwindowadapter.rs` `set_visibility(WindowVisibility::Hidden)`
// takes the `set_visible(false)` branch, never `suspend()`). But the
// unmap of a *managed* window is a withdraw: the window manager deletes
// `WM_STATE`, and when the window is mapped again the WM treats it as a
// fresh window and re-places it (observed live on KDE/KWin 6.7.4:
// restored at the placement position instead of the pre-close one,
// while the minimize→hide path stays iconic/managed and keeps its
// position). To keep the tray-Open restore geometry-exact, the
// close-to-tray arm records the pre-hide geometry here, and the tray
// show-window hook (`main.rs`) consumes it once and re-applies it
// after `show()`. Consume-once keeps every other restore path
// (minimize-to-tray, tray-open while visible) untouched.
// ---------------------------------------------------------------------------

/// Pre-hide geometry captured by the close-to-tray arm, consumed by the
/// next tray-Open restore. `Mutex<Option<Rect>>` in a static because the
/// setter (close-requested handler) and the consumer (tray show hook)
/// are separate closures; both run on the Slint event-loop thread.
static HIDDEN_TO_TRAY_GEOMETRY: Mutex<Option<Rect>> = Mutex::new(None);

/// Record the geometry to re-apply at the next tray-Open restore
/// (`None` records a failed capture and makes the restore fall
/// back to the window-manager placement).
pub fn record_hidden_to_tray_geometry(geometry: Option<Rect>) {
    *HIDDEN_TO_TRAY_GEOMETRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = geometry;
}

/// Take (consume-once) the geometry recorded by the close-to-tray hide.
/// `None` after the first call or when no close-to-tray hide happened.
pub fn take_hidden_to_tray_geometry() -> Option<Rect> {
    HIDDEN_TO_TRAY_GEOMETRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Re-apply the geometry captured by the close-to-tray hide after the
/// tray-Open `show()` re-mapped the (withdrawn) window. Called on the
/// event-loop thread from the tray show hook in `main`.
pub(crate) fn reapply_hidden_to_tray_geometry(window: &slint::Window) {
    if let Some(geometry) = take_hidden_to_tray_geometry() {
        apply_window_geometry(window, geometry);
        eprintln!(
            "[d4lc] tray: reapplied close-to-tray window geometry to x={}, y={}, width={}, height={}",
            geometry.x, geometry.y, geometry.width, geometry.height
        );
    }
}

/// Pure headless policy: decide the launch action from the
/// `launch_minimized` setting, refusing to start minimized while an
/// update is pending. No side effects, no logging, no IO.
///
/// Upstream `ApplicationLoadedExecute` uses the same guard
/// (`LaunchMinimized && !_releaseManager.UpdateAvailable`,
/// `MainWindowViewModel.cs:120-123`): a known update keeps the window
/// visible so the user can act on the banner. The port's check runs
/// `update_check::FIRST_CHECK_DELAY` after launch, so the flag is only
/// ever `true` here once a check has already completed in this session
/// — the same per-session fact upstream keeps on `ReleaseManager`.
pub fn decide_launch_action(launch_minimized: bool, update_pending: bool) -> LaunchAction {
    if launch_minimized && !update_pending {
        LaunchAction::Minimized
    } else {
        LaunchAction::Normal
    }
}

/// Pure headless policy: decide what the close button should do.
pub fn decide_close_action(
    minimize_to_tray: bool,
    capabilities: WindowLifecycleCapabilities,
) -> CloseAction {
    if !minimize_to_tray {
        return CloseAction::Close;
    }
    if capabilities.tray_available && capabilities.window_hide_supported {
        CloseAction::MinimizeToTray
    } else {
        CloseAction::KeepVisibleNoTray
    }
}

/// Pure headless policy: decide what happens on a window-state
/// change (typically: window minimized). Mirrors upstream's
/// `WindowStateChangedExecute` decision table.
pub fn decide_window_state_change(
    is_minimized: bool,
    minimize_to_tray: bool,
    capabilities: WindowLifecycleCapabilities,
    update_pending: bool,
) -> WindowStateAction {
    if !is_minimized || update_pending {
        return WindowStateAction::NoOp;
    }
    if !minimize_to_tray {
        return WindowStateAction::MinimizeOnly;
    }
    if capabilities.tray_available && capabilities.window_hide_supported {
        WindowStateAction::HideToTray
    } else {
        WindowStateAction::MinimizeToTrayRequestedNoTray
    }
}

// ---------------------------------------------------------------------------
// Lifecycle handlers
// ---------------------------------------------------------------------------

/// Called when the main window has loaded.
///
/// Upstream (`MainWindowViewModel.cs:116-126`):
///   - Logs the window title (version string).
///   - Minimizes the window when `LaunchMinimized` is true.
///
/// After the event loop creates the backend window, its native handle comes
/// from Slint's winit/raw-window-handle bridge and the X11 minimize request
/// is sent through it. Capability failures are recorded in
/// `WindowLifecycleState`.
pub fn handle_app_loaded<H>(window: slint::Weak<H>, state: Arc<Mutex<AppState>>)
where
    H: slint::ComponentHandle + 'static,
{
    eprintln!("d4lc v{APP_VERSION}");

    let config_root = crate::actions::settings::resolve_config_root();
    let persisted_window_geometry = load_app_settings(&config_root)
        .ok()
        .and_then(|settings| settings.window_geometry);
    {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if persisted_window_geometry.is_some() {
            state.window_lifecycle.window_geometry = persisted_window_geometry;
        }
    }

    if let Some(live_window) = window.upgrade() {
        let close_window = window.clone();
        let close_state = Arc::clone(&state);
        let close_config_root = config_root.clone();
        live_window.window().on_close_requested(move || {
            let geometry = if let Some(window) = close_window.upgrade() {
                let geometry_supported = {
                    let state = close_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.window_lifecycle.capabilities.window_geometry_supported
                };
                match capture_window_geometry(window.window(), geometry_supported) {
                    Ok(geometry) => Some(geometry),
                    Err(error) => {
                        let note = format!(
                            "Close requested but d4lc could not capture reliable live window geometry: {error}. Keeping the stored snapshot unchanged."
                        );
                        append_window_lifecycle_note(&close_state, note.clone());
                        eprintln!("[d4lc] {note}");
                        None
                    }
                }
            } else {
                let note = "Close requested after the Slint window was dropped; keeping the stored window geometry unchanged.".to_string();
                append_window_lifecycle_note(&close_state, note.clone());
                eprintln!("[d4lc] {note}");
                None
            };

            let mut state = close_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            handle_window_close_requested(&mut state, &close_config_root, geometry)
        });
    }

    let launch_minimized = {
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.window_lifecycle.last_launch_action == LaunchAction::Minimized
    };

    let runtime_state = Arc::clone(&state);
    let startup_window = window.clone();
    if let Err(error) = slint::spawn_local(async move {
        let Some(window) = startup_window.upgrade() else {
            if persisted_window_geometry.is_some() {
                let note = "Stored window geometry exists but the Slint window was dropped before startup restore could run. The persisted snapshot remains unchanged.".to_string();
                append_window_lifecycle_note(&runtime_state, note.clone());
                eprintln!("[d4lc] {note}");
            }
            if launch_minimized {
                let note = "LaunchMinimized=true but the Slint window was dropped before the startup minimize task could run. The window will open normally.".to_string();
                append_launch_minimize_note(&runtime_state, note.clone());
                eprintln!("[d4lc] {note}");
            }
            return;
        };

        let ui_window = window.window();

        match ui_window.winit_window().await {
            Ok(winit_window) => {
                match live_window_support_from_raw_handle(winit_window.as_ref()) {
                    Ok((window_hide_supported, window_geometry_supported)) => {
                        set_live_window_capabilities(
                            &runtime_state,
                            window_hide_supported,
                            window_geometry_supported,
                        );

                        if let Some(geometry) = persisted_window_geometry {
                            if window_geometry_supported {
                                apply_window_geometry(ui_window, geometry);
                                eprintln!(
                                    "[d4lc] restored window geometry to x={}, y={}, width={}, height={}",
                                    geometry.x, geometry.y, geometry.width, geometry.height
                                );
                            } else {
                                let note = "Stored window geometry exists but the active backend does not expose reliable position/size restore. The persisted snapshot remains unchanged.".to_string();
                                append_window_lifecycle_note(&runtime_state, note.clone());
                                eprintln!("[d4lc] {note}");
                            }
                        }
                    }
                    Err(error) => {
                        set_live_window_capabilities(&runtime_state, false, false);
                        if persisted_window_geometry.is_some() {
                            let note = format!(
                                "Stored window geometry exists but d4lc could not classify the live desktop backend: {error}. The persisted snapshot remains unchanged."
                            );
                            append_window_lifecycle_note(&runtime_state, note.clone());
                            eprintln!("[d4lc] {note}");
                        }
                    }
                }

                if !launch_minimized {
                    return;
                }

                #[cfg(feature = "x11rb-backend")]
                {
                    match minimize_live_x11_window(winit_window.as_ref()) {
                        Ok(window_id) => {
                            mark_launch_minimize_supported(&runtime_state);
                            eprintln!(
                            "[d4lc] launch minimized — sent X11 minimize request to window {window_id:#x}"
                        );
                        }
                        Err(error) => {
                            let note = format!(
                            "LaunchMinimized=true but d4lc could not minimize the live X11 window: {error}. The window will open normally."
                        );
                            append_launch_minimize_note(&runtime_state, note.clone());
                            eprintln!("[d4lc] {note}");
                        }
                    }
                }
                #[cfg(not(feature = "x11rb-backend"))]
                {
                    let note = "LaunchMinimized=true but d4lc was built without the x11rb-backend minimize transport. The window will open normally.".to_string();
                    append_launch_minimize_note(&runtime_state, note.clone());
                    eprintln!("[d4lc] {note}");
                }
            }
            Err(error) => {
                set_live_window_capabilities(&runtime_state, false, false);
                if persisted_window_geometry.is_some() {
                    let note = format!(
                        "Stored window geometry exists but d4lc could not access the live backend window handle for startup restore: {error}. The persisted snapshot remains unchanged."
                    );
                    append_window_lifecycle_note(&runtime_state, note.clone());
                    eprintln!("[d4lc] {note}");
                }
                if !launch_minimized {
                    return;
                }
                let note = format!(
                    "LaunchMinimized=true but d4lc could not access the live backend window handle: {error}. The window will open normally."
                );
                append_launch_minimize_note(&runtime_state, note.clone());
                eprintln!("[d4lc] {note}");
            }
        }
    }) {
        let note = format!(
            "LaunchMinimized=true but d4lc could not schedule the startup minimize task: {error}. The window will open normally."
        );
        append_launch_minimize_note(&state, note.clone());
        eprintln!("[d4lc] {note}");
    }
}

/// Called when the main window is about to close.
///
/// Upstream (`MainWindowViewModel.cs:319-321`):
///   - Persists application state before exit.
///
/// The handler consults the close policy, records the decision into
/// `WindowLifecycleState`, and persists `SettingsPanelState` through
/// `d4lc-config`. Load or save errors are recorded in
/// `persistence_outcome`.
///
/// Runtime shutdown is handled by `LiveLoopHandle`'s `Drop` implementation:
/// the handle sets `AtomicBool shutdown = true`, fences the loop's UI
/// publications, and lets a reaper thread join the background thread.
/// The `event_tx` sender is leaked for app lifetime and never explicitly closed.
/// Shutdown signalling is solely through the `AtomicBool`.
pub fn handle_window_closing(state: &mut AppState, config_root: &Path) {
    let caps = state.window_lifecycle.capabilities;
    let close_action = record_close_action(state);

    eprintln!(
        "[d4lc] window closing — close_action={} (minimize_to_tray={}, tray_available={})",
        close_action.description(),
        state.shell.settings.minimize_to_tray,
        caps.tray_available,
    );

    persist_close_state(state, config_root, "window closing — settings");
    build_import_dialogs::shutdown_downloads(state);
}

pub fn handle_window_close_requested(
    state: &mut AppState,
    config_root: &Path,
    geometry: Option<Rect>,
) -> CloseRequestResponse {
    if let Some(geometry) = geometry {
        state.window_lifecycle.window_geometry = Some(geometry);
    }

    let caps = state.window_lifecycle.capabilities;
    let close_action = record_close_action(state);

    eprintln!(
        "[d4lc] window close requested — close_action={} (minimize_to_tray={}, tray_available={})",
        close_action.description(),
        state.shell.settings.minimize_to_tray,
        caps.tray_available,
    );

    persist_close_state(state, config_root, "window close requested");

    if close_action == CloseAction::MinimizeToTray {
        // The returned `HideWindow` makes Slint hide (not
        // destroy) the window, but on X11 the hide withdraws the managed
        // window, so the later tray-Open re-map gets re-placed by the
        // window manager. Record the pre-hide geometry for the
        // consume-once re-apply in the tray show hook (see
        // `HIDDEN_TO_TRAY_GEOMETRY` above).
        record_hidden_to_tray_geometry(state.window_lifecycle.window_geometry);
        return CloseRequestResponse::HideWindow;
    }

    build_import_dialogs::shutdown_downloads(state);
    exit_process(0)
}

/// Called when the window state changes (minimize / restore).
///
/// Upstream (`MainWindowViewModel.cs:328-350`,
/// `WindowStateChangedExecute`):
///   - When `MinimizeToTray` is enabled and the window is minimized,
///     hide the window (collapse to tray) unless an update is pending.
///
/// The decision table is now driven by the
/// live X11 `WM_STATE` observer (`crate::window_observer`), which
/// polls the real window state on a dedicated Slint timer — the Linux
/// equivalent of the WPF `StateChanged` event binding. The observer
/// hides the window only when this function returns `HideToTray`.
/// Returns the decided action so the platform caller can perform (or
/// skip) the matching hide.
pub fn handle_window_state_changed(
    state: &mut AppState,
    is_minimized: bool,
    update_pending: bool,
) -> WindowStateAction {
    let caps = state.window_lifecycle.capabilities;
    let action = decide_window_state_change(
        is_minimized,
        state.shell.settings.minimize_to_tray,
        caps,
        update_pending,
    );
    state.window_lifecycle.last_window_state_action = action;
    if action == WindowStateAction::MinimizeToTrayRequestedNoTray {
        state
            .window_lifecycle
            .capability_notes
            .push(format!(
                "window minimized with minimize_to_tray=true but tray backend is unavailable (action={}); recording gap and leaving the window visible.",
                action.description()
            ));
    }
    eprintln!(
        "[d4lc] window state changed (is_minimized={}, minimize_to_tray={}, update_pending={}) → {}",
        is_minimized,
        state.shell.settings.minimize_to_tray,
        update_pending,
        action.description(),
    );
    action
}

// ---------------------------------------------------------------------------
// Title-bar link commands
// ---------------------------------------------------------------------------

/// Bind the four title-bar link buttons to `open`, each with its own
/// [`TitleBarLink`]. Production passes [`handle_launch_title_bar_link`].
pub fn wire_title_bar_links(
    app: &crate::slint_generated::AppWindow,
    open: impl Fn(&crate::slint_generated::AppWindow, TitleBarLink) + Clone + 'static,
) {
    use slint::ComponentHandle as _;
    let bind = |link: TitleBarLink| {
        let weak = app.as_weak();
        let open = open.clone();
        move || {
            if let Some(app) = weak.upgrade() {
                open(&app, link);
            }
        }
    };
    app.on_d4lc_github_clicked(bind(TitleBarLink::D4lcRepository));
    app.on_upstream_github_clicked(bind(TitleBarLink::UpstreamRepository));
    app.on_upstream_kofi_clicked(bind(TitleBarLink::UpstreamKofi));
    app.on_upstream_wiki_clicked(bind(TitleBarLink::UpstreamWiki));
}

/// Opens a title-bar link via `xdg-open`. Failures are logged on stderr and
/// the Logging screen, as upstream's `LaunchGitHubExecute` /
/// `LaunchGitHubWikiExecute` / `LaunchKofiExecute` log them
/// (`MainWindowViewModel.cs:218-252`).
pub fn handle_launch_title_bar_link(
    app: &crate::slint_generated::AppWindow,
    app_state: &Mutex<AppState>,
    link: TitleBarLink,
) {
    launch_title_bar_link(app, app_state, link, open_url);
}

/// [`handle_launch_title_bar_link`] with the URL opener injected.
fn launch_title_bar_link(
    app: &crate::slint_generated::AppWindow,
    app_state: &Mutex<AppState>,
    link: TitleBarLink,
    opener: impl FnOnce(&str) -> Result<(), String>,
) {
    if let Err(error) = opener(link.url()) {
        eprintln!("[d4lc] title-bar link {link:?}: {error}");
        let mut state = app_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.append_log_message(
            format!("Opening {} failed: {error}", link.url()),
            crate::clock::hh_mm_now(),
        );
        crate::actions::logging::project_log_messages(app, &state.logging.messages);
    }
}

// ---------------------------------------------------------------------------
// Tray icon commands
// ---------------------------------------------------------------------------

/// Show / restore the main window from the tray.
///
/// Mirrors `NotifyIconOpenExecute` and `NotifyIconDoubleClickExecute`
/// (`MainWindowViewModel.cs:264-272`, `MainWindowViewModel.cs:254-262`).
///
/// The action is recorded into `WindowLifecycleState`; the real
/// show/restore runs through the live-window hook `main` installs via
/// `tray::set_show_window_hook` (the tray module routes both the
/// projection and the hook through the Slint event loop).
pub fn handle_tray_open(state: &mut AppState) {
    state.window_lifecycle.last_tray_open_action = Some(TrayOpenAction::OpenOrRestore);
    state
        .window_lifecycle
        .capability_notes
        .push(
            "tray open/restore recorded; the window show runs through the live window hook installed by main (tray::set_show_window_hook)."
                .to_string(),
        );
}

/// Quit the application from the tray exit menu item.
///
/// Mirrors `NotifyIconExitExecute` (`MainWindowViewModel.cs:274-280`).
///
/// The exit is recorded in `WindowLifecycleState`, then
/// `std::process::exit(0)` is called so the process terminates
/// immediately, matching upstream's `Application.Shutdown()` semantics.
pub fn handle_tray_exit(state: &mut AppState) {
    state.window_lifecycle.last_tray_exit_action = Some(TrayExitAction::Exit);
    build_import_dialogs::shutdown_downloads(state);
    exit_process(0);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Open a URL with the desktop default handler (`xdg-open`). Shared by
/// the title-bar link commands and the build-import Web / Visit buttons.
///
/// `xdg-open` is waited on by a detached thread so it does not stay a
/// zombie until the app exits; a non-zero exit (no handler) is logged there.
/// The thread lives as long as `xdg-open`, which in its generic mode runs
/// the browser in the foreground.
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    open_with("xdg-open", url)
}

/// Run `program url` and reap it on a waiter thread. The thread starts
/// first: when it cannot, nothing is launched and the error is honest;
/// once the program runs, the result is `Ok` and the child is always
/// handed to the waiter.
fn open_with(program: &str, url: &str) -> Result<(), String> {
    let (child_tx, child_rx) = std::sync::mpsc::sync_channel::<process::Child>(1);
    let logged = format!("{program} '{url}'");
    std::thread::Builder::new()
        .name("xdg-open-wait".to_owned())
        .spawn(move || {
            // A closed channel means the program never started.
            let Ok(mut child) = child_rx.recv() else {
                return;
            };
            match child.wait() {
                Ok(status) if !status.success() => {
                    eprintln!("[d4lc] {logged} exited with {status}");
                }
                Ok(_) => {}
                Err(err) => eprintln!("[d4lc] waiting for {logged} failed: {err}"),
            }
        })
        .map_err(|err| format!("cannot open '{url}': no thread to reap {program}: {err}"))?;
    let child = process::Command::new(program)
        .arg(url)
        .spawn()
        .map_err(|err| format!("failed to spawn {program} for '{url}': {err}"))?;
    // The waiter holds the receiver until it takes this child, so the
    // one-slot send cannot fail.
    if child_tx.send(child).is_err() {
        eprintln!("[d4lc] {program} '{url}' started but its waiter is gone; it stays unreaped");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
//
// Policy unit tests cover the three decision tables (launch / close /
// window-state). Handler tests cover the AppState projection and the
// close-time persistence path.
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod lifecycle_tests {
    use super::*;
    use crate::app_state::WindowLifecycleState;
    use d4lc_gui::GuiShellState;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -------------------------------------------------------------------
    // decide_launch_action
    // -------------------------------------------------------------------

    #[test]
    fn decide_launch_action_normal_when_setting_false() {
        assert_eq!(decide_launch_action(false, false), LaunchAction::Normal);
    }

    #[test]
    fn decide_launch_action_minimized_when_setting_true() {
        assert_eq!(decide_launch_action(true, false), LaunchAction::Minimized);
    }

    #[test]
    fn decide_launch_action_keeps_the_window_visible_when_an_update_is_pending() {
        // Upstream `MainWindowViewModel.cs:120-123`: a pending update wins
        // over `LaunchMinimized`, so the user sees the release banner.
        assert_eq!(decide_launch_action(true, true), LaunchAction::Normal);
        assert_eq!(decide_launch_action(false, true), LaunchAction::Normal);
    }

    // -------------------------------------------------------------------
    // decide_close_action
    // -------------------------------------------------------------------

    #[test]
    fn decide_close_action_close_when_setting_false() {
        let caps = WindowLifecycleCapabilities::SLINT_1X_LINUX;
        assert_eq!(decide_close_action(false, caps), CloseAction::Close);
        let tray_on = WindowLifecycleCapabilities {
            tray_available: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        assert_eq!(
            decide_close_action(false, tray_on),
            CloseAction::Close,
            "minimize_to_tray=false must always be Close regardless of capability"
        );
    }

    #[test]
    fn decide_close_action_minimize_to_tray_when_setting_true_and_tray_available() {
        let caps = WindowLifecycleCapabilities {
            tray_available: true,
            window_hide_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        assert_eq!(decide_close_action(true, caps), CloseAction::MinimizeToTray);
    }

    #[test]
    fn decide_close_action_keep_visible_when_setting_true_but_no_tray() {
        let caps = WindowLifecycleCapabilities::SLINT_1X_LINUX;
        assert_eq!(
            decide_close_action(true, caps),
            CloseAction::KeepVisibleNoTray
        );
    }

    // -------------------------------------------------------------------
    // decide_window_state_change
    // -------------------------------------------------------------------

    #[test]
    fn decide_window_state_change_noop_when_not_minimized() {
        let caps = WindowLifecycleCapabilities {
            tray_available: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        assert_eq!(
            decide_window_state_change(false, true, caps, false),
            WindowStateAction::NoOp
        );
    }

    #[test]
    fn decide_window_state_change_noop_when_update_pending() {
        let caps = WindowLifecycleCapabilities {
            tray_available: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        assert_eq!(
            decide_window_state_change(true, true, caps, true),
            WindowStateAction::NoOp
        );
    }

    #[test]
    fn decide_window_state_change_minimize_only_when_setting_false() {
        let caps = WindowLifecycleCapabilities {
            tray_available: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        assert_eq!(
            decide_window_state_change(true, false, caps, false),
            WindowStateAction::MinimizeOnly
        );
    }

    #[test]
    fn decide_window_state_change_hide_to_tray_when_setting_true_and_tray_available() {
        let caps = WindowLifecycleCapabilities {
            tray_available: true,
            window_hide_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        assert_eq!(
            decide_window_state_change(true, true, caps, false),
            WindowStateAction::HideToTray
        );
    }

    #[test]
    fn decide_window_state_change_requested_no_tray_when_setting_true_but_no_tray() {
        let caps = WindowLifecycleCapabilities::SLINT_1X_LINUX;
        assert_eq!(
            decide_window_state_change(true, true, caps, false),
            WindowStateAction::MinimizeToTrayRequestedNoTray
        );
    }

    // -------------------------------------------------------------------
    // AppState window-lifecycle projection
    // -------------------------------------------------------------------

    #[test]
    fn app_state_from_shell_records_launch_action_from_settings() {
        let mut shell = GuiShellState::default_startup();
        shell.settings.launch_minimized = true;
        shell.settings.minimize_to_tray = false;
        let state = AppState::from_shell(shell);
        assert_eq!(
            state.window_lifecycle.last_launch_action,
            LaunchAction::Minimized
        );
    }

    #[test]
    fn app_state_from_shell_records_normal_launch_when_setting_false() {
        let mut shell = GuiShellState::default_startup();
        shell.settings.launch_minimized = false;
        let state = AppState::from_shell(shell);
        assert_eq!(
            state.window_lifecycle.last_launch_action,
            LaunchAction::Normal
        );
    }

    #[test]
    fn app_state_from_shell_defers_launch_gap_until_runtime_probe() {
        let mut shell = GuiShellState::default_startup();
        shell.settings.launch_minimized = true;
        let state = AppState::from_shell(shell);
        assert!(state.window_lifecycle.capability_notes.is_empty());
    }

    #[test]
    fn app_state_window_lifecycle_default_has_safe_baseline() {
        let state = AppState::default_startup();
        let w: &WindowLifecycleState = &state.window_lifecycle;
        assert_eq!(w.last_launch_action, LaunchAction::Normal);
        assert_eq!(w.last_close_action, CloseAction::Close);
        assert_eq!(w.last_window_state_action, WindowStateAction::NoOp);
        assert!(w.last_tray_open_action.is_none());
        assert!(w.last_tray_exit_action.is_none());
        assert!(!w.persisted_at_close);
        assert_eq!(w.window_geometry, None);
        assert_eq!(w.persistence_outcome, PersistenceOutcome::Persisted); // default placeholder, set on close
    }

    // -------------------------------------------------------------------
    // handle_window_closing — close decision + persistence
    // -------------------------------------------------------------------

    static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_tmp_root() -> std::path::PathBuf {
        let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("d4lc-window-lifecycle-{pid}-{n}"))
    }

    /// Serializes tests that touch the process-global
    /// `HIDDEN_TO_TRAY_GEOMETRY` static.
    static HIDDEN_GEOMETRY_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn handle_window_closing_persists_and_records_persisted_at_close() {
        let mut state = AppState::default_startup();
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        handle_window_closing(&mut state, &root);

        assert!(state.window_lifecycle.persisted_at_close);
        assert_eq!(
            state.window_lifecycle.persistence_outcome,
            PersistenceOutcome::Persisted
        );
        assert!(root.join(d4lc_config::SETTINGS_PATH).is_file());

        // Settings persisted reflect the in-memory state (defaults: both false).
        let loaded = d4lc_config::load_app_settings(&root).expect("load after save");
        assert!(!loaded.launch_minimized);
        assert!(!loaded.minimize_to_tray);
        assert_eq!(loaded.window_geometry, None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_close_persistence_reports_the_config_error() {
        let mut state = AppState::default_startup();
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        // A directory where the settings file belongs: the write fails.
        std::fs::create_dir_all(root.join(d4lc_config::SETTINGS_PATH)).expect("create blocker");

        handle_window_closing(&mut state, &root);

        assert!(!state.window_lifecycle.persisted_at_close);
        assert_eq!(
            state.window_lifecycle.persistence_outcome,
            PersistenceOutcome::SaveFailed
        );
        let logged = &state
            .logging
            .messages
            .last()
            .expect("logged failure")
            .message;
        assert!(
            logged.starts_with("window closing — settings persistence failed (SaveFailed): ")
                && logged.contains("Settings.json"),
            "{logged}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handle_window_closing_records_keep_visible_when_tray_setting_on() {
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = true;
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        handle_window_closing(&mut state, &root);

        // Slint 1.x Linux default: tray_available=false → KeepVisibleNoTray.
        assert_eq!(
            state.window_lifecycle.last_close_action,
            CloseAction::KeepVisibleNoTray
        );
        assert!(state
            .window_lifecycle
            .capability_notes
            .iter()
            .any(|n| n.contains("tray hide is unavailable")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handle_window_closing_preserves_existing_window_geometry_when_no_live_snapshot_exists() {
        let mut state = AppState::default_startup();
        state.window_lifecycle.window_geometry = Some(Rect {
            x: 10,
            y: 20,
            width: 1150,
            height: 625,
        });
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        handle_window_closing(&mut state, &root);

        let loaded = d4lc_config::load_app_settings(&root).expect("load after save");
        assert_eq!(
            loaded.window_geometry,
            state.window_lifecycle.window_geometry
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handle_window_close_requested_persists_geometry_for_close_to_tray() {
        // The hide arm must also record the pre-hide
        // geometry for the consume-once tray-Open re-apply (Slint's X11
        // hide withdraws the managed window, so the WM re-places it on
        // the later re-map).
        let _guard = HIDDEN_GEOMETRY_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = true;
        state.window_lifecycle.capabilities = WindowLifecycleCapabilities {
            tray_available: true,
            window_hide_supported: true,
            window_geometry_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let response = handle_window_close_requested(
            &mut state,
            &root,
            Some(Rect {
                x: 120,
                y: 240,
                width: 1280,
                height: 720,
            }),
        );

        assert_eq!(response, CloseRequestResponse::HideWindow);
        assert_eq!(
            state.window_lifecycle.last_close_action,
            CloseAction::MinimizeToTray
        );
        assert!(state.window_lifecycle.persisted_at_close);
        let loaded = d4lc_config::load_app_settings(&root).expect("load after save");
        assert_eq!(
            loaded.window_geometry,
            state.window_lifecycle.window_geometry
        );
        // The recorded geometry is consumable exactly once by the
        // tray-Open restore hook (`main.rs`).
        assert_eq!(
            take_hidden_to_tray_geometry(),
            state.window_lifecycle.window_geometry
        );
        assert_eq!(take_hidden_to_tray_geometry(), None, "consume-once");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn close_to_tray_without_captured_geometry_records_none_for_restore_fallback() {
        // A failed pre-hide capture records None so
        // the tray-Open restore falls back to the window-manager
        // placement instead of re-applying a stale geometry.
        let _guard = HIDDEN_GEOMETRY_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = true;
        state.window_lifecycle.capabilities = WindowLifecycleCapabilities {
            tray_available: true,
            window_hide_supported: true,
            window_geometry_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let response = handle_window_close_requested(&mut state, &root, None);

        assert_eq!(response, CloseRequestResponse::HideWindow);
        assert_eq!(state.window_lifecycle.window_geometry, None);
        assert_eq!(
            take_hidden_to_tray_geometry(),
            None,
            "no captured geometry must fall back to the WM placement"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_to_tray_geometry_record_take_are_consume_once() {
        let _guard = HIDDEN_GEOMETRY_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let geometry = Rect {
            x: 12,
            y: 34,
            width: 1100,
            height: 720,
        };

        assert_eq!(take_hidden_to_tray_geometry(), None, "starts empty");

        record_hidden_to_tray_geometry(Some(geometry));
        assert_eq!(take_hidden_to_tray_geometry(), Some(geometry));
        assert_eq!(take_hidden_to_tray_geometry(), None, "consume-once");

        // An explicit None record (failed capture) is also honored.
        record_hidden_to_tray_geometry(Some(geometry));
        record_hidden_to_tray_geometry(None);
        assert_eq!(take_hidden_to_tray_geometry(), None);
    }

    #[test]
    fn handle_window_close_requested_falls_back_to_exit_and_persists_geometry_without_tray_hide() {
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = true;
        state.window_lifecycle.capabilities = WindowLifecycleCapabilities {
            tray_available: false,
            window_hide_supported: false,
            window_geometry_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let panic = catch_unwind(AssertUnwindSafe(|| {
            handle_window_close_requested(
                &mut state,
                &root,
                Some(Rect {
                    x: 320,
                    y: 180,
                    width: 1440,
                    height: 810,
                }),
            )
        }))
        .expect_err("fallback close path should request process exit in tests");

        let exit = panic
            .downcast::<ProcessExit>()
            .map(|payload| *payload)
            .expect("panic payload should be ProcessExit");
        assert_eq!(exit, ProcessExit(0));
        assert_eq!(
            state.window_lifecycle.last_close_action,
            CloseAction::KeepVisibleNoTray
        );
        assert!(state.window_lifecycle.persisted_at_close);
        assert!(state
            .window_lifecycle
            .capability_notes
            .iter()
            .any(|note| note.contains("tray hide is unavailable")));

        let loaded = d4lc_config::load_app_settings(&root).expect("load after save");
        assert_eq!(
            loaded.window_geometry,
            Some(Rect {
                x: 320,
                y: 180,
                width: 1440,
                height: 810,
            })
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handle_window_close_requested_exits_when_minimize_to_tray_setting_is_off() {
        // Quit-condition pin: with the
        // `slint::run_event_loop_until_quit` seam in `main`, the event
        // loop does not end when windows hide or close, so this
        // explicit close-and-exit fallback is what terminates the
        // process when the close button means a real close
        // (`minimize_to_tray=false` → `CloseAction::Close`). Without it
        // the until-quit loop would keep a windowless process alive
        // forever.
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = false;
        state.window_lifecycle.capabilities = WindowLifecycleCapabilities {
            tray_available: true,
            window_hide_supported: true,
            window_geometry_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let panic = catch_unwind(AssertUnwindSafe(|| {
            handle_window_close_requested(&mut state, &root, None)
        }))
        .expect_err("close with the tray setting off should request process exit in tests");

        let exit = panic
            .downcast::<ProcessExit>()
            .map(|payload| *payload)
            .expect("panic payload should be ProcessExit");
        assert_eq!(exit, ProcessExit(0));
        assert_eq!(state.window_lifecycle.last_close_action, CloseAction::Close);
        assert!(state.window_lifecycle.persisted_at_close);

        let loaded = d4lc_config::load_app_settings(&root).expect("load after save");
        assert_eq!(
            loaded.window_geometry,
            state.window_lifecycle.window_geometry
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handle_window_closing_records_close_when_setting_off() {
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = false;
        let root = unique_tmp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        handle_window_closing(&mut state, &root);

        assert_eq!(state.window_lifecycle.last_close_action, CloseAction::Close);
        assert!(state.window_lifecycle.persisted_at_close);
        // When minimize_to_tray=false, no tray capability note should be added.
        assert!(state
            .window_lifecycle
            .capability_notes
            .iter()
            .all(|n| !n.contains("tray backend is unavailable")));

        let _ = std::fs::remove_dir_all(&root);
    }

    // -------------------------------------------------------------------
    // handle_window_state_changed — AppState projection
    // -------------------------------------------------------------------

    #[test]
    fn handle_window_state_changed_records_hide_to_tray_when_capable() {
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = true;
        state.window_lifecycle.capabilities = WindowLifecycleCapabilities {
            tray_available: true,
            window_hide_supported: true,
            ..WindowLifecycleCapabilities::SLINT_1X_LINUX
        };

        handle_window_state_changed(&mut state, true, false);

        assert_eq!(
            state.window_lifecycle.last_window_state_action,
            WindowStateAction::HideToTray
        );
        assert!(state.window_lifecycle.capability_notes.is_empty());
    }

    #[test]
    fn handle_window_state_changed_records_gap_when_tray_unavailable() {
        let mut state = AppState::default_startup();
        state.shell.settings.minimize_to_tray = true;
        // Default capabilities: tray_available=false (Slint 1.x Linux).

        handle_window_state_changed(&mut state, true, false);

        assert_eq!(
            state.window_lifecycle.last_window_state_action,
            WindowStateAction::MinimizeToTrayRequestedNoTray
        );
        assert!(state
            .window_lifecycle
            .capability_notes
            .iter()
            .any(|n| n.contains("minimize_to_tray=true")));
    }

    #[test]
    fn handle_window_state_changed_noop_when_not_minimized() {
        let mut state = AppState::default_startup();
        handle_window_state_changed(&mut state, false, false);
        assert_eq!(
            state.window_lifecycle.last_window_state_action,
            WindowStateAction::NoOp
        );
    }

    // -------------------------------------------------------------------
    // handle_tray_open / handle_tray_exit — AppState projection
    // -------------------------------------------------------------------

    #[test]
    fn handle_tray_open_records_action_and_capability_note() {
        let mut state = AppState::default_startup();
        handle_tray_open(&mut state);
        assert_eq!(
            state.window_lifecycle.last_tray_open_action,
            Some(TrayOpenAction::OpenOrRestore)
        );
        assert!(state
            .window_lifecycle
            .capability_notes
            .iter()
            .any(|n| n.contains("tray open/restore recorded")));
    }

    #[test]
    fn handle_tray_exit_records_action_and_requests_process_exit() {
        let mut state = AppState::default_startup();

        let panic = catch_unwind(AssertUnwindSafe(|| {
            handle_tray_exit(&mut state);
        }))
        .expect_err("tray exit should request process exit in tests");

        let exit = panic
            .downcast::<ProcessExit>()
            .map(|payload| *payload)
            .expect("panic payload should be ProcessExit");
        assert_eq!(exit, ProcessExit(0));
        assert_eq!(
            state.window_lifecycle.last_tray_exit_action,
            Some(TrayExitAction::Exit)
        );
    }

    // -------------------------------------------------------------------
    // X11 minimize — explicit window-id targeting using
    // the existing `x11rb-backend` feature.
    // -------------------------------------------------------------------

    #[cfg(feature = "x11rb-backend")]
    #[test]
    fn window_minimize_builds_iconic_state_client_message_for_target_window() {
        let event = build_x11_minimize_request(0x00ab_cdef, 0x42);

        assert_eq!(event.format, 32);
        assert_eq!(event.window, 0x00ab_cdef);
        assert_eq!(event.type_, 0x42);
        assert_eq!(event.data.as_data32(), [X11_ICONIC_STATE, 0, 0, 0, 0]);
    }

    #[cfg(feature = "x11rb-backend")]
    #[test]
    fn window_minimize_extracts_xcb_target_from_raw_handles() {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle,
        };
        use std::num::NonZeroU32;

        let target = x11_window_target_from_raw_handles(
            RawWindowHandle::Xcb(XcbWindowHandle::new(
                NonZeroU32::new(0x1234).expect("nonzero"),
            )),
            RawDisplayHandle::Xcb(XcbDisplayHandle::new(None, 2)),
        )
        .expect("xcb target");

        assert_eq!(target.window_id, 0x1234);
        assert_eq!(target.screen_num, 2);
    }

    #[cfg(feature = "x11rb-backend")]
    #[test]
    fn window_minimize_extracts_xlib_target_from_raw_handles() {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, XlibDisplayHandle, XlibWindowHandle,
        };

        let target = x11_window_target_from_raw_handles(
            RawWindowHandle::Xlib(XlibWindowHandle::new(0x5678)),
            RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 1)),
        )
        .expect("xlib target");

        assert_eq!(target.window_id, 0x5678);
        assert_eq!(target.screen_num, 1);
    }

    #[cfg(feature = "x11rb-backend")]
    #[test]
    fn window_minimize_rejects_non_x11_raw_handles() {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
        };
        use std::ptr::NonNull;

        let error = x11_window_target_from_raw_handles(
            RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::dangling())),
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(NonNull::dangling())),
        )
        .expect_err("wayland should be rejected");

        assert!(error.contains("not X11-compatible"));
    }

    // -------------------------------------------------------------------
    // Description stability — the log-line vocabulary must not change.
    // -------------------------------------------------------------------

    #[test]
    fn close_action_descriptions_are_stable() {
        assert_eq!(CloseAction::Close.description(), "close-and-exit");
        assert_eq!(
            CloseAction::MinimizeToTray.description(),
            "minimize-to-tray"
        );
        assert_eq!(
            CloseAction::KeepVisibleNoTray.description(),
            "close-and-exit (minimize-to-tray requested but tray unavailable)"
        );
    }

    #[test]
    fn window_state_action_descriptions_are_stable() {
        assert_eq!(WindowStateAction::NoOp.description(), "no-op");
        assert_eq!(
            WindowStateAction::MinimizeOnly.description(),
            "minimize-only"
        );
        assert_eq!(WindowStateAction::HideToTray.description(), "hide-to-tray");
        assert_eq!(
            WindowStateAction::MinimizeToTrayRequestedNoTray.description(),
            "minimize-to-tray-requested-but-tray-unavailable"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod title_bar_link_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn testing_app() -> crate::slint_generated::AppWindow {
        crate::test_window::new_app_window()
    }

    #[test]
    fn each_title_bar_button_opens_its_own_link() {
        let app = testing_app();
        let opened = Rc::new(RefCell::new(Vec::new()));
        let record = Rc::clone(&opened);
        wire_title_bar_links(&app, move |_, link| record.borrow_mut().push(link));

        app.invoke_d4lc_github_clicked();
        app.invoke_upstream_github_clicked();
        app.invoke_upstream_kofi_clicked();
        app.invoke_upstream_wiki_clicked();

        assert_eq!(
            *opened.borrow(),
            [
                TitleBarLink::D4lcRepository,
                TitleBarLink::UpstreamRepository,
                TitleBarLink::UpstreamKofi,
                TitleBarLink::UpstreamWiki,
            ]
        );
    }

    #[test]
    fn a_failed_title_bar_link_reaches_the_logging_screen() {
        let app = testing_app();
        let app_state = Mutex::new(AppState::default_startup());

        launch_title_bar_link(&app, &app_state, TitleBarLink::UpstreamWiki, |_| Ok(()));
        assert!(app_state.lock().unwrap().logging.messages.is_empty());

        launch_title_bar_link(&app, &app_state, TitleBarLink::UpstreamWiki, |url| {
            Err(format!("failed to spawn xdg-open for '{url}': not found"))
        });
        let state = app_state.lock().unwrap();
        let message = &state.logging.messages.last().expect("logged").message;
        assert!(
            message.starts_with(
                "Opening https://github.com/josdemmers/Diablo4Companion/wiki failed: "
            ),
            "{message}"
        );
        use slint::Model as _;
        assert_eq!(
            app.get_log_messages().row_count(),
            1,
            "projected to the screen"
        );
    }

    #[test]
    fn open_with_reports_only_a_program_that_did_not_start() {
        assert_eq!(open_with("true", "https://example.invalid/"), Ok(()));
        let error = open_with("/nonexistent/d4lc-opener", "https://example.invalid/")
            .expect_err("missing program");
        assert!(
            error.starts_with("failed to spawn /nonexistent/d4lc-opener for "),
            "{error}"
        );
    }

    #[test]
    fn own_repository_link_opens_d4lc() {
        assert_eq!(
            TitleBarLink::D4lcRepository.url(),
            "https://github.com/florianaltendorfer/d4lc"
        );
        // Same source as the update check's release URL.
        assert_eq!(
            TitleBarLink::D4lcRepository.url(),
            env!("CARGO_PKG_REPOSITORY")
        );
    }

    #[test]
    fn upstream_links_keep_upstream_targets() {
        assert_eq!(
            TitleBarLink::UpstreamRepository.url(),
            "https://github.com/josdemmers/Diablo4Companion"
        );
        assert_eq!(
            TitleBarLink::UpstreamWiki.url(),
            "https://github.com/josdemmers/Diablo4Companion/wiki"
        );
        assert_eq!(
            TitleBarLink::UpstreamKofi.url(),
            "https://ko-fi.com/josdemmers"
        );
    }
}
