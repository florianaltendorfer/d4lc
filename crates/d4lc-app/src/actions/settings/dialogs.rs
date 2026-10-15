//! Settings-family dialog confirm/cancel routing and the open handlers for
//! the colors, controller, overlay and paragon config dialogs.

use std::path::{Path, PathBuf};

use d4lc_config::load_app_settings;
use d4lc_gui::settings::SettingsViewModel;
use slint::{ModelRc, VecModel};

use super::confirm::{
    confirm_colors_config, confirm_controller_config_draft, confirm_overlay_config,
    confirm_paragon_config, ColorsConfigDraft, ControllerConfigDraft, OverlayConfigDraft,
    ParagonConfigDraft,
};
use super::hotkey_config::confirm_hotkeys_config;
use super::panel::{dispatch_settings_effects, resolve_config_root};
use super::presets::system_presets_asset_dir;
use super::root_props::{
    sync_colors_props, sync_controller_props, sync_overlay_props, sync_paragon_props,
    sync_root_props_from_settings,
};
use crate::app_state::{AppState, DialogKind};
use crate::slint_generated::AppWindow;

/// Handle the `dialog-confirm()` callback.
///
/// Calls `ViewModel::confirm_dialog()`, persists config, and closes the dialog UI.
pub fn handle_dialog_confirm(app: &AppWindow, state: &mut AppState, config_root: &Path) {
    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let effects = vm.confirm_dialog();
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();
    dispatch_settings_effects(&effects, state, config_root);
    app.set_dialog_visible(false);
}

/// Handle the `dialog-cancel()` callback.
///
/// Calls `ViewModel::cancel_dialog()` which discards draft changes.
pub fn handle_dialog_cancel(app: &AppWindow, state: &mut AppState) {
    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let effects = vm.cancel_dialog();
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();
    let _ = effects; // Effects are empty on cancel
    app.set_dialog_visible(false);
}

/// Router-friendly variant of `handle_dialog_confirm` used by the
/// shared [`crate::actions::dialogs`] router.
///
/// Dispatches by `state.dialog.kind` so the five settings-family
/// dialogs run kind-specific confirm bodies that read their
/// form values off the root `AppWindow` in-out properties. Every body
/// validates its numeric ranges / color strings first and returns `Err`
/// without mutating state when a value is out of range, so the shared
/// dialog router keeps the dialog open with the validation error.
///
/// - `ControllerConfig` — reads the controller-mode toggle plus the
///   staged controller image selection, writes `Config/Controllers.json`
///   and persists the mode.
/// - `OverlayConfig` — reads the `settings-overlay-*` values.
/// - `ParagonConfig` — reads the paragon toggles and the five layout
///   numerics.
/// - `ColorsConfig` — reads the seven `settings-colors-*` color strings.
/// - `HotkeysConfig` — persists the binding drafts and re-registers the
///   global hotkeys.
///
/// On success the router calls `actions::dialogs::close` to
/// clear the shared dialog state; the dialog UI itself is
/// already toggled off by the inner `handle_dialog_confirm`
/// for the kind-specific paths that call it.
pub fn handle_dialog_confirm_with_router(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let kind = state.dialog.kind;
    match kind {
        DialogKind::ControllerConfig => {
            // Read the controller form values (mode + staged image
            // selection), apply them through the headless helper, and
            // close the dialog.
            let draft = ControllerConfigDraft::new(
                app.get_settings_controller_mode(),
                state.shell.settings.active_controller_images.clone(),
            );
            confirm_controller_config_draft(&draft, state, config_root)?;
            sync_root_props_from_settings(app, &state.shell.settings);
            app.set_dialog_visible(false);
            Ok(())
        }
        DialogKind::OverlayConfig => {
            // Read the overlay form values, build a headless draft,
            // validate + apply + persist through the headless helper,
            // and close the dialog.
            let draft = OverlayConfigDraft {
                overlay_enabled: overlay_enabled_to_write(app, state),
                overlay_top_most: app.get_settings_overlay_top_most(),
                overlay_font_size: app.get_settings_overlay_font_size(),
                overlay_menu_locked: app.get_settings_overlay_menu_locked(),
                type_area_offset_left: app.get_settings_overlay_type_area_offset_left(),
                show_overlay_icon: app.get_settings_overlay_show_icon(),
                overlay_icon_pos_x: app.get_settings_overlay_icon_pos_x(),
                overlay_icon_pos_y: app.get_settings_overlay_icon_pos_y(),
                item_power_limit_enabled: app.get_settings_overlay_item_power_enabled(),
                item_power_limit: app.get_settings_overlay_item_power_limit(),
                overlay_marker_mode: app.get_settings_overlay_marker_mode().to_string(),
                trade_overlay_enabled: app.get_settings_overlay_trade_overlay(),
                socket_detection_enabled: app.get_settings_overlay_socket_detection(),
                tempered_detection_enabled: app.get_settings_overlay_tempered_detection(),
                aspect_detection_enabled: app.get_settings_overlay_aspect_detection(),
                screen_capture_delay: app.get_settings_overlay_screen_capture_delay().round()
                    as i32,
                overlay_update_delay: app.get_settings_overlay_update_delay().round() as i32,
                scan_height: app.get_settings_overlay_scan_height().round() as i32,
            };
            confirm_overlay_config(&draft, state, config_root)?;
            sync_root_props_from_settings(app, &state.shell.settings);
            // The always-on-top row is a port addition: upstream edits
            // `IsTopMost` only on the Debug view (DebugView.xaml:68). The live
            // apply follows upstream's `Topmost="{Binding IsTopMost}"`
            // (MainWindow.xaml:18) re-read on `TopMostStateChangedMessage`
            // (MainWindowViewModel.cs:199-202), so no restart is needed.
            app.set_window_topmost(state.shell.settings.overlay_top_most);
            app.set_dialog_visible(false);
            Ok(())
        }
        DialogKind::ParagonConfig => {
            // Read the paragon form values, validate the layout ranges,
            // apply + persist through the headless helper, and close the
            // dialog.
            let draft = ParagonConfigDraft {
                paragon_mode_active: app.get_settings_paragon_mode_active(),
                is_show_browser_mobalytics_enabled: app
                    .get_settings_paragon_show_browser_mobalytics_enabled(),
                collapsed_paragonboard_enabled: app.get_settings_paragon_collapsed_board(),
                paragon_border_size: app.get_settings_paragon_border_size(),
                paragon_node_size: app.get_settings_paragon_node_size(),
                paragon_node_size_collapsed: app.get_settings_paragon_node_size_collapsed(),
                paragon_left_offset_collapsed: app.get_settings_paragon_left_offset_collapsed(),
                paragon_top_offset_collapsed: app.get_settings_paragon_top_offset_collapsed(),
            };
            confirm_paragon_config(&draft, state, config_root)?;
            sync_root_props_from_settings(app, &state.shell.settings);
            app.set_dialog_visible(false);
            Ok(())
        }
        DialogKind::ColorsConfig => {
            let draft = ColorsConfigDraft::new(
                app.get_settings_colors_implicit().to_string(),
                app.get_settings_colors_normal().to_string(),
                app.get_settings_colors_greater().to_string(),
                app.get_settings_colors_tempered().to_string(),
                app.get_settings_colors_aspects().to_string(),
                app.get_settings_colors_uniques().to_string(),
                app.get_settings_colors_runes().to_string(),
            );
            confirm_colors_config(&draft, state, config_root)?;
            sync_root_props_from_settings(app, &state.shell.settings);
            app.set_dialog_visible(false);
            Ok(())
        }
        DialogKind::HotkeysConfig => {
            confirm_hotkeys_config(state, config_root)?;
            app.set_dialog_visible(false);
            Ok(())
        }
        _ => {
            // Any other Settings-family dialog (e.g. the debug-mode
            // flows the SettingsViewModel handles) goes through the
            // general confirm path, which cannot fail.
            handle_dialog_confirm(app, state, config_root);
            Ok(())
        }
    }
}

/// The `overlay_enabled` value an overlay dialog confirm writes: the row's
/// value when the user changed it from the seed, else the current setting
/// (which the live loop may have flipped since the dialog opened).
fn overlay_enabled_to_write(app: &AppWindow, state: &AppState) -> bool {
    let row = app.get_settings_overlay_enabled();
    if state.dialog.overlay_enabled_seed == Some(row) {
        state.shell.settings.overlay_enabled
    } else {
        row
    }
}

/// Router-friendly variant of `handle_dialog_cancel` used by the
/// shared [`crate::actions::dialogs`] router.
///
/// Re-seeds the settings-family root form props from the settings (see
/// [`sync_root_props_from_settings`]) and discards the drafts that live
/// outside the root props:
///
/// - the controller image selection stages on
///   `SettingsPanelState::active_controller_images`, so cancel reloads
///   `Config/Controllers.json`;
/// - the hotkey binding drafts stage on `AppState::hotkey_drafts`, so
///   cancel re-hydrates them from `Config/Settings.json`.
pub fn handle_dialog_cancel_with_router(app: &AppWindow, state: &mut AppState) {
    let kind = state.dialog.kind;
    match kind {
        DialogKind::ControllerConfig => {
            let config_root = resolve_config_root();
            match d4lc_config::load_controller_config(&config_root) {
                Ok(images) => {
                    state.shell.settings.active_controller_images = images;
                    state.bump_rules_input_revision();
                }
                Err(error) => state.append_log_message(
                    format!("controller config load failed: {error}"),
                    crate::clock::hh_mm_now(),
                ),
            }
        }
        DialogKind::HotkeysConfig => {
            let config_root = resolve_config_root();
            match load_app_settings(&config_root) {
                Ok(settings) => state.hydrate_hotkey_drafts_from_settings(&settings),
                Err(error) => state.append_log_message(
                    format!("settings load failed while discarding hotkey drafts: {error}"),
                    crate::clock::hh_mm_now(),
                ),
            }
        }
        _ => {}
    }
    handle_dialog_cancel(app, state);
    // Re-seed every form prop from the settings the cancel keeps, so a
    // discarded edit never lingers in a prop and no other dialog's props
    // are blanked.
    sync_root_props_from_settings(app, &state.shell.settings);
}

// ── Open-dialog command handlers ─────────────────────────────────────────────

/// Handle the `open-colors-config` callback from the settings screen.
///
/// Seeds the seven `settings-colors-*` root props from
/// `state.shell.settings.default_colors` (normalized through
/// [`d4lc_config::PresetColor::to_string_form`] so the swatch palette matches).
pub fn handle_open_colors_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let _effects = vm.open_colors_config();
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();
    sync_colors_props(app, &state.shell.settings);
    // Route through the shared dialog router so
    // `state.dialog.kind` becomes `ColorsConfig` (the
    // router default is `ColorsConfig`, but we set it
    // explicitly so the routing contract does not depend
    // on the default). Without this call the
    // `dialog-confirm` callback would dispatch as the
    // stale `state.dialog.kind` from the previous
    // dialog and no-op.
    crate::actions::dialogs::open(app, state, DialogKind::ColorsConfig, None);
}

/// Handle the `open-controller-config` callback from the settings screen.
///
/// Seeds the `settings-controller-mode` toggle, reloads the active
/// controller image list from `Config/Controllers.json` (the upstream
/// `SystemPresetManager.LoadControllerConfig` read) into the draft, and
/// projects the available-image rows for the selected system preset +
/// language.
pub fn handle_open_controller_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let _effects = vm.open_controller_config();
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();
    let config_root = resolve_config_root();
    match d4lc_config::load_controller_config(&config_root) {
        Ok(images) => {
            state.shell.settings.active_controller_images = images;
            state.bump_rules_input_revision();
        }
        Err(error) => state.append_log_message(
            format!("controller config load failed: {error}"),
            crate::clock::hh_mm_now(),
        ),
    }
    sync_controller_props(app, &state.shell.settings);
    project_controller_image_rows(app, state);
    // Route through the shared dialog router so
    // `state.dialog.kind` becomes `ControllerConfig` and
    // the shared `dialog-confirm` callback dispatches to
    // the controller-config confirm body. Without this
    // call the confirm path dispatches as the stale
    // default `ColorsConfig` and no-ops.
    crate::actions::dialogs::open(app, state, DialogKind::ControllerConfig, None);
}

/// Project the controller image rows (available images for the selected
/// preset + language, each flagged with its draft membership) and the
/// selected count onto the root props.
///
/// Available images mirror the upstream `LoadControllerImages`
/// enumeration (`SystemPresetManager.cs:155-230`): the `tooltip_gc_*`
/// images of `Images/{preset}/Tooltips/` for the selected language, with
/// the general-image fallback the vendored asset repository already
/// implements. The ordered image list is the same one the runtime's
/// controller-mode template selection consumes, so a selected row is a
/// row the prepared detector can actually use.
pub fn project_controller_image_rows(app: &AppWindow, state: &AppState) {
    let rows = available_controller_images(state);
    let selected = &state.shell.settings.active_controller_images;
    let model: Vec<crate::slint_generated::SettingsControllerImageRow> = rows
        .iter()
        .map(|name| crate::slint_generated::SettingsControllerImageRow {
            file_name: name.as_str().into(),
            selected: selected
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(name)),
        })
        .collect();
    app.set_settings_controller_image_rows(ModelRc::new(VecModel::from(model)));
    app.set_settings_controller_images_count(i32::try_from(selected.len()).unwrap_or(i32::MAX));
}

/// The available controller tooltip images for the selected system preset
/// and language, in the repository's sorted order.
///
/// Two roots are consulted and merged (case-insensitive de-duplication,
/// sorted):
///
/// 1. the config root — extracted user presets under the upstream-shaped
///    `Images\{preset}\Tooltips\` tree, which is where a downloaded preset
///    unpacks;
/// 2. the vendored/installed preset tree beside the application (the same
///    CWD-first / checkout-root seam `system_presets_asset_dir` resolves
///    for the preset lists).
///
/// A root whose preset directory cannot be read is skipped rather than
/// failing the dialog: the list shows whatever root does resolve, and an
/// entirely empty result renders the "no images found" state.
fn available_controller_images(state: &AppState) -> Vec<String> {
    let preset = state.shell.settings.selected_system_preset.clone();
    let language = state.shell.settings.language.clone();
    let mut names: Vec<String> = Vec::new();
    for root in preset_asset_roots() {
        let repo = d4lc_assets::AssetRepository::new(root);
        let Ok(found) = repo.controller_image_names(&preset, &language) else {
            continue;
        };
        for name in found {
            if !name.to_ascii_lowercase().contains("tooltip_gc_") {
                continue;
            }
            if !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

/// The roots that can hold the selected preset's tooltip images: the
/// config root (extracted user presets) plus the vendored/installed tree
/// beside the application when it exists.
fn preset_asset_roots() -> Vec<PathBuf> {
    let mut roots = vec![resolve_config_root()];
    if let Some(dir) = system_presets_asset_dir() {
        // `<root>/assets/original/systempresets` → `<root>`.
        if let Some(root) = dir.parent().and_then(|dir| dir.parent()) {
            let root = root.to_path_buf();
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    roots
}

/// Handle one controller image add ("+") / remove ("−") click.
///
/// Upstream `ControllerConfigViewModel.AddControllerExecute` /
/// `RemoveControllerExecute` mutate the `Controllers.json` list through
/// `SystemPresetManager`; the port stages the same mutation in the dialog
/// draft and writes the file on confirm. The add guard mirrors upstream:
/// adding an image that is already selected is a no-op, and the match is
/// case-insensitive.
pub fn handle_controller_image_selected(
    app: &AppWindow,
    state: &mut AppState,
    file_name: &str,
    selected: bool,
) {
    let name = file_name.trim();
    if name.is_empty() {
        return;
    }
    let images = &mut state.shell.settings.active_controller_images;
    if selected {
        if !images
            .iter()
            .any(|configured| configured.eq_ignore_ascii_case(name))
        {
            images.push(name.to_string());
        }
    } else {
        images.retain(|configured| !configured.eq_ignore_ascii_case(name));
    }
    state.bump_rules_input_revision();
    project_controller_image_rows(app, state);
}

/// Handle the `open-overlay-config` callback from the settings screen.
///
/// Seeds the `settings-overlay-*` root props from
/// `state.shell.settings`.
pub fn handle_open_overlay_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let _effects = vm.open_overlay_config();
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();
    sync_overlay_props(app, &state.shell.settings);
    state.dialog.overlay_enabled_seed = Some(state.shell.settings.overlay_enabled);
    // Route through the shared dialog router so
    // `state.dialog.kind` becomes `OverlayConfig` and the
    // shared `dialog-confirm` callback dispatches to the
    // overlay-config confirm body (which validates
    // `font_size` in `10..=50`, `type_area_offset_left`
    // in `0..=25`, the icon placement in `0..=1000` and the
    // item-power limit in `100..=800`). Without this call the
    // confirm path dispatches as the stale default
    // `ColorsConfig` and no-ops.
    crate::actions::dialogs::open(app, state, DialogKind::OverlayConfig, None);
}

/// Handle the `open-paragon-config` callback from the settings screen.
///
/// Seeds the two paragon toggles plus the collapsed-board switch and the
/// five layout numerics from `state.shell.settings`.
pub fn handle_open_paragon_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let _effects = vm.open_paragon_config();
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();
    sync_paragon_props(app, &state.shell.settings);
    // Route through the shared dialog router so
    // `state.dialog.kind` becomes `ParagonConfig` and the
    // shared `dialog-confirm` callback dispatches to the
    // paragon-config confirm body. Without this call the
    // confirm path dispatches as the stale default
    // `ColorsConfig` and no-ops.
    crate::actions::dialogs::open(app, state, DialogKind::ParagonConfig, None);
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::actions::settings::hotkey_config::handle_open_hotkeys_config;
    use crate::actions::settings::test_support::with_temp_config_root;
    use slint::Model;

    // ── Open handlers set the dialog kind ──────────────────────────
    //
    // The shared `dialog-confirm()` callback dispatches on
    // `state.dialog.kind`; an open handler that only seeded the root
    // props would leave the default `ColorsConfig` kind and make the
    // controller, overlay, paragon and hotkeys confirms no-ops. These
    // tests open each dialog on a real `AppWindow` and check the kind.
    //
    // These tests exercise the Slint setter side of the open
    // handlers, which is otherwise only covered by
    // `cargo check -p d4lc-app --features real-runtime` (the
    // `cargo test -p d4lc-app` build also compiles
    // `main_window.slint` via `slint::include_modules!()`, so
    // `AppWindow::new()` is available here).

    /// Open a Controller dialog and assert the confirm path
    /// dispatches as `ControllerConfig`, not as the default
    /// `ColorsConfig` kind.
    #[test]
    fn handle_open_controller_config_routes_to_controller_kind() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        // The default kind is ColorsConfig; the open handler must
        // overwrite it with ControllerConfig.
        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ColorsConfig
        );

        handle_open_controller_config(&app, &mut state);

        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ControllerConfig
        );
        assert!(state.dialog.error.is_empty());
        assert!(!state.dialog.busy);
    }

    /// Open an Overlay dialog and assert the confirm path
    /// dispatches as `OverlayConfig` (regression for the dialog-kind routing).
    #[test]
    fn handle_open_overlay_config_routes_to_overlay_kind() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ColorsConfig
        );

        handle_open_overlay_config(&app, &mut state);

        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::OverlayConfig
        );
        assert!(state.dialog.error.is_empty());
        assert!(!state.dialog.busy);
    }

    /// Open a Paragon dialog and assert the confirm path
    /// dispatches as `ParagonConfig` (regression for the dialog-kind routing).
    #[test]
    fn handle_open_paragon_config_routes_to_paragon_kind() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ColorsConfig
        );

        handle_open_paragon_config(&app, &mut state);

        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ParagonConfig
        );
        assert!(state.dialog.error.is_empty());
        assert!(!state.dialog.busy);
    }

    /// Open a Colors dialog and assert the confirm path
    /// dispatches as `ColorsConfig`. `ColorsConfig` is the default kind, so
    /// the handler routes explicitly and the contract does not depend on
    /// the default.
    #[test]
    fn handle_open_colors_config_routes_to_colors_kind() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ColorsConfig
        );

        handle_open_colors_config(&app, &mut state);

        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ColorsConfig
        );
        assert!(state.dialog.error.is_empty());
        assert!(!state.dialog.busy);
    }

    /// A confirmed paragon / controller mode must reach the settings-screen
    /// toggles, or the next `settings-changed` reads the stale toggle back
    /// and reverts the confirmed value.
    #[test]
    fn confirmed_modes_survive_the_next_settings_changed() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();

            handle_open_paragon_config(&app, &mut state);
            app.set_settings_paragon_mode_active(true);
            handle_dialog_confirm_with_router(&app, &mut state, config_root).unwrap();
            assert!(app.get_paragon_mode());

            handle_open_controller_config(&app, &mut state);
            app.set_settings_controller_mode(true);
            handle_dialog_confirm_with_router(&app, &mut state, config_root).unwrap();
            assert!(app.get_controller_mode());

            crate::actions::settings::handle_settings_changed(&app, &mut state, config_root);
            assert!(state.shell.settings.paragon_mode_active);
            assert!(state.shell.settings.controller_mode);
            let persisted = d4lc_config::load_app_settings(config_root).unwrap();
            assert!(persisted.paragon_mode_active);
            assert!(persisted.controller_mode);
        });
    }

    /// The always-on-top row applies to the main window on confirm, not
    /// only at the next start.
    #[test]
    fn overlay_confirm_applies_topmost_to_the_window() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            assert!(!app.get_window_topmost());

            handle_open_overlay_config(&app, &mut state);
            app.set_settings_overlay_top_most(true);
            handle_dialog_confirm_with_router(&app, &mut state, config_root).unwrap();
            assert!(app.get_window_topmost());
            assert!(state.shell.settings.overlay_top_most);

            handle_open_overlay_config(&app, &mut state);
            app.set_settings_overlay_top_most(false);
            handle_dialog_confirm_with_router(&app, &mut state, config_root).unwrap();
            assert!(!app.get_window_topmost());
        });
    }

    /// The live loop can flip `overlay_enabled` (overlay menu lock) while the
    /// dialog is open: an untouched row must not revert it, a changed row
    /// still applies.
    #[test]
    fn overlay_confirm_keeps_a_live_flip_unless_the_row_was_changed() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            state.shell.settings.overlay_enabled = true;

            handle_open_overlay_config(&app, &mut state);
            // Menu lock unlocked while the dialog is open.
            state.shell.settings.overlay_enabled = false;
            handle_dialog_confirm_with_router(&app, &mut state, config_root).unwrap();
            assert!(!state.shell.settings.overlay_enabled);

            handle_open_overlay_config(&app, &mut state);
            app.set_settings_overlay_enabled(true);
            handle_dialog_confirm_with_router(&app, &mut state, config_root).unwrap();
            assert!(state.shell.settings.overlay_enabled);
        });
    }

    /// Cancelling one dialog re-seeds the props from the settings instead
    /// of resetting every mirrored prop to a hard-coded default.
    #[test]
    fn cancel_reseeds_props_from_settings_instead_of_defaults() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.shell.settings.paragon_border_size = 4;
        state.shell.settings.item_power_limit = 750;
        state.shell.settings.controller_mode = true;

        handle_open_paragon_config(&app, &mut state);
        handle_open_overlay_config(&app, &mut state);
        app.set_settings_overlay_item_power_limit(300);
        handle_dialog_cancel_with_router(&app, &mut state);

        assert_eq!(app.get_settings_overlay_item_power_limit(), 750);
        assert_eq!(app.get_settings_paragon_border_size(), 4);
        assert!(app.get_settings_controller_mode());
        assert_eq!(state.shell.settings.item_power_limit, 750);
    }

    /// Every settings-dialog open handler seeds the form the
    /// dialog edits from the persisted panel state, so the rendered
    /// controls start on the values a confirm would write back.
    #[test]
    fn settings_dialog_open_handlers_seed_every_row_from_panel_state() {
        with_temp_config_root(|config_root| {
            let app = crate::test_window::new_app_window();
            let mut state = AppState::default_startup();
            state.shell.settings.default_colors.implicit = "#FF8800".to_string();
            state.shell.settings.paragon_border_size = 4;
            state.shell.settings.paragon_node_size = 55;
            state.shell.settings.paragon_node_size_collapsed = 30;
            state.shell.settings.paragon_left_offset_collapsed = -120;
            state.shell.settings.paragon_top_offset_collapsed = 75;
            state.shell.settings.collapsed_paragonboard_enabled = true;
            state.shell.settings.show_overlay_icon = true;
            state.shell.settings.overlay_icon_pos_x = 120;
            state.shell.settings.overlay_icon_pos_y = 340;
            state.shell.settings.item_power_limit_enabled = true;
            state.shell.settings.item_power_limit = 925;
            state.shell.settings.overlay_marker_mode =
                d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
            state.shell.settings.trade_overlay_enabled = false;
            state.shell.settings.socket_detection_enabled = false;
            state.shell.settings.tempered_detection_enabled = false;

            handle_open_colors_config(&app, &mut state);
            assert_eq!(app.get_settings_colors_implicit().as_str(), "#FF8800");
            // A stored color name is seeded in the canonical hex form so
            // the palette swatch can preview it.
            assert_eq!(app.get_settings_colors_normal().as_str(), "#008000");

            handle_open_paragon_config(&app, &mut state);
            assert!(app.get_settings_paragon_collapsed_board());
            assert_eq!(app.get_settings_paragon_border_size(), 4);
            assert_eq!(app.get_settings_paragon_node_size(), 55);
            assert_eq!(app.get_settings_paragon_node_size_collapsed(), 30);
            assert_eq!(app.get_settings_paragon_left_offset_collapsed(), -120);
            assert_eq!(app.get_settings_paragon_top_offset_collapsed(), 75);

            handle_open_overlay_config(&app, &mut state);
            assert!(app.get_settings_overlay_show_icon());
            assert_eq!(app.get_settings_overlay_icon_pos_x(), 120);
            assert_eq!(app.get_settings_overlay_icon_pos_y(), 340);
            assert!(app.get_settings_overlay_item_power_enabled());
            // 925 exceeds the upstream dialog's NumericUpDown maximum
            // (`OverlayConfigView.xaml:115`), so the seed clamps to 800 —
            // the dialog never shows a value its own control cannot set.
            assert_eq!(app.get_settings_overlay_item_power_limit(), 800);
            assert_eq!(
                app.get_settings_overlay_marker_mode().as_str(),
                "Hide Unwanted"
            );
            assert!(!app.get_settings_overlay_trade_overlay());
            assert!(!app.get_settings_overlay_socket_detection());
            assert!(!app.get_settings_overlay_tempered_detection());

            // The controller dialog reloads the selection from
            // `Config/Controllers.json` and projects the available rows for
            // the selected preset.
            d4lc_config::save_controller_config(
                config_root,
                &["tooltip_gc_enus_amulet_0.png".to_string()],
            )
            .expect("seed Controllers.json");
            handle_open_controller_config(&app, &mut state);
            assert_eq!(
                state.shell.settings.active_controller_images,
                vec!["tooltip_gc_enus_amulet_0.png".to_string()]
            );
            assert_eq!(app.get_settings_controller_images_count(), 1);

            // The manager projects seven rows for the persisted bindings.
            handle_open_hotkeys_config(&app, &mut state);
            assert_eq!(app.get_settings_hotkey_rows().row_count(), 7);
        });
    }
}
