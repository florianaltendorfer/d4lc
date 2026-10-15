//! Composition root: wires Slint callbacks to action handlers.
//!
//! Each screen gets a `wire_*_actions()` function that captures
//! the required dependencies and registers callbacks on the AppWindow.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;

use crate::actions;
use crate::app_state::AppState;
use crate::runtime_bridge;
use crate::slint_generated::AppWindow;

/// Position of the Debug tab in the shell's `tab-labels` (main_window.slint:
/// `["Current Item", "Affix Presets", "Trade Imports", "Debug", "Settings"]`).
/// The Debug screen exists only while that tab is active (`if active-tab == 3`),
/// so the live debug panels refresh only while it is visible.
pub(crate) const DEBUG_TAB_INDEX: i32 = 3;

/// Helper: upgrade a weak AppWindow reference.
fn upgrade_app(app_weak: &slint::Weak<AppWindow>) -> Option<AppWindow> {
    app_weak.upgrade()
}

/// Helper: lock AppState, handling poison.
fn lock_state(app_state: &Arc<Mutex<AppState>>) -> std::sync::MutexGuard<'_, AppState> {
    match app_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Wire the affix-filters-changed aggregate callback.
///
/// Registers a single handler that reads all three toggle arrays,
/// updates AppState, and persists through config.
///
/// This is also the startup hydration seam for the affix-preset
/// editor: it is the affix wiring path that receives the
/// config root, it runs before `wire_affix_command_actions`' initial
/// entry projection, and it runs in every startup composition
/// (production `main()` and the headless harness alike). Presets load
/// from `Config/AffixPresets-v2.json`, the persisted
/// `SelectedAffixPreset` is restored, and the preset ComboBox, the
/// selection and the entry models are re-projected through the same
/// shared refresh every in-session preset mutation uses
/// (`actions::affix::refresh_affix_screen_preset_models`); the
/// `preset-names` seed in `main()` runs before the config root is
/// available to the editor and covers only that pre-hydration window.
pub fn wire_affix_filters_actions(
    app: &AppWindow,
    app_state: Arc<Mutex<AppState>>,
    config_root: PathBuf,
) {
    {
        let mut state = lock_state(&app_state);
        state.hydrate_affix_presets_from_config(&config_root);
        actions::affix::refresh_affix_screen_preset_models(app, &mut state);
    }

    let app_weak = app.as_weak();
    let state_for_filters = Arc::clone(&app_state);
    let config_root_for_filters = config_root.clone();
    app.on_affix_filters_changed(move |class, category, rune| {
        if let Some(app) = app_weak.upgrade() {
            let mut state = match state_for_filters.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_affix_filters_changed(
                &app,
                &mut state,
                &config_root_for_filters,
                &class,
                &category,
                &rune,
            );
        }
    });

    // Live catalog keyword filter: every edit of the affix-screen filter
    // field re-projects the grids with the new `;`-keyword text (upstream
    // `AffixTextFilter` setter, AffixViewModel.cs:248-256).
    let app_weak = app.as_weak();
    let state_for_window = Arc::clone(&app_state);
    app.on_affix_filter_text_changed(move |text| {
        if let Some(app) = app_weak.upgrade() {
            let mut state = match app_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_affix_filter_text_changed(&app, &mut state, &text);
        }
    });

    // Catalog render window: the toolbar's "Show more" grows the projected
    // rows per tab (upstream's VirtualizingWrapPanel materializes only the
    // visible viewport).
    let app_weak = app.as_weak();
    app.on_affix_catalog_window_changed(move |size| {
        if let Some(app) = app_weak.upgrade() {
            let mut state = match state_for_window.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_affix_catalog_window_changed(&app, &mut state, size);
        }
    });
}

/// Wire settings screen callbacks.
///
/// Registers:
/// - `settings-changed` — persist toggle state changes
/// - `dialog-confirm` — confirm active dialog and persist
/// - `dialog-cancel` — cancel active dialog, discard drafts
/// - 5 `dialog-open-*` callbacks — update ViewModel state when a dialog opens
pub fn wire_settings_actions(
    app: &AppWindow,
    app_state: Arc<Mutex<AppState>>,
    config_root: PathBuf,
) {
    // Before the first preset listing: an install interrupted between its
    // two renames must not leave the preset missing.
    if let Err(error) = actions::settings::recover_interrupted_preset_installs(&config_root) {
        let message = format!("preset install recovery failed: {error}");
        eprintln!("[d4lc] {message}");
        lock_state(&app_state).append_log_message(message, crate::clock::hh_mm_now());
    }
    {
        let mut state = lock_state(&app_state);
        state.rebuild_overlay_descriptor_state();
        actions::settings::apply_settings_preset_projection(app, &state, &config_root, None, false);
    }

    // Display-backend selector seed: the same startup
    // projection every launch runs, so the selector re-seeds from the
    // persisted `DisplayBackend` token in `Config/Settings.json`.
    actions::settings::apply_settings_display_backend_projection(app, &config_root);

    // Language combo seeds: the affix screen's game-data combo re-seeds from
    // the persisted `SelectedAffixLanguage` token (an unknown stored token
    // selects nothing, upstream InitAffixLanguages guard); the settings
    // screen's UI-language combo from `SelectedAppLanguage`, which also
    // applies that language.
    actions::settings::apply_settings_language_projection(app, &config_root);
    // The tab captions are Rust text, so they follow the language just applied.
    {
        let state = lock_state(&app_state);
        actions::settings::project_tab_labels(app, &state);
    }

    // Game-data language selection: forwarded from the affix screen's
    // language combo, run through the SettingsPanelState
    // set_language → SettingsChangeEffects chain, and persisted via the
    // normal settings save path (the hot-reload monitor then requests
    // the OCR/fuzzy rebuild on the running loop).
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_language_selected(move |language| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::settings::handle_language_selected(
                &app,
                &mut state,
                &config_clone,
                &language,
            ) {
                eprintln!("[d4lc] language_selected failed for '{language}': {error}");
            }
        }
    });

    // UI-language selection: switch the translation, persist the culture and
    // re-project the text Rust owns.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_app_language_selected(move |caption| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::settings::handle_app_language_selected(
                &app,
                &mut state,
                &config_clone,
                &caption,
            ) {
                eprintln!("[d4lc] app_language_selected failed for '{caption}': {error}");
            }
        }
    });

    // Display-backend selection: validate the chosen
    // label through the display-backend seam and persist the canonical
    // token. Invalid labels are structurally rejected (the selector
    // cannot produce them; the guard protects against programmatic
    // callers) and are logged like the other settings failures.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_display_backend_changed(move |label| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::settings::handle_display_backend_changed(
                &app,
                &mut state,
                &config_clone,
                &label,
            ) {
                eprintln!("[d4lc] display_backend_changed failed for '{label}': {error}");
            }
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_settings_changed(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_settings_changed(&app, &mut state, &config_clone);
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_preset_selected(move |preset_name| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::settings::handle_preset_selected(
                &app,
                &mut state,
                &config_clone,
                &preset_name,
            ) {
                eprintln!("[d4lc] preset_selected failed for '{preset_name}': {error}");
            }
        }
    });

    let app_weak = app.as_weak();
    app.on_community_preset_selected(move |preset_name| {
        if let Some(app) = upgrade_app(&app_weak) {
            app.set_selected_community_preset(preset_name);
            app.set_preset_download_success(false);
        }
    });

    // Dialog confirm — routed through the shared dialog router
    // so all 22 dialogs share one
    // confirm/cancel/error contract. The router dispatches to the
    // family-specific confirm handler based on
    // `state.dialog.kind.family()`.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_dialog_confirm(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            let _ = actions::dialogs::handle_confirm(&app, &mut state, &config_clone);
        }
    });

    // Dialog cancel — routed through the shared dialog router.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_dialog_cancel(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::dialogs::handle_cancel(&app, &mut state);
        }
    });

    // Dialog keyboard host: the FocusScope inside the
    // dialog host in `main_window.slint` routes Escape/Enter pressed
    // while a dialog is open here. The router applies the
    // upstream-derived per-kind semantics (see
    // `actions::dialogs::dialog_key_action`); confirm/cancel flow
    // through the same handlers the dialog buttons use.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_dialog_key(move |key_text| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            let routed =
                actions::dialogs::handle_dialog_key(&app, &mut state, &key_text, &config_clone);
            // Observability for the real-input harness: record the text
            // of the last routed key so tests can assert the host fired
            // (a no-op route is otherwise indistinguishable from an
            // unconsumed key event).
            if routed.is_some() {
                app.set_dialog_last_key(key_text);
            }
        }
    });

    // Open-dialog command handlers
    // Connector fix: pass the AppWindow
    // to the five `handle_open_*_config` functions so they can
    // seed the root in-out props (e.g. `settings-controller-mode`,
    // `settings-overlay-*`, `settings-paragon-*`) before the
    // dialog is rendered.
    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_dialog_open_colors(move || {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_open_colors_config(&app, &mut state);
        }
    });

    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_dialog_open_controller(move || {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_open_controller_config(&app, &mut state);
        }
    });

    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_dialog_open_hotkeys(move || {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_open_hotkeys_config(&app, &mut state);
        }
    });

    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_dialog_open_overlay(move || {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_open_overlay_config(&app, &mut state);
        }
    });

    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_dialog_open_paragon(move || {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_open_paragon_config(&app, &mut state);
        }
    });

    // Settings-dialog row channels.
    //
    // `settings-controller-image-selected` — one controller image add ("+")
    // / remove ("−") click from the controller config dialog; the draft
    // lives on `SettingsPanelState::active_controller_images` and the rows
    // are re-projected after every click so the two lists stay in step.
    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_settings_controller_image_selected(move |file_name, selected| {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_controller_image_selected(
                &app, &mut state, &file_name, selected,
            );
        }
    });

    // `settings-hotkey-row-toggled` — one manager-row enable switch flip;
    // the draft is in-memory until the dialog confirms.
    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_settings_hotkey_row_toggled(move |name, enabled| {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_hotkey_row_toggled(&app, &mut state, &name, enabled);
        }
    });

    // `settings-hotkey-row-edit` — the manager's per-row capture button;
    // opens the single-binding dialog seeded for that target.
    let app_weak_open = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_settings_hotkey_row_edit(move |name| {
        if let Some(app) = upgrade_app(&app_weak_open) {
            let mut state = lock_state(&state_clone);
            actions::settings::handle_hotkey_row_edit(&app, &mut state, &name);
        }
    });

    // Connector fix: wire the 5 build-import
    // dialog open callbacks so the 5 dialogs in the
    // DialogFamily::BuildImport family are reachable from the UI.
    wire_build_import_actions(app, Arc::clone(&app_state));

    // Download-preset: the root `download-preset(string)` callback hands
    // the fetch and the extraction to a worker thread
    // (`start_preset_download`, upstream `SystemPresetManager.cs:107-111`
    // awaits the download too). The Slint thread only flips the screen into
    // its downloading state; the worker's result is projected back through
    // the event loop, and a failure is logged on the Logging screen.
    let app_weak_dl = app.as_weak();
    let state_clone_dl = Arc::clone(&app_state);
    let config_clone_dl = config_root.clone();
    app.on_download_preset(move |file_name| {
        if let Some(app) = upgrade_app(&app_weak_dl) {
            actions::settings::start_preset_download(
                &app,
                &state_clone_dl,
                &config_clone_dl,
                &file_name,
                Arc::new(actions::settings::UreqDownloader),
            );
        }
    });
}

mod build_import;
pub use build_import::wire_build_import_actions;

/// Wire affix screen preset CRUD callbacks.
///
/// Registers `create-preset`, `remove-preset` and `rename-preset` as handlers
/// that delegate to the affix actions / commands modules, and
/// `import-preset`, which opens the ImportPreset dialog on the provider
/// selected on the affix screen.
pub fn wire_affix_actions(app: &AppWindow, app_state: Arc<Mutex<AppState>>) {
    // The preset create/remove/switch handlers persist through
    // d4lc-config. This wiring keeps its no-config-root
    // signature (the composition roots call it before resolving
    // user-facing settings), so the storage root is resolved here once
    // and captured by the closures; `resolve_config_root` honors
    // `D4LC_CONFIG_ROOT`, so the headless harness stays hermetic.
    let config_root = actions::settings::resolve_config_root();
    let app_weak = app.as_weak();

    // create-preset: real handler
    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    let preset_config_root = config_root.clone();
    app.on_create_preset(move || {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_create_preset(&app, &mut state, &preset_config_root);
        }
    });

    // remove-preset: real handler
    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    let preset_config_root = config_root.clone();
    app.on_remove_preset(move || {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_remove_preset(&app, &mut state, &preset_config_root);
        }
    });

    // rename-preset: the pencil (✎) icon next to the preset ComboBox on
    // the AffixScreen opens the RenamePresetDialog via the same handler the
    // root open-callback uses.
    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    app.on_rename_preset(move || {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_rename_preset(&app, &mut state);
        }
    });

    // Wire the new root open callbacks for the two
    // singleton dialogs whose natural invoker lives in the
    // build-import toolbar (or in the case of RenamePreset,
    // the pencil icon — see above). The setters seed the
    // root in-out props, then `actions::dialogs::open`
    // makes the dialog visible.
    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    app.on_open_new_preset_dialog(move || {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix_cmds::handle_open_set_preset_name(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    app.on_open_hotkey_config(move || {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::hotkeys_dialogs::handle_open_hotkey_config(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    // Also wire the dedicated `open-rename-preset` root
    // callback (added in main_window.slint for completeness)
    // to the same handler. This is a no-op today (the
    // pencil icon uses `rename-preset()` directly) but keeps
    // the dispatch surface uniform.
    app.on_open_rename_preset(move || {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix_cmds::handle_open_rename_preset(&app, &mut state);
        }
    });

    // import-preset: open the shared offline ImportPreset dialog,
    // seeded from the provider currently selected on AffixScreen.
    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    app.on_import_preset(move || {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_import_preset(&app, &mut state);
        }
    });

    // affix-preset-selected: an AffixScreen preset ComboBox switch
    // selects the preset in the editor and re-projects the entry
    // rows/cards (upstream re-binds the wanted lists on switches;
    // same seam as the hotkey preset switch).
    let weak = app_weak.clone();
    let state = Arc::clone(&app_state);
    let preset_config_root = config_root;
    app.on_affix_preset_selected(move |name| {
        if let Some(app) = weak.upgrade() {
            let mut state = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            actions::affix::handle_affix_preset_selected(
                &app,
                &mut state,
                &preset_config_root,
                &name,
            );
        }
    });
}

/// Wire affix command callbacks (config dialogs, entry CRUD, overlay toggle).
///
/// Also performs the initial projection of the selected preset's
/// entries onto the AffixScreen entry models (see
/// `actions::affix::project_affix_entries`).
///
/// Registers:
/// - `open-affix-config` — opens affix config dialog
/// - `open-aspect-config` — opens aspect config dialog
/// - `open-sigil-config` — opens sigil config dialog
/// - `open-unique-config` — opens unique config dialog
/// - `open-rune-config` — opens rune config dialog
/// - `remove-affix-entry(string)` — removes the kind-tagged-key entry
///   from the current preset
/// - `set-aspect-entry(string)` / `set-sigil-entry(string)` /
///   `set-unique-entry(string)` / `set-rune-entry(string)` — the
///   per-family add ("+") callbacks; the shared kind-aware handler
///   upserts into the matching wanted list
/// - `toggle-affix-overlay` — flips the persisted overlay flag through
///   the shared `confirm_overlay_config` path
/// - the affix-family config dialog channels:
///   `affix-rule-toggle-changed(name, value)` /
///   `affix-rule-selection-changed(name, value)` /
///   `affix-config-reset-minimal-values` and the SetAffix dialog's
///   `set-affix-slot-add(slot)` / `minimal-affix-value-edited(text)`,
///   plus the Sigil dialog's `cycle-sigil-dungeon-tier(id)` — each
///   forwards to its `actions::affix_dialogs` handler (write-through
///   persistence) and surfaces failures through the shared dialog error
///   contract (`actions::dialogs::set_error`).
pub fn wire_affix_command_actions(app: &AppWindow, app_state: Arc<Mutex<AppState>>) {
    // Resolved here for the entry-removal persistence seam
    // same reasoning as `wire_affix_actions` above.
    let config_root = actions::settings::resolve_config_root();

    // Initial entry projection: the AffixScreen entry models are
    // root-declared and have no population path of their own, so the
    // selected preset's entries (persisted presets included) must be
    // projected once the window exists — the same projection fills the
    // catalog models the right-column grids browse. The overlay indicator
    // starts from the persisted overlay flag so the power button is
    // before the first toggle.
    //
    // This projection also runs from the preset-model refresh of the
    // hydration seam (`wire_affix_filters_actions`); the call here keeps
    // this wiring entry point complete on its own and is idempotent (the
    // catalog is cached in `AppState`, no data is re-read).
    {
        let mut state = lock_state(&app_state);
        actions::settings::project_overlay_gate(app, &state.shell.settings);
        actions::affix::project_affix_entries(app, &mut state);
    }

    wire_affix_entry_commands(app, &app_state, &config_root);
    wire_affix_config_dialog_channels(app, &app_state, &config_root);
}

/// Wire the affix screen's config-dialog openers, entry removal, per-family
/// add ("+") callbacks and the overlay power toggle.
fn wire_affix_entry_commands(app: &AppWindow, app_state: &Arc<Mutex<AppState>>, root: &Path) {
    let app_weak = app.as_weak();

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_affix_config(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_open_affix_config(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_aspect_config(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_open_aspect_config(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_sigil_config(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_open_sigil_config(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_unique_config(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_open_unique_config(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_rune_config(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_open_rune_config(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_remove_affix_entry(move |key| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_remove_affix_entry(&app, &mut state, &config_root, &key);
        }
    });

    // Per-family add ("+"): the shared kind-aware handler resolves the
    // key's kind and appends the entry to the matching wanted list.
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_set_aspect_entry(move |key| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_set_family_entry(&app, &mut state, &config_root, &key);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_set_sigil_entry(move |key| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_set_family_entry(&app, &mut state, &config_root, &key);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_set_unique_entry(move |key| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_set_family_entry(&app, &mut state, &config_root, &key);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_set_rune_entry(move |key| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_set_family_entry(&app, &mut state, &config_root, &key);
        }
    });

    // The AffixScreen power toggle flips the persisted overlay flag and then
    // reconciles the live loop through the single-writer seam: upstream
    // `ToggleOverlayFromGUIMessage` sets `IsEnabled` and the handler runs
    // the capture/detect/OCR/overlay pipeline only while it is true
    // (`ScreenProcessHandler.cs:88,116-169`), and the port's equivalent of
    // "the handler runs" is the live loop. The lifecycle call happens after
    // the `AppState` lock is released and only when the persist actually
    // changed the flag, so a failed save never reports a state change.
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let toggle_config_root = root.to_path_buf();
    app.on_toggle_affix_overlay(move || {
        if let Some(app) = upgrade_app(&weak) {
            let enabled = {
                let mut state = lock_state(&state_clone);
                actions::affix_cmds::handle_toggle_affix_overlay(
                    &app,
                    &mut state,
                    &toggle_config_root,
                )
            };
            if let Some(enabled) = enabled {
                apply_overlay_loop_request(enabled, "power toggle");
            }
        }
    });

    // Wire open-set-affix and
    // open-set-affix-color callbacks so the affix screen's
    // edit/color buttons open the SetAffix / SetAffixColor
    // dialogs. The string param is the kind-tagged entry key
    // resolved against the mixed-kind preset entry list.
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_set_affix(move |key| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_set_affix_entry(&app, &mut state, &key);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_set_affix_color(move |key| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::affix_cmds::handle_set_affix_color(&app, &mut state, &key);
        }
    });
}

/// Wire the affix-family config dialog channels.
fn wire_affix_config_dialog_channels(
    app: &AppWindow,
    app_state: &Arc<Mutex<AppState>>,
    root: &Path,
) {
    let app_weak = app.as_weak();

    // ── Affix-family config dialog channels ──
    //
    // The five per-family config dialogs and the SetAffix dialog write
    // through on interaction (upstream saves from every setter), so every
    // gesture forwards here: the handler persists the settings / threshold
    // / tier / preset file and re-projects the live consumers, and a
    // failure surfaces through the shared dialog error contract (the same
    // shape as the trade picker channels). A successful per-slot add and
    // tier cycle also re-seed the dialog's host-projected model so the
    // slot captions / tier labels track the persisted state while the
    // dialog stays open.

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_affix_rule_toggle_changed(move |name, value| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::affix_dialogs::handle_rule_setting_toggled(
                &app,
                &mut state,
                &config_root,
                &name,
                value,
            ) {
                actions::dialogs::set_error(&app, &mut state, error);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_affix_rule_selection_changed(move |name, value| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::affix_dialogs::handle_rule_setting_selected(
                &app,
                &mut state,
                &config_root,
                &name,
                &value,
            ) {
                actions::dialogs::set_error(&app, &mut state, error);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_affix_config_reset_minimal_values(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::affix_dialogs::handle_reset_minimal_affix_values(
                &app,
                &mut state,
                &config_root,
            ) {
                actions::dialogs::set_error(&app, &mut state, error);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_set_affix_slot_add(move |slot_token| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            // The affix the dialog edits is the seeded entry id (the
            // dialog's per-slot buttons carry only the slot token).
            let affix_id = app.get_set_affix_entry_id().to_string();
            match actions::affix_dialogs::handle_add_affix_slot(
                &app,
                &mut state,
                &config_root,
                &affix_id,
                &slot_token,
            ) {
                Ok(()) => actions::affix_cmds::seed_set_affix_dialog_model(&app, &state),
                Err(error) => actions::dialogs::set_error(&app, &mut state, error),
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_minimal_affix_value_edited(move |text| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            let affix_id = app.get_set_affix_entry_id().to_string();
            if let Err(error) = actions::affix_dialogs::handle_set_affix_minimal_value(
                &app,
                &mut state,
                &config_root,
                &affix_id,
                &text,
            ) {
                actions::dialogs::set_error(&app, &mut state, error);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = root.to_path_buf();
    app.on_cycle_sigil_dungeon_tier(move |sigil_id| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            match actions::affix_dialogs::handle_cycle_sigil_dungeon_tier(
                &app,
                &mut state,
                &config_root,
                &sigil_id,
            ) {
                Ok(()) => actions::affix_cmds::seed_sigil_config_dialog(&app, &state),
                Err(error) => actions::dialogs::set_error(&app, &mut state, error),
            }
        }
    });
}

/// Wire logging screen callbacks.
pub fn wire_logging_actions(app: &AppWindow, app_state: Arc<Mutex<AppState>>) {
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_clear_log(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::logging::handle_clear_log(&app, &mut state);
        }
    });
}

/// Wire debug screen callbacks.
///
/// `controller` is the app-owned `DebugCommandController` that
/// owns the live-loop handle. Pass `None` from headless tests
/// to keep the-`Blocked` policy on `ExportDebugImages` /
/// `ReloadPresetImages` for tests that do not run a live loop.
///
/// `crash_dir` is the resolved local crash-log directory (see
/// `crash_report::resolve_crash_dir`); it backs the Debug screen's
/// recent-crash-log list and the opt-in local export action.
pub fn wire_debug_actions(
    app: &AppWindow,
    app_state: Arc<Mutex<AppState>>,
    config_root: PathBuf,
    controller: Option<runtime_bridge::DebugCommandController>,
    crash_dir: Option<PathBuf>,
) {
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);

    app.on_take_screenshot(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::debug::handle_take_screenshot(&app, &mut state);
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let controller_clone = controller.clone();
    app.on_export_debug_images(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            // The handler phases the AppState lock itself: the export runs
            // with the lock released (lock order on DebugCommandController).
            actions::debug::handle_export_debug_images(
                &app,
                &state_clone,
                controller_clone.as_ref(),
            );
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let controller_clone2 = controller.clone();
    app.on_reload_preset_images(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            // The handler phases the AppState lock itself: the
            // controller call restarts the live loop with the lock
            // released, so no guard is held across the runtime
            // preparation it performs.
            actions::debug::handle_reload_preset_images(
                &app,
                &state_clone,
                controller_clone2.as_ref(),
            );
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_reset_perf(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::debug::handle_reset_perf(&app, &mut state);
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_debug_info_toggled(move |enabled| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::debug::handle_debug_info_toggled(&app, &mut state, &config_clone, enabled);
        }
    });

    // Crash-log export (local copy only, gated on the opt-in
    // diagnostics toggle in actions::debug — no network code exists
    // on this path).
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_export_crash_log(move |file_name| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::debug::handle_export_crash_log(
                &app,
                &mut state,
                crash_dir.as_deref(),
                &config_clone,
                &file_name,
            );
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_topmost_toggled(move |topmost| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::debug::handle_topmost_toggled(&app, &mut state, &config_clone, topmost);
        }
    });

    // ── DebugInfo toggle state (SettingsD4.cs:29) ───────────────────
    // The persisted `IsDebugInfoEnabled` bit seeds the toggle so it
    // survives a restart (upstream DebugViewModel.cs:168-178 saves it on
    // every flip); the screen's two-way binding then carries edits back
    // through `on_debug_info_toggled`.
    {
        let mut state = lock_state(&app_state);
        let persisted = actions::debug::seed_debug_info_from_settings(&mut state, &config_root);
        app.set_debug_info_enabled(persisted);
    }

    // ── Debug stepper values (DebugView.xaml:74-184) ────────────────
    // Project every stepper from the settings state — including the four
    // area offsets, which the OCR region assembly reads from the settings
    // and a preset change re-seeds — once the window exists; the screen's
    // two-way bindings keep the values flowing back on every edit.
    {
        let state = lock_state(&app_state);
        actions::debug::project_debug_stepper_values(app, &state);
    }

    // ── Debug panel projection (artifacts, stages, perf, OCR) ───────
    // The panels read the debug-output directory this build writes:
    // newest export bundle + newest
    // screenshot, plus the live parse values for the OCR panels.
    {
        let mut state = lock_state(&app_state);
        actions::debug::project_debug_panels(
            app,
            &mut state,
            &actions::debug::screenshot::default_screenshot_dir(),
        );
    }

    // ── Persist one stepper edit ────────────────────────────────────
    // Every stepper on the screen sends its stable id through one
    // callback; the handler maps the id to its settings field, clamps
    // to the upstream range and saves through the shared settings path.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_debug_stepper_changed(move |id, value| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::debug::handle_debug_stepper_changed(
                &app,
                &mut state,
                &config_clone,
                &id,
                f64::from(value),
            );
        }
    });

    // The dedicated `type-area-offset-left` callback (the first stepper
    // wired, kept as its own root callback) routes through the same
    // handler so both paths clamp, persist and re-project identically.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let config_clone = config_root.clone();
    app.on_type_area_offset_left_changed(move |value| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::debug::handle_debug_stepper_changed(
                &app,
                &mut state,
                &config_clone,
                "type_area_offset_left",
                f64::from(value),
            );
        }
    });

    // ── Open the artifacts folder (upstream explorer-open) ──────────
    let state_clone = Arc::clone(&app_state);
    app.on_debug_open_artifacts(move || {
        let mut state = lock_state(&state_clone);
        actions::debug::handle_open_artifacts_folder(
            &mut state,
            &actions::debug::screenshot::default_screenshot_dir(),
        );
    });

    // ── Stepper re-projection on settings changes ───────────────────
    // Upstream re-reads the offset/threshold fields into the panel when
    // the system preset changes (`DebugViewModel.cs:574-587`). The
    // settings file is the single source both the Settings tab and an
    // out-of-app edit write, so a 500 ms stamp watcher re-projects the
    // steppers from the file when it changed. Leaked for the app
    // lifetime like the other wiring timers.
    //
    // The same tick keeps the stored-run timings' provenance line
    // the timing table reads a previous export, so its "captured …
    // ago" part must not freeze at whatever it was when the tab was opened.
    // It refreshes only while the Debug tab is visible.
    let timer_app_weak = app.as_weak();
    let timer_state = Arc::clone(&app_state);
    let timer_config_root = config_root.clone();
    let debug_dir = actions::debug::screenshot::default_screenshot_dir();
    let mut projection = actions::debug::DebugSettingsProjection::new(&config_root);
    let mut debug_output = actions::debug::DebugOutputWatcher::default();
    // Whether the previous tick saw the Debug tab. Entering it forces one
    // live-panel projection so the first view shows the current accepted
    // snapshot instead of whatever was projected before the tab was opened
    // (the per-frame drain only projects while the tab is visible).
    let mut debug_tab_was_active = false;
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(500),
        move || {
            if let Some(app) = timer_app_weak.upgrade() {
                {
                    let mut state = lock_state(&timer_state);
                    projection.observe_and_reproject(&app, &mut state, &timer_config_root);
                    // The live loop flips the overlay gate from the overlay
                    // menu lock on its own thread; this UI-thread tick keeps
                    // the preset gate (and power indicator) in step.
                    actions::settings::project_overlay_gate(&app, &state.shell.settings);
                }
                let debug_active = app.get_active_tab() == DEBUG_TAB_INDEX;
                if debug_active {
                    if !debug_tab_was_active {
                        let state = lock_state(&timer_state);
                        actions::debug::project_live_debug_stages(&app, &state);
                        actions::debug::project_live_diagnostics(&app, &state);
                    }
                    actions::debug::refresh_stored_run_note(&app, &mut debug_output, &debug_dir);
                }
                debug_tab_was_active = debug_active;
            }
        },
    );
    Box::leak(Box::new(timer));
}

mod live_loop;
use live_loop::apply_overlay_loop_request;
pub use live_loop::{
    install_live_loop_controller, restore_overlay_live_loop, wire_runtime_actions,
};

/// Wire the persistent hotkey backend and its drain timer.
///
/// Builds the per-binding request list from the persisted
/// `AppSettings::hotkey_config.bindings`, starts the X11 event-loop thread
/// for any binding whose registration probe succeeded, and
/// runs a periodic Slint `Timer` that drains `HotkeyEvent`
/// values from the channel and dispatches them through
/// `hotkeys::dispatch_hotkey_event`.
///
/// Called once by `main.rs` with the loaded `Config/Settings.json`:
/// 1. Projects the persisted `hotkey_config.bindings` into
///    `Vec<HotkeyRequest>` (respecting the user's enabled / disabled flags
///    and binding strings, not the all-disabled defaults).
/// 2. Runs the registration probe to decide the capability and the
///    per-binding outcome; the resulting `HotkeyBackendStatus` is stored on
///    `AppState` (and a capability note is appended when X11 is
///    unavailable).
/// 3. When a backend thread started, installs its handle + event receiver
///    in the restartable runtime registry
///    ([`actions::settings::install_hotkey_runtime`]).
/// 4. Starts the 50 ms drain timer. The timer reads the *current* receiver
///    from the registry on every tick, so a binding edit that restarts the
///    backend ([`actions::settings::restart_hotkey_backend`], driven by the
///    Hotkeys config dialogs) is picked up without a second timer or an app
///    restart — the upstream `UpdateHotkeysRequestMessage` →
///    `InitKeyBindings` re-registration (`MainWindowViewModel.cs:204-208`,
///    `:380-441`). A drained `ToggleOverlay` (Ctrl+F12) request also
///    reconciles the live loop through the same controller seam the affix
///    power toggle uses, after the `AppState` lock is released.
///
/// When no thread starts (no X11 session, Wayland without a portal
/// backend, no enabled binding, x11rb feature off) the status already
/// reports the reason per binding; the timer still runs so a later restart
/// can begin draining the moment a backend does start.
pub fn wire_hotkey_actions(
    app: &AppWindow,
    app_state: std::sync::Arc<std::sync::Mutex<AppState>>,
    config_root: std::path::PathBuf,
    config_settings: &d4lc_config::AppSettings,
) {
    let app_weak = app.as_weak();

    // 1. Start the backend. Returns `(status, Some(handle),
    //    Some(rx))` on a successful probe + thread spawn,
    //    or `(status, None, None)` on every unavailable
    //    path (no DISPLAY, Wayland, no x11rb, no enabled
    //    binding).
    let (status, handle, rx) = crate::hotkeys::start_hotkey_backend(config_settings);

    // 2. Record the status on AppState so the UI / log
    //    surface can introspect what the backend did. This
    //    also appends a capability note when X11 was
    //    unavailable.
    {
        let mut state = lock_state(&app_state);
        state.set_hotkey_backend_status(status);
    }

    // 3. Install the runtime handle when a thread started; otherwise the
    //    status is the "could not register" report and a later
    //    restart may still install one.
    match (handle, rx) {
        (Some(handle), Some(event_rx)) => {
            actions::settings::install_hotkey_runtime(handle, event_rx);
        }
        _ => {
            eprintln!(
                "[d4lc] hotkeys: backend did not start a thread; running without global hotkey dispatch \
                 (see AppState::hotkey_backend for the per-binding outcome)"
            );
        }
    }

    // 4. Periodic drain timer. Mirrors the `wire_runtime_actions`
    //    pattern: 50ms `Repeated` `slint::Timer`, `try_recv`
    //    only (never blocks the UI thread), dispatch each
    //    event through the shared dispatcher. The receiver is re-read
    //    every tick so a backend restart swaps the stream in place.
    let state_clone = std::sync::Arc::clone(&app_state);
    let config_clone = config_root.clone();

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(50),
        move || {
            if let Some(app) = app_weak.upgrade() {
                let Some(rx) = actions::settings::current_hotkey_receiver() else {
                    return;
                };
                // Dispatch under the `AppState` lock, then reconcile the
                // live loop after releasing it: the Ctrl+F12 arm flips the
                // persisted overlay flag first.
                let request = {
                    let mut state = lock_state(&state_clone);
                    let rx = match rx.lock() {
                        Ok(g) => g,
                        Err(e) => e.into_inner(),
                    };
                    crate::hotkeys::drain_hotkey_events(&app, &mut state, &config_clone, &rx)
                };
                if let Some(request) = request {
                    apply_overlay_loop_request(request.enabled, "Ctrl+F12 hotkey");
                }
            }
        },
    );

    // 5. The timer must live for the full app lifetime; the backend
    //    handle lives in the runtime registry (so the Hotkeys dialog can
    //    stop and replace it). `Box::leak` is the same escape hatch
    //    `wire_runtime_actions` uses for its timer.
    Box::leak(Box::new(timer));

    eprintln!("[d4lc] hotkeys: persistent backend wired, event loop draining on the UI thread");
}

/// Wire trade screen callbacks.
///
/// Also performs the initial projection of the trade state onto the
/// TradeScreen models (see `actions::trade::project_trade_state`).
///
/// Registers:
/// - `add-trade-item` — opens the add-trade-item dialog
/// - `edit-trade-item(string)` — sets selected item ID and opens dialog
/// - `remove-trade-item(string)` — removes a trade item
/// - `open-trade-config` — opens the trade config dialog
/// - the AddTradeItem picker channels:
///   `trade-add-affix` / `trade-add-rune-affix` / `trade-remove-affix`
///   / `trade-affix-is-greater` mutate the dialog's staged draft,
///   `trade-affix-filter-changed` re-filters the candidate models,
///   and `trade-form-type-edited` applies the rune-boundary draft
///   clearing — all through the ready `d4lc_gui::trade` API.
pub fn wire_trade_actions(app: &AppWindow, app_state: Arc<Mutex<AppState>>) {
    // Startup hydration (upstream `TradeItemManager.LoadTradeItems`
    // plus the `HandleApplicationLoadedMessage` AddRange): the wanted
    // rows load from `Config/TradeItems.json` before the initial
    // projection, so a restart restores the persisted list. The
    // ShowCurrentItem gate hydrates from the persisted
    // `Config/Settings.json` in the same seam (upstream default
    // `true`, SettingsD4.cs:83) so the first projection already
    // mirrors the setting.
    {
        let mut state = lock_state(&app_state);
        let config_root = actions::settings::resolve_config_root();
        actions::trade::hydrate_trade_items(&mut state, &config_root);
        if let Ok(config) = d4lc_config::load_app_settings(&config_root) {
            state.trade_show_current_item = config.show_current_item;
        }
    }

    // Initial projection: the TradeScreen models are root-declared
    // and have no population path of their own, so the persisted
    // trade items and the current-item card must be projected once
    // the window exists. `project_trade_state` also seeds the
    // `trade-show-current-item` gate from the hydrated state.
    {
        let state = lock_state(&app_state);
        actions::trade::project_trade_state(app, &state);
    }

    // Picker support state for the AddTradeItem dialog: the full
    // candidate catalogs (loaded when the dialog opens; the filter
    // box re-projects from them) and the previous form type the
    // rune-boundary detection compares against (upstream
    // `SelectedItemType` setter tracks `previousType` the same way).
    // In-memory only — the dialog lifetime bounds its use.
    struct TradePickerCache {
        affixes: Vec<d4lc_gui::trade::TradeAffixCandidate>,
        runes: Vec<d4lc_gui::trade::TradeRuneCandidate>,
        previous_form_type: String,
    }
    let picker_cache: Arc<Mutex<TradePickerCache>> = Arc::new(Mutex::new(TradePickerCache {
        affixes: Vec::new(),
        runes: Vec::new(),
        previous_form_type: String::new(),
    }));

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let picker_clone = Arc::clone(&picker_cache);
    app.on_add_trade_item(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::trade::handle_add_trade_item(&app, &mut state);
            // Seed the picker models for the fresh dialog (empty
            // staged draft, full catalogs) and reset the boundary
            // tracker to the (cleared) form type.
            let (affixes, runes) = actions::trade::seed_trade_picker_models(&app, &state);
            let mut cache = lock_cell(&picker_clone);
            cache.affixes = affixes;
            cache.runes = runes;
            cache.previous_form_type = app.get_trade_form_item_type().to_string();
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    let picker_clone = Arc::clone(&picker_cache);
    app.on_edit_trade_item(move |item_id| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::trade::handle_edit_trade_item(&app, &mut state, &item_id);
            // Seed the picker models from the addressed row (staged
            // draft = the row's stored affixes, edit mode = on).
            let (affixes, runes) = actions::trade::seed_trade_picker_models(&app, &state);
            let mut cache = lock_cell(&picker_clone);
            cache.affixes = affixes;
            cache.runes = runes;
            cache.previous_form_type = app.get_trade_form_item_type().to_string();
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_remove_trade_item(move |item_id| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::trade::handle_remove_trade_item(&app, &mut state, &item_id);
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_open_trade_config(move || {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            actions::trade::handle_open_trade_config(&app, &mut state);
        }
    });

    // ── AddTradeItem picker channels ──────────────────────────────────

    // Stage one affix candidate on the dialog draft. The draft lives
    // in the dialog models until the confirm applies it to the real
    // row through the ViewModel (upstream mutates the dialog's
    // `TradeItem.Affixes` live and saves on Done). A bypassed rune
    // gate surfaces the ViewModel's error on the shared dialog error
    // contract instead of mutating the draft.
    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_trade_add_affix(move |affix_id| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::trade::stage_trade_affix(&app, &state, &affix_id, false) {
                actions::dialogs::set_error(&app, &mut state, error);
            }
        }
    });

    let app_weak = app.as_weak();
    let state_clone = Arc::clone(&app_state);
    app.on_trade_add_rune_affix(move |affix_id| {
        if let Some(app) = upgrade_app(&app_weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::trade::stage_trade_affix(&app, &state, &affix_id, true) {
                actions::dialogs::set_error(&app, &mut state, error);
            }
        }
    });

    let app_weak = app.as_weak();
    app.on_trade_remove_affix(move |affix_id| {
        if let Some(app) = upgrade_app(&app_weak) {
            actions::trade::unstage_trade_affix(&app, &affix_id);
        }
    });

    let app_weak = app.as_weak();
    app.on_trade_affix_is_greater(move |affix_id, is_greater| {
        if let Some(app) = upgrade_app(&app_weak) {
            actions::trade::restage_trade_affix_is_greater(&app, &affix_id, is_greater);
        }
    });

    // Re-filter the candidate models through the headless
    // `;`-keyword filters (upstream `AffixTextFilter` setter
    // refreshes both filtered views).
    let app_weak = app.as_weak();
    let picker_clone = Arc::clone(&picker_cache);
    app.on_trade_affix_filter_changed(move |filter| {
        if let Some(app) = upgrade_app(&app_weak) {
            let cache = lock_cell(&picker_clone);
            actions::trade::project_filtered_trade_candidates(
                &app,
                &cache.affixes,
                &cache.runes,
                &filter,
            );
        }
    });

    // The type text changed: apply the rune-boundary draft clearing
    // (upstream `SelectedItemType` setter,
    // AddTradeItemViewModel.cs:108-119 — a non-blank previous type
    // crossing the rune boundary clears `TradeItem.Affixes`) and
    // re-project the rune flag that switches the candidate list.
    let app_weak = app.as_weak();
    let picker_clone = Arc::clone(&picker_cache);
    app.on_trade_form_type_edited(move |item_type| {
        if let Some(app) = upgrade_app(&app_weak) {
            let crossing = {
                let cache = lock_cell(&picker_clone);
                d4lc_gui::trade::crosses_rune_boundary(&cache.previous_form_type, &item_type)
            };
            if crossing {
                actions::trade::clear_trade_staged_affixes(&app);
            }
            let mut cache = lock_cell(&picker_clone);
            cache.previous_form_type = item_type.to_string();
            app.set_trade_item_type_is_rune(d4lc_gui::trade::is_rune_item_type(&item_type));
        }
    });
}

/// Lock helper for the wiring-held in-memory cells (same poison
/// recovery as `lock_state`).
fn lock_cell<T>(cell: &Arc<Mutex<T>>) -> std::sync::MutexGuard<'_, T> {
    match cell.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
