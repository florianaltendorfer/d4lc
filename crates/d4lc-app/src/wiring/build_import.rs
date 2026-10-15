//! Build-import dialog wiring: website/URL gate, row actions, preset dialog,
//! local import/merge, multi-build config and the background-event drain timer.

use std::sync::{Arc, Mutex};

use slint::ComponentHandle;

use crate::actions;
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

use super::{lock_state, upgrade_app};

/// Wire the tabbed build-import dialog: one `ImportPreset` dialog with
/// seven tabs; downloads run on background workers whose events the
/// main-thread 50 ms drain timer projects into the window properties.
///
/// Registers the root callbacks declared in `main_window.slint`:
/// - `open-import-preset-tab(int)` — open on an explicit tab (the
///   AffixScreen provider chips)
/// - `import-url-changed(string, string)` — live Add-gate evaluation
/// - `import-add-clicked(string)` — gated background download
/// - `import-build-selected/update/web/remove(string, int)` — row actions
/// - `import-variant-add(string, int)` — Confirm-name overlay
/// - `import-visit-web(string)` — provider overview page
/// - `import-paragon-toggled(string, bool)` /
///   `import-show-browser-toggled(bool)` — persisted toggles
/// - `import-preset-remove()` / `import-delete-confirm()` /
///   `import-delete-cancel()` — preset removal with the Delete confirm
/// - `import-name-done()` / `import-name-cancel()` /
///   `import-name-suggestion-selected(int)` — Confirm-name flow
/// - `import-progress-cancel()` — cancel the running import
/// - `import-progress-done()` — close the failure panel
/// - `import-local-import()` / `import-local-export()` — Local tab
/// - `import-merge-clicked()` / `import-merge-color-clicked(int)` /
///   `import-color-picker-done()` / `import-color-picker-cancel()` —
///   Merge tab
/// - `open-multi-build-config()`
pub fn wire_build_import_actions(app: &AppWindow, app_state: Arc<Mutex<AppState>>) {
    wire_import_website_and_add_actions(app, &app_state);
    wire_import_build_row_actions(app, &app_state);
    wire_import_preset_dialog_actions(app, &app_state);
    wire_import_local_and_merge_actions(app, &app_state);
    wire_multi_build_config_actions(app, &app_state);
    start_build_import_drain_timer(app, &app_state);
}

/// Wire the website combo, the import dialog opener, the URL gate and the
/// Add button.
fn wire_import_website_and_add_actions(app: &AppWindow, app_state: &Arc<Mutex<AppState>>) {
    let app_weak = app.as_weak();

    // Startup seed (upstream `AffixViewModel.InitBuildImportWebsites`,
    // AffixViewModel.cs:1664-1675): the website combo starts on the
    // persisted `SelectedBuildImportWebsite`; an unknown stored value
    // falls back to `InfinityBuilds.gg`.
    {
        let state = lock_state(app_state);
        let persisted = state.shell.settings.selected_build_import_website.clone();
        let website = if crate::actions::build_import_dialogs::ALL_BUILD_IMPORT_WEBSITES
            .contains(&persisted.as_str())
        {
            persisted
        } else {
            crate::actions::build_import_dialogs::WEBSITE_INFINITY_BUILDS.to_string()
        };
        app.set_build_import_provider(website.into());
    }

    // Website change persists immediately (upstream
    // `AffixViewModel` saves `SelectedBuildImportWebsite` on every
    // combo change, AffixViewModel.cs:415-418) through the same
    // persist-with-warnings path the settings steppers use.
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let website_config_root = actions::settings::resolve_config_root();
    app.on_import_website_changed(move |website| {
        if upgrade_app(&weak).is_some()
            && crate::actions::build_import_dialogs::ALL_BUILD_IMPORT_WEBSITES
                .contains(&website.as_str())
        {
            let mut state = lock_state(&state_clone);
            state.shell.settings.selected_build_import_website = website.to_string();
            state.bump_rules_input_revision();
            actions::settings::persist_settings_with_warnings(&mut state, &website_config_root);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_import_preset_tab(move |tab| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::build_import_dialogs::open_import_dialog(&app, &mut state, tab);
        }
    });

    let weak = app_weak.clone();
    app.on_import_url_changed(move |provider, url| {
        if let Some(app) = upgrade_app(&weak) {
            actions::build_import_dialogs::handle_url_changed(&app, &provider, &url);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_add_clicked(move |provider| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_add_clicked(&app, &mut state, &provider)
            {
                eprintln!("[d4lc] build import add failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });
}

/// Wire the per-build and per-profile row actions and the persisted import
/// toggles.
fn wire_import_build_row_actions(app: &AppWindow, app_state: &Arc<Mutex<AppState>>) {
    let app_weak = app.as_weak();

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_build_selected(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            let _ = actions::build_import_dialogs::handle_build_selected(
                &app, &state, &provider, index,
            );
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_build_update(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_build_update(
                &app, &mut state, &provider, index,
            ) {
                eprintln!("[d4lc] build import update failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_build_web(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_build_web(&state, &provider, index)
            {
                eprintln!("[d4lc] build import web failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_build_remove(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_build_remove(
                &app, &mut state, &provider, index,
            ) {
                eprintln!("[d4lc] build import remove failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_variant_add(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_variant_add(
                &app, &mut state, &provider, index,
            ) {
                eprintln!("[d4lc] build import variant add failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    app.on_import_visit_web(move |provider| {
        if let Some(app) = upgrade_app(&weak) {
            if let Err(error) = actions::build_import_dialogs::handle_visit_web(&app, &provider) {
                eprintln!("[d4lc] build import visit failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_profile_selected(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            let _ = actions::build_import_dialogs::handle_profile_selected(
                &app, &state, &provider, index,
            );
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_profile_update(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_profile_update(
                &app, &mut state, &provider, index,
            ) {
                eprintln!("[d4lc] profile update failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_profile_web(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_profile_web(&state, &provider, index)
            {
                eprintln!("[d4lc] profile web failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_profile_remove(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_profile_remove(
                &app, &mut state, &provider, index,
            ) {
                eprintln!("[d4lc] profile remove failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_profile_variant_add(move |provider, index| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_profile_variant_add(
                &app, &state, &provider, index,
            ) {
                eprintln!("[d4lc] profile variant add failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_paragon_toggled(move |provider, enabled| {
        if let Some(_app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_paragon_toggled(&mut state, &provider, enabled);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_show_browser_toggled(move |enabled| {
        if let Some(_app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_show_browser_toggled(&mut state, enabled);
        }
    });
}

/// Wire the preset removal confirm, the Confirm-name flow and the manual
/// progress fallback.
fn wire_import_preset_dialog_actions(app: &AppWindow, app_state: &Arc<Mutex<AppState>>) {
    let app_weak = app.as_weak();

    let weak = app_weak.clone();
    app.on_import_preset_remove(move || {
        if let Some(app) = upgrade_app(&weak) {
            if let Err(error) = actions::build_import_dialogs::handle_preset_remove(&app) {
                eprintln!("[d4lc] import preset remove failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = actions::settings::resolve_config_root();
    app.on_import_delete_confirm(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_delete_confirm(&app, &mut state, &config_root)
            {
                eprintln!("[d4lc] import delete confirm failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    app.on_import_delete_cancel(move || {
        if let Some(app) = upgrade_app(&weak) {
            actions::build_import_dialogs::handle_delete_cancel(&app);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = actions::settings::resolve_config_root();
    app.on_import_name_done(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_name_done(&app, &mut state, &config_root)
            {
                eprintln!("[d4lc] add-as-preset failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_name_cancel(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_name_cancel(&app, &mut state);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_name_suggestion_selected(move |value| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_name_suggestion_selected(&app, &state, &value);
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_name_text_changed(move || {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_name_text_changed(&app, &state);
        }
    });

    // The failure panel's Close (the import already finished).
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_progress_done(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_progress_close(&app, &mut state);
        }
    });

    // The running import's Cancel: cancels the job immediately.
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_progress_cancel(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_progress_cancel(&app, &mut state);
        }
    });
}

/// Wire the Local tab (import/export) and the Merge tab.
fn wire_import_local_and_merge_actions(app: &AppWindow, app_state: &Arc<Mutex<AppState>>) {
    let app_weak = app.as_weak();

    // ── Local tab (upstream tab 0) ───────────────────────────────────
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = actions::settings::resolve_config_root();
    app.on_import_local_import(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_local_import(&app, &mut state, &config_root)
            {
                eprintln!("[d4lc] local preset import failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let config_root = actions::settings::resolve_config_root();
    app.on_import_local_export(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_local_export(&app, &mut state, &config_root)
            {
                eprintln!("[d4lc] local preset export failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    // ── Merge tab (upstream tab 1) ───────────────────────────────────
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_import_merge_clicked(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_merge_clicked(&app, &mut state)
            {
                eprintln!("[d4lc] preset merge failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    app.on_import_merge_color_clicked(move |row| {
        if let Some(app) = upgrade_app(&weak) {
            if let Err(error) = actions::build_import_dialogs::handle_merge_color_clicked(&app, row)
            {
                eprintln!("[d4lc] merge color picker failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    app.on_import_color_picker_done(move || {
        if let Some(app) = upgrade_app(&weak) {
            if let Err(error) = actions::build_import_dialogs::handle_color_picker_done(&app) {
                eprintln!("[d4lc] merge color apply failed: {error}");
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    app.on_import_color_picker_cancel(move || {
        if let Some(app) = upgrade_app(&weak) {
            actions::build_import_dialogs::handle_color_picker_cancel(&app);
        }
    });
}

/// Wire the multi-build config dialog.
fn wire_multi_build_config_actions(app: &AppWindow, app_state: &Arc<Mutex<AppState>>) {
    let app_weak = app.as_weak();

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_open_multi_build_config(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_open_multi_build_config(&app, &mut state);
        }
    });

    // Multi-build dialog editing (upstream `MultiBuildConfigViewModel`
    // add / remove / set colour commands, each saved at once).
    let multi_build_config_root = actions::settings::resolve_config_root();
    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_multi_build_preset_selected(move |preset| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            actions::build_import_dialogs::handle_multi_build_preset_selected(
                &app, &state, &preset,
            );
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let root = multi_build_config_root.clone();
    app.on_multi_build_add(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_multi_build_add(&app, &mut state, &root)
            {
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let root = multi_build_config_root.clone();
    app.on_multi_build_remove(move |index| {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_multi_build_remove(
                &app, &mut state, &root, index,
            ) {
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    app.on_multi_build_color_clicked(move |index| {
        if let Some(app) = upgrade_app(&weak) {
            let state = lock_state(&state_clone);
            if let Err(error) =
                actions::build_import_dialogs::handle_multi_build_color_clicked(&app, &state, index)
            {
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    let state_clone = Arc::clone(app_state);
    let root = multi_build_config_root;
    app.on_multi_build_color_picker_done(move || {
        if let Some(app) = upgrade_app(&weak) {
            let mut state = lock_state(&state_clone);
            if let Err(error) = actions::build_import_dialogs::handle_multi_build_color_picker_done(
                &app, &mut state, &root,
            ) {
                app.set_dialog_error(error.into());
                app.set_dialog_error_visible(true);
            }
        }
    });

    let weak = app_weak.clone();
    app.on_multi_build_color_picker_cancel(move || {
        if let Some(app) = upgrade_app(&weak) {
            actions::build_import_dialogs::handle_multi_build_color_picker_cancel(&app);
        }
    });
}

/// Start the periodic timer that drains the build-import worker events.
fn start_build_import_drain_timer(app: &AppWindow, app_state: &Arc<Mutex<AppState>>) {
    // ── Periodic drain timer ─────────────────────────────────────────
    // The worker threads append progress events into the shared buffer;
    // this 50 ms timer (the established wiring pattern) moves them into
    // the window properties on the main thread and refreshes the build
    // lists on completion. Leaked for the app lifetime like the runtime
    // drain timer.
    let timer_app_weak = app.as_weak();
    let timer_state = Arc::clone(app_state);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(50),
        move || {
            if let Some(app) = timer_app_weak.upgrade() {
                let mut state = lock_state(&timer_state);
                actions::build_import_dialogs::drain_build_import_events(&app, &mut state);
            }
        },
    );
    Box::leak(Box::new(timer));
}
