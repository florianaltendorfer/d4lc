//! Hotkeys config dialog handlers and the hotkey backend runtime.

use std::path::Path;

use d4lc_config::{load_app_settings, save_app_settings};
use d4lc_gui::settings::SettingsViewModel;
use slint::{ModelRc, VecModel};

use crate::app_state::{AppState, DialogKind};
use crate::hotkeys::{HotkeyBackendHandle, HotkeyBackendStatus, HotkeyEvent};
use crate::slint_generated::AppWindow;

/// Handle the `open-hotkeys-config` callback from the settings screen.
///
/// Projects the seven binding rows (draft state + last registration
/// outcome per binding) and opens the manager dialog. The manager's
/// per-row confirm path persists the drafts and restarts the hotkey
/// backend, so the rows shown here always reflect the current drafts.
pub fn handle_open_hotkeys_config(app: &AppWindow, state: &mut AppState) {
    let mut vm = SettingsViewModel::new(state.shell.settings.clone());
    let _effects = vm.open_hotkeys_config();
    state.shell.settings = vm.state;
    state.bump_rules_input_revision();
    project_hotkey_rows(app, state);
    // Route through the shared dialog router so
    // `state.dialog.kind` becomes `HotkeysConfig` and the
    // shared `dialog-confirm` callback dispatches to the
    // hotkeys-config confirm body. Without this call the
    // confirm path dispatches as the stale default
    // `ColorsConfig` and no-ops.
    crate::actions::dialogs::open(app, state, DialogKind::HotkeysConfig, None);
}

/// Handle one manager-row enable toggle.
///
/// The toggle edits the in-memory draft only (upstream
/// `HotkeysConfigViewModel` saves on the property set; the port keeps the
/// draft/confirm pattern) and re-projects the rows so the switch state is
/// visible immediately. Confirming the dialog persists the drafts and
/// re-registers the grabs.
pub fn handle_hotkey_row_toggled(app: &AppWindow, state: &mut AppState, name: &str, enabled: bool) {
    if let Some(draft) = state.hotkey_drafts.iter_mut().find(|d| d.name == name) {
        draft.enabled = enabled;
    } else {
        let label = d4lc_config::HOTKEY_TARGETS
            .iter()
            .find(|(target, _, _)| *target == name)
            .map(|(_, label, _)| (*label).to_string())
            .unwrap_or_else(|| name.to_string());
        let binding = d4lc_config::HOTKEY_TARGETS
            .iter()
            .find(|(target, _, _)| *target == name)
            .map(|(_, _, binding)| (*binding).to_string())
            .unwrap_or_default();
        state.hotkey_drafts.push(crate::app_state::HotkeyDraft {
            name: name.to_string(),
            label,
            binding,
            enabled,
        });
    }
    project_hotkey_rows(app, state);
}

/// Handle a manager-row edit click: seed the single-binding dialog for
/// that target and open it.
///
/// Upstream `HotkeysConfigView.xaml:37-111` has one gesture button per
/// binding; the port routes every row here, so the capture dialog is
/// reachable for all seven targets instead of only the first one.
pub fn handle_hotkey_row_edit(app: &AppWindow, state: &mut AppState, name: &str) {
    crate::actions::hotkeys_dialogs::handle_open_hotkey_config_for_target(app, state, Some(name));
}

/// Project the seven hotkey binding rows onto the root model.
///
/// The row set is the canonical upstream target list
/// (`d4lc_config::HOTKEY_TARGETS`, `SettingsD4.cs:99-153`) in declaration
/// order — a hard invariant: the dialog addresses rows by index, and a
/// settings file that carries only a subset of the bindings must still
/// render seven rows (missing entries fall back to the upstream default
/// binding, disabled). Each row carries the last registration outcome from
/// `AppState::hotkey_backend` when the backend reported one.
pub fn project_hotkey_rows(app: &AppWindow, state: &AppState) {
    let model: Vec<crate::slint_generated::SettingsHotkeyRow> = d4lc_config::HOTKEY_TARGETS
        .iter()
        .map(|(name, label, default_binding)| {
            let draft = state.hotkey_drafts.iter().find(|d| d.name == *name);
            let registration = state
                .hotkey_backend
                .registrations
                .iter()
                .find(|registration| registration.name == *name);
            let (status, problem) = match registration {
                Some(registration) => hotkey_status_label(&registration.status),
                None => ("not registered".to_string(), false),
            };
            crate::slint_generated::SettingsHotkeyRow {
                name: (*name).into(),
                label: (*label).into(),
                binding: draft
                    .map(|draft| draft.binding.clone())
                    .unwrap_or_else(|| (*default_binding).to_string())
                    .into(),
                enabled: draft.map(|draft| draft.enabled).unwrap_or(false),
                status: status.into(),
                problem,
            }
        })
        .collect();
    app.set_settings_hotkey_rows(ModelRc::new(VecModel::from(model)));
}

/// User-facing label for one binding's registration outcome plus whether
/// the row should render in the warning tone.
///
/// Mirrors the upstream per-binding reporting: a successful grab is
/// silent upstream, a `HotkeyAlreadyRegisteredException` raises the
/// "already registered by another application" notice and portal failures
/// raise the warning message (`MainWindowViewModel.cs:212-221,443-450`).
/// The port keeps the structured status rows
/// (`crate::hotkeys::HotkeyRegistrationStatus`) and renders one short
/// label per binding.
pub fn hotkey_status_label(status: &crate::hotkeys::HotkeyRegistrationStatus) -> (String, bool) {
    use crate::hotkeys::HotkeyRegistrationStatus as Status;
    match status {
        Status::Registered => ("registered".to_string(), false),
        Status::Disabled => ("disabled".to_string(), false),
        Status::Conflict { detail } => (format!("conflict: {detail}"), true),
        Status::ParseError { detail } => (format!("invalid: {detail}"), true),
        Status::Unavailable { reason } => (format!("unavailable: {reason}"), true),
        Status::Refused { detail } => (format!("refused: {detail}"), true),
        Status::Unconfirmed { detail } => (format!("unconfirmed: {detail}"), true),
    }
}

/// Process-wide hotkey backend runtime.
///
/// The backend handle and its event receiver live outside `AppState`
/// because `HotkeyBackendHandle` owns a join handle (not `Clone` /
/// `PartialEq`, which `AppState` derives). `wire_hotkey_actions` installs
/// the startup backend here, the drain timer reads the current receiver
/// from here, and [`restart_hotkey_backend`] stops the running backend and
/// installs a fresh one after a binding edit — the upstream
/// `UpdateHotkeysRequestMessage` → `InitKeyBindings` re-registration
/// (`MainWindowViewModel.cs:204-208,425-432`) without an app restart.
static HOTKEY_RUNTIME: std::sync::Mutex<Option<HotkeyRuntime>> = std::sync::Mutex::new(None);

struct HotkeyRuntime {
    handle: HotkeyBackendHandle,
    receiver: std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<HotkeyEvent>>>,
}

fn lock_hotkey_runtime() -> std::sync::MutexGuard<'static, Option<HotkeyRuntime>> {
    match HOTKEY_RUNTIME.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Install the running hotkey backend after a successful start.
///
/// Any previously installed backend is stopped first, so a restart can
/// reuse this entry point.
pub fn install_hotkey_runtime(
    handle: HotkeyBackendHandle,
    receiver: std::sync::mpsc::Receiver<HotkeyEvent>,
) {
    let mut runtime = lock_hotkey_runtime();
    if let Some(previous) = runtime.take() {
        previous.handle.stop();
    }
    *runtime = Some(HotkeyRuntime {
        handle,
        receiver: std::sync::Arc::new(std::sync::Mutex::new(receiver)),
    });
}

/// The receiver the 50 ms drain timer must read this tick, if a backend
/// thread is running.
pub fn current_hotkey_receiver(
) -> Option<std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<HotkeyEvent>>>> {
    lock_hotkey_runtime()
        .as_ref()
        .map(|runtime| std::sync::Arc::clone(&runtime.receiver))
}

/// Stop the running hotkey backend (if any) and start a fresh one from the
/// supplied settings, recording the new registration status on `state`.
///
/// This is the port's live re-registration seam: the previous connection is
/// stopped FIRST so the re-taken grabs of the edited binding set never race
/// it (X11 passive grabs are exclusive across clients, so the registration
/// probe of a still-running old backend gets `BadAccess` on every unchanged
/// combo); on Wayland the portal bind request runs again and its
/// per-binding outcome (registered / refused / unconfirmed) is reported.
/// When no backend thread can start (no session, no enabled
/// binding, no portal) the status still carries the per-binding rows, so
/// the dialog can say exactly why nothing is active.
pub fn restart_hotkey_backend(
    state: &mut AppState,
    settings: &d4lc_config::AppSettings,
) -> HotkeyBackendStatus {
    restart_hotkey_backend_with(state, || crate::hotkeys::start_hotkey_backend(settings))
}

/// [`restart_hotkey_backend`] with the start step injected, so a test can
/// observe that the previous runtime is stopped before the new
/// registration probe runs.
fn restart_hotkey_backend_with(
    state: &mut AppState,
    start: impl FnOnce() -> (
        HotkeyBackendStatus,
        Option<HotkeyBackendHandle>,
        Option<std::sync::mpsc::Receiver<HotkeyEvent>>,
    ),
) -> HotkeyBackendStatus {
    // Stop the previous backend BEFORE probing the new bindings: the old
    // connection's exclusive X11 grabs would fail every unchanged combo in
    // the probe, so a no-op Apply would find nothing registered, start no
    // replacement thread, and leave the session with no hotkeys at all.
    // Upstream re-registers from a clean state
    // (`MainWindowViewModel.cs:204-208`).
    if let Some(previous) = lock_hotkey_runtime().take() {
        previous.handle.stop();
    }
    let (status, handle, receiver) = start();
    if let (Some(handle), Some(receiver)) = (handle, receiver) {
        install_hotkey_runtime(handle, receiver);
    }
    state.set_hotkey_backend_status(status.clone());
    status
}

/// Persist the hotkey drafts and re-register the global hotkeys.
///
/// The manager dialog's confirm path: write `state.hotkey_drafts` onto
/// `Config/Settings.json`, then restart the hotkey backend so the edited
/// bindings are grabbed without an app restart. A refused settings save
/// (newer schema file) fails the confirm and leaves the running backend
/// untouched.
pub(super) fn confirm_hotkeys_config(
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let mut config =
        load_app_settings(config_root).map_err(|e| format!("Failed to load app settings: {e}"))?;
    state.shell.settings.apply_to_config(&mut config);
    state.apply_hotkey_drafts_to_settings(&mut config);
    save_app_settings(config_root, &config)
        .map_err(|e| format!("Failed to save app settings: {e}"))?;

    // Re-register from the file that was just written, so a binding edit
    // applies to the live session (upstream re-registers on save).
    let persisted = load_app_settings(config_root)
        .map_err(|e| format!("Failed to reload app settings: {e}"))?;
    let armed: Vec<String> = persisted
        .hotkey_config
        .bindings
        .iter()
        .filter(|binding| binding.enabled)
        .map(|binding| format!("{}={}", binding.name, binding.binding))
        .collect();
    let status = restart_hotkey_backend(state, &persisted);
    state.append_log_message(
        format!(
            "hotkey bindings saved; re-registration reported {} registration row(s) for [{}]",
            status.registrations.len(),
            armed.join(", ")
        ),
        crate::clock::hh_mm_now(),
    );
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::actions::settings::dialogs::handle_dialog_cancel_with_router;
    use crate::actions::settings::test_support::{
        load_persisted, with_temp_config_root, ENV_MUTEX,
    };
    use slint::Model;
    use std::sync::Mutex;

    #[test]
    fn confirm_hotkeys_config_persists_hotkey_drafts_and_re_registers() {
        with_temp_config_root(|config_root| {
            let mut state = crate::app_state::AppState::default_startup();
            state.hotkey_drafts.push(crate::app_state::HotkeyDraft {
                name: "SwitchPreset".to_string(),
                label: "Switch preset".to_string(),
                binding: "Ctrl+Shift+F5".to_string(),
                enabled: true,
            });

            confirm_hotkeys_config(&mut state, config_root)
                .expect("hotkeys confirm should persist");

            let persisted = load_persisted(config_root);
            let binding = persisted
                .hotkey_config
                .find_by_name("SwitchPreset")
                .expect("persisted hotkey binding");
            assert_eq!(binding.binding, "Ctrl+Shift+F5");
            assert!(binding.enabled);

            // The confirm path re-registers the grabs without
            // an app restart and records the per-binding outcome. The seven
            // target rows must mirror the persisted bindings in upstream
            // order, with the edited binding carried verbatim, so the
            // manager dialog can report what the backend did per row.
            let rows = state.hotkey_backend.registrations.len();
            assert_eq!(
                rows, 7,
                "the re-registration report must cover all seven bindings"
            );
            let names: Vec<&str> = state
                .hotkey_backend
                .registrations
                .iter()
                .map(|registration| registration.name.as_str())
                .collect();
            assert_eq!(
                names,
                vec![
                    "SwitchPreset",
                    "SwitchOverlay",
                    "TakeScreenshot",
                    "ToggleController",
                    "ToggleOverlay",
                    "ToggleDebugLockScreencapture",
                    "ToggleTuningPrismHints",
                ]
            );
            let edited = state
                .hotkey_backend
                .registrations
                .iter()
                .find(|registration| registration.name == "SwitchPreset")
                .expect("SwitchPreset registration row");
            assert_eq!(edited.binding, "Ctrl+Shift+F5");
            assert!(
                state
                    .logging
                    .messages
                    .iter()
                    .any(|message| message.message.contains("hotkey bindings saved")),
                "the re-registration must be observable on the log surface"
            );
        });
    }

    #[test]
    fn restart_stops_the_previous_backend_before_starting_the_new_one() {
        // The hotkey runtime is process-wide; serialize with the other test
        // that restarts it (`confirm_hotkeys_config_...` holds this same
        // guard for its whole body) so this injected start cannot observe
        // another test's stop.
        let _serial = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // X11 passive grabs are exclusive across clients, so the
        // replacement backend's registration probe is refused
        // (`BadAccess`) for every combo the previous connection still
        // holds. The previous backend must therefore be stopped before the
        // new probe runs: otherwise an Apply with unchanged bindings finds
        // nothing registered, starts no replacement thread, and leaves the
        // session without hotkeys. The start step is injected so the
        // interleaving is observable without a live X session.
        let events: std::sync::Arc<Mutex<Vec<&'static str>>> =
            std::sync::Arc::new(Mutex::new(Vec::new()));

        // A previous runtime whose stop is observable: `stop()` waits for
        // the thread, and the thread records its exit.
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_shutdown = std::sync::Arc::clone(&shutdown);
        let thread_events = std::sync::Arc::clone(&events);
        let thread = std::thread::spawn(move || {
            while !thread_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            thread_events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("stop");
        });
        let (_sender, receiver) = std::sync::mpsc::channel::<HotkeyEvent>();
        install_hotkey_runtime(
            HotkeyBackendHandle {
                shutdown,
                thread: Some(thread),
            },
            receiver,
        );

        let mut state = crate::app_state::AppState::default_startup();
        let status = restart_hotkey_backend_with(&mut state, || {
            events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("start");
            (HotkeyBackendStatus::default(), None, None)
        });

        assert_eq!(
            *events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec!["stop", "start"],
            "the previous backend must be stopped before the new registration probe runs"
        );
        assert_eq!(
            status,
            HotkeyBackendStatus::default(),
            "the failed start reports its own status"
        );
        assert_eq!(state.hotkey_backend, HotkeyBackendStatus::default());
        assert!(
            current_hotkey_receiver().is_none(),
            "a failed start reports no backend instead of keeping the stopped one"
        );
    }

    #[test]
    fn hotkey_status_labels_distinguish_conflicts_from_success() {
        use crate::hotkeys::HotkeyRegistrationStatus as Status;

        let (label, problem) = hotkey_status_label(&Status::Registered);
        assert_eq!(label, "registered");
        assert!(!problem);

        let (label, problem) = hotkey_status_label(&Status::Disabled);
        assert_eq!(label, "disabled");
        assert!(
            !problem,
            "a disabled binding is the user's choice, not a conflict"
        );

        let (label, problem) = hotkey_status_label(&Status::Conflict {
            detail: "another client already owns Ctrl+F5".to_string(),
        });
        assert!(label.starts_with("conflict:"));
        assert!(problem, "a conflict renders in the warning tone");

        for status in [
            Status::ParseError {
                detail: "bad key".to_string(),
            },
            Status::Unavailable {
                reason: "no DISPLAY".to_string(),
            },
            Status::Refused {
                detail: "consent denied".to_string(),
            },
            Status::Unconfirmed {
                detail: "shortcut missing".to_string(),
            },
        ] {
            let (_, problem) = hotkey_status_label(&status);
            assert!(problem, "{status:?} must render in the warning tone");
        }
    }

    #[test]
    fn hotkey_row_toggle_edits_the_draft_and_survives_cancel_hydration() {
        with_temp_config_root(|config_root| {
            let app = crate::test_window::new_app_window();
            let mut state = crate::app_state::AppState::default_startup();
            state.hydrate_hotkey_drafts_from_settings(&d4lc_config::AppSettings::default());

            // The manager row switch edits the draft (upstream
            // HotkeysConfigViewModel saves on the property set; the port
            // keeps the draft until the dialog confirms).
            handle_hotkey_row_toggled(&app, &mut state, "ToggleTuningPrismHints", true);
            let draft = state
                .hotkey_drafts
                .iter()
                .find(|draft| draft.name == "ToggleTuningPrismHints")
                .expect("tuning-prism draft");
            assert!(draft.enabled);
            assert_eq!(draft.binding, "Ctrl+F7");
            assert_eq!(
                state
                    .hotkey_drafts
                    .iter()
                    .filter(|draft| draft.enabled)
                    .count(),
                1
            );

            // The projected rows mirror the drafts in upstream order and
            // always carry seven entries (the dialog indexes by position).
            let rows = app.get_settings_hotkey_rows();
            assert_eq!(rows.row_count(), 7);
            let projected = app
                .get_settings_hotkey_rows()
                .row_data(6)
                .expect("seventh row");
            assert_eq!(projected.name.as_str(), "ToggleTuningPrismHints");
            assert_eq!(projected.label.as_str(), "Toggle tuning prism hints");
            assert_eq!(projected.binding.as_str(), "Ctrl+F7");
            assert!(projected.enabled);
            assert_eq!(projected.status.as_str(), "not registered");

            // Cancel discards the draft: the cancel path re-hydrates from
            // the persisted settings file.
            crate::actions::dialogs::open(&app, &mut state, DialogKind::HotkeysConfig, None);
            handle_dialog_cancel_with_router(&app, &mut state);
            assert!(
                state.hotkey_drafts.iter().all(|draft| !draft.enabled),
                "a cancelled manager edit must not linger in the drafts"
            );
            let _ = config_root;
        });
    }

    #[test]
    fn hotkey_row_edit_opens_the_single_binding_dialog_for_that_target() {
        let app = crate::test_window::new_app_window();
        let mut state = crate::app_state::AppState::default_startup();
        state.hydrate_hotkey_drafts_from_settings(&d4lc_config::AppSettings::default());

        handle_hotkey_row_edit(&app, &mut state, "ToggleOverlay");
        assert_eq!(app.get_dialog_id().as_str(), "hotkey_config");
        assert_eq!(app.get_hotkey_config_target().as_str(), "ToggleOverlay");
        assert_eq!(app.get_hotkey_config_binding().as_str(), "Ctrl+F12");
        assert!(!app.get_hotkey_config_enabled());

        // An unknown target keeps the staged form, so the dialog never opens
        // on an empty one.
        handle_hotkey_row_edit(&app, &mut state, "NotABinding");
        assert_eq!(app.get_hotkey_config_target().as_str(), "ToggleOverlay");
    }

    #[test]
    fn open_hotkey_config_keeps_the_staged_target_across_reopens() {
        let app = crate::test_window::new_app_window();
        let mut state = crate::app_state::AppState::default_startup();
        state.hydrate_hotkey_drafts_from_settings(&d4lc_config::AppSettings::default());

        // The target-less invoker (the AffixScreen button) opens on the
        // first binding when nothing is staged.
        app.set_hotkey_config_target("".into());
        crate::actions::hotkeys_dialogs::handle_open_hotkey_config(&app, &mut state);
        assert_eq!(app.get_hotkey_config_target().as_str(), "SwitchPreset");

        // A staged target re-opens the same binding (the manager's row
        // buttons and any future invoker share this path).
        app.set_hotkey_config_target("TakeScreenshot".into());
        crate::actions::hotkeys_dialogs::handle_open_hotkey_config(&app, &mut state);
        assert_eq!(app.get_hotkey_config_target().as_str(), "TakeScreenshot");
        assert_eq!(app.get_hotkey_config_binding().as_str(), "Ctrl+F10");
    }

    /// Open a Hotkeys dialog and assert the confirm path
    /// dispatches as `HotkeysConfig` (regression for the dialog-kind routing).
    #[test]
    fn handle_open_hotkeys_config_routes_to_hotkeys_kind() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::ColorsConfig
        );

        handle_open_hotkeys_config(&app, &mut state);

        assert_eq!(
            state.dialog.kind,
            crate::app_state::DialogKind::HotkeysConfig
        );
        assert!(state.dialog.error.is_empty());
        assert!(!state.dialog.busy);
    }

    /// The hotkey drafts the manager edits are discarded on cancel: the
    /// cancel path re-hydrates them from `Config/Settings.json`.
    #[test]
    fn hotkey_manager_cancel_restores_persisted_bindings() {
        with_temp_config_root(|config_root| {
            let app = crate::test_window::new_app_window();
            let mut state = AppState::default_startup();
            state.hydrate_hotkey_drafts_from_settings(&d4lc_config::AppSettings::default());
            confirm_hotkeys_config(&mut state, config_root).expect("persist the default bindings");

            // A staged edit, then a cancel.
            handle_hotkey_row_toggled(&app, &mut state, "SwitchPreset", true);
            assert!(state
                .hotkey_drafts
                .iter()
                .any(|draft| draft.name == "SwitchPreset" && draft.enabled));
            crate::actions::dialogs::open(&app, &mut state, DialogKind::HotkeysConfig, None);
            handle_dialog_cancel_with_router(&app, &mut state);

            let persisted = load_persisted(config_root);
            assert!(
                persisted
                    .hotkey_config
                    .bindings
                    .iter()
                    .all(|binding| !binding.enabled),
                "the rejected edit must not reach the persisted bindings"
            );
            assert!(state.hotkey_drafts.iter().all(|draft| !draft.enabled));
        });
    }
}
