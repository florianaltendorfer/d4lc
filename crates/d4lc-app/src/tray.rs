//! Linux system-tray icon provider.
//!
//! ## Upstream evidence
//!
//! `MainWindow.xaml:88-103` — WPF `TaskbarIcon` with `NotifyIcon*` commands.
//! `MainWindowViewModel.cs:254-280` — Double-click, Open, and Exit handlers.
//!
//! ## Expected behaviour (parity target)
//!
//! - Tooltip: `"Diablo IV Companion"` (upstream-consistent SNI tooltip
//!   title; the item id and icon name stay `"d4lc"`).
//! - Left-click or right-click context menu with **Open** and **Exit** items.
//! - Activate (left-click) on the tray icon → show/restore main window.
//! - Open menu item → show/restore main window.
//! - Exit menu item → quit application.
//!
//! ## Linux platform notes
//!
//! Upstream uses the Windows-only `Hardcodet.NotifyIcon.Wpf` library.
//! On Linux, tray icons require either:
//!
//! - `libappindicator` (Ubuntu/Unity/GNOME classic)
//! - `StatusNotifierItem` D-Bus protocol (modern KDE, some GNOME shells)
//! - `libayatana-appindicator` (Ayatana fork, preferred on Debian/Ubuntu)
//!
//! This module is the real Linux tray backend. The `tray-icon` Cargo feature
//! gates the StatusNotifierItem integration via the pure-Rust `ksni` crate.
//! When the feature is disabled, `init_tray` is a no-op that logs a
//! capability note — no fake tray is created.
//!
//! When the feature is enabled but the runtime lacks a reachable D-Bus
//! session (e.g. headless CI), `ksni::Tray::spawn` returns an error and we
//! log the failure and keep the app running without a tray icon.

use std::sync::{Arc, Mutex, OnceLock};

use crate::app_state::AppState;

/// Shared `AppState` handle so the tray menu callbacks can record
/// their actions. Set once from `main` before `init_tray`.
static APP_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();

/// Optional hook invoked from the tray "Open" action after the
/// `AppState` projection is recorded. The default is a no-op; `main`
/// installs a closure that asks the live Slint window to show/restore.
type ShowWindowHook = Box<dyn Fn() + Send + Sync + 'static>;
static SHOW_WINDOW: OnceLock<ShowWindowHook> = OnceLock::new();

/// Register the shared `AppState` handle so the tray menu callbacks
/// can record their actions. Must be called once before `init_tray`.
pub fn set_app_state(state: Arc<Mutex<AppState>>) {
    let _ = APP_STATE.set(state);
}

/// Register the closure that shows/restores the live Slint window
/// when the user clicks the tray "Open" item. Must be called once
/// before `init_tray`.
pub fn set_show_window_hook(hook: ShowWindowHook) {
    let _ = SHOW_WINDOW.set(hook);
}

/// Initialise the system-tray icon.
///
/// When the `tray-icon` feature is enabled this spawns a real
/// `ksni` StatusNotifierItem on the session D-Bus. Otherwise it
/// logs a capability note — no fake tray.
#[cfg(feature = "tray-icon")]
pub fn init_tray() {
    imp::start();
}

/// Initialise the system-tray icon (no-op stub when the
/// `tray-icon` feature is disabled).
#[cfg(not(feature = "tray-icon"))]
pub fn init_tray() {
    eprintln!(
        "[d4lc] tray: feature 'tray-icon' is disabled; no system tray icon will be created. \
         Rebuild with --features tray-icon to enable the StatusNotifierItem backend."
    );
    mark_tray_available(false);
}

fn mark_tray_available(available: bool) {
    if let Some(state) = APP_STATE.get() {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.window_lifecycle.capabilities.tray_available = available;
    }
}

/// Show / restore the main window from the tray.
///
/// This is the callback target for both the tray "Open" menu item
/// and the tray-icon double-click handler. It records the action
/// in `AppState` (via the existing `actions::handle_tray_open`
/// projection) and then invokes the live-window show hook installed
/// by `main`.
///
/// Safe to call when no tray backend is active — it simply records
/// the action and runs the hook.
pub fn handle_tray_open() {
    if let Some(state) = APP_STATE.get() {
        let state_clone = Arc::clone(state);
        // The ksni menu callbacks fire on the D-Bus thread. Route
        // the AppState mutation and window-show through the Slint
        // event loop so we touch the UI from the right thread.
        let _ = slint::invoke_from_event_loop(move || {
            let mut state = state_clone
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            crate::actions::handle_tray_open(&mut state);
            if let Some(hook) = SHOW_WINDOW.get() {
                hook();
            }
        });
    } else if let Some(hook) = SHOW_WINDOW.get() {
        let _ = slint::invoke_from_event_loop(hook);
    }
}

/// Quit the application from the tray exit menu item.
///
/// Records the action in `AppState` (via the existing
/// `actions::handle_tray_exit` projection, which itself calls
/// `process::exit(0)`) and falls back to a direct `process::exit(0)`
/// if no `AppState` has been registered yet.
pub fn handle_tray_exit() {
    if let Some(state) = APP_STATE.get() {
        let state_clone = Arc::clone(state);
        let _ = slint::invoke_from_event_loop(move || {
            let mut state = state_clone
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            crate::actions::handle_tray_exit(&mut state);
        });
    } else {
        std::process::exit(0);
    }
}

// ── Real ksni backend (feature-gated) ──────────────────────────────

#[cfg(feature = "tray-icon")]
mod imp {
    use super::*;
    use ksni::menu::{MenuItem, StandardItem};
    use ksni::Tray;
    use ksni::TrayMethods as _;

    /// The `ksni::Tray` implementation for d4lc.
    ///
    /// The struct is a unit type — the menu callbacks close over
    /// `crate::tray::{handle_tray_open, handle_tray_exit}` and read
    /// the shared `AppState` / `SHOW_WINDOW` from the module-level
    /// statics set by `main`.
    pub struct D4lcTray;

    impl Tray for D4lcTray {
        fn id(&self) -> String {
            "d4lc".into()
        }

        fn title(&self) -> String {
            "d4lc".into()
        }

        fn icon_name(&self) -> String {
            "d4lc".into()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            // Upstream tooltip text (`MainWindow.xaml:88-95`):
            // "Diablo IV Companion".
            ksni::ToolTip {
                icon_name: "d4lc".into(),
                icon_pixmap: Vec::new(),
                title: "Diablo IV Companion".into(),
                description: String::new(),
            }
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: "Open".into(),
                    activate: Box::new(|_tray: &mut D4lcTray| {
                        handle_tray_open();
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Exit".into(),
                    activate: Box::new(|_tray: &mut D4lcTray| {
                        handle_tray_exit();
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            // Upstream `MainWindow.xaml:95` DoubleClickCommand →
            // `NotifyIconDoubleClickExecute`
            // (`MainWindowViewModel.cs:254-262`): activating the tray
            // icon shows/restores the main window. On the SNI protocol
            // this is the primary (left-click) Activate request. Route
            // through the same shared show path as the Open menu item.
            handle_tray_open();
        }
    }

    /// Holds the live `ksni` service handle for the app's lifetime.
    /// Dropping the handle stops the D-Bus service, so we keep it
    /// parked behind a `OnceLock<Mutex<Option<_>>>`.
    static TRAY_HANDLE: OnceLock<Mutex<Option<ksni::Handle<D4lcTray>>>> = OnceLock::new();

    /// Process-lifetime runtime context owned by the dedicated tray
    /// runtime thread. Set by that thread right after it builds the
    /// runtime, before the registration handshake, so the runtime is
    /// observable (and provably still alive) from the outside.
    ///
    /// Why a dedicated thread: zbus 5.16 caches a process-global executor
    /// handle the first time any D-Bus connection is built in the process.
    /// Driving `ksni::Tray::spawn` on a throwaway current-thread runtime
    /// and dropping it after `block_on` would leave that cached handle
    /// pointing at a dead reactor; Slint's `accesskit_unix` and the winit
    /// xdg-settings zbus connections, which only start on a real desktop
    /// session, then panic shortly after launch (`there is no reactor
    /// running, must be called from the context of a Tokio 1.x runtime`).
    /// The runtime is owned by a thread that parks on it forever, so the
    /// cached handle and the ksni service task stay valid for the process
    /// lifetime.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the tray runtime tests")
    )]
    pub(super) struct TrayRuntime {
        pub(super) handle: tokio::runtime::Handle,
        pub(super) thread_id: std::thread::ThreadId,
    }
    static TRAY_RUNTIME: OnceLock<TrayRuntime> = OnceLock::new();

    /// The process-lifetime tray runtime, once the tray thread has built
    /// it. Returns `None` before the first `start` call (or if the thread
    /// itself could not be spawned).
    #[cfg(test)]
    pub(super) fn runtime_handle() -> Option<&'static TrayRuntime> {
        TRAY_RUNTIME.get()
    }

    /// Registration outcome reported from the tray runtime thread back to
    /// the synchronous `start` caller.
    enum Registration {
        Spawned(ksni::Handle<D4lcTray>),
        Failed(String),
    }

    /// Spawn the `ksni` StatusNotifierItem service. Idempotent —
    /// repeated calls after the first one are no-ops. On failure
    /// (no session bus, no StatusNotifierWatcher, etc.) the error
    /// is logged and the app keeps running without a tray icon.
    ///
    /// The registration handshake runs on a dedicated thread that owns
    /// a tokio runtime and parks on it forever: the ksni
    /// service task keeps executing there, and zbus's process-global
    /// cached executor handle never points at a dropped runtime.
    pub fn start() {
        let cell = TRAY_HANDLE.get_or_init(|| Mutex::new(None));
        let mut guard = match cell.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_some() {
            return;
        }
        let (result_tx, result_rx) = std::sync::mpsc::channel::<Registration>();
        if let Err(error) = std::thread::Builder::new()
            .name("d4lc-tray-runtime".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = result_tx.send(Registration::Failed(format!(
                            "failed to build the tray tokio runtime ({error})"
                        )));
                        return;
                    }
                };
                let _ = TRAY_RUNTIME.set(TrayRuntime {
                    handle: runtime.handle().clone(),
                    thread_id: std::thread::current().id(),
                });
                // Drive the registration handshake (and host the ksni
                // service task) on the thread-owned runtime.
                let spawned = runtime.block_on(D4lcTray.spawn());
                let _ = result_tx.send(match spawned {
                    Ok(handle) => Registration::Spawned(handle),
                    Err(error) => Registration::Failed(format!(
                        "failed to register StatusNotifierItem ({error})"
                    )),
                });
                // Park on the runtime forever. This thread never returns:
                // dropping the runtime (or letting the service task die)
                // kills every zbus connection that cached its handle.
                runtime.block_on(std::future::pending::<()>());
            })
        {
            eprintln!(
                "[d4lc] tray: failed to spawn the tray runtime thread ({error}); \
                 running without a tray icon."
            );
            mark_tray_available(false);
            return;
        }
        match result_rx.recv() {
            Ok(Registration::Spawned(handle)) => {
                eprintln!(
                    "[d4lc] tray: StatusNotifierItem registered (id={}, title={})",
                    D4lcTray.id(),
                    D4lcTray.title()
                );
                mark_tray_available(true);
                *guard = Some(handle);
            }
            Ok(Registration::Failed(reason)) => {
                eprintln!(
                    "[d4lc] tray: {reason}. No system tray icon will be shown; \
                     the app will run without tray support."
                );
                mark_tray_available(false);
            }
            Err(_) => {
                eprintln!(
                    "[d4lc] tray: the tray runtime thread exited without reporting a \
                     registration result; running without a tray icon."
                );
                mark_tray_available(false);
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────
//
// Pure construction / metadata / action mapping tests. They do not
// require a live D-Bus session: the ksni `Tray` trait methods return
// plain strings, and the menu items can be inspected without spawning
// the service.

#[cfg(all(test, feature = "tray-icon"))]
mod tray_tests {
    use super::*;
    use ksni::Tray as _;

    #[test]
    fn tray_id_is_stable_d4lc() {
        assert_eq!(imp::D4lcTray.id(), "d4lc");
    }

    #[test]
    fn tray_title_is_d4lc() {
        assert_eq!(imp::D4lcTray.title(), "d4lc");
    }

    #[test]
    fn tray_icon_name_is_d4lc() {
        assert_eq!(imp::D4lcTray.icon_name(), "d4lc");
    }

    #[test]
    fn tray_tool_tip_title_is_upstream_name() {
        assert_eq!(
            imp::D4lcTray.tool_tip().title,
            "Diablo IV Companion",
            "the SNI tooltip title must carry the upstream-consistent name"
        );
    }

    #[test]
    fn tray_menu_has_open_and_exit_items() {
        let menu = imp::D4lcTray.menu();
        assert_eq!(
            menu.len(),
            2,
            "tray menu should have exactly Open and Exit items"
        );

        let (open_label, exit_label) = match (&menu[0], &menu[1]) {
            (ksni::menu::MenuItem::Standard(open), ksni::menu::MenuItem::Standard(exit)) => {
                (open.label.clone(), exit.label.clone())
            }
            _ => panic!("tray menu items must both be StandardItem"),
        };
        assert_eq!(open_label, "Open");
        assert_eq!(exit_label, "Exit");
    }
}

#[cfg(all(test, feature = "tray-icon"))]
#[test]
fn tray_init_tray_is_idempotent_and_does_not_panic_without_session_bus() {
    // Calling init_tray on a machine without a D-Bus session must
    // not panic — the ksni error path is logged and swallowed.
    init_tray();
    init_tray();
}

// Test-only relaxation: the workspace denies clippy::expect_used, but these
// regression tests deliberately use expect() to assert on the presence and
// liveness of the process-lifetime runtime (repo-standard test-module
// allowance, see window_observer.rs::tests).
#[cfg(all(test, feature = "tray-icon"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tray_runtime_lifetime_tests {
    // Regression tests. The defect class: the runtime that
    // hosted the ksni registration handshake was dropped as soon as
    // `start` returned, leaving zbus's process-global cached executor
    // handle dead — every later tokio/zbus user in the process (Slint
    // accesskit, winit xdg-settings) then panicked. These tests prove
    // the fix shape headlessly: after `start`, a process-lifetime
    // runtime must still exist, still drive detached tasks, and be
    // reused across repeated starts.
    use super::*;

    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    #[test]
    fn tray_runtime_survives_start_and_drives_detached_tasks() {
        init_tray();
        let runtime =
            imp::runtime_handle().expect("tray start must leave a process-lifetime runtime behind");

        // A runtime dropped after the registration handshake (the
        // Defect) could neither accept this spawn nor drive
        // it to completion.
        let (tx, rx) = std::sync::mpsc::channel();
        // `Handle::spawn` itself panics if the runtime is gone.
        runtime
            .handle
            .spawn(async move { tx.send("alive").expect("receiver alive") });
        assert_eq!(
            rx.recv_timeout(TIMEOUT),
            Ok("alive"),
            "the tray runtime must still drive detached tasks after the start \
             handshake returned — a dropped runtime leaves zbus's cached \
             process-global handle dead"
        );
    }

    #[test]
    fn tray_repeated_start_reuses_the_same_process_lifetime_runtime() {
        init_tray();
        let first =
            imp::runtime_handle().expect("tray start must leave a process-lifetime runtime behind");
        init_tray();
        imp::start();
        let second = imp::runtime_handle()
            .expect("the process-lifetime runtime must survive repeated starts");
        assert_eq!(
            first.thread_id, second.thread_id,
            "repeated start calls must reuse the same runtime thread, not build \
             a fresh throwaway runtime per call"
        );

        // The runtime observed before the repeated starts must still be
        // the one driving work afterwards.
        let (tx, rx) = std::sync::mpsc::channel();
        first
            .handle
            .spawn(async move { tx.send(()).expect("receiver alive") });
        assert_eq!(rx.recv_timeout(TIMEOUT), Ok(()));
    }
}
