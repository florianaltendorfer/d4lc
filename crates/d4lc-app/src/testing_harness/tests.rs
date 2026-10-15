use super::*;
use slint::Model as _;

mod affix_controls;
mod affix_surface;
mod build_import;
mod entry_projection;
mod import_management;
mod interaction_matrix;
mod modal_keyboard;
mod preset_download;
mod preset_persistence;
mod settings_presets;
mod toggle_sweep;
mod trade_crud;
mod trade_persistence;

use affix_controls::{dead_controls_scenarios, rename_keyboard_scenario};
use affix_surface::{
    affix_rail_scroll_scenario, affix_row_count, affix_surface_scenarios, catalog_scenarios,
    preset_entry, reproject_affixes, seed_affix_presets, select_preset, select_website,
    wanted_affix_count,
};
use build_import::{
    browser_download_gate_scenarios, build_import_url_scenarios, import_overlay_modality_scenario,
    type_into_field,
};
use entry_projection::run_entry_projection_scenario;
use import_management::{
    import_delete_edge_scenarios, local_merge_tab_scenarios, mobalytics_subtab_scenarios,
};
use interaction_matrix::{
    log_messages, preset_names, run_interaction_matrix, CANCEL_MATCHES_PER_INSTANCE, UI_SOURCES,
};
use modal_keyboard::run_modal_keyboard_scenarios;
use preset_download::{app_state_presets, preset_download_scenarios, seeded_build_rows};
use preset_persistence::preset_persistence_scenario;
use settings_presets::scenario_settings_presets_complete;
use toggle_sweep::{restore_standard_presets, toggle_sweep};
use trade_crud::{
    reproject_trade, seed_trade_items, trade_crud_scenarios, wanted_row, wanted_row_count,
};
use trade_persistence::{trade_item_editing_scenario, trade_persistence_scenario};

/// Point a per-test config root at a throwaway directory so dialog
/// persistence never touches the real user config.
struct ConfigRootGuard {
    path: std::path::PathBuf,
}

impl ConfigRootGuard {
    fn install() -> Self {
        let path = std::env::temp_dir().join(format!("d4lc-gui-harness-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create config root");
        std::env::set_var("D4LC_CONFIG_ROOT", &path);
        Self { path }
    }
}

impl Drop for ConfigRootGuard {
    fn drop(&mut self) {
        std::env::remove_var("D4LC_CONFIG_ROOT");
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Force the documented headless session for the pilot process and
/// restore the ambient session variables when the suite ends.
///
/// The suite drives the Slint testing backend, which needs no display
/// server — and the app's global-hotkey backend must stay on its
/// documented no-session path. With `DISPLAY` set, `hotkeys::
/// start_hotkey_backend` opens a real X11 connection and takes real
/// keyboard grabs from a test process, and the hotkey re-registration
/// path (`actions::settings::restart_hotkey_backend` →
/// `HotkeyBackendHandle::stop`) *joins* that backend thread. The backend
/// event loop only re-checks its shutdown flag after `wait_for_event`
/// returns, so on an idle X session the join never returns: the UI thread
/// blocks inside the dialog-confirm dispatch and the whole suite hangs
/// (reproduced with a core dump — the UI thread in `Thread::join`, the
/// backend thread in `x11rb wait_for_event`). The suite must therefore
/// not inherit the caller's session.
///
/// `XDG_SESSION_TYPE` and `WAYLAND_DISPLAY` are cleared alongside
/// `DISPLAY` so the Wayland portal path cannot start a backend thread
/// either; the same three variables are restored on drop, mirroring
/// `visual_smoke::EnvGuard`.
struct HeadlessSessionGuard {
    saved: [(&'static str, Option<String>); 3],
}

impl HeadlessSessionGuard {
    /// Session variables that would route the hotkey backend into a live
    /// session (`hotkeys::is_wayland_session`, `RustConnection::connect`).
    const SESSION_VARS: [&'static str; 3] = ["DISPLAY", "XDG_SESSION_TYPE", "WAYLAND_DISPLAY"];

    fn install() -> Self {
        let saved = Self::SESSION_VARS.map(|name| {
            let value = std::env::var(name).ok();
            std::env::remove_var(name);
            (name, value)
        });
        Self { saved }
    }
}

impl Drop for HeadlessSessionGuard {
    fn drop(&mut self) {
        for (name, value) in self.saved.iter() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// The pilot's hotkey contract: with the ambient session removed
/// ([`HeadlessSessionGuard`]) the backend must never install a live
/// thread, so no code path can block the UI thread joining one.
fn assert_no_live_hotkey_backend(step: &str) {
    assert!(
        crate::actions::settings::current_hotkey_receiver().is_none(),
        "toggle-sweep: a live global-hotkey backend is registered {step} — the pilot must stay \
         on the documented headless hotkey path: a live backend takes real X11 grabs and its \
         re-registration joins a thread that only wakes on an X11 event, which would block \
         the UI thread and hang the suite"
    );
}

/// Everything one pilot test needs, plus the process-wide guards that
/// must outlive its body.
struct Pilot {
    client: McpClient,
    window: McpHandle,
    root: McpHandle,
    config_root: std::path::PathBuf,
    // Declared after the handles so they drop last: the guards restore the
    // ambient session and delete the harness config root.
    _session: HeadlessSessionGuard,
    _config_root_guard: ConfigRootGuard,
}

/// Wire one app window on the testing backend of this process.
///
/// `set_platform` installs the testing backend once per process, so every
/// pilot test owns its process and calls this exactly once:
/// `scripts/run_pilot_suite.py` starts one process per pilot test and runs
/// them in parallel. They are ignored in the default parallel run on
/// purpose: the harness backend runs with threading and claims Slint's
/// process-wide event-loop proxy, which a second pilot in the same process
/// cannot claim again (it fails with `AlreadySet`).
fn start_pilot() -> Pilot {
    start_pilot_seeded(|_| {})
}

/// [`start_pilot`] with `seed` run on the fresh config root before the app
/// is wired, for scenarios that need on-disk state at launch.
fn start_pilot_seeded(seed: impl FnOnce(&std::path::Path)) -> Pilot {
    let config_root_guard = ConfigRootGuard::install();
    let config_root = config_root_guard.path.clone();
    seed(&config_root);
    // Installed before `start_harness` spawns the window thread: the
    // process must not carry the caller's session into the wired app.
    let session = HeadlessSessionGuard::install();

    // Explicit, never-skipping backend assertion: `start_harness` panics
    // unless `set_platform` installed OUR testing backend and the
    // embedded MCP server answered on loopback — no display server is
    // involved anywhere.
    let client = start_harness();

    // Session precondition: no live global-hotkey backend may exist,
    // because its re-registration path joins a thread that only wakes on
    // an X11 event (see `HeadlessSessionGuard`).
    assert_no_live_hotkey_backend("at pilot start");

    let windows = client.list_windows().expect("list windows");
    assert_eq!(windows.len(), 1, "exactly one harness window is tracked");
    let window = windows[0];

    // The window is real: physical size matches the layout constraints
    // of AppWindow (1150x625 preferred). The root element handle is the
    // entry point for MCP-side element queries.
    let properties = client
        .get_window_properties(window)
        .expect("window properties");
    assert!(
        properties.get("size").is_some(),
        "window properties missing size: {properties:.200}"
    );
    let root = properties
        .get("rootElementHandle")
        .map(parse_handle)
        .unwrap_or_else(|| panic!("window properties missing rootElementHandle: {properties:.200}"))
        .unwrap_or_else(|error| harness_panic("parse rootElementHandle", &error));

    Pilot {
        client,
        window,
        root,
        config_root,
        _session: session,
        _config_root_guard: config_root_guard,
    }
}

/// Give a scenario the tall viewport the coverage matrix established:
/// Slint exposes only elements whose geometry intersects the clip (clipped
/// content is as invisible to the a11y tree as it is to a screen reader),
/// and the Debug screen's 15 numeric steppers exceed the 625 px default
/// viewport. In the single-window suite every family that ran after the
/// matrix saw this size; the per-process tests reproduce it.
fn enlarge_window_for_scenarios() {
    read_ui(|app| {
        app.window()
            .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
                1150.0, 1200.0,
            )));
    });
}

/// Stop the mock clock pump and leave the event loop so the process exits
/// cleanly once a pilot test's scenario finished.
fn finish_pilot() {
    stop_mock_clock_pump();
    let _ = slint::quit_event_loop();
}

/// Base contract of the wired window plus the input-path pilots: nav by
/// real click, the dialog open/close path, Escape routing, typing, double
/// click and drag-away. Runs on the natural 1150x625 window, the state no
/// other family disturbs any more because each owns its own process.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_core_window_and_input() {
    let pilot = start_pilot();
    let (client, window, root) = (pilot.client, pilot.window, pilot.root);

    // Main window state screenshot.
    let png = client
        .take_screenshot(window)
        .expect("screenshot main window");
    let (width, height) = assert_png_and_dimensions(&png);
    assert_eq!(
        (width, height),
        (1150, 625),
        "screenshot dimensions must match the window size"
    );
    write_screenshot("01-main-window.png", &png);

    // ── Pilot 1: nav click switches tab (real pointer → hit-testing) ──
    click_on_ui("Settings");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        4,
        "nav click must switch to Settings"
    );
    click_on_ui("Current Item");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        0,
        "nav click must switch back"
    );

    // Rust-side element-id lookup works (debug info is embedded).
    let line_edit_count = read_ui(|app| {
        ElementHandle::find_by_element_id(app, "AffixScreen::new-preset-name-field").count()
    });
    assert_eq!(
        line_edit_count, 1,
        "exactly one named new-preset-name field must exist on the AffixScreen"
    );

    // MCP-side by-id element query (same debug info, MCP arena handles).
    // The affix catalog grids are windowed (60 rows per tab, upstream's
    // VirtualizingWrapPanel equivalent), so this full-tree walk stays
    // fast; the printed duration is the interactive-performance evidence
    // for that window (an unwindowed 894-row grid took over a minute).
    {
        let start = std::time::Instant::now();
        let probe =
            read_ui(|app| ElementHandle::find_by_element_id(app, "DarkLineEdit::input").count());
        eprintln!(
            "[gui-harness] affix-screen tree walk: {probe} matches in {:?}",
            start.elapsed()
        );
    }
    let inputs = client
        .find_elements_by_id(window, "DarkLineEdit::input")
        .expect("find DarkLineEdit inputs");
    assert!(
        inputs.len() >= 2,
        "the two DarkLineEdit TextInputs (preset name + filter) must be findable by id"
    );

    // ── Pilot 2: ImportPreset dialog opens by real click, closes by
    //    Done click (upstream parity: the tabbed dialog has no
    //    Cancel) ─────────────────────────────────────────────────────
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(visible, "Import click must open a dialog");
    assert_eq!(
        dialog_id, "import_preset",
        "the opened dialog must be ImportPreset"
    );

    let png = client
        .take_screenshot(window)
        .expect("screenshot import dialog");
    assert_png_and_dimensions(&png);
    write_screenshot("02-import-preset-dialog.png", &png);

    click_on_ui("Done");
    let (visible, dialog_id) = dialog_state();
    assert!(!visible, "Done click must close the dialog");
    assert_eq!(dialog_id, "import_preset", "dialog id persists after close");

    // ── Pilot 3: Escape reaches the dialog keyboard host and is
    //    routed (upstream parity: ignored) ─────────────────────────
    // The dialog host FocusScope consumes Escape while a dialog is
    // open and routes it through `actions::dialogs::handle_dialog_key`.
    // Upstream verification (MahApps CustomDialogs bind no keys) means
    // every route is currently `Ignored`: the key is delivered AND the
    // dialog stays open. Both halves are asserted: the recorded events
    // prove delivery, `dialog-last-key` proves the route fired.
    click_on_ui("Import preset");
    assert!(dialog_state().0, "Import click must reopen the dialog");
    client
        .start_event_recording()
        .expect("start event recording");
    client
        .dispatch_key_event(window, "\u{1b}")
        .expect("dispatch Escape");
    let recording = client.stop_event_recording().expect("stop event recording");
    assert!(
        recorded_events(&recording, "keyPressed")
            .chain(recorded_events(&recording, "keyReleased"))
            .any(|event| str_property(event, "text") == Some("\u{1b}")),
        "the recorded events must contain the Escape key press: {recording:.400}"
    );
    assert_eq!(
        read_ui(|app| app.get_dialog_last_key().to_string()),
        "\u{1b}",
        "the dialog keyboard host must have routed the Escape press"
    );
    assert!(
        dialog_state().0,
        "upstream parity: Escape does not close the dialog (no upstream dialog binds Escape)"
    );
    click_on_ui("Done");
    assert!(!dialog_state().0, "cleanup: close the dialog via Done");

    // ── Pilot 4: typing into the preset-name TextInput ────────────────
    // Click into the presets block's "Enter name" LineEdit (real
    // click → focus), type through `dispatch_key_event`, read the value
    // back through MCP (accessible value) and the Rust property.
    let text_inputs = client
        .query_element_descendants(root, descendants_with_role("TextInput"), true)
        .expect("query TextInput descendants");
    assert!(
        !text_inputs.is_empty(),
        "the presets block's LineEdit must expose a TextInput role"
    );

    let preset_input = text_inputs
        .iter()
        .copied()
        .find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    // The shared DarkLineEdit exposes the placeholder
                    // as the input's accessible label.
                    str_property(&properties, "accessibleLabel") == Some("Enter name")
                        || str_property(&properties, "accessiblePlaceholderText")
                            == Some("Enter name")
                })
        })
        .unwrap_or_else(|| {
            panic!("real-input GUI harness: preset-name TextInput (placeholder) not found")
        });

    client
        .set_element_value(preset_input, "")
        .expect("clear the preset-name input via set_element_value");
    client
        .click_element(preset_input, None)
        .expect("click into the preset-name input");
    for character in ['a', 'b', 'c', '1'] {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| harness_panic("type into preset-name input", &error));
    }

    let properties = client
        .get_element_properties(preset_input)
        .expect("input properties");
    assert!(
        str_property(&properties, "accessibleValue") == Some("abc1"),
        "typed characters must land in the input's accessible value: {properties:.400}"
    );
    assert_eq!(
        read_ui(|app| app.get_new_preset_name().to_string()),
        "abc1",
        "typed characters must flow into the inline preset-name form property"
    );

    client
        .set_element_value(preset_input, "")
        .expect("clear the preset-name input again");
    assert_eq!(
        read_ui(|app| app.get_new_preset_name().to_string()),
        "",
        "set_element_value must clear the text input"
    );

    // ── Pilot 5: double click (real click-interval gesture) ───────────
    // `click_element` with `DoubleClick` runs `ElementHandle::double_click`
    // on the server: press/release/press/release inside the 500 ms
    // platform click interval. The recording proves both press+release
    // pairs were dispatched and processed; the Core chip is bound to
    // `class-toggles[0]`, so two real clicks must toggle twice (ending
    // back at the initial `true`).
    click_on_ui("Core");
    assert!(
        !read_ui(|app| app.get_class_toggles().row_data(0).unwrap_or(true)),
        "a single real click on the Core chip must toggle it off"
    );
    // Restore the arm the single click flipped: the class filter feeds
    // the catalog projection, so a scenario must leave the toggles as it
    // found them (the later catalog scenarios assert against the default
    // all-on state).
    click_on_ui("Core");
    assert!(
        read_ui(|app| app.get_class_toggles().row_data(0).unwrap_or(false)),
        "the restore click on the Core chip must toggle it back on"
    );

    let barb = find_button_by_label(&client, root, "Barbarian");
    client
        .start_event_recording()
        .expect("start event recording");
    client
        .click_element(barb, Some("DoubleClick"))
        .expect("double click the Barb chip");
    let recording = client.stop_event_recording().expect("stop event recording");
    assert_eq!(
        recorded_events(&recording, "pointerPressed").count(),
        2,
        "the double click must dispatch two real presses: {recording:.400}"
    );
    assert_eq!(
        recorded_events(&recording, "pointerReleased").count(),
        2,
        "the double click must dispatch two real releases: {recording:.400}"
    );
    assert!(
        read_ui(|app| app.get_class_toggles().row_data(1).unwrap_or(false)),
        "two real clicks on the Barb chip must toggle it twice (back on)"
    );

    // ── Pilot 6: drag away from a control does not activate it ────────
    // `drag_element` presses at the element center, interpolates pointer
    // moves (~5 px steps) and releases at the target: real drag input.
    let import_button = find_button_by_label(&client, root, "Import preset");
    client
        .drag_element(import_button, 900.0, 550.0)
        .expect("drag away from the Import button");
    let (visible, _dialog_id) = dialog_state();
    assert!(
        !visible,
        "a drag that releases off the chip must not activate it"
    );

    // The state the base contract leaves behind: the standard seed every
    // per-process family below starts from.
    let png = client.take_screenshot(window).expect("final screenshot");
    assert_png_and_dimensions(&png);
    write_screenshot("03-final-state.png", &png);
    finish_pilot();
}

/// Build-import URL scenarios on the tabbed dialog.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_build_import_url() {
    let pilot = start_pilot();
    build_import_url_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Browser-download gate scenarios on the tabbed dialog surface.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_browser_download_gates() {
    let pilot = start_pilot();
    browser_download_gate_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Download-progress overlay modality.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_import_overlay_modality() {
    let pilot = start_pilot();
    import_overlay_modality_scenario(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Coverage matrix: every nav tab by real click (active-tab
/// asserted + per-tab screenshot), all 24 dialog kinds opened through
/// their real user path (dialog-id asserted, on-open screenshot, closed by
/// a real Cancel click), observable click-throughs, and the
/// disallowed-marker source scan over the whole UI tree. Zero silent
/// skips: everything not driven is a printed expected-mismatch row with a
/// one-line reason. Enlarges the window itself.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_interaction_matrix() {
    let pilot = start_pilot();
    run_interaction_matrix(&pilot.client, pilot.window);
    finish_pilot();
}

/// Entry projection scenario: `AppState` onto the affix screen models.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_entry_projection() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    run_entry_projection_scenario(&pilot.client, pilot.window);
    finish_pilot();
}

/// Settings system-preset completeness.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_settings_presets_complete() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    scenario_settings_presets_complete(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Settings preset download against the local stub: the zip
/// lands under the harness config root's `Images/`, extracts next to it
/// and flips the download-success flag.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_preset_download() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    preset_download_scenarios(&pilot.client, pilot.window);
    finish_pilot();
}

/// Launch after a preset install was interrupted between its two renames
/// (the previous tree moved aside, the new one not yet in place): startup
/// recovery restores the previous tree before the first preset listing.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_preset_install_recovery_at_launch() {
    let pilot = start_pilot_seeded(|root| {
        let previous = root.join("Images/.d4lc-install/9300p_RECOVER/old");
        std::fs::create_dir_all(previous.join("Tooltips")).expect("seed previous tree");
        std::fs::write(previous.join("config.json"), b"{\"TooltipWidth\": 375}")
            .expect("seed config");
        std::fs::write(previous.join("Tooltips/a.png"), b"png").expect("seed template");
        let staged = root.join("Images/.d4lc-install/9300p_RECOVER/new");
        std::fs::create_dir_all(&staged).expect("seed staged tree");
        std::fs::write(staged.join("config.json"), b"half").expect("seed staged config");
    });
    let images = pilot.config_root.join("Images");
    assert_eq!(
        std::fs::read(images.join("9300p_RECOVER/config.json")).ok(),
        Some(b"{\"TooltipWidth\": 375}".to_vec()),
        "startup must restore the previous preset tree"
    );
    assert_eq!(
        std::fs::read(images.join("9300p_RECOVER/Tooltips/a.png")).ok(),
        Some(b"png".to_vec())
    );
    assert!(
        !images.join(".d4lc-install").exists(),
        "startup must remove the install work directory"
    );
    let system_presets = read_ui(|app| {
        use slint::Model as _;
        app.get_system_presets()
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    });
    assert!(
        system_presets.iter().any(|name| name == "9300p_RECOVER"),
        "the restored preset is listed at launch: {system_presets:?}"
    );
    finish_pilot();
}

/// Modal and keyboard edge scenarios on the shared dialog host.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_modal_keyboard_edges() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    run_modal_keyboard_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Settings/config toggle sweep across the dialog families.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_toggle_sweep() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    toggle_sweep(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Affix/filter deep-interaction scenarios.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_affix_surface() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    affix_surface_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Affix catalog browsing: re-seeds the standard presets and
/// restores them before returning.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_catalog() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    catalog_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Gear-slot rail geometry at short and sub-minimum window
/// heights; restores the size it found.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_affix_rail_scroll() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    affix_rail_scroll_scenario(&pilot.client, pilot.window);
    finish_pilot();
}

/// Dead-controls family: the per-family add paths and the
/// overlay power/hotkey consistency.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_dead_controls() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    dead_controls_scenarios(&pilot.client, pilot.root, &pilot.config_root);
    finish_pilot();
}

/// Import/delete end-to-end edges.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_import_delete_edges() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    import_delete_edge_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Local and Merge tab scenarios.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_local_merge_tabs() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    local_merge_tab_scenarios(&pilot.client, pilot.window, pilot.root, &pilot.config_root);
    finish_pilot();
}

/// Mobalytics nested Builds/Profiles sub-tabs: opens and closes its own
/// dialog and only reads the bundled fixture corpus.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_mobalytics_subtabs() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    mobalytics_subtab_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Trade CRUD on the wanted list.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_trade_crud() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    trade_crud_scenarios(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Affix-preset persistence, including the hotkey switch leg
/// and the restart simulation through the production startup seam.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_preset_persistence() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    preset_persistence_scenario(&pilot.client, pilot.window, pilot.root, &pilot.config_root);
    finish_pilot();
}

/// Trade persistence across a simulated restart plus the live
/// lookup through the runtime `CurrentItemUpdate` event path.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_trade_persistence() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    trade_persistence_scenario(&pilot.client, pilot.window, pilot.root, &pilot.config_root);
    finish_pilot();
}

/// Add-dialog picker scenario.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_trade_item_editing() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    trade_item_editing_scenario(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Rename preset through the real dialog keyboard path: it
/// creates its own preset and renames it.
#[test]
#[ignore = "one process per pilot test: scripts/run_pilot_suite.py runs these in parallel"]
fn pilot_rename_keyboard() {
    let pilot = start_pilot();
    enlarge_window_for_scenarios();
    rename_keyboard_scenario(&pilot.client, pilot.window, pilot.root);
    finish_pilot();
}

/// Live end-to-end import through the real UI (not a suite pilot: it needs
/// the network and a local Chromium, and the name keeps it out of
/// `run_pilot_suite.py`). Opt in with
///   D4LC_LIVE_GUI_IMPORT='D2Core.com=https://www.d2core.com/d4/planner?bd=23tb' \
///     cargo test -p d4lc-app --bin d4lc-app live_gui_import -- --ignored --nocapture
/// Types the URL into the provider tab, clicks Add, follows the progress
/// panel until the build is listed, then takes variant 0 into a new affix
/// preset through the Confirm-name flow.
#[test]
#[ignore = "live: real provider site + Chromium; set D4LC_LIVE_GUI_IMPORT"]
fn live_gui_import() {
    let spec = std::env::var("D4LC_LIVE_GUI_IMPORT").expect("set D4LC_LIVE_GUI_IMPORT");
    let (website, url) = spec.split_once('=').expect("Website=url");
    let (website, url) = (website.to_string(), url.to_string());
    let pilot = start_pilot();
    let (tab, placeholder) = match website.as_str() {
        "D2Core.com" => (2, build_import::D2CORE_PLACEHOLDER),
        "D4Builds.gg" => (3, "e.g. 660881f7-cb6a-4162-be62-29f0afeb37bf"),
        "InfinityBuilds.gg" => (
            4,
            "e.g. https://infinitybuilds.gg/en/builds/pestilent-swarmskuna-spiritborn-vAB2c3aMt2",
        ),
        "Maxroll.gg" => (5, "e.g. t3jx0916"),
        "Mobalytics.gg" => (6, build_import::MOBALYTICS_PLACEHOLDER),
        other => panic!("unknown website {other}"),
    };
    select_website(&pilot.client, pilot.root, &website);
    click_on_ui("Import preset");
    assert_eq!(read_ui(|app| app.get_import_tab_index()), tab);
    type_into_field(&pilot.client, pilot.window, pilot.root, placeholder, &url);

    let clicked = std::time::Instant::now();
    click_on_ui("Add build");
    assert!(
        read_ui(|app| app.get_import_progress_visible()),
        "the progress panel opens on the first click"
    );
    assert!(
        !read_ui(|app| app.get_dialog_error_visible()),
        "no error on the first Add"
    );
    let mut steps = Vec::new();
    loop {
        let (visible, failed, step, status) = read_ui(|app| {
            (
                app.get_import_progress_visible(),
                app.get_import_progress_failed(),
                app.get_import_progress_step().to_string(),
                app.get_import_progress_status().to_string(),
            )
        });
        let seen = format!("{step} | {status}");
        if steps.last() != Some(&seen) {
            eprintln!("[live-gui] {:>6} ms  {seen}", clicked.elapsed().as_millis());
            steps.push(seen);
        }
        if failed {
            let error = read_ui(|app| app.get_import_progress_error().to_string());
            panic!("live import failed: {error}");
        }
        if !visible {
            break;
        }
        assert!(
            clicked.elapsed() < std::time::Duration::from_secs(90),
            "import hangs"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!(
        "[live-gui] panel closed after {} ms",
        clicked.elapsed().as_millis()
    );
    take_evidence_screenshot(
        &pilot.client,
        pilot.window,
        "live-gui-import",
        "imported.png",
    );

    // The build is listed and selected, its variants are shown.
    let provider = [
        "",
        "",
        "D2Core",
        "D4Builds",
        "InfinityBuilds",
        "Maxroll",
        "Mobalytics",
    ][tab as usize];
    let (selected, variants) = read_ui(move |app| match tab {
        2 => (
            app.get_import_d2core_selected_build_index(),
            slint::Model::row_count(&app.get_import_d2core_variants()),
        ),
        3 => (
            app.get_import_d4builds_selected_build_index(),
            slint::Model::row_count(&app.get_import_d4builds_variants()),
        ),
        4 => (
            app.get_import_infinity_selected_build_index(),
            slint::Model::row_count(&app.get_import_infinity_variants()),
        ),
        5 => (
            app.get_import_maxroll_selected_build_index(),
            slint::Model::row_count(&app.get_import_maxroll_variants()),
        ),
        _ => (
            app.get_import_mobalytics_selected_build_index(),
            slint::Model::row_count(&app.get_import_mobalytics_variants()),
        ),
    });
    assert!(selected >= 0, "the imported build is selected");
    assert!(variants > 0, "its variants are listed");
    eprintln!("[live-gui] {provider}: build row {selected} with {variants} variant(s)");

    // Variant 0 into a new affix preset.
    on_ui(move |app| app.invoke_import_variant_add(provider.into(), 0));
    assert!(read_ui(|app| app.get_import_name_visible()));
    // The stored preset name is the trimmed field text.
    let name = read_ui(|app| app.get_import_name_text().trim().to_string());
    on_ui(|app| app.invoke_import_name_done());
    let dialog_error = read_ui(|app| {
        app.get_dialog_error_visible()
            .then(|| app.get_dialog_error().to_string())
    });
    assert!(
        dialog_error.is_none(),
        "add-as-preset failed: {dialog_error:?}"
    );
    let (names, ..) = preset_download::app_state_presets();
    assert!(names.contains(&name), "preset {name:?} in {names:?}");
    let entries = on_ui(move |_| {
        let state = HARNESS_APP_STATE
            .get()
            .expect("state")
            .lock()
            .expect("lock");
        state
            .affix_preset_editor
            .presets
            .iter()
            .find(|preset| preset.name == name)
            .map(|preset| preset.entries.len())
            .unwrap_or(0)
    });
    assert!(entries > 0, "the preset carries the variant's affixes");
    eprintln!("[live-gui] preset created with {entries} entries");
    take_evidence_screenshot(&pilot.client, pilot.window, "live-gui-import", "preset.png");
    click_on_ui("Done");
    finish_pilot();
}

/// The d4lc-launched browsers still running (their private profile lives
/// under the system temp dir; the browser rewrites its argv, so the whole
/// command line is matched).
fn d4lc_browser_pids() -> Vec<u32> {
    let temp = std::env::temp_dir().join(".tmp").display().to_string();
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| {
                let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
                cmdline.contains("--remote-debugging-port=0")
                    && cmdline.contains(&format!("--user-data-dir={temp}"))
                    && !cmdline.contains("--type=")
            })
        })
        .collect()
}

/// Live cancel through the real UI: Cancel during the browser start,
/// during the page load and during the D4Builds page walk; every cancel
/// closes the panel at once, stores nothing and leaves no browser behind;
/// the next Add then imports normally.
///   D4LC_LIVE_GUI_CANCEL=1 cargo test -p d4lc-app --bin d4lc-app live_gui_cancel -- --ignored --nocapture
#[test]
#[ignore = "live: real provider sites + Chromium; set D4LC_LIVE_GUI_CANCEL=1"]
fn live_gui_cancel_and_retry() {
    assert!(
        std::env::var_os("D4LC_LIVE_GUI_CANCEL").is_some(),
        "set D4LC_LIVE_GUI_CANCEL=1"
    );
    let pilot = start_pilot();
    let builds_dir = pilot.config_root.join("Builds").join("D4Builds");
    let stored = || {
        std::fs::read_dir(&builds_dir)
            .map(|dir| {
                dir.filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                    .count()
            })
            .unwrap_or(0)
    };
    let staging_left = || {
        std::fs::read_dir(&builds_dir)
            .map(|dir| {
                dir.filter_map(Result::ok).any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".d4lc-import")
                })
            })
            .unwrap_or(false)
    };
    select_website(&pilot.client, pilot.root, "D4Builds.gg");
    click_on_ui("Import preset");
    type_into_field(
        &pilot.client,
        pilot.window,
        pilot.root,
        "e.g. 660881f7-cb6a-4162-be62-29f0afeb37bf",
        "https://d4builds.gg/builds/blazing-scream-warlock-endgame/",
    );
    let wait_for_step = |wanted: &str, within: std::time::Duration| {
        let started = std::time::Instant::now();
        loop {
            let step = read_ui(|app| app.get_import_progress_step().to_string());
            if step == wanted {
                return;
            }
            assert!(
                started.elapsed() < within,
                "step {wanted:?} not reached (at {step:?})"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    let cancel_now = |phase: &str| {
        let clicked = std::time::Instant::now();
        click_on_ui("Cancel");
        let closed = clicked.elapsed();
        assert!(
            !read_ui(|app| app.get_import_progress_visible()),
            "{phase}: panel closes"
        );
        assert!(
            !read_ui(|app| app.get_import_download_active()),
            "{phase}: guard released"
        );
        eprintln!(
            "[live-cancel] {phase}: panel closed {} ms after the click",
            closed.as_millis()
        );
    };
    let browsers_gone = |phase: &str| {
        let started = std::time::Instant::now();
        while !d4lc_browser_pids().is_empty() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "{phase}: browser still running: {:?}",
                d4lc_browser_pids()
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        eprintln!(
            "[live-cancel] {phase}: browser gone {} ms after the cancel",
            started.elapsed().as_millis()
        );
    };
    assert!(
        d4lc_browser_pids().is_empty(),
        "no d4lc browser runs before the test"
    );

    // 1. While the browser starts.
    click_on_ui("Add build");
    cancel_now("browser start");
    browsers_gone("browser start");
    assert_eq!(stored(), 0);

    // 2. While the page loads.
    click_on_ui("Add build");
    wait_for_step("Loading build page", std::time::Duration::from_secs(10));
    std::thread::sleep(std::time::Duration::from_millis(500));
    cancel_now("page load");
    browsers_gone("page load");
    assert_eq!(stored(), 0);

    // 3. While the page walk reads the variants (processing).
    click_on_ui("Add build");
    wait_for_step("Reading build data", std::time::Duration::from_secs(30));
    std::thread::sleep(std::time::Duration::from_millis(400));
    cancel_now("processing");
    browsers_gone("processing");
    assert_eq!(stored(), 0, "a cancelled import stores nothing");
    assert!(!staging_left(), "no staging directory is left");

    // 4. The next Add imports normally, unaffected by the cancelled jobs.
    let clicked = std::time::Instant::now();
    click_on_ui("Add build");
    loop {
        let (visible, failed) = read_ui(|app| {
            (
                app.get_import_progress_visible(),
                app.get_import_progress_failed(),
            )
        });
        assert!(
            !failed,
            "{}",
            read_ui(|app| app.get_import_progress_error().to_string())
        );
        if !visible {
            break;
        }
        assert!(
            clicked.elapsed() < std::time::Duration::from_secs(90),
            "retry hangs"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!(
        "[live-cancel] retry imported in {} ms",
        clicked.elapsed().as_millis()
    );
    assert_eq!(stored(), 1);
    assert!(read_ui(|app| app.get_import_d4builds_selected_build_index()) >= 0);
    // Every job is one diagnostics record with its own id (the Logging
    // screen folds identical consecutive lines into one).
    let records: Vec<serde_json::Value> = std::fs::read_to_string(
        crate::diagnostics::resolve_diagnostics_dir(&pilot.config_root).join("build-import.jsonl"),
    )
    .expect("import diagnostics written")
    .lines()
    .map(|line| serde_json::from_str(line).expect("json line"))
    .collect();
    let outcomes: Vec<(u64, String)> = records
        .iter()
        .map(|record| {
            (
                record["job_id"].as_u64().unwrap_or(u64::MAX),
                record["outcome"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            (0, "cancelled".to_string()),
            (1, "cancelled".to_string()),
            (2, "cancelled".to_string()),
            (3, "stored".to_string())
        ],
        "{records:#?}"
    );
    browsers_gone("after the retry");
    click_on_ui("Done");
    finish_pilot();
}
