//! Global hotkeys: binding parser, X11 and Wayland registration, and the
//! dispatch of fired hotkeys to the action handlers.
//!
//! ## Upstream evidence
//!
//! - `SettingsD4.cs:93-139` — hotkey defaults (7 bindings, all disabled by
//!   default, including `KeyBindingConfigToggleTuningPrismHints`, Ctrl+F7).
//! - `MainWindowViewModel.cs:282-317` — handlers that forward to MVVM messages.
//! - `MainWindowViewModel.cs:353-439` — `InitKeyBindings()` registration loop.
//!
//! ## Flow
//!
//! 1. A binding string (e.g. `"Ctrl+F5"`, `"Ctrl+Shift+Tab"`) is parsed into
//!    a normalized `ParsedBinding { modifiers, key }`.
//! 2. `start_hotkey_backend` registers every enabled binding — a passive X11
//!    grab on the root window, or the GlobalShortcuts portal on Wayland —
//!    and reports a `HotkeyBackendStatus` (capability plus per-binding
//!    success / conflict / unavailable / parse error).
//! 3. Fired hotkeys arrive as `HotkeyEvent`s and `drain_hotkey_events`
//!    dispatches them to the action handlers on the UI thread.
//!
//! ## Behaviour matrix
//!
//! | `XDG_SESSION_TYPE` | `DISPLAY` env | `RustConnection::connect` | enabled binding | per-binding result                                 |
//! |--------------------|---------------|---------------------------|-----------------|----------------------------------------------------|
//! | `wayland`          | any           | n/a                       | none enabled    | `Capability::WaylandPortalUnavailable`, all bindings → `Disabled` (portal not probed — nothing to register) |
//! | `wayland`          | any           | n/a                       | any             | GlobalShortcuts portal path (see the Wayland section below) |
//! | unset / other      | unset         | n/a                       | any             | `Capability::X11Unavailable`, all bindings → `Unavailable{no-display}` |
//! | unset / other      | set           | connect fails             | any             | `Capability::X11Unavailable`, all bindings → `Unavailable{connect: ...}` |
//! | unset / other      | set           | ok                        | parse error     | `ParseError{...}` recorded for that binding only   |
//! | unset / other      | set           | ok                        | grab ok         | `Registered`                                       |
//! | unset / other      | set           | ok                        | grab `Access`   | `Conflict{...}` for that binding (other clients may own the combo) |
//!
//! ## Wayland: GlobalShortcuts portal backend
//!
//! On `XDG_SESSION_TYPE=wayland` the registration path is the
//! `org.freedesktop.portal.GlobalShortcuts` portal via
//! `ashpd::desktop::global_shortcuts` (feature `wayland-hotkeys`,
//! default-on; ashpd + zbus are pure Rust over the session bus —
//! no C deps). The portal is the supported third-party mechanism on
//! Plasma 6 (kglobalaccel runs inside KWin), GNOME 48+, and
//! Hyprland; sway and vanilla wlroots have **no** GlobalShortcuts
//! portal backend and report a structured
//! `WaylandPortalUnavailable` naming the missing piece.
//!
//! - Trigger strings are derived from the parsed binding via
//!   `parsed_binding_to_portal_trigger` (XDG accelerator string,
//!   GTK-style modifiers: `"<Control>F5"`).
//! - The **first** registration triggers the compositor's
//!   user-confirmation dialog (portal contract). The dialog blocks
//!   the bind call, which therefore runs synchronously on the main
//!   thread before the Slint event loop starts. Refused / cancelled
//!   dialogs surface as per-binding `Refused` rows; a bind response that
//!   omits a shortcut surfaces `Unconfirmed`. Nothing is reported
//!   `Registered` without portal evidence.
//! - `Activated` portal events are translated into the same
//!   `HotkeyEvent` channel as the X11 path.
//! - The portal interaction sits behind the `GlobalShortcutsPortal` trait
//!   so tests inject deterministic outcomes without a session bus.
//!
//! GNOME caveat (surfaced in-app in the hotkey config dialog): users
//! manage the resulting bindings in GNOME Settings, and the app does not
//! observe compositor-side edits.
//!
//! ## Limits
//!
//! - The X11 grab is passive on the **root** window. Some window managers
//!   re-route key events before they reach the root (e.g. KWin global
//!   shortcuts or GNOME shortcut overrides) and can still swallow the combo.
//! - A binding that fails to grab is reported as `Conflict` or `ParseError`
//!   in `HotkeyBackendStatus::registrations`, never as `Registered`.

mod backend;
mod binding;
mod dispatch;
#[cfg(test)]
#[allow(clippy::expect_used)]
mod test_support;
mod wayland;
mod x11;

pub use backend::{
    start_hotkey_backend, HotkeyBackendCapability, HotkeyBackendHandle, HotkeyBackendStatus,
    HotkeyEvent, HotkeyRegistrationStatus,
};
pub use dispatch::drain_hotkey_events;
#[cfg(test)]
pub use dispatch::{dispatch_hotkey_event, handle_switch_preset};
