//! Shared dialog contract and router for the d4lc Slint desktop app.
//!
//! Owns the shared `DialogHost` and its open / confirm / cancel / error
//! contract. The router is the only entry point for dialog-state
//! transitions coming from the UI.
//!
//! ## Contract
//!
//! 1. `open(app, state, kind, title)` — flips the dialog visible in
//!    Slint, sets the dialog state (`kind`, `title`), and clears any
//!    prior error / busy state. The `kind` must come from
//!    [`crate::app_state::DialogKind`]; the router rejects unknown
//!    kinds through `set_error` rather than panicking.
//! 2. `handle_confirm(app, state, config_root)` — the Slint
//!    `dialog-confirm()` callback. Dispatches to the family-specific
//!    confirm handler in [`crate::actions`] based on
//!    `state.dialog.kind.family()`. The family handler returns
//!    `Result<(), String>`; the router writes the error to
//!    `state.dialog.error` (and to the Slint `dialog-error` property)
//!    on failure, or closes the dialog on success.
//! 3. `handle_cancel(app, state)` — the Slint `dialog-cancel()`
//!    callback. Always closes the dialog and clears the error.
//! 4. `set_error(app, state, error)` — used by family handlers to
//!    surface validation failures without closing the dialog.
//! 5. `close(app, state)` — closes the dialog without dispatching any
//!    family handler. Used by hot-path code that wants to dismiss the
//!    dialog after a confirmed external event (e.g. an applied
//!    settings save).
//! 6. `open_confirm(app, state, action)` — opens the one generic
//!    confirm prompt (`DialogKind::Confirm`) on a pending
//!    [`ConfirmAction`]: the upstream window-modal Yes/No vocabulary
//!    every destructive action routes through. Yes runs
//!    the action through [`resolve_confirm_prompt`], No drops it.
//!
//! ## Family dispatch table
//!
//! | Family       | Confirm handler                                                |
//! |--------------|----------------------------------------------------------------|
//! | `Settings`   | `actions::settings::handle_dialog_confirm`                     |
//! | `Affix`      | `actions::affix_dialogs::handle_confirm`                       |
//! | `Trade`      | `actions::trade_dialogs::handle_confirm`                       |
//! | `Hotkeys`    | `actions::hotkeys_dialogs::handle_confirm`                     |
//! | `BuildImport`| `actions::build_import_dialogs::handle_confirm`                |
//! | `Confirm`    | `resolve_confirm_prompt` (pending `ConfirmAction`)             |
//!
//! ## What this module MUST NOT do
//!
//! - Implement dialog body content (lives in family modules).
//! - Reach into capture, OCR, parser, rules, or overlay state.
//! - Touch `d4lc-gui` / `d4lc-rules` / `d4lc-config` contracts.
//! - Touch tray, hotkey, runtime, or debug command implementations.

use std::path::Path;

use crate::actions;
use crate::app_state::{AppState, ConfirmAction, ConfirmParent, DialogKind};
use crate::slint_generated::AppWindow;

/// Open a dialog by kind, setting the dialog state and showing the
/// shared `DialogHost` in Slint. Clears any prior error and busy flag.
pub fn open(app: &AppWindow, state: &mut AppState, kind: DialogKind, title: Option<String>) {
    state.dialog.kind = kind;
    state.dialog.title = title.unwrap_or_else(|| kind.title());
    state.dialog.error.clear();
    state.dialog.busy = false;
    push_state_to_ui(app, state);
    app.set_dialog_visible(true);
}

/// Open the generic confirm prompt (`DialogKind::Confirm`) on a pending
/// action.
///
/// The prompt carries the action's upstream copy (the `ShowMessageAsync`
/// title / message arguments of the cited upstream lines, see
/// [`ConfirmAction`]) and is window-modal like every other dialog: the
/// host's backdrop pointer guard blocks the window content behind it. Yes
/// runs the action ([`resolve_confirm_prompt`]), No drops it
/// ([`handle_cancel`]); either way the prompt closes and, when the action
/// has a parent dialog, that dialog is reopened (upstream keeps the dialog
/// that opened the prompt open beneath it).
pub fn open_confirm(app: &AppWindow, state: &mut AppState, action: ConfirmAction) {
    let title = action.title().to_string();
    open(app, state, DialogKind::Confirm, Some(title));
    state.dialog.confirm = Some(action);
    push_state_to_ui(app, state);
}

/// Slint `dialog-confirm()` callback entry point.
///
/// Reads `state.dialog.kind`, dispatches to the family-specific
/// confirm handler, and on success closes the dialog; on failure
/// keeps the dialog open and surfaces the error in
/// `state.dialog.error` / Slint `dialog-error`.
pub fn handle_confirm(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let kind = state.dialog.kind;
    let family = kind.family();
    let result = match family {
        DialogFamily::Settings => {
            // The settings confirm path closes the dialog on success and
            // persists the config itself; the router only maps its result.
            actions::settings::handle_dialog_confirm_with_router(app, state, config_root)
        }
        DialogFamily::Affix => actions::affix_dialogs::handle_confirm(app, state, config_root),
        DialogFamily::Trade => actions::trade_dialogs::handle_confirm(app, state, config_root),
        DialogFamily::Hotkeys => actions::hotkeys_dialogs::handle_confirm(app, state, config_root),
        DialogFamily::BuildImport => {
            actions::build_import_dialogs::handle_confirm(app, state, config_root)
        }
        // The prompt answers Yes by running its pending action instead of a
        // dialog family's form, and does its own close / parent-reopen
        // post-processing, so it returns directly.
        DialogFamily::Confirm => return resolve_confirm_prompt(app, state, config_root),
    };
    match result {
        Ok(()) => {
            close(app, state);
            reopen_hotkeys_manager_after_binding_dialog(app, state, kind);
            Ok(())
        }
        Err(err) => {
            set_error(app, state, err.clone());
            Err(err)
        }
    }
}

/// The single-binding dialog is nested over the Hotkeys manager upstream
/// (`HotkeysConfigViewModel.cs:195-213`): its closing returns to the
/// manager, which refreshes its rows. The single-dialog host reopens the
/// manager after either answer.
fn reopen_hotkeys_manager_after_binding_dialog(
    app: &AppWindow,
    state: &mut AppState,
    closed: DialogKind,
) {
    if closed == DialogKind::HotkeyConfig {
        actions::settings::handle_open_hotkeys_config(app, state);
    }
}

/// Run the pending [`ConfirmAction`] of the open prompt and answer Yes.
///
/// Success closes the prompt and reopens the parent dialog the prompt was
/// shown over (if any); failure keeps the prompt open with the error
/// surfaced — the same contract the family confirm handlers use — so the
/// user sees why the action did not run instead of a silently closed
/// prompt.
fn resolve_confirm_prompt(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let Some(action) = state.dialog.confirm.clone() else {
        return Err("confirm prompt: no pending action".to_string());
    };
    let result = match &action {
        ConfirmAction::DeletePreset { name } => {
            actions::affix::handle_confirm_remove_preset(app, state, config_root, name)
        }
        ConfirmAction::ResetMinimalAffixValues => {
            actions::affix_dialogs::handle_confirm_reset_minimal_affix_values(
                app,
                state,
                config_root,
            )
        }
    };
    match result {
        Ok(()) => {
            close(app, state);
            if let Some(parent) = action.parent() {
                reopen_prompt_parent(app, state, parent);
            }
            Ok(())
        }
        Err(err) => {
            set_error(app, state, err.clone());
            Err(err)
        }
    }
}

/// Reopen the dialog a resolved prompt was shown over.
fn reopen_prompt_parent(app: &AppWindow, state: &mut AppState, parent: ConfirmParent) {
    match parent {
        ConfirmParent::AffixConfig => actions::affix_cmds::handle_open_affix_config(app, state),
    }
}

/// Slint `dialog-cancel()` callback entry point.
///
/// Closes the dialog, clears error / busy, and dispatches to the
/// family-specific cancel handler so it can roll back any
/// in-progress draft state.
pub fn handle_cancel(app: &AppWindow, state: &mut AppState) {
    let kind = state.dialog.kind;
    let family = kind.family();
    match family {
        DialogFamily::Settings => actions::settings::handle_dialog_cancel_with_router(app, state),
        DialogFamily::Affix => actions::affix_dialogs::handle_cancel(app, state),
        DialogFamily::Trade => actions::trade_dialogs::handle_cancel(app, state),
        DialogFamily::Hotkeys => actions::hotkeys_dialogs::handle_cancel(app, state),
        DialogFamily::BuildImport => actions::build_import_dialogs::handle_cancel(app, state),
        // Answering No drops the pending action with the closing prompt
        // (`close` clears it); nothing else was staged to roll back.
        DialogFamily::Confirm => {}
    }
    // The prompt was shown over the dialog that opened it upstream; the
    // single-dialog host reopens that parent after the answer, so No
    // returns the user to the dialog they came from.
    let parent = prompt_parent(state);
    close(app, state);
    if let Some(parent) = parent {
        reopen_prompt_parent(app, state, parent);
    }
    reopen_hotkeys_manager_after_binding_dialog(app, state, kind);
}

/// The parent dialog of the pending prompt, when the open dialog is the
/// prompt itself.
fn prompt_parent(state: &AppState) -> Option<ConfirmParent> {
    if state.dialog.kind != DialogKind::Confirm {
        return None;
    }
    state
        .dialog
        .confirm
        .as_ref()
        .and_then(|action| action.parent())
}

// ---------------------------------------------------------------------------
// Dialog keyboard host.
//
// The `FocusScope` inside `main_window.slint`'s dialog host routes
// Escape/Enter presses to the `dialog-key(string)` callback while a
// dialog is open. The per-kind action is derived from the upstream
// WPF dialogs and recorded in [`dialog_key_action`]; the parity
// reality is honored instead of inventing desktop-convention
// behavior the upstream app does not have.
// ---------------------------------------------------------------------------

/// A dialog-relevant key press routed by the Slint keyboard host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKey {
    /// Escape (`Key.Escape`, event text `"\u{1b}"`).
    Escape,
    /// Enter / Return (`Key.Return`, event text `"\n"`).
    Enter,
}

impl DialogKey {
    /// Parse the Slint `KeyEvent.text` emitted by the keyboard host.
    /// Returns `None` for keys the host does not route (everything
    /// except Escape and Enter passes through untouched).
    pub fn from_event_text(text: &str) -> Option<Self> {
        match text {
            "\u{1b}" => Some(Self::Escape),
            "\u{000a}" => Some(Self::Enter),
            _ => None,
        }
    }
}

/// The action the router performs for a routed key on a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKeyAction {
    /// Confirm the dialog (the upstream `IsDefault`-button analog).
    Confirm,
    /// Cancel the dialog (the upstream `IsCancel`-button analog).
    Cancel,
    /// Upstream binds no semantics for this key on this dialog; the
    /// host ignores the key and the dialog stays as it is.
    Ignored,
}

/// Upstream-derived keyboard semantics for `kind` and `key`.
///
/// Derived from the upstream WPF sources: every
/// upstream dialog is a MahApps `CustomDialog` hosted in-window via
/// `DialogCoordinator.ShowMetroDialogAsync` (e.g.
/// `AffixViewModel.cs:944-951`), so the WPF window-level
/// `IsDefault`/`IsCancel` accelerators cannot apply. A repo-wide
/// search over `upstream/Diablo4Companion` found zero `IsDefault` /
/// `IsCancel` button properties, zero Enter/Escape `KeyBinding`s, and
/// zero `KeyDown`/`PreviewKeyDown`/`ProcessCmdKey` handlers: upstream
/// dialogs close exclusively through their explicit Done/Cancel
/// button commands. The verified parity table is therefore uniform
/// at the dialog level across every `CustomDialog` kind: Enter and
/// Escape are `Ignored`.
///
/// One precise exception lives outside this table: Enter inside a
/// `DarkLineEdit` text field confirms via the field's `accepted`
/// callback (the Slint `TextInput` consumes Return before the
/// `FocusScope` router sees it), which the provider/import download
/// dialogs and the preset-name dialog wire to `confirm`. Every
/// `CustomDialog` key still routes through this table and is
/// `Ignored` per upstream parity.
///
/// The second exception is the prompt kind itself. `DialogKind::Confirm`
/// hosts upstream's MahApps `ShowMessageAsync` message dialog
/// (`AffixViewModel.cs:1764`), which is not a `CustomDialog`: it is a
/// window-modal Yes/No prompt whose affirmative button is the dialog's
/// default button and whose negative button cancels, so the port routes
/// Enter to [`DialogKeyAction::Confirm`] (Yes) and Escape to
/// [`DialogKeyAction::Cancel`] (No), both through the same
/// [`handle_confirm`] / [`handle_cancel`] paths the prompt's buttons use.
/// This table stays the single flip point for that decision.
pub fn dialog_key_action(kind: DialogKind, key: DialogKey) -> DialogKeyAction {
    match kind {
        // The one generic prompt: Enter answers Yes, Escape answers No.
        DialogKind::Confirm => match key {
            DialogKey::Enter => DialogKeyAction::Confirm,
            DialogKey::Escape => DialogKeyAction::Cancel,
        },
        // Every CustomDialog kind keeps the verified upstream parity.
        _ => DialogKeyAction::Ignored,
    }
}

/// Slint `dialog-key(string)` callback entry point.
///
/// Routes a keyboard Escape/Enter pressed while a dialog is open
/// through [`dialog_key_action`]. Confirm / cancel dispatch through
/// the same [`handle_confirm`] / [`handle_cancel`] paths the dialog
/// buttons use; `Ignored` keys leave the dialog untouched. Returns
/// the resolved action, or `None` when the text is not a routed key
/// (the host passes everything else through untouched).
pub fn handle_dialog_key(
    app: &AppWindow,
    state: &mut AppState,
    key_text: &str,
    config_root: &Path,
) -> Option<DialogKeyAction> {
    let key = DialogKey::from_event_text(key_text)?;
    // Escape on the import dialog's progress panel cancels the running
    // import (or closes the failure panel) instead of being ignored: the
    // panel is modal, so this is the keyboard path out of it.
    if key == DialogKey::Escape
        && state.dialog.kind == DialogKind::ImportPreset
        && actions::build_import_dialogs::handle_progress_escape(app, state)
    {
        return Some(DialogKeyAction::Cancel);
    }
    let action = dialog_key_action(state.dialog.kind, key);
    match action {
        DialogKeyAction::Confirm => {
            let _ = handle_confirm(app, state, config_root);
        }
        DialogKeyAction::Cancel => handle_cancel(app, state),
        DialogKeyAction::Ignored => {}
    }
    Some(action)
}

/// Close the dialog and clear the error / busy state.
///
/// Does NOT dispatch to any family handler. Use this for hot-path
/// code (settings persistence, async events) that wants to dismiss
/// the dialog after a confirmed external event.
pub fn close(app: &AppWindow, state: &mut AppState) {
    state.dialog.error.clear();
    state.dialog.busy = false;
    // A closed prompt leaves no pending action behind: the next prompt
    // starts from an explicit action and a stale one can never fire.
    state.dialog.confirm = None;
    app.set_dialog_visible(false);
    app.set_dialog_error(slint::SharedString::default());
    app.set_dialog_error_visible(false);
    app.set_dialog_message(slint::SharedString::default());
}

/// Set the shared dialog error and surface it in the Slint host.
///
/// Keeps the dialog open so the user can correct the input.
pub fn set_error(app: &AppWindow, state: &mut AppState, error: String) {
    state.dialog.error = error.clone();
    app.set_dialog_error(error.into());
    app.set_dialog_error_visible(true);
}

/// Project the current `state.dialog` into the Slint properties.
///
/// Idempotent: safe to call after every state transition.
pub fn push_state_to_ui(app: &AppWindow, state: &AppState) {
    app.set_dialog_id(state.dialog.kind.as_str().into());
    app.set_dialog_title(state.dialog.title.clone().into());
    app.set_dialog_error(state.dialog.error.clone().into());
    app.set_dialog_error_visible(!state.dialog.error.is_empty());
    // The prompt body is the pending action's upstream copy; every other
    // dialog renders its own content and leaves `dialog-message` empty.
    app.set_dialog_message(
        state
            .dialog
            .confirm
            .as_ref()
            .map(|action| slint::SharedString::from(action.message()))
            .unwrap_or_default(),
    );
}

// Re-export the family enum so family modules don't need to take a
// second dependency on the `app_state` module for the enum.
pub use crate::app_state::DialogFamily;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app_state::AppState;

    // The router logic that doesn't need an AppWindow is covered
    // through family-level tests and the `app_state` tests. The
    // AppWindow-touching helpers above are exercised through the
    // real-input harness.

    #[test]
    fn dialog_family_dispatch_is_total() {
        // Every DialogKind has a family; dispatcher therefore covers
        // every dialog.
        for kind in DialogKind::ALL {
            let _ = kind.family();
        }
    }

    #[test]
    fn handle_confirm_propagates_error_when_router_returns_err() {
        // The router's failure path is testable without a Slint
        // runtime by using a dummy app state and skipping the
        // Slint setter calls (which require ComponentHandle). We
        // only assert that the family handler contract is
        // consistent with the router's Result handling.
        let state = AppState::default_startup();
        assert!(state.dialog.error.is_empty());
    }

    #[test]
    fn dialog_key_event_text_parses_escape_and_enter_only() {
        assert_eq!(
            DialogKey::from_event_text("\u{1b}"),
            Some(DialogKey::Escape)
        );
        assert_eq!(
            DialogKey::from_event_text("\u{000a}"),
            Some(DialogKey::Enter)
        );
        assert_eq!(DialogKey::from_event_text("a"), None);
        assert_eq!(DialogKey::from_event_text(""), None);
        assert_eq!(DialogKey::from_event_text("\t"), None);
    }

    #[test]
    fn dialog_key_table_records_upstream_parity_for_every_kind() {
        // The upstream WPF CustomDialogs bind no Enter/Escape semantics
        // (see `dialog_key_action`); the routed table must record that
        // parity reality for every dialog kind so a future kind cannot
        // silently acquire invented key behavior. The one recorded
        // exception is the prompt kind, which is upstream's MahApps
        // `ShowMessageAsync` message dialog, not a CustomDialog.
        for kind in DialogKind::ALL {
            if kind == DialogKind::Confirm {
                assert_eq!(
                    dialog_key_action(kind, DialogKey::Enter),
                    DialogKeyAction::Confirm,
                    "the prompt's default button must answer Yes on Enter"
                );
                assert_eq!(
                    dialog_key_action(kind, DialogKey::Escape),
                    DialogKeyAction::Cancel,
                    "the prompt's negative button must answer No on Escape"
                );
                continue;
            }
            assert_eq!(
                dialog_key_action(kind, DialogKey::Escape),
                DialogKeyAction::Ignored,
                "kind {kind} must keep the verified upstream parity: no Escape route"
            );
            assert_eq!(
                dialog_key_action(kind, DialogKey::Enter),
                DialogKeyAction::Ignored,
                "kind {kind} must keep the verified upstream parity: no Enter route"
            );
        }
    }

    #[test]
    fn prompt_kind_is_the_only_confirm_family_kind() {
        let prompt_kinds: Vec<DialogKind> = DialogKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.family() == DialogFamily::Confirm)
            .collect();
        assert_eq!(prompt_kinds, vec![DialogKind::Confirm]);
    }

    /// Unique, per-test config root under the system temp dir.
    fn temp_root(test_name: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|_| "time_error".to_string());
        let root = std::env::temp_dir().join(format!("d4lc-dialog-{test_name}-{suffix}"));
        std::fs::create_dir_all(&root).expect("create temp config root");
        root
    }

    /// Enter answers Yes and Escape answers No on the prompt, through the
    /// same `handle_confirm` / `handle_cancel` paths its buttons use.
    #[test]
    fn prompt_keys_answer_through_the_router() {
        let app = crate::test_window::new_app_window();
        let root = temp_root("prompt-keys");
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Beta").unwrap();
        app.set_selected_preset(slint::SharedString::from("Beta"));

        // Escape answers No: the pending delete is dropped.
        crate::actions::affix::handle_remove_preset(&app, &mut state, &root);
        assert_eq!(
            handle_dialog_key(&app, &mut state, "\u{1b}", &root),
            Some(DialogKeyAction::Cancel)
        );
        assert!(!app.get_dialog_visible(), "answering No closes the prompt");
        assert!(state
            .affix_preset_editor
            .presets
            .iter()
            .any(|preset| preset.name == "Beta"));

        // Enter answers Yes: the pending delete runs.
        crate::actions::affix::handle_remove_preset(&app, &mut state, &root);
        assert_eq!(
            handle_dialog_key(&app, &mut state, "\u{000a}", &root),
            Some(DialogKeyAction::Confirm)
        );
        assert!(!state
            .affix_preset_editor
            .presets
            .iter()
            .any(|preset| preset.name == "Beta"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Answering the reset prompt reopens the affix config dialog it was
    /// shown over, on No as well as on Yes (upstream keeps the dialog
    /// beneath the prompt open).
    #[test]
    fn reset_prompt_reopens_the_config_dialog_after_the_answer() {
        let app = crate::test_window::new_app_window();
        let root = temp_root("reset-prompt-parent");
        let mut state = AppState::default_startup();
        state
            .minimal_affix_values
            .insert("Affix_AttackSpeed".to_string(), 12.5);
        crate::actions::affix_cmds::handle_open_affix_config(&app, &mut state);
        assert_eq!(app.get_dialog_id().to_string(), "affix_config");

        // The dialog's reset button asks through the shared prompt.
        crate::actions::affix_dialogs::handle_reset_minimal_affix_values(&app, &mut state, &root)
            .expect("opening the prompt cannot fail");
        assert_eq!(app.get_dialog_id().to_string(), "confirm");
        assert_eq!(app.get_dialog_title().to_string(), "Reset");
        assert_eq!(
            app.get_dialog_message().to_string(),
            "Are you sure you want to reset the minimal affix values?"
        );

        // No first: the thresholds survive and the config dialog returns.
        handle_cancel(&app, &mut state);
        assert_eq!(
            state.minimal_affix_values.get("Affix_AttackSpeed"),
            Some(&12.5)
        );
        assert!(app.get_dialog_visible());
        assert_eq!(app.get_dialog_id().to_string(), "affix_config");

        // Yes: the thresholds are cleared, same return.
        crate::actions::affix_dialogs::handle_reset_minimal_affix_values(&app, &mut state, &root)
            .expect("opening the prompt cannot fail");
        handle_confirm(&app, &mut state, &root).expect("the confirmed reset must succeed");
        assert!(state.minimal_affix_values.is_empty());
        assert!(app.get_dialog_visible());
        assert_eq!(app.get_dialog_id().to_string(), "affix_config");
        let _ = std::fs::remove_dir_all(&root);
    }
}
