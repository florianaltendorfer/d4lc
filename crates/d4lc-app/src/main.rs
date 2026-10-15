// Without `real-runtime` the app runs a stub live loop, so the capture,
// diagnostics and overlay plumbing is compiled but not reached. The release
// build enables the feature and keeps the dead-code lint.
#![cfg_attr(not(feature = "real-runtime"), allow(dead_code))]

mod actions;
mod app_state;
mod clock;
mod crash_report;
mod diagnostics;
mod display_backend;
#[cfg(feature = "real-runtime")]
mod game_window_observer;
mod hotkeys;
mod i18n;
#[cfg(test)]
mod icon_geometry;
mod openmp_limit;
mod runtime_bridge;
#[cfg(feature = "real-runtime")]
mod runtime_prepare;
mod settings_consumers;
mod single_instance;
#[cfg(test)]
#[allow(clippy::expect_used)]
mod test_root;
#[cfg(test)]
mod test_window;
#[cfg(test)]
mod testing_harness;
mod tray;
mod ui_renderer;
mod update_check;
#[cfg(test)]
mod visual_smoke;
mod window_observer;
mod wiring;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use app_state::AppState;
use d4lc_gui::{GuiTab, GuiTabKind};
use slint::{SharedString, VecModel};

#[rustfmt::skip]
#[allow(unused_qualifications, clippy::unwrap_used, clippy::expect_used)]
mod slint_generated {
    slint::include_modules!();
}
use slint_generated::*;

fn main() -> Result<(), slint::PlatformError> {
    // ── OpenMP thread limit ──────────────────────────────────────────
    // Before anything else: a relaunch replaces this process, so nothing
    // may be acquired or started yet (see `openmp_limit`).
    #[cfg(feature = "leptess-backend")]
    openmp_limit::ensure_single_thread_per_call();

    let instance_lock = acquire_single_instance();

    // ── `--no-updates` CLI-style flag ─────────────────────────────────
    // Suppresses the check-only release banner check for this session.
    // The persisted Settings toggle suppresses it persistently; both
    // gate the same bounded check (see `update_check`).
    let updates_suppressed_by_flag = std::env::args().any(|arg| arg == "--no-updates");

    // ── App state: canonical user-facing state owner ──────────────────
    let app_state = Arc::new(Mutex::new(AppState::default_startup()));

    // ── Config persistence ──────────────────────────────────
    let config_root = actions::settings::resolve_config_root();
    // A settings file that cannot be read starts the app on the defaults;
    // the reason goes to the Logging screen once the state exists.
    // Like upstream's startup load, the settings and the controller list are
    // written back, so a first run creates both files.
    let (config_settings, settings_load_error, startup_write_errors) =
        match d4lc_config::load_app_settings_and_save(&config_root) {
            Ok((settings, save_error)) => {
                let controller_error = d4lc_config::load_controller_config_and_save(&config_root)
                    .err()
                    .map(|error| format!("Config/Controllers.json was not written: {error}"));
                let save_error = save_error
                    .map(|error| format!("Config/Settings.json was not written: {error}"));
                (
                    settings,
                    None,
                    save_error
                        .into_iter()
                        .chain(controller_error)
                        .collect::<Vec<_>>(),
                )
            }
            Err(error) => (d4lc_config::AppSettings::default(), Some(error), Vec::new()),
        };

    // ── Local crash reporting ─────────────────────────────────────────
    // Install the panic hook before any window / runtime work so a
    // startup panic is captured too. Every panic writes one bounded,
    // self-describing crash log (app version, panic message +
    // location, non-personal settings snapshot, backtrace) under the
    // local crash directory; logs survive restarts. Diagnostics stay
    // strictly local: exporting a log is an explicit, opt-in user
    // action for attaching it to a GitHub issue opened manually — the
    // app never uploads anything.
    crash_report::install_panic_hook(
        env!("CARGO_PKG_VERSION"),
        crash_report::settings_snapshot(&config_settings),
    );
    let crash_dir = crash_report::resolve_crash_dir();

    init_app_state_from_settings(&app_state, &config_settings, settings_load_error);
    for message in startup_write_errors {
        eprintln!("[d4lc] {message}");
        app_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .append_log_message(message, clock::hh_mm_now());
    }

    let shell_state = {
        let state = app_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.shell.clone()
    };

    // ── UI renderer ────────────────────────────────────
    // femtovg over OpenGL; a GL startup failure relaunches with the
    // software renderer (named in the log and on the Debug panel).
    // `SLINT_BACKEND` stays an explicit override.
    let renderer_plan = ui_renderer::plan_from_env();
    ui_renderer::select(&renderer_plan)?;
    eprintln!(
        "d4lc-app: UI renderer: {}",
        ui_renderer::renderer_text(&renderer_plan, ui_renderer::RendererObservation::Waiting)
    );

    let app = AppWindow::new()?;

    app.set_ui_renderer_text(
        ui_renderer::renderer_text(&renderer_plan, ui_renderer::RendererObservation::Waiting)
            .into(),
    );
    let renderer_observed = {
        let app_weak = app.as_weak();
        let plan = renderer_plan.clone();
        ui_renderer::observe(app.window(), move |observed| {
            let text = ui_renderer::renderer_text(&plan, observed);
            eprintln!("d4lc-app: UI renderer: {text}");
            if let Some(app) = app_weak.upgrade() {
                app.set_ui_renderer_text(text.into());
            }
        })
    };

    populate_window(&app, &app_state, shell_state, crash_dir.as_deref());
    wire_shell_callbacks(&app, &app_state, updates_suppressed_by_flag);
    wire_screen_actions(&app, &app_state, &config_root, crash_dir);
    wire_shell_integration(&app, &app_state, &config_root, &config_settings);

    // Slint 1.x delegates initial placement to the platform window manager.
    // Leave position unset so desktop centering policy can apply.
    //
    // Slint's `AppWindow::run()` returns as soon as the last window is
    // hidden, so hide-to-tray (a minimize or a window close with
    // `MinimizeToTray=true` and a live tray) would end the process and take
    // the tray icon with it. Upstream WPF keeps running because `Hide()` is
    // not a shutdown
    // (`MainWindowViewModel.cs:254-280` tray handlers).
    // `AppWindow::run()` is only `show()` + `run_event_loop()` +
    // `hide()` (i-slint-core `ComponentHandle::run`), so the equivalent
    // seam keeps the explicit `show()` and swaps the loop for
    // `slint::run_event_loop_until_quit()` (slint 1.16), which stays
    // alive while no window is visible and only returns when
    // `slint::quit_event_loop()` is called. The sanctioned explicit
    // process exits are the tray-Exit path (`actions::handle_tray_exit`)
    // and the close-and-exit fallback of
    // `actions::handle_window_close_requested` (every non-hide close
    // decision), both terminating via `exit_process(0)` after recording
    // and persisting the close state.
    app.show()?;
    let result = slint::run_event_loop_until_quit();

    // A GPU renderer that never rendered a frame: relaunch once with the
    // software renderer instead of exiting with the GL error.
    if let Err(error) = &result {
        if let ui_renderer::LoopErrorAction::RelaunchWithSoftware { reason } =
            ui_renderer::on_event_loop_error(
                &renderer_plan,
                renderer_observed.get(),
                &error.to_string(),
            )
        {
            relaunch_with_software_renderer(app, instance_lock, &reason);
            return result;
        }
    }

    // Event loop ended (explicit quit path) — persist state before exit.
    {
        let mut state = app_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        actions::handle_window_closing(&mut state, &config_root);
    }

    // Clean exit: release the single-instance flock and remove the
    // show-request socket entry. Exit(0)/crash paths leave that cleanup
    // to the kernel and to the next holder's stale-entry removal.
    drop(instance_lock);

    result
}

/// Take the single-instance guard; exits the process when this launch must
/// not run.
fn acquire_single_instance() -> single_instance::InstanceLock {
    // ── Single-instance guard ────────────────────────────────────────
    // Upstream `App.xaml.cs:56-72` takes a named `Mutex("D4Companion")`
    // in `OnStartup`; a second launch shuts down immediately. The Linux
    // equivalent holds an advisory flock under `XDG_RUNTIME_DIR`. A
    // second launch asks the running instance — over a local
    // show-request socket routed to `tray::handle_tray_open`, the same
    // show path the tray uses — to restore its window, then exits 0.
    // Refusing to start unguarded keeps the no-double-instance
    // guarantee when the guard cannot be established.
    match single_instance::acquire() {
        single_instance::AcquireOutcome::Acquired(mut lock) => {
            lock.start_show_request_server(tray::handle_tray_open);
            lock
        }
        // The second-launch message was already printed by the guard.
        single_instance::AcquireOutcome::AlreadyRunning { .. } => std::process::exit(0),
        single_instance::AcquireOutcome::Failed => std::process::exit(1),
    }
}

/// Initialize `AppState` from the loaded config.
fn init_app_state_from_settings(
    app_state: &Mutex<AppState>,
    config_settings: &d4lc_config::AppSettings,
    settings_load_error: Option<String>,
) {
    let mut state = app_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.shell.settings = d4lc_gui::SettingsPanelState::from_config(
        config_settings,
        Vec::new(), // no controller images in GUI mode yet
    );
    if let Some(error) = &settings_load_error {
        let message = format!("{error}; starting with the default settings");
        eprintln!("[d4lc] {message}");
        state.append_log_message(message, clock::hh_mm_now());
    }
    // Upstream gates the launch minimize on `LaunchMinimized &&
    // !_releaseManager.UpdateAvailable` (MainWindowViewModel.cs:120-123).
    // The check-only release check starts `FIRST_CHECK_DELAY` after
    // launch, so at this point no result exists yet in this session and
    // the guard is a no-op — the flag is threaded anyway so a pending
    // update always wins over `LaunchMinimized` once one is known.
    state.window_lifecycle.last_launch_action = actions::decide_launch_action(
        state.shell.settings.launch_minimized,
        state.window_lifecycle.update_pending,
    );
    // Hydrate the in-memory hotkey drafts
    // from the persisted `AppSettings::hotkey_config` so the
    // HotkeyConfig dialog opens with the user's saved bindings
    // rather than always seeding a fallback binding. The
    // `d4lc_config` parser already fills the 6-entry default
    // when the field is missing, so this is a pure projection.
    state.hydrate_hotkey_drafts_from_settings(config_settings);
    // Hydrate the build-import state (multi-build rows
    // and the per-provider "Import paragon boards" toggles) from the
    // loaded config so the tabbed import dialog restores the
    // persisted toggle state after a restart instead of resetting to
    // the all-true defaults.
    state.hydrate_build_import_from_settings(config_settings);
}

/// Seed the window's properties and models from the shell state, the
/// `AppState` and the crash directory.
fn populate_window(
    app: &AppWindow,
    app_state: &Mutex<AppState>,
    shell_state: d4lc_gui::GuiShellState,
    crash_dir: Option<&Path>,
) {
    app.set_window_title(shell_state.window_title.into());
    seed_version_caption(app);
    // TopMost follows the persisted `Settings.IsTopMost` (upstream
    // MainWindow.xaml:18 binds `Topmost` to it, MainWindowViewModel.cs:95-98
    // re-applies it on `TopMostStateChangedMessage`). The Debug screen's
    // toggle and the overlay config dialog both persist this field, so the
    // window starts in the state the user last chose.
    app.set_window_topmost(shell_state.settings.overlay_top_most);

    let tab_labels = shell_state
        .tabs
        .iter()
        .map(map_tab_label)
        .map(SharedString::from)
        .collect::<Vec<_>>();
    app.set_tab_labels(Rc::new(VecModel::from(tab_labels)).into());

    let active_tab_index = shell_state
        .tabs
        .iter()
        .position(|tab| tab.kind == shell_state.active_tab)
        .unwrap_or(0) as i32;
    app.set_active_tab(active_tab_index);

    // ── Populate AppWindow properties from AppState ─────────
    {
        let state = app_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        app.set_debug_mode(state.shell.settings.debug_mode);
        // Upstream `SettingsD4.DevMode` gates the Debug Mode toggle's
        // visibility (SettingsView.xaml:88,97): the stored value seeds the
        // settings screen's `dev-mode` property.
        app.set_dev_mode(state.shell.settings.dev_mode);
        app.set_check_updates(state.shell.settings.check_for_updates);
        app.set_launch_minimized(state.shell.settings.launch_minimized);
        app.set_minimize_to_tray(state.shell.settings.minimize_to_tray);
        app.set_controller_mode(state.shell.settings.controller_mode);
        app.set_paragon_mode(state.shell.settings.paragon_mode_active);
        app.set_diagnostics_export(state.shell.settings.diagnostics_export_enabled);
        app.set_update_check_enabled(state.shell.settings.update_check_enabled);
    }

    // ── Crash-log list for the Debug screen ──────────────────────────
    // Newest-first file names from the resolved crash directory; an
    // empty model renders the screen's "no crash logs" state.
    {
        let entries: Vec<slint::SharedString> = crash_dir
            .map(crash_report::list_crash_logs)
            .unwrap_or_default()
            .into_iter()
            .map(slint::SharedString::from)
            .collect();
        app.set_crash_log_entries(Rc::new(VecModel::from(entries)).into());
    }

    // ── Initialize preset-names model from AppState ─────────────────
    // Pre-hydration window only: the editor has no presets until
    // `wire_affix_filters_actions` loads the config, which then seeds the
    // ComboBox, the selection and the entry models through the single
    // shared preset-model refresh (`actions::affix`).
    {
        let state = app_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        actions::affix::update_preset_names(app, &state);
    }

    {
        let state = app_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        actions::affix::set_filter_toggle_models(app, &state.shell.settings.toggles);
    }
}

/// Wire the shell-level callbacks: title-bar links, navigation and the
/// check-only release check.
fn wire_shell_callbacks(
    app: &AppWindow,
    app_state: &Arc<Mutex<AppState>>,
    updates_suppressed_by_flag: bool,
) {
    // ── Wire Slint callbacks ──────────────────────────────────────────
    {
        let app_state = Arc::clone(app_state);
        actions::wire_title_bar_links(app, move |app, link| {
            actions::handle_launch_title_bar_link(app, &app_state, link);
        });
    }

    // ── Wire the check-only release banner link ───────────────────
    // Activating the banner opens the release page in the user's
    // browser. No download, no install, no upload.
    {
        let app_weak = app.as_weak();
        app.on_update_banner_activated(move || {
            if let Some(app) = app_weak.upgrade() {
                update_check::open_release_url(app.get_update_url().as_str());
            }
        });
    }

    // ── Start the bounded release check ───────────────────────────
    // Dedicated background thread (off the startup path and the hot
    // path), hard HTTP timeout, failures logged only. Suppressed by
    // the `--no-updates` flag or the persisted Settings toggle.
    update_check::wire_update_check(app, Arc::clone(app_state), updates_suppressed_by_flag);
}

/// Wire the screen action handlers, the runtime bridge and the persisted
/// overlay restore.
fn wire_screen_actions(
    app: &AppWindow,
    app_state: &Arc<Mutex<AppState>>,
    config_root: &Path,
    crash_dir: Option<PathBuf>,
) {
    // ── Wire screen action handlers ─────────────────────────
    wiring::wire_settings_actions(app, Arc::clone(app_state), config_root.to_path_buf());

    // ── Fetch the system preset catalog (upstream: on application load) ─
    actions::settings::start_system_preset_catalog_fetch(
        app,
        app_state,
        config_root,
        Arc::new(actions::settings::UreqDownloader),
    );

    // ── Wire affix preset CRUD actions ──────────────────────────────
    wiring::wire_affix_actions(app, Arc::clone(app_state));

    // ── Wire affix filter toggle actions ──────────────────────────
    wiring::wire_affix_filters_actions(app, Arc::clone(app_state), config_root.to_path_buf());

    // ── Wire affix command actions (config dialogs, entry CRUD, overlay) ─
    wiring::wire_affix_command_actions(app, Arc::clone(app_state));

    // ── Wire trade screen actions ────────────────────────────────
    wiring::wire_trade_actions(app, Arc::clone(app_state));

    // ── Wire logging screen actions ──────────────────────────────
    wiring::wire_logging_actions(app, Arc::clone(app_state));

    // ── Wire runtime bridge actions (must run before debug so
    //    the DebugCommandController is available) ─────────────
    let debug_controller =
        wiring::wire_runtime_actions(app, Arc::clone(app_state), config_root.to_path_buf());

    // Publish the controller in the late-bound slot the AffixScreen power
    // toggle and the Ctrl+F12 hotkey drain read at event time (both are
    // registered without a controller parameter, the toggle even before the
    // runtime wiring runs).
    wiring::install_live_loop_controller(debug_controller.clone());

    // ── Restore the persisted overlay state ──────────────────────────
    // The port persists the overlay on/off flag with the overlay config, and
    // upstream's `ScreenProcessHandler` runs the capture/detect/OCR/overlay
    // pipeline whenever `IsEnabled` is true (`ScreenProcessHandler.cs:88`,
    // `:116-169`), so restoring the flag at launch keeps the overlay live
    // after a restart instead of making the user press the power button
    // again every session. The reconcile goes through the same single-writer
    // seam as the power toggle and the Ctrl+F12 hotkey, so the running state
    // can never disagree with the flag; a build without `real-runtime` starts
    // the documented structured stub and reports its events through
    // the normal drain path.
    wiring::restore_overlay_live_loop(app_state, &debug_controller);

    // ── Wire debug screen actions ────────────────────────────────
    wiring::wire_debug_actions(
        app,
        Arc::clone(app_state),
        config_root.to_path_buf(),
        Some(debug_controller),
        crash_dir,
    );
}

/// Wire the shell initialisation, the window-state observer, the tray and
/// the hotkey backend.
fn wire_shell_integration(
    app: &AppWindow,
    app_state: &Arc<Mutex<AppState>>,
    config_root: &Path,
    config_settings: &d4lc_config::AppSettings,
) {
    // ── Shell initialisation ──────────────────────────────────────────
    actions::handle_app_loaded(app.as_weak(), Arc::clone(app_state));

    // The X11 `WM_STATE` minimize observer — the Linux equivalent of
    // upstream's WPF `StateChanged` binding (`MainWindow.xaml:28-29` →
    // `WindowStateChangedExecute`, `MainWindowViewModel.cs:328-350`).
    // On a Normal→Iconic transition with `MinimizeToTray` enabled and an
    // available tray backend, the window is hidden like on close-to-tray;
    // restore transitions reset the lifecycle projection. Wiring this before
    // the tray/hotkey init lets the observer also cover the
    // `LaunchMinimized` startup minimize the same way the upstream
    // `WindowState.Minimized` assignment triggered the handler.
    window_observer::wire_window_state_observer(app.as_weak(), Arc::clone(app_state));

    // Wire the tray backend to the AppState and the Slint window. The tray
    // menu callbacks run on the D-Bus thread and route their AppState
    // mutations and window-show through `slint::invoke_from_event_loop`
    // inside `tray::handle_tray_open` / `tray::handle_tray_exit`.
    tray::set_app_state(Arc::clone(app_state));
    let app_weak_for_tray = app.as_weak();
    tray::set_show_window_hook(Box::new(move || {
        if let Some(window) = app_weak_for_tray.upgrade() {
            let _ = window.show();
            // The close-to-tray hide withdraws the managed
            // X11 window (the WM deletes WM_STATE), so the re-map above
            // gets re-placed by the window manager. Consume-once
            // re-apply of the pre-hide geometry keeps the tray-Open
            // restore geometry-exact; every other restore path leaves
            // the recorded geometry at None and is untouched.
            actions::reapply_hidden_to_tray_geometry(window.window());
        }
    }));
    tray::init_tray();
    // Start the hotkey backend from the persisted `config_settings`:
    //
    // 1. Projects `config_settings.hotkey_config.bindings` into a
    //    `Vec<HotkeyRequest>` (the user's enabled flags and bindings).
    // 2. Runs the registration probe and stores the resulting
    //    `HotkeyBackendStatus` on `AppState`.
    // 3. When at least one binding registered successfully,
    //    spawns a background X11 event-loop thread and
    //    registers a periodic `slint::Timer` that drains
    //    `HotkeyEvent` values into the shared
    //    `dispatch_hotkey_event` router.
    // 4. Returns silently when X11 / Wayland / the binding
    //    list made a thread impossible; the status already
    //    describes the per-binding outcome.
    wiring::wire_hotkey_actions(
        app,
        Arc::clone(app_state),
        config_root.to_path_buf(),
        config_settings,
    );
}

/// Relaunch with the software renderer after the GPU renderer failed to
/// start. Returns only when the relaunch itself failed.
fn relaunch_with_software_renderer(
    app: AppWindow,
    instance_lock: single_instance::InstanceLock,
    reason: &str,
) {
    eprintln!(
        "d4lc-app: the femtovg GPU renderer failed to start ({reason}); \
         relaunching with the software renderer"
    );
    // The single-instance lock stays held across the exec: the
    // relaunch inherits it instead of racing other launches for it.
    drop(app);
    let handover = match instance_lock.into_handover() {
        Ok(lock) => Some(lock),
        Err(error) => {
            eprintln!(
                "d4lc-app: could not hand the single-instance lock to the relaunch \
                 ({error}); the relaunch acquires it anew"
            );
            None
        }
    };
    let relaunch_error = ui_renderer::relaunch_with_software(reason, handover);
    eprintln!("d4lc-app: software-renderer relaunch failed: {relaunch_error}");
}

/// Seed the version caption the settings screen renders as
/// "DIABLO IV COMPANION V" + `app-version` (`ui/main_window.slint:44` ships a
/// placeholder `"0.1.0"`, rendered at `:829`).
///
/// Upstream stamps the same fact from the assembly version in the window
/// title (`MainWindowViewModel.cs:33`) and re-stamps it with the update
/// suffix when a release is announced (`:144`); the port's caption carries
/// the running crate version only. Split out of `main` so the seeded value is
/// covered by a test.
fn seed_version_caption(app: &AppWindow) {
    app.set_app_version(actions::APP_VERSION.into());
}

/// The d4lc-gui crate owns tab definitions (GuiTabKind).  These labels are
/// **intentional Linux adaptations** of the upstream WPF captions:
///
/// | d4lc-gui      | Upstream WPF  | Rationale                        |
/// |---------------|---------------|----------------------------------|
/// | Current Item  | Affixes       | More descriptive for the user    |
/// | Affix Presets | Trading       | Split into presets + imports     |
/// | Trade Imports | Logging       | Imports tab with log feel        |
/// | Debug         | Debug         | Same                             |
/// | Settings      | Settings      | Same (Options in XAML)           |
///
/// The captions upstream names (`rsCapCurrentItem`, `rsCapDebug`,
/// `rsCapSettings`) follow the UI language; the two port-only captions stay
/// English.
///
fn map_tab_label(tab: &GuiTab) -> String {
    match (tab.kind, tab.label.as_str()) {
        (GuiTabKind::CurrentItem, _) => i18n::tr("Current Item"),
        (GuiTabKind::AffixPresets, _) => "Affix Presets".to_string(),
        (GuiTabKind::TradeImports, _) => "Trade Imports".to_string(),
        (GuiTabKind::Debug, _) => i18n::tr("Debug"),
        (GuiTabKind::Settings, _) => i18n::tr("Settings"),
    }
}

#[cfg(test)]
mod startup_projection_tests {
    use super::*;

    #[test]
    fn tab_captions_follow_the_ui_language_where_upstream_has_the_text() {
        let tab = |kind| GuiTab {
            kind,
            label: String::new(),
        };
        assert_eq!(map_tab_label(&tab(GuiTabKind::Settings)), "Settings");
        i18n::with_language("es-ES", || {
            assert_eq!(map_tab_label(&tab(GuiTabKind::Settings)), "Parámetros");
            assert_eq!(
                map_tab_label(&tab(GuiTabKind::CurrentItem)),
                "Objeto actual"
            );
            // The port-only captions have no upstream text and stay English.
            assert_eq!(
                map_tab_label(&tab(GuiTabKind::AffixPresets)),
                "Affix Presets"
            );
            assert_eq!(
                map_tab_label(&tab(GuiTabKind::TradeImports)),
                "Trade Imports"
            );
        });
    }

    #[test]
    fn version_caption_carries_the_crate_version() {
        // The settings caption is the only place the running version is
        // shown; it must not render the Slint placeholder ("0.1.0").
        let app = crate::test_window::new_app_window();
        seed_version_caption(&app);
        assert_eq!(app.get_app_version().as_str(), actions::APP_VERSION);
        assert_eq!(app.get_app_version().as_str(), env!("CARGO_PKG_VERSION"));
    }
}
