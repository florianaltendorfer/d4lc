use super::*;

// ── Element find helpers (Rust side) — loud failures, zero silent skips ───

/// Exactly one element by qualified id (`ComponentName::element-id`).
pub(super) fn find_by_element_id(app: &AppWindow, id: &str) -> ElementHandle {
    let mut found = ElementHandle::find_by_element_id(app, id);
    let Some(first) = found.next() else {
        panic!(
            "real-input GUI harness: no element matches element id {id:?} — the UI is missing \
             the named element, is not instantiated, or was built without Slint debug info \
             (build.rs `with_debug_info`)"
        );
    };
    assert!(
        found.next().is_none(),
        "real-input GUI harness: expected exactly one element with id {id:?}"
    );
    first
}

/// One element by `accessible-label`. Slint propagates an element's
/// accessible label to its text children, so several tree entries can carry
/// the same label; they share the same screen region, so the first (outer)
/// match is the click target.
fn find_by_accessible_label(app: &AppWindow, label: &str) -> ElementHandle {
    expect_found(
        ElementHandle::find_by_accessible_label(app, label),
        &format!("accessible label {label:?}"),
    )
}

fn expect_found(mut found: impl Iterator<Item = ElementHandle>, what: &str) -> ElementHandle {
    let Some(first) = found.next() else {
        panic!(
            "real-input GUI harness: no element matches {what} — the UI is missing the matching \
             id/label, is not instantiated, or was built without Slint debug info \
             (build.rs `with_debug_info`)"
        );
    };
    first
}

/// Click an element through the real hit-testing pipeline: press and
/// release at the element center, mock-time paced (synchronous variant
/// of `ElementHandle::single_click`).
pub(super) fn click_on_ui(label: &str) {
    let label = label.to_string();
    on_ui_ctx(&format!("click_on_ui({label:?})"), move |app| {
        find_by_accessible_label(app, &label)
            .mock_single_click(slint::platform::PointerEventButton::Left);
    });
}

/// `click_on_ui` for dynamic (non-'static) labels, e.g. entry rows
/// labeled by preset data ("Remove Life").
pub(super) fn click_on_label(label: &str) {
    let label = label.to_string();
    on_ui_ctx(&format!("click_on_label({label:?})"), move |app| {
        find_by_accessible_label(app, &label)
            .mock_single_click(slint::platform::PointerEventButton::Left);
    });
}

/// Role-aware click: among all elements carrying `label`, click the
/// first one whose accessible role is a control (button / switch /
/// checkbox). Plain first-match clicking is ambiguous when a static
/// text shares the label with a control (e.g. the "Runes" panel
/// header vs. the "Runes" category switch, or the "Affixes" page
/// title vs. its switch) — a click on static text is a no-op.
pub(super) fn click_control_by_label(label: &'static str) {
    on_ui_ctx(&format!("click_control_by_label({label:?})"), move |app| {
        let target = ElementHandle::find_by_accessible_label(app, label)
            .find(|element| {
                matches!(
                    element.accessible_role(),
                    Some(
                        i_slint_backend_testing::AccessibleRole::Button
                            | i_slint_backend_testing::AccessibleRole::Switch
                            | i_slint_backend_testing::AccessibleRole::Checkbox
                    )
                )
            })
            .unwrap_or_else(|| {
                panic!(
                    "coverage matrix: no control (button/switch/checkbox) carries the \
                     accessible label {label:?}"
                )
            });
        target.mock_single_click(slint::platform::PointerEventButton::Left);
    });
}

pub(super) fn read_ui<T: Send + 'static>(f: impl FnOnce(&AppWindow) -> T + Send + 'static) -> T {
    on_ui_ctx("read_ui", f)
}

pub(super) fn dialog_state() -> (bool, String) {
    on_ui_ctx("dialog_state", |app| {
        (app.get_dialog_visible(), app.get_dialog_id().to_string())
    })
}

/// Assert that closing `dialog_id` left no dialog open. The single
/// hotkey binding dialog is nested over the hotkeys manager upstream
/// (`HotkeysConfigViewModel.cs:195-213`), so closing it, by Cancel or
/// by Apply, shows the manager again; that one is asserted and then
/// closed with Done.
pub(super) fn assert_dialog_closed(dialog_id: &str, message: &str) {
    if dialog_id == "hotkey_config" {
        let (visible, id) = dialog_state();
        assert!(
            visible && id == "hotkeys_config",
            "{message}: closing the single binding dialog must return to the hotkeys \
             manager (saw visible={visible}, id={id:?})"
        );
        click_on_ui("Done");
    }
    assert!(!dialog_state().0, "{message}");
}

/// Find a button-role element by its accessible label through MCP
/// queries (the MCP surface has no find-by-label, so this composes
/// `query_element_descendants` + `get_element_properties`).
pub(super) fn find_button_by_label(client: &McpClient, root: McpHandle, label: &str) -> McpHandle {
    let buttons = client
        .query_element_descendants(root, descendants_with_role("Button"), true)
        .unwrap_or_else(|error| harness_panic("query Button descendants", &error));
    buttons
        .into_iter()
        .find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessibleLabel") == Some(label)
                })
        })
        .unwrap_or_else(|| {
            panic!("real-input GUI harness: no button with accessible label {label:?} found")
        })
}

/// Loud findability probe: panics when no element carries `label`.
pub(super) fn assert_label_findable(label: &str) {
    let label = label.to_string();
    read_ui(move |app| {
        let matches = ElementHandle::find_by_accessible_label(app, &label).count();
        assert!(
            matches > 0,
            "coverage matrix: no element carries the accessible label {label:?} — \
             the findability pass regressed or the element is not instantiated"
        );
    });
}

/// Count elements carrying an accessible label; 0 is a valid,
/// asserted result here (unlike `assert_label_findable`).
pub(super) fn label_count(label: &str) -> usize {
    let label = label.to_string();
    read_ui(move |app| ElementHandle::find_by_accessible_label(app, &label).count())
}

/// Find a TextInput descendant by its placeholder. The DarkLineEdit
/// exposes the placeholder as its accessible label, while the std
/// LineEdit (rename dialog) exposes it as the accessible placeholder
/// text — both are matched.
pub(super) fn find_text_input(client: &McpClient, root: McpHandle, placeholder: &str) -> McpHandle {
    let text_inputs = client
        .query_element_descendants(root, descendants_with_role("TextInput"), true)
        .unwrap_or_else(|error| harness_panic("query TextInput descendants", &error));
    text_inputs
        .into_iter()
        .find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessibleLabel") == Some(placeholder)
                        || str_property(&properties, "accessiblePlaceholderText")
                            == Some(placeholder)
                })
        })
        .unwrap_or_else(|| {
            panic!("affix-surface: no TextInput with placeholder {placeholder:?} found")
        })
}

/// Read the current text of an input through the MCP accessible
/// value (the same property a screen reader announces).
pub(super) fn input_value(client: &McpClient, root: McpHandle, placeholder: &str) -> String {
    let handle = find_text_input(client, root, placeholder);
    let properties = client
        .get_element_properties(handle)
        .unwrap_or_else(|error| harness_panic("read input properties", &error));
    str_property(&properties, "accessibleValue")
        .unwrap_or_default()
        .to_string()
}

/// Type one character at a time into the focused input through the
/// real key pipeline.
pub(super) fn type_text(window: McpHandle, client: &McpClient, text: &str) {
    for character in text.chars() {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| harness_panic("type into the trade form input", &error));
    }
}
