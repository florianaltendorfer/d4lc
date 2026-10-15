use super::*;

// ── Backend + event loop bring-up ─────────────────────────────────────────

/// Reserve a free loopback port for the embedded MCP server.
pub(super) fn reserve_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| harness_panic("reserve MCP port", &error.to_string()));
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

/// Initialize the testing backend + embedded MCP server, start the mock-clock
/// pump and the UI thread, and wait until the wired `AppWindow` is shown.
///
/// Must be called before any scenario. Asserts (never skips) that the
/// testing backend is the active platform: if another test in this process
/// installed a windowing backend first, `set_platform` fails and this panics
/// with the precise reason.
pub(super) fn start_harness() -> McpClient {
    let port = reserve_free_port();
    // Read by `mcp_server::init()` on the UI thread below.
    std::env::set_var("SLINT_MCP_PORT", port.to_string());

    let (tx, rx) = channel();
    *UI_START.lock().expect("ui start lock") = Some(tx);

    let ui_thread = std::thread::Builder::new()
        .name("d4lc-gui-harness-ui".into())
        .spawn(ui_thread_main)
        .expect("spawn ui thread");

    // The UI thread reports once the platform is installed and the window is
    // shown (or with the exact failure).
    let reported = rx.recv_timeout(Duration::from_secs(30));
    match reported {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = ui_thread.join();
            harness_panic("testing backend could not be installed", &error);
        }
        Err(error) => harness_panic("ui thread did not become ready", &error.to_string()),
    };

    start_mock_clock_pump();
    McpClient::connect(port)
}

/// UI-thread body: install the testing backend (this thread also runs the
/// event loop — the testing backend binds its wake-up queue to the thread
/// that created it), init the MCP server, build the wired app and show it.
fn ui_thread_main() {
    let report = UI_START.lock().expect("ui start lock").take();
    match run_ui_thread() {
        Ok(()) => {
            if let Some(report) = report {
                let _ = report.send(Ok(()));
            }
            let _ = slint::run_event_loop();
        }
        Err(error) => {
            if let Some(report) = report {
                let _ = report.send(Err(error));
            }
        }
    }
}

fn run_ui_thread() -> HarnessResult<()> {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: true,
            // Headless rasterizer so MCP `take_screenshot` produces real
            // pixels; `None` would keep the mock renderer, which cannot
            // snapshot.
            renderer_name: Some("software".into()),
        },
    )))
    .map_err(|error| {
        format!(
            "set_platform failed ({error:?}) — a windowing backend was already initialized in \
             this process; the real-input harness requires the testing backend (headless)"
        )
    })?;

    // Installs the window/event tracking hooks and the MCP HTTP server
    // (bound when the first window is shown). Reads SLINT_MCP_PORT.
    i_slint_backend_testing::mcp_server::init()
        .map_err(|error| format!("mcp server init failed: {error:?}"))?;

    let app = wired_test_app();
    let _ = APP_WINDOW.set(app.as_weak());
    app.show()
        .map_err(|error| format!("AppWindow::show failed: {error:?}"))?;

    // Keep the strong handle for the process lifetime; scenarios use the weak.
    Box::leak(Box::new(app));
    Ok(())
}

/// Real-time bound on one UI-thread round trip. A healthy round trip is
/// sub-millisecond (the closure is queued, an already-running event loop
/// picks it up); the bound is loose enough to survive a loaded machine and
/// still short enough that a stalled event loop fails the suite in seconds
/// instead of hanging it for hours.
const UI_ROUNDTRIP_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `f` on the event-loop thread and return its result. All direct UI
/// access (property reads/writes, `ElementHandle` work) is marshalled through
/// here; MCP calls go straight from the test thread.
///
/// The wait is bounded ([`UI_ROUNDTRIP_TIMEOUT`]): the queue entry is drained
/// by the window thread, so a stall there (a livelock, a deadlock or an
/// unbounded handler loop inside the app) would otherwise block the test
/// thread forever — the failure mode this watchdog exists to convert into a
/// loud panic naming the call that never came back.
pub(super) fn on_ui<T: Send + 'static>(f: impl FnOnce(&AppWindow) -> T + Send + 'static) -> T {
    on_ui_ctx("on_ui", f)
}

/// [`on_ui`] with the calling helper named in the watchdog panic, so a stall
/// points at the harness call waiting for the window thread.
pub(super) fn on_ui_ctx<T: Send + 'static>(
    context: &str,
    f: impl FnOnce(&AppWindow) -> T + Send + 'static,
) -> T {
    let (tx, rx) = channel();
    slint::invoke_from_event_loop(move || {
        let Some(app) = APP_WINDOW.get().and_then(Weak::upgrade) else {
            let _ = tx.send(None);
            return;
        };
        let _ = tx.send(Some(f(&app)));
    })
    .unwrap_or_else(|error| harness_panic("invoke_from_event_loop", &error.to_string()));
    match rx.recv_timeout(UI_ROUNDTRIP_TIMEOUT) {
        Ok(Some(value)) => value,
        Ok(None) => harness_panic("invoke_from_event_loop", "app window was dropped"),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => harness_panic(
            context,
            &format!(
                "the window thread did not run the queued UI closure within {}s — the Slint \
                 event loop is stalled (livelock, deadlock or an unbounded handler loop); the \
                 suite stops here instead of waiting forever",
                UI_ROUNDTRIP_TIMEOUT.as_secs()
            ),
        ),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            harness_panic("ui thread dropped the on_ui closure", "sender disconnected")
        }
    }
}

/// Pump the mock clock on the event-loop thread so timers and animations
/// advance deterministically: the app's 50 ms runtime drain timer, the MCP
/// server's awaited clicks/double clicks (`single_click`/`double_click` wait
/// on mock-time timers) and any pending change handlers.
///
/// Mock time advances only here — `TestingBackend` with `mock_time` parks its
/// event loop without touching the system clock.
fn start_mock_clock_pump() {
    *PUMP_SHUTDOWN.lock().expect("pump lock") = false;
    std::thread::Builder::new()
        .name("d4lc-gui-harness-clock".into())
        .spawn(move || loop {
            if *PUMP_SHUTDOWN.lock().expect("pump lock") {
                return;
            }
            let _ = slint::invoke_from_event_loop(move || {
                i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(PUMP_TICK_MS));
            });
            std::thread::sleep(Duration::from_millis(2));
        })
        .expect("spawn clock pump");
}

pub(super) fn stop_mock_clock_pump() {
    *PUMP_SHUTDOWN.lock().expect("pump lock") = true;
}

// ── Wired app construction (mirrors main()) ───────────────────────────────

/// Build the real `AppWindow` with the full `wiring.rs` handler set and a
/// seeded `AppState` — the same composition as `main()`, minus the
/// process-level integrations a test process must not start (tray, window
/// observer, live update check).
fn wired_test_app() -> AppWindow {
    let app_state = Arc::new(Mutex::new(AppState::default_startup()));
    let _ = HARNESS_APP_STATE.set(Arc::clone(&app_state));
    let config_root = crate::actions::settings::resolve_config_root();
    let config_settings = d4lc_config::load_app_settings(&config_root).unwrap_or_default();

    {
        let mut state = app_state.lock().expect("app state lock");
        state.shell.settings =
            d4lc_gui::SettingsPanelState::from_config(&config_settings, Vec::new());
        state.hydrate_hotkey_drafts_from_settings(&config_settings);
        // NOTE: the affix-preset entry seed moved below the wiring
        // calls. The wiring now hydrates the editor from
        // `Config/AffixPresets-v2.json` (preset-persistence startup load, the
        // same seam production `main()` uses via
        // `wire_affix_filters_actions`); seeding before the wiring
        // would be overwritten by that hydration. The seed itself is
        // unchanged — see `seed_standard_editor_presets` below.
    }

    let shell_state = app_state.lock().expect("app state lock").shell.clone();
    let app = AppWindow::new().expect("AppWindow::new on the testing backend");

    app.set_window_title(format!("d4lc real-input test {}", std::process::id()).into());
    app.set_window_topmost(false);

    let tab_labels: Vec<slint::SharedString> = shell_state
        .tabs
        .iter()
        .map(crate::map_tab_label)
        .map(slint::SharedString::from)
        .collect();
    app.set_tab_labels(Rc::new(slint::VecModel::from(tab_labels)).into());
    app.set_active_tab(0);

    {
        let state = app_state.lock().expect("app state lock");
        app.set_debug_mode(state.shell.settings.debug_mode);
        app.set_dev_mode(state.shell.settings.dev_mode);
        app.set_check_updates(state.shell.settings.check_for_updates);
        app.set_launch_minimized(state.shell.settings.launch_minimized);
        app.set_minimize_to_tray(state.shell.settings.minimize_to_tray);
        app.set_controller_mode(state.shell.settings.controller_mode);
        app.set_paragon_mode(state.shell.settings.paragon_mode_active);
        app.set_diagnostics_export(state.shell.settings.diagnostics_export_enabled);
        app.set_update_check_enabled(state.shell.settings.update_check_enabled);

        let names: Vec<slint::SharedString> = state
            .affix_preset_editor
            .presets
            .iter()
            .map(|preset| slint::SharedString::from(preset.name.as_str()))
            .collect();
        app.set_preset_names(Rc::new(slint::VecModel::from(names)).into());

        crate::actions::affix::set_filter_toggle_models(&app, &state.shell.settings.toggles);
    }

    // ── The full wiring.rs handler set ────────────────────────────────────
    crate::wiring::wire_settings_actions(&app, Arc::clone(&app_state), config_root.clone());
    crate::wiring::wire_affix_actions(&app, Arc::clone(&app_state));
    crate::wiring::wire_affix_filters_actions(&app, Arc::clone(&app_state), config_root.clone());
    crate::wiring::wire_affix_command_actions(&app, Arc::clone(&app_state));
    crate::wiring::wire_trade_actions(&app, Arc::clone(&app_state));
    crate::wiring::wire_logging_actions(&app, Arc::clone(&app_state));
    let debug_controller =
        crate::wiring::wire_runtime_actions(&app, Arc::clone(&app_state), config_root.clone());
    crate::wiring::wire_debug_actions(
        &app,
        Arc::clone(&app_state),
        config_root.clone(),
        Some(debug_controller),
        None,
    );
    // Headless: the hotkey backend reports `X11Unavailable` and
    // starts no thread; the seeded status is the documented no-display path.
    // `HeadlessSessionGuard` (installed by the suite before `start_harness`)
    // makes that path deterministic on a developer machine with a live X or
    // Wayland session.
    crate::wiring::wire_hotkey_actions(&app, Arc::clone(&app_state), config_root, &config_settings);

    // The browser-launching shell callbacks stay unwired — the suite must be
    // network-free and must never open a browser.

    // Entry seed (post-wiring): the projection scenario (and the
    // SetAffix / SetAffixColor matrix rows) drive rows/cards that only
    // exist when the selected preset has entries. Seeding here is the
    // only way to get *preset* entries without driving the dialogs —
    // the SetAffix create mode (index -1) is reached through the catalog
    // card "+", which only opens a form and persists on confirm.
    //
    // The seed stays pure-Affix so the projection scenario's
    // labeled-click assertions are stable; mixed-kind live flows run in
    // the affix-surface scenario family, which seeds its own mixed-kind
    // presets through `HARNESS_APP_STATE`.
    seed_standard_editor_presets(&app, &app_state);

    app
}

/// The `AffixIdToDescriptionConverter` text of the standard seed's two ids
/// (the vendored enUS `Description`, which is what the rail chip and the card
/// both show): the seed stores ids the shipped preset files use, one `IdName`
/// list member at a time, so the projection resolves them to the index rows
/// instead of the seed's own labels.
pub(super) const SEED_ATTACK_SPEED_TEXT: &str = "+#% Attack Speed";
pub(super) const SEED_LIFE_TEXT: &str = "+# Maximum Life";

/// Seed the standard "Starter" editor preset and re-project every
/// AffixScreen surface that was initialized from the (still empty)
/// editor during wiring: the preset-names model, the selected-preset
/// property, the entry rows/cards and the overlay descriptor state.
pub(super) fn seed_standard_editor_presets(app: &AppWindow, app_state: &Arc<Mutex<AppState>>) {
    let mut state = app_state.lock().expect("app state lock");
    state.bump_rules_input_revision();
    state.affix_preset_editor =
        d4lc_gui::AffixPresetEditorState::new(vec![d4lc_gui::AffixPresetEditorPreset {
            name: "Starter".into(),
            entries: vec![
                d4lc_gui::PresetEntry {
                    kind: d4lc_gui::PresetEntryKind::Affix,
                    id: "AttackSpeed".into(),
                    label: Some("Attack Speed".into()),
                    item_type: Some("Amulet".into()),
                    color: "#22C55E".into(),
                    is_any_type: false,
                    is_greater: true,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                },
                // label: None exercises the label.unwrap_or(id)
                // projection path for ids the data does not carry (this
                // id resolves to its index row, so the chip and card text
                // come from `Description`).
                d4lc_gui::PresetEntry {
                    kind: d4lc_gui::PresetEntryKind::Affix,
                    id: "Life".into(),
                    label: None,
                    item_type: Some("helm".into()),
                    color: "#EF4444".into(),
                    is_any_type: false,
                    is_greater: false,
                    is_implicit: true,
                    is_tempered: true,
                    tuning_prisms: Vec::new(),
                },
            ],
        }]);
    state.affix_preset_editor.selected_preset_name = Some("Starter".into());

    // The same shared refresh every wired preset mutation runs, so the
    // harness seed projects exactly what production projects.
    crate::actions::affix::refresh_affix_screen_preset_models(app, &mut state);
    state.rebuild_overlay_descriptor_state();
}
