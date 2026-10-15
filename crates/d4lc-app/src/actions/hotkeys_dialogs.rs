//! Hotkeys dialog confirm / cancel handlers.
//!
//! The per-dialog confirm / cancel / open handlers for the
//! hotkey-family dialog (`HotkeyConfig`, the single-binding capture
//! dialog) and the target-routing entry the Hotkeys manager uses. The
//! dialog family router dispatches to these handlers based on
//! `state.dialog.kind`.
//!
//! ## Scope
//!
//! The confirm body validates the binding string format, stores the draft
//! in `AppState::hotkey_drafts`, round-trips the updated draft through
//! `d4lc_config::load_app_settings` / `save_app_settings`, and restarts
//! the hotkey backend so the edited grab is registered without an app
//! restart. Upstream re-registers on every edit the same way
//! (`UpdateHotkeysRequestMessage` → `MainWindowViewModel.InitKeyBindings`,
//! `MainWindowViewModel.cs:204-208`, `:380-441`); the Linux registration
//! itself (X11 passive `XGrabKey` grabs, Wayland GlobalShortcuts portal)
//! lives in `crates/d4lc-app/src/hotkeys.rs`, and the per-binding outcome
//! is reported by the Hotkeys manager dialog.
//!
//! See [`crate::actions::dialogs`] for the shared contract.

use std::path::Path;

use d4lc_config::{load_app_settings, save_app_settings};

use crate::app_state::{AppState, DialogKind, HotkeyDraft};
use crate::slint_generated::AppWindow;

/// All known upstream hotkey binding action names
/// (`SettingsD4.cs:93-139`).
///
/// The dialog open handler uses this list to seed the
/// read-only `hotkey-config-target` field. When the user opens
/// the dialog with no specific target (e.g. from the
/// build-import toolbar), we default to the first binding.
pub const KNOWN_HOTKEY_TARGETS: &[(&str, &str)] = &[
    ("SwitchPreset", "Switch preset"),
    ("SwitchOverlay", "Switch overlay"),
    ("TakeScreenshot", "Take screenshot"),
    ("ToggleController", "Toggle controller mode"),
    ("ToggleOverlay", "Toggle overlay on/off"),
    ("ToggleDebugLockScreencapture", "Debug lock capture"),
    // Seventh upstream binding (SettingsD4.cs line 131).
    ("ToggleTuningPrismHints", "Toggle tuning prism hints"),
];

/// Upstream default binding of `target` (`SettingsD4.cs:99-153`), used when
/// seeding a draft for a target that has no entry in
/// `state.hotkey_drafts`. Empty for a name outside the seven known targets.
fn default_binding(target: &str) -> String {
    d4lc_config::HOTKEY_TARGETS
        .iter()
        .find(|(name, _, _)| *name == target)
        .map(|(_, _, binding)| (*binding).to_string())
        .unwrap_or_default()
}

/// Handle the `open-hotkey-config()` callback.
///
/// Seeds the root in-out props from `state.hotkey_drafts` for the
/// requested target. The manager dialog's per-row edit buttons pass their
/// target through [`handle_open_hotkey_config_for_target`]; the
/// target-less invoker (the AffixScreen's single "Hotkey" button, which
/// cannot carry a parameter) uses the target already staged on
/// `hotkey-config-target` when it names a known binding, and otherwise
/// falls back to the first known binding (upstream's first row is
/// `SwitchPreset`). If no draft exists for the target, we seed the default
/// binding + disabled (matching the upstream "all disabled by default"
/// default).
pub fn handle_open_hotkey_config(app: &AppWindow, state: &mut AppState) {
    let staged = app.get_hotkey_config_target().to_string();
    let target = if staged.is_empty() {
        None
    } else {
        Some(staged.as_str())
    };
    handle_open_hotkey_config_for_target(app, state, target);
}

/// Handle an open request for one specific binding target.
///
/// `target` is the stable action name (`"SwitchPreset"`, …) — the manager
/// dialog passes the row's name. An unknown name keeps the binding staged
/// on the form when that one is known, else the first known binding, so
/// the capture dialog always opens on a real binding. The upstream manager has one gesture button per binding
/// (`HotkeysConfigView.xaml:37-111`); the port routes every row here.
pub fn handle_open_hotkey_config_for_target(
    app: &AppWindow,
    state: &mut AppState,
    target: Option<&str>,
) {
    let requested = target
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .and_then(|name| {
            KNOWN_HOTKEY_TARGETS
                .iter()
                .find(|(known, _)| *known == name)
                .copied()
        });

    // Default: the target currently staged on the form when it names a
    // known binding (so a re-open keeps the last selection), else the
    // first known target.
    let (target, label) = requested
        .or_else(|| {
            let staged = app.get_hotkey_config_target().to_string();
            KNOWN_HOTKEY_TARGETS
                .iter()
                .find(|(known, _)| *known == staged)
                .copied()
        })
        .or_else(|| KNOWN_HOTKEY_TARGETS.first().copied())
        .unwrap_or(("SwitchPreset", "Switch preset"));

    let existing = state
        .hotkey_drafts
        .iter()
        .find(|d| d.name == target)
        .cloned();
    let (binding, enabled) = match existing {
        Some(d) => (d.binding, d.enabled),
        None => (default_binding(target), false),
    };
    let _ = label;

    app.set_hotkey_config_target(target.into());
    app.set_hotkey_config_binding(binding.into());
    app.set_hotkey_config_enabled(enabled);

    crate::actions::dialogs::open(app, state, DialogKind::HotkeyConfig, None);
}

/// Confirm the `HotkeyConfig` dialog.
///
/// Validates the binding string, stores a `HotkeyDraft` in
/// `AppState::hotkey_drafts`, writes **only this target's binding** into
/// `Config/Settings.json` (drafts the manager has staged for other rows
/// stay pending until the manager confirms), and restarts the hotkey
/// backend so the edited grab is registered without an app restart
/// (upstream `UpdateHotkeysRequestMessage` →
/// `MainWindowViewModel.InitKeyBindings`, `MainWindowViewModel.cs:204-208`,
/// `:380-441`). The per-binding outcome of the re-registration is recorded
/// on `AppState::hotkey_backend` and rendered by the Hotkeys config
/// manager dialog, which the dialog router reopens afterwards: upstream
/// shows this dialog nested over the manager
/// (`HotkeysConfigViewModel.cs:195-213`).
///
/// Returns `Ok(())` on success (the router closes the dialog)
/// or `Err(msg)` on validation / persistence failure (the
/// router keeps the dialog open and surfaces the error through
/// the shared `dialog-error` prop).
pub fn handle_confirm(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let target = app.get_hotkey_config_target().to_string();
    if target.is_empty() {
        return Err("No hotkey action selected.".to_string());
    }
    let binding = app.get_hotkey_config_binding().to_string();
    let enabled = app.get_hotkey_config_enabled();
    store_binding(state, &target, &binding, enabled, config_root)?;
    clear_hotkey_config_props(app);
    Ok(())
}

/// The headless body of [`handle_confirm`]: validate, stage the draft,
/// persist this target's binding alone and re-register.
fn store_binding(
    state: &mut AppState,
    target: &str,
    binding: &str,
    enabled: bool,
    config_root: &Path,
) -> Result<(), String> {
    validate_binding_format(binding)?;

    // Resolve the display label from the known targets table.
    let label = KNOWN_HOTKEY_TARGETS
        .iter()
        .find(|(name, _)| *name == target)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| target.to_string());

    let draft = HotkeyDraft {
        name: target.to_string(),
        label: label.clone(),
        binding: binding.to_string(),
        enabled,
    };

    // Upsert: replace any existing draft with the same name.
    if let Some(existing) = state.hotkey_drafts.iter_mut().find(|d| d.name == target) {
        *existing = draft;
    } else {
        state.hotkey_drafts.push(draft);
    }

    // Persist through the existing d4lc_config load/save path. The on-disk
    // settings file stays authoritative; only this target's entry changes.
    // The re-registration below runs from the file that was just written.
    let mut settings = load_app_settings(config_root)
        .map_err(|error| format!("failed to load app settings: {error}"))?;
    settings.hotkey_config.upsert(d4lc_config::HotkeyBinding {
        name: target.to_string(),
        label,
        binding: binding.to_string(),
        enabled,
    });
    save_app_settings(config_root, &settings)
        .map_err(|error| format!("failed to save app settings: {error}"))?;

    // Live re-registration: the running backend is stopped
    // and a fresh one is started from the persisted bindings, so the
    // edited grab applies immediately instead of at the next launch.
    let persisted = load_app_settings(config_root)
        .map_err(|error| format!("failed to reload app settings: {error}"))?;
    let status = crate::actions::settings::restart_hotkey_backend(state, &persisted);
    let outcome = status
        .registrations
        .iter()
        .find(|registration| registration.name == target)
        .map(|registration| crate::actions::settings::hotkey_status_label(&registration.status).0)
        .unwrap_or_else(|| "no registration row reported".to_string());
    eprintln!(
        "[d4lc] hotkey_config: stored draft for '{target}' binding='{binding}' enabled={enabled} \
         (re-registration: {outcome})"
    );
    Ok(())
}

/// Cancel the `HotkeyConfig` dialog.
///
/// Clears the root in-out props so a subsequent open starts
/// from a clean seed.
pub fn handle_cancel(app: &AppWindow, _state: &AppState) {
    clear_hotkey_config_props(app);
}

fn clear_hotkey_config_props(app: &AppWindow) {
    app.set_hotkey_config_target(slint::SharedString::default());
    app.set_hotkey_config_binding(slint::SharedString::default());
    app.set_hotkey_config_enabled(false);
}

/// Validate the binding string format.
///
/// Accepts a non-empty string of the form
/// `(Modifier+)*Key` where Modifier is one of `Ctrl`, `Alt`,
/// `Shift`, `Meta` (case-insensitive) and Key is a single
/// token. Whitespace is trimmed before validation.
///
/// This is a deliberately loose format check — the
/// HotkeyConfig dialog is a draft / validation UI only, not
/// the OS registration path. Real key-event capture (when
/// added) will use the X11 keycode path in
/// `crates/d4lc-app/src/hotkeys.rs`.
pub fn validate_binding_format(binding: &str) -> Result<(), String> {
    let trimmed = binding.trim();
    if trimmed.is_empty() {
        return Err("Hotkey binding must not be empty.".to_string());
    }
    if !trimmed.contains('+') {
        return Err(
            "Hotkey binding must include at least one modifier (e.g. 'Ctrl+F5').".to_string(),
        );
    }
    let parts: Vec<&str> = trimmed.split('+').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err("Hotkey binding has empty modifier or key.".to_string());
    }
    // Last token is the key. Require non-empty + at least one
    // character that is not a modifier.
    let last = parts.last().copied().unwrap_or("");
    if last.is_empty() {
        return Err("Hotkey binding must end with a key.".to_string());
    }
    // Modifiers (everything except the last) must be one of
    // the known modifier names. Case-insensitive.
    for modifier in &parts[..parts.len() - 1] {
        let lc = modifier.to_ascii_lowercase();
        if !matches!(lc.as_str(), "ctrl" | "alt" | "shift" | "meta" | "super") {
            return Err(format!(
                "Unknown modifier '{modifier}' in hotkey binding; \
                 expected one of Ctrl, Alt, Shift, Meta, Super."
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::app_state::{DialogFamily, DialogKind};

    #[test]
    fn hotkeys_family_has_exactly_one_kind() {
        let kinds: Vec<DialogKind> = DialogKind::ALL
            .iter()
            .copied()
            .filter(|k| k.family() == DialogFamily::Hotkeys)
            .collect();
        assert_eq!(kinds, vec![DialogKind::HotkeyConfig]);
    }

    #[test]
    fn validate_binding_accepts_canonical_form() {
        assert!(validate_binding_format("Ctrl+F5").is_ok());
        assert!(validate_binding_format("Ctrl+Shift+F1").is_ok());
        assert!(validate_binding_format("alt+F12").is_ok());
    }

    #[test]
    fn validate_binding_rejects_empty() {
        assert!(validate_binding_format("").is_err());
        assert!(validate_binding_format("   ").is_err());
    }

    #[test]
    fn validate_binding_rejects_no_modifier() {
        assert!(validate_binding_format("F5").is_err());
    }

    #[test]
    fn validate_binding_rejects_unknown_modifier() {
        assert!(validate_binding_format("Hyper+F5").is_err());
    }

    #[test]
    fn validate_binding_rejects_empty_segments() {
        assert!(validate_binding_format("Ctrl++F5").is_err());
        assert!(validate_binding_format("+F5").is_err());
    }

    #[test]
    fn known_targets_have_seven_entries() {
        // Matches the 7 bindings in `SettingsD4.cs:93-139`.
        assert_eq!(KNOWN_HOTKEY_TARGETS.len(), 7);
        assert!(KNOWN_HOTKEY_TARGETS
            .iter()
            .any(|(name, _)| *name == "ToggleTuningPrismHints"));
    }

    fn temp_config_root(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn unsaved_target_seeds_its_own_upstream_default_binding() {
        // `SettingsD4.cs:99-153`: every target has its own default; the old
        // single fallback (`Ctrl+F5`) was only right for SwitchPreset.
        for (name, _, binding) in d4lc_config::HOTKEY_TARGETS {
            assert_eq!(default_binding(name), *binding, "{name}");
        }
        assert_eq!(default_binding("ToggleTuningPrismHints"), "Ctrl+F7");
        assert_eq!(default_binding("ToggleOverlay"), "Ctrl+F12");
        assert_eq!(default_binding("NotATarget"), "");
    }

    #[test]
    fn confirming_one_binding_writes_only_that_target() {
        let root = temp_config_root("d4lc-test-hotkey-single-save");
        let mut state = AppState::default_startup();
        state.hydrate_hotkey_drafts_from_settings(&d4lc_config::AppSettings::default());
        // A manager edit that is staged but not confirmed.
        state
            .hotkey_drafts
            .iter_mut()
            .find(|draft| draft.name == "SwitchPreset")
            .unwrap()
            .binding = "Ctrl+Shift+F5".to_string();

        store_binding(&mut state, "ToggleOverlay", "Ctrl+Shift+F12", false, &root).unwrap();

        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert_eq!(
            persisted
                .hotkey_config
                .find_by_name("ToggleOverlay")
                .unwrap()
                .binding,
            "Ctrl+Shift+F12"
        );
        assert_eq!(
            persisted
                .hotkey_config
                .find_by_name("SwitchPreset")
                .unwrap()
                .binding,
            "Ctrl+F5",
            "another target's staged draft must not reach the file"
        );
        // The staged draft itself is untouched.
        assert_eq!(
            state
                .hotkey_drafts
                .iter()
                .find(|draft| draft.name == "SwitchPreset")
                .unwrap()
                .binding,
            "Ctrl+Shift+F5"
        );

        assert!(store_binding(&mut state, "ToggleOverlay", "F12", false, &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn single_dialog_opens_on_its_target_default_and_returns_to_the_manager() {
        let app = crate::test_window::new_app_window();
        let root = temp_config_root("d4lc-test-hotkey-manager-reopen");
        let mut state = AppState::default_startup();

        handle_open_hotkey_config_for_target(&app, &mut state, Some("ToggleOverlay"));
        assert_eq!(app.get_hotkey_config_binding().as_str(), "Ctrl+F12");
        assert_eq!(state.dialog.kind, DialogKind::HotkeyConfig);

        // Confirm returns to the manager.
        crate::actions::dialogs::handle_confirm(&app, &mut state, &root).unwrap();
        assert_eq!(state.dialog.kind, DialogKind::HotkeysConfig);
        assert!(app.get_dialog_visible());

        // So does cancel.
        handle_open_hotkey_config_for_target(&app, &mut state, Some("ToggleOverlay"));
        crate::actions::dialogs::handle_cancel(&app, &mut state);
        assert_eq!(state.dialog.kind, DialogKind::HotkeysConfig);
        assert!(app.get_dialog_visible());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn upsert_replaces_existing_draft() {
        let mut state = AppState::default_startup();
        let draft = HotkeyDraft {
            name: "SwitchPreset".to_string(),
            label: "Switch preset".to_string(),
            binding: "Ctrl+F5".to_string(),
            enabled: false,
        };
        state.hotkey_drafts.push(draft);
        // Replace via the same code path the handler uses.
        let existing = state
            .hotkey_drafts
            .iter_mut()
            .find(|d| d.name == "SwitchPreset");
        if let Some(existing) = existing {
            existing.binding = "Ctrl+F9".to_string();
            existing.enabled = true;
        }
        assert_eq!(state.hotkey_drafts.len(), 1);
        assert_eq!(state.hotkey_drafts[0].binding, "Ctrl+F9");
        assert!(state.hotkey_drafts[0].enabled);
    }
}
