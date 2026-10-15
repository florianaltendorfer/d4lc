//! Wayland global hotkeys through the GlobalShortcuts portal.

use std::sync::Arc;

use super::backend::{
    HotkeyBackendCapability, HotkeyBackendHandle, HotkeyBackendStatus, HotkeyEvent,
    HotkeyRegistration, HotkeyRegistrationStatus, HotkeyRequest,
};
use super::binding::{parse_binding, ModifierSet, NamedKey, ParsedBinding};

/// True when the runtime session is Wayland per `XDG_SESSION_TYPE`.
/// This is the same gate the X11 path uses — the two backends are
/// mutually exclusive by session type.
pub(super) fn is_wayland_session() -> bool {
    crate::display_backend::wayland_display_available()
}

/// One shortcut handed to the GlobalShortcuts portal
/// `BindShortcuts` request. `id` is the stable action name (the
/// same id comes back on `Activated` events); `trigger` is the XDG
/// accelerator string derived from the parsed binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortalShortcut {
    pub id: String,
    /// The binding string the shortcut was derived from (kept for
    /// the per-registration report row).
    pub binding: String,
    /// Human-readable description the compositor consent dialog
    /// shows. The stable action name doubles as the description so
    /// the portal surface matches the config identifiers.
    pub description: String,
    pub trigger: String,
}

/// Why a portal interaction failed. `BackendMissing` is the
/// structured "this compositor has no GlobalShortcuts portal"
/// state (sway / vanilla wlroots / no xdg-desktop-portal service);
/// it must surface as `WaylandPortalUnavailable`, never as fake
/// registrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PortalBindFailure {
    BackendMissing {
        detail: String,
    },
    /// The user refused the consent dialog / cancelled the request.
    Cancelled {
        detail: String,
    },
    /// The portal exists but the interaction failed for another
    /// reason (transport error, unexpected response, …).
    Other {
        detail: String,
    },
}

/// The injectable GlobalShortcuts portal boundary.
///
/// The production implementation ([`portal_impl::AshpdGlobalShortcutsPortal`])
/// wraps `ashpd::desktop::global_shortcuts` behind `zbus::block_on`;
/// tests inject deterministic outcomes so the dispatch and
/// status contracts are testable without a session bus.
/// The portal is a live-path capability: real-session evidence
/// comes only from the opt-in `#[ignore]` test (this machine is
/// X11; nothing live is claimed in the default suites).
pub(crate) trait GlobalShortcutsPortal: Send {
    /// Bind all supplied shortcuts. Returns the ids the portal
    /// reported as bound; a requested id missing from the response
    /// is reported `Unconfirmed`, a cancelled consent dialog maps
    /// to [`PortalBindFailure::Cancelled`].
    fn bind(&mut self, shortcuts: &[PortalShortcut]) -> Result<Vec<String>, PortalBindFailure>;
    /// Block until the next `Activated` event and return its
    /// shortcut id. `None` means the activation stream is
    /// unavailable or ended (the event loop exits).
    fn next_activated(&mut self) -> Option<String>;
}

/// Derive the GlobalShortcuts portal trigger string from a parsed
/// binding, in the XDG accelerator format ("shortcuts" portal
/// spec): GTK-style modifier names + keysym key name, e.g.
/// `"<Control>F5"`, `"<Control><Shift>Tab"`.
///
/// The live acceptance of this exact spelling by each compositor
/// backend is **assumed** until a real Wayland session validates
/// it; a backend that cannot parse the trigger degrades
/// to an unset trigger in its consent dialog, not to a fake
/// registration.
pub fn parsed_binding_to_portal_trigger(binding: &ParsedBinding) -> String {
    let mut out = String::new();
    if binding.modifiers.contains(ModifierSet::CTRL) {
        out.push_str("<Control>");
    }
    if binding.modifiers.contains(ModifierSet::ALT) {
        out.push_str("<Alt>");
    }
    if binding.modifiers.contains(ModifierSet::SHIFT) {
        out.push_str("<Shift>");
    }
    if binding.modifiers.contains(ModifierSet::SUPER) {
        out.push_str("<Super>");
    }
    // `NUMLOCK` is deliberately not expressible as a portal
    // accelerator modifier; the upstream defaults never use it.
    out.push_str(&named_key_to_portal_key(binding.key));
    out
}

/// Map a [`NamedKey`] to its keysym-style portal key name (the XDG
/// accelerator format uses the X keysym names: `"Return"`,
/// `"Page_Up"`, `"BackSpace"`, …).
fn named_key_to_portal_key(key: NamedKey) -> String {
    match key {
        NamedKey::F1 => "F1".to_string(),
        NamedKey::F2 => "F2".to_string(),
        NamedKey::F3 => "F3".to_string(),
        NamedKey::F4 => "F4".to_string(),
        NamedKey::F5 => "F5".to_string(),
        NamedKey::F6 => "F6".to_string(),
        NamedKey::F7 => "F7".to_string(),
        NamedKey::F8 => "F8".to_string(),
        NamedKey::F9 => "F9".to_string(),
        NamedKey::F10 => "F10".to_string(),
        NamedKey::F11 => "F11".to_string(),
        NamedKey::F12 => "F12".to_string(),
        // Canonical accelerator spelling for letters is the
        // lowercase key (Shift is a modifier, not the case).
        NamedKey::Letter(c) => c.to_ascii_lowercase().to_string(),
        NamedKey::Digit(d) => d.to_string(),
        NamedKey::Tab => "Tab".to_string(),
        NamedKey::Enter => "Return".to_string(),
        NamedKey::Space => "space".to_string(),
        NamedKey::Escape => "Escape".to_string(),
        NamedKey::Backspace => "BackSpace".to_string(),
        NamedKey::Insert => "Insert".to_string(),
        NamedKey::Delete => "Delete".to_string(),
        NamedKey::Home => "Home".to_string(),
        NamedKey::End => "End".to_string(),
        NamedKey::PageUp => "Page_Up".to_string(),
        NamedKey::PageDown => "Page_Down".to_string(),
        NamedKey::Up => "Up".to_string(),
        NamedKey::Down => "Down".to_string(),
        NamedKey::Left => "Left".to_string(),
        NamedKey::Right => "Right".to_string(),
        NamedKey::Minus => "minus".to_string(),
        NamedKey::Equal => "equal".to_string(),
        NamedKey::Comma => "comma".to_string(),
        NamedKey::Period => "period".to_string(),
        NamedKey::Slash => "slash".to_string(),
        NamedKey::Backslash => "backslash".to_string(),
        NamedKey::Semicolon => "semicolon".to_string(),
        NamedKey::Apostrophe => "apostrophe".to_string(),
        NamedKey::BracketLeft => "bracketleft".to_string(),
        NamedKey::BracketRight => "bracketright".to_string(),
        NamedKey::Grave => "grave".to_string(),
    }
}

/// Classify the requests for the portal path: disabled bindings and
/// parse failures become finished rows (`Some`) immediately; enabled,
/// parseable bindings leave an empty slot (`None`) plus a bindable
/// portal shortcut carried with its slot index so the per-binding
/// report preserves the request order. The empty slots are filled by
/// [`finish_wayland_bind`] / [`wayland_unavailable_status`].
fn classify_wayland_requests(
    requests: &[HotkeyRequest],
) -> (
    Vec<Option<HotkeyRegistration>>,
    Vec<(usize, PortalShortcut)>,
) {
    let mut rows: Vec<Option<HotkeyRegistration>> = Vec::with_capacity(requests.len());
    let mut bindable = Vec::new();
    for (idx, req) in requests.iter().enumerate() {
        if !req.enabled {
            rows.push(Some(HotkeyRegistration {
                name: req.name.clone(),
                binding: req.binding.clone(),
                status: HotkeyRegistrationStatus::Disabled,
            }));
            continue;
        }
        match parse_binding(&req.binding) {
            Ok(parsed) => {
                rows.push(None);
                bindable.push((
                    idx,
                    PortalShortcut {
                        id: req.name.clone(),
                        binding: req.binding.clone(),
                        description: req.name.clone(),
                        trigger: parsed_binding_to_portal_trigger(&parsed),
                    },
                ));
            }
            Err(e) => rows.push(Some(HotkeyRegistration {
                name: req.name.clone(),
                binding: req.binding.clone(),
                status: HotkeyRegistrationStatus::ParseError {
                    detail: e.to_string(),
                },
            })),
        }
    }
    (rows, bindable)
}

/// Fill the bindable slots and flatten to the final per-binding
/// report. Every `None` slot has exactly one bindable entry by
/// construction ([`classify_wayland_requests`]).
fn assemble_wayland_rows(
    mut rows: Vec<Option<HotkeyRegistration>>,
    bindable_rows: Vec<(usize, HotkeyRegistration)>,
) -> Vec<HotkeyRegistration> {
    debug_assert_eq!(
        rows.iter().filter(|slot| slot.is_none()).count(),
        bindable_rows.len(),
        "every empty slot must have exactly one bindable outcome"
    );
    for (idx, row) in bindable_rows {
        rows[idx] = Some(row);
    }
    rows.into_iter().flatten().collect()
}

/// Status for a Wayland session where nothing was bindable: all
/// rows are `Disabled` / `ParseError` and the portal was never
/// probed (no session-bus touch for the default-disabled state).
fn wayland_not_probed_status(rows: Vec<Option<HotkeyRegistration>>) -> HotkeyBackendStatus {
    HotkeyBackendStatus {
        capability: HotkeyBackendCapability::WaylandPortalUnavailable,
        unavailable_reason: Some(
            "no enabled Wayland hotkey bindings; the GlobalShortcuts portal was not probed"
                .to_string(),
        ),
        registrations: assemble_wayland_rows(rows, Vec::new()),
    }
}

/// Structured unavailable status with the bindable rows flattened to
/// `Unavailable`. Used when the portal backend is missing (sway /
/// vanilla wlroots), the portal service is unreachable, or the
/// `wayland-hotkeys` build feature is disabled — never with fake
/// registrations.
fn wayland_unavailable_status(
    rows: Vec<Option<HotkeyRegistration>>,
    bindable: Vec<(usize, PortalShortcut)>,
    detail: String,
) -> HotkeyBackendStatus {
    let bindable_rows = bindable
        .into_iter()
        .map(|(idx, shortcut)| {
            (
                idx,
                HotkeyRegistration {
                    name: shortcut.id.clone(),
                    binding: shortcut.binding.clone(),
                    status: HotkeyRegistrationStatus::Unavailable {
                        reason: detail.clone(),
                    },
                },
            )
        })
        .collect();
    HotkeyBackendStatus {
        capability: HotkeyBackendCapability::WaylandPortalUnavailable,
        unavailable_reason: Some(detail),
        registrations: assemble_wayland_rows(rows, bindable_rows),
    }
}

/// Bind through the injected portal and fill the bindable rows with
/// the per-binding outcome. The capability is
/// `WaylandPortalAvailable`: the portal demonstrably exists; per-row
/// statuses carry the bind result.
fn finish_wayland_bind(
    rows: Vec<Option<HotkeyRegistration>>,
    bindable: Vec<(usize, PortalShortcut)>,
    portal: &mut dyn GlobalShortcutsPortal,
) -> HotkeyBackendStatus {
    let shortcuts: Vec<PortalShortcut> = bindable.iter().map(|(_, s)| s.clone()).collect();
    let outcome = portal.bind(&shortcuts);
    let bindable_rows = bindable
        .into_iter()
        .map(|(idx, shortcut)| {
            let status = match &outcome {
                Ok(bound_ids) if bound_ids.iter().any(|id| id == &shortcut.id) => {
                    HotkeyRegistrationStatus::Registered
                }
                Ok(_) => HotkeyRegistrationStatus::Unconfirmed {
                    detail: "the portal bind response did not include this shortcut; the binding is unconfirmed"
                        .to_string(),
                },
                Err(PortalBindFailure::Cancelled { detail }) => {
                    HotkeyRegistrationStatus::Refused { detail: detail.clone() }
                }
                Err(PortalBindFailure::BackendMissing { detail })
                | Err(PortalBindFailure::Other { detail }) => {
                    HotkeyRegistrationStatus::Unavailable { reason: detail.clone() }
                }
            };
            (
                idx,
                HotkeyRegistration {
                    name: shortcut.id.clone(),
                    binding: shortcut.binding.clone(),
                    status,
                },
            )
        })
        .collect();
    HotkeyBackendStatus {
        capability: HotkeyBackendCapability::WaylandPortalAvailable,
        unavailable_reason: None,
        registrations: assemble_wayland_rows(rows, bindable_rows),
    }
}

/// Full portal status for the supplied requests through the
/// injected boundary. Production entry points go through
/// [`probe_wayland_portal`]; tests call this directly with a fake.
#[cfg(test)]
fn wayland_portal_bind_status(
    requests: &[HotkeyRequest],
    portal: &mut dyn GlobalShortcutsPortal,
) -> HotkeyBackendStatus {
    let (rows, bindable) = classify_wayland_requests(requests);
    if bindable.is_empty() {
        return wayland_not_probed_status(rows);
    }
    finish_wayland_bind(rows, bindable, portal)
}

/// Production portal probe: classify, skip the session bus entirely
/// when nothing is bindable, otherwise open the real GlobalShortcuts
/// portal, bind, and hand the portal back so the caller can run the
/// `Activated` event loop when at least one binding registered.
pub(super) fn probe_wayland_portal(
    requests: &[HotkeyRequest],
) -> (HotkeyBackendStatus, Option<Box<dyn GlobalShortcutsPortal>>) {
    let (rows, bindable) = classify_wayland_requests(requests);
    if bindable.is_empty() {
        return (wayland_not_probed_status(rows), None);
    }
    #[cfg(feature = "wayland-hotkeys")]
    {
        let mut portal = match portal_impl::AshpdGlobalShortcutsPortal::open() {
            Ok(portal) => Box::new(portal) as Box<dyn GlobalShortcutsPortal>,
            Err(PortalBindFailure::BackendMissing { detail })
            | Err(PortalBindFailure::Other { detail }) => {
                return (wayland_unavailable_status(rows, bindable, detail), None);
            }
            // `open` never cancels — a consent dialog only appears on
            // bind. Kept exhaustive for the non_exhaustive-proof enum.
            Err(PortalBindFailure::Cancelled { detail }) => {
                return (wayland_unavailable_status(rows, bindable, detail), None);
            }
        };
        let status = finish_wayland_bind(rows, bindable, portal.as_mut());
        let has_registered = status
            .registrations
            .iter()
            .any(|r| matches!(r.status, HotkeyRegistrationStatus::Registered));
        let portal_out = if has_registered { Some(portal) } else { None };
        (status, portal_out)
    }
    #[cfg(not(feature = "wayland-hotkeys"))]
    {
        (
            wayland_unavailable_status(
                rows,
                bindable,
                "the wayland-hotkeys build feature is not enabled in this build".to_string(),
            ),
            None,
        )
    }
}

/// Start the Wayland portal backend: registration status plus, when
/// at least one binding registered, the persistent `Activated` event
/// loop on a dedicated thread feeding the shared `HotkeyEvent` mpsc.
pub(super) fn start_wayland_portal_backend(
    requests: &[HotkeyRequest],
) -> (
    HotkeyBackendStatus,
    Option<HotkeyBackendHandle>,
    Option<std::sync::mpsc::Receiver<HotkeyEvent>>,
) {
    let (status, portal) = probe_wayland_portal(requests);
    let Some(portal) = portal else {
        return (status, None, None);
    };

    let (tx, rx) = std::sync::mpsc::channel::<HotkeyEvent>();
    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);

    let thread = std::thread::Builder::new()
        .name("d4lc-hotkey-portal-loop".to_string())
        .spawn(move || run_wayland_portal_event_loop(portal, tx, shutdown_clone))
        .ok();

    let handle = thread.map(|t| HotkeyBackendHandle {
        shutdown,
        thread: Some(t),
    });
    (status, handle, Some(rx))
}

/// Background-thread body for the portal path: block on the next
/// `Activated` signal and forward its shortcut id as a
/// [`HotkeyEvent`]. Exits when the shared shutdown flag flips
/// (observable only between activations — the portal call blocks
/// inside the DBus signal stream, so unlike the X11 loop this one
/// cannot poll for the flag; production handles live for the
/// process lifetime via the leaked handle) or when the activation
/// stream ends.
///
/// A re-registration therefore never waits on this thread:
/// [`HotkeyBackendHandle::stop`] bounds the wait and detaches it,
/// and the detached loop exits on the next activation at the
/// latest (a closed `HotkeyEvent` receiver also ends it).
fn run_wayland_portal_event_loop(
    mut portal: Box<dyn GlobalShortcutsPortal>,
    tx: std::sync::mpsc::Sender<HotkeyEvent>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) {
    loop {
        if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        match portal.next_activated() {
            Some(id) => {
                // A closed receiver means the UI side is gone.
                if tx.send(HotkeyEvent::new(id)).is_err() {
                    break;
                }
            }
            None => {
                eprintln!(
                    "[d4lc] hotkey: portal activation stream ended or is unavailable; \
                     exiting the wayland hotkey loop"
                );
                break;
            }
        }
    }
}

/// The real `ashpd::desktop::global_shortcuts` portal behind the
/// [`GlobalShortcutsPortal`] boundary. All DBus conversation is
/// bridged into this synchronous module with `zbus::block_on`
/// (async-io flavor, matching the workspace zbus discipline — see
/// the d4lc-capture portal adapter and the ksni note in
/// `Cargo.toml`).
#[cfg(feature = "wayland-hotkeys")]
mod portal_impl {
    use std::pin::Pin;

    use ashpd::desktop::global_shortcuts::{
        Activated, BindShortcutsOptions, GlobalShortcuts, NewShortcut,
    };
    use ashpd::desktop::{CreateSessionOptions, ResponseError, Session};
    use ashpd::Error as AshpdError;
    use zbus::block_on;
    use zbus::export::futures_core::Stream;

    use super::{GlobalShortcutsPortal, PortalBindFailure, PortalShortcut};

    pub(super) struct AshpdGlobalShortcutsPortal {
        portal: GlobalShortcuts,
        session: Session<GlobalShortcuts>,
        activated: Option<Pin<Box<dyn Stream<Item = Activated> + Send>>>,
    }

    impl AshpdGlobalShortcutsPortal {
        /// Connect to the portal service and create the session.
        /// Neither step shows a dialog; the consent dialog belongs
        /// to the bind request. Every failure here means no usable
        /// GlobalShortcuts portal on this compositor.
        pub(super) fn open() -> Result<Self, PortalBindFailure> {
            let portal = block_on(GlobalShortcuts::new()).map_err(|error| {
                PortalBindFailure::BackendMissing {
                    detail: format!(
                        "the GlobalShortcuts portal is not reachable on this compositor: {error}"
                    ),
                }
            })?;
            let session = block_on(portal.create_session(CreateSessionOptions::default()))
                .map_err(|error| PortalBindFailure::BackendMissing {
                    detail: format!(
                        "the GlobalShortcuts portal session could not be created: {error}"
                    ),
                })?;
            Ok(Self {
                portal,
                session,
                activated: None,
            })
        }

        fn cancelled(detail: &str) -> PortalBindFailure {
            PortalBindFailure::Cancelled {
                detail: format!(
                    "{detail}: the compositor consent dialog was refused or the \
                     bind request was cancelled"
                ),
            }
        }
    }

    impl GlobalShortcutsPortal for AshpdGlobalShortcutsPortal {
        fn bind(&mut self, shortcuts: &[PortalShortcut]) -> Result<Vec<String>, PortalBindFailure> {
            let new_shortcuts: Vec<NewShortcut> = shortcuts
                .iter()
                .map(|shortcut| {
                    NewShortcut::new(shortcut.id.clone(), shortcut.description.clone())
                        .preferred_trigger(shortcut.trigger.as_str())
                })
                .collect();
            let request = block_on(self.portal.bind_shortcuts(
                &self.session,
                &new_shortcuts,
                None,
                BindShortcutsOptions::default(),
            ))
            .map_err(|error| match error {
                AshpdError::Response(ResponseError::Cancelled) => {
                    Self::cancelled("portal BindShortcuts request")
                }
                other => PortalBindFailure::Other {
                    detail: format!("portal BindShortcuts failed: {other}"),
                },
            })?;
            let response = request.response().map_err(|error| match error {
                AshpdError::Response(ResponseError::Cancelled) => {
                    Self::cancelled("portal BindShortcuts response")
                }
                other => PortalBindFailure::Other {
                    detail: format!("portal BindShortcuts response failed: {other}"),
                },
            })?;
            Ok(response
                .shortcuts()
                .iter()
                .map(|shortcut| shortcut.id().to_string())
                .collect())
        }

        fn next_activated(&mut self) -> Option<String> {
            if self.activated.is_none() {
                let stream = block_on(self.portal.receive_activated()).ok()?;
                self.activated = Some(Box::pin(stream));
            }
            let stream = self.activated.as_mut()?;
            let item = block_on(std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)));
            item.map(|activated| activated.shortcut_id().to_string())
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use crate::hotkeys::backend::{requests_from_settings, start_hotkey_backend};
    use crate::hotkeys::dispatch::drain_hotkey_events;
    use crate::hotkeys::test_support::with_temp_config_root;

    // -- Wayland GlobalShortcuts portal path ---------------------------
    //
    // The default suite drives the injectable `GlobalShortcutsPortal`
    // boundary with a deterministic fake: no session bus, no compositor
    // dialog. A real session is covered by the opt-in `#[ignore]` test at
    // the bottom of this module.

    /// Deterministic portal for the default suite: `bind` replays
    /// `bind_result`; `next_activated` pops `activated` (`None` ends
    /// the event loop).
    struct FakePortal {
        bind_result: Result<Vec<String>, PortalBindFailure>,
        activated: std::collections::VecDeque<String>,
    }

    impl FakePortal {
        fn accepting_all(requests: &[HotkeyRequest]) -> Self {
            Self {
                bind_result: Ok(requests.iter().map(|r| r.name.clone()).collect()),
                activated: Default::default(),
            }
        }
    }

    impl GlobalShortcutsPortal for FakePortal {
        fn bind(
            &mut self,
            _shortcuts: &[PortalShortcut],
        ) -> Result<Vec<String>, PortalBindFailure> {
            self.bind_result.clone()
        }
        fn next_activated(&mut self) -> Option<String> {
            self.activated.pop_front()
        }
    }

    /// All seven upstream defaults, enabled — the same settings-backed
    /// set the X11 real-session tests use.
    fn wayland_test_requests() -> Vec<HotkeyRequest> {
        let mut settings = d4lc_config::AppSettings::default();
        for binding in &mut settings.hotkey_config.bindings {
            binding.enabled = true;
        }
        requests_from_settings(&settings)
    }

    #[test]
    fn portal_trigger_strings_follow_the_xdg_accelerator_format() {
        // XDG accelerator string ("shortcuts" portal spec):
        // GTK-style modifiers + keysym key name. Live compositor
        // acceptance is assumed, not validated.
        assert_eq!(
            parsed_binding_to_portal_trigger(&parse_binding("Ctrl+F5").unwrap()),
            "<Control>F5"
        );
        assert_eq!(
            parsed_binding_to_portal_trigger(&parse_binding("Ctrl+Shift+Tab").unwrap()),
            "<Control><Shift>Tab"
        );
        assert_eq!(
            parsed_binding_to_portal_trigger(&parse_binding("Alt+F4").unwrap()),
            "<Alt>F4"
        );
        assert_eq!(
            parsed_binding_to_portal_trigger(&parse_binding("Super+L").unwrap()),
            "<Super>l"
        );
        assert_eq!(
            parsed_binding_to_portal_trigger(&parse_binding("F12").unwrap()),
            "F12"
        );
        assert_eq!(
            parsed_binding_to_portal_trigger(&parse_binding("Ctrl+5").unwrap()),
            "<Control>5"
        );
    }

    #[test]
    fn wayland_portal_bind_registers_accepted_bindings() {
        let requests = wayland_test_requests();
        let mut portal = FakePortal::accepting_all(&requests);
        let status = wayland_portal_bind_status(&requests, &mut portal);
        assert_eq!(
            status.capability,
            HotkeyBackendCapability::WaylandPortalAvailable
        );
        assert!(status.unavailable_reason.is_none());
        assert_eq!(status.registrations.len(), 7);
        for row in &status.registrations {
            assert_eq!(
                row.status,
                HotkeyRegistrationStatus::Registered,
                "{} must be portal-registered",
                row.name
            );
        }
    }

    #[test]
    fn wayland_portal_consent_refused_reports_refused_rows() {
        let requests = wayland_test_requests();
        let mut portal = FakePortal {
            bind_result: Err(PortalBindFailure::Cancelled {
                detail: "the compositor consent dialog was refused".to_string(),
            }),
            activated: Default::default(),
        };
        let status = wayland_portal_bind_status(&requests, &mut portal);
        // The portal exists; the refusal is reported per binding.
        assert_eq!(
            status.capability,
            HotkeyBackendCapability::WaylandPortalAvailable
        );
        assert_eq!(status.registrations.len(), 7);
        for row in &status.registrations {
            assert!(
                matches!(row.status, HotkeyRegistrationStatus::Refused { .. }),
                "{} must be reported refused, not registered",
                row.name
            );
        }
    }

    #[test]
    fn wayland_portal_missing_backend_reports_structured_unavailable() {
        // The sway / vanilla wlroots contract: no GlobalShortcuts
        // portal backend → structured unavailable naming the missing
        // piece, no fake registrations.
        let requests = wayland_test_requests();
        let (rows, bindable) = classify_wayland_requests(&requests);
        assert_eq!(bindable.len(), 7);
        let status = wayland_unavailable_status(
            rows,
            bindable,
            "the GlobalShortcuts portal is not reachable on this compositor: \
             xdg-desktop-portal has no GlobalShortcuts backend (sway / vanilla wlroots)"
                .to_string(),
        );
        assert_eq!(
            status.capability,
            HotkeyBackendCapability::WaylandPortalUnavailable
        );
        let reason = status.unavailable_reason.as_deref().unwrap_or_default();
        assert!(reason.contains("GlobalShortcuts"), "{reason}");
        assert_eq!(status.registrations.len(), 7);
        for row in &status.registrations {
            assert!(
                matches!(row.status, HotkeyRegistrationStatus::Unavailable { .. }),
                "{} must carry its own unavailable row",
                row.name
            );
        }
    }

    #[test]
    fn wayland_portal_response_missing_shortcut_is_unconfirmed() {
        let requests = wayland_test_requests();
        let mut portal = FakePortal {
            bind_result: Ok(vec!["SwitchPreset".to_string()]),
            activated: Default::default(),
        };
        let status = wayland_portal_bind_status(&requests, &mut portal);
        let switch = status
            .registrations
            .iter()
            .find(|r| r.name == "SwitchPreset")
            .unwrap();
        assert_eq!(switch.status, HotkeyRegistrationStatus::Registered);
        // The portal response omitted the rest — reported unconfirmed,
        // never as registered.
        let prism = status
            .registrations
            .iter()
            .find(|r| r.name == "ToggleTuningPrismHints")
            .unwrap();
        assert!(matches!(
            prism.status,
            HotkeyRegistrationStatus::Unconfirmed { .. }
        ));
    }

    #[test]
    fn wayland_portal_activated_events_dispatch_through_existing_handlers() {
        // The acceptance contract: a portal `Activated` event for each
        // bound hotkey reaches the SAME handler path the UI buttons
        // use — the production event loop forwards ids onto the
        // `HotkeyEvent` mpsc and `drain_hotkey_events` dispatches
        // them through `dispatch_hotkey_event`.
        let app = crate::test_window::new_app_window();
        let requests = wayland_test_requests();
        let mut portal = FakePortal::accepting_all(&requests);
        let status = wayland_portal_bind_status(&requests, &mut portal);
        for row in &status.registrations {
            assert_eq!(
                row.status,
                HotkeyRegistrationStatus::Registered,
                "{}",
                row.name
            );
        }

        // Queue one Activated event per bound hotkey, in binding
        // order, and run the production loop body inline (the thread
        // runs exactly this function).
        portal.activated = requests
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>()
            .into();
        let (tx, rx) = std::sync::mpsc::channel::<HotkeyEvent>();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        run_wayland_portal_event_loop(Box::new(portal), tx, shutdown);

        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            drain_hotkey_events(&app, &mut state, config_root, &rx);

            // Every activated id was dispatched: the last one is
            // recorded, and each stateful handler ran its real
            // mutation.
            assert_eq!(
                state.last_hotkey_event.as_deref(),
                Some("ToggleTuningPrismHints")
            );
            assert!(state.shell.settings.paragon_mode_active, "SwitchOverlay");
            assert!(
                state.runtime_status.last_debug_action.is_some(),
                "TakeScreenshot"
            );
            assert!(state.shell.settings.controller_mode, "ToggleController");
            assert!(state.shell.settings.overlay_enabled, "ToggleOverlay");
            assert!(
                state.runtime_status.debug_lock_screencapture,
                "ToggleDebugLockScreencapture"
            );
            assert!(
                state.shell.settings.is_tuning_prism_hints_enabled,
                "ToggleTuningPrismHints"
            );
        });
    }

    /// Opt-in live-path evidence test: run the production
    /// Wayland portal backend on a real Wayland session and record the
    /// structured per-binding matrix. The FIRST registration triggers
    /// the compositor's consent dialog — answer it for rows.
    /// The verdict is a structured status, not success: on a portal-less
    /// compositor (sway / vanilla wlroots) the run must report
    /// `WaylandPortalUnavailable` with the missing-backend reason and
    /// start no thread.
    ///
    /// Never runs in the default suite (the reference machine is X11).
    #[test]
    #[ignore = "real Wayland session: opt-in; run on a Wayland session with `cargo test -p d4lc-app hotkey_real_session_wayland -- --ignored --nocapture`"]
    fn hotkey_real_session_wayland_portal_matrix() {
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        assert_eq!(
            session.to_ascii_lowercase(),
            "wayland",
            "requires a real Wayland session (XDG_SESSION_TYPE=wayland)"
        );
        println!(
            "session: XDG_SESSION_TYPE={session} — the compositor consent dialog may appear \
             on first registration; answer it for per-binding rows"
        );

        let mut settings = d4lc_config::AppSettings::default();
        for binding in &mut settings.hotkey_config.bindings {
            binding.enabled = true;
        }
        let (status, handle, rx) = start_hotkey_backend(&settings);
        println!("portal matrix:");
        for row in &status.registrations {
            println!("  {:30} {:8} {:?}", row.name, row.binding, row.status);
        }
        match status.capability {
            HotkeyBackendCapability::WaylandPortalAvailable => {
                for row in &status.registrations {
                    assert!(
                        matches!(
                            row.status,
                            HotkeyRegistrationStatus::Registered
                                | HotkeyRegistrationStatus::Refused { .. }
                                | HotkeyRegistrationStatus::Unconfirmed { .. }
                                | HotkeyRegistrationStatus::Unavailable { .. }
                        ),
                        "{}: portal rows must be explicit outcomes, got {:?}",
                        row.name,
                        row.status
                    );
                }
                let registered = status
                    .registrations
                    .iter()
                    .filter(|r| matches!(r.status, HotkeyRegistrationStatus::Registered))
                    .count();
                println!(
                    "portal matrix summary: {registered}/{} upstream defaults portal-registered; \
                     the rest are structured Refused/Unconfirmed/Unavailable rows above",
                    status.registrations.len()
                );
                if registered >= 1 {
                    let handle = handle.expect("registered rows must start the activation loop");
                    let event_rx = rx.expect("registered rows must return the event receiver");
                    println!(
                        "portal backend live: the Activated loop is running; press a bound \
                         combo to see it arrive (5s listen window)"
                    );
                    match event_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                        Ok(event) => {
                            println!("portal: Activated → HotkeyEvent::{}", event.name);
                            assert!(
                                settings.hotkey_config.find_by_name(&event.name).is_some(),
                                "activated id must be a configured action name"
                            );
                        }
                        Err(_) => println!("portal: no activation within 5s (nothing pressed)"),
                    }
                    // Deliberate drop, not `stop()`: the portal loop
                    // blocks inside the DBus signal stream, so a join
                    // would wait for the next activation. The Drop impl
                    // flips the flag without joining (the same
                    // best-effort teardown the X11 handle documents);
                    // the thread ends with the test process.
                    drop(handle);
                }
            }
            HotkeyBackendCapability::WaylandPortalUnavailable => {
                let reason = status.unavailable_reason.clone().unwrap_or_default();
                assert!(
                    !reason.is_empty(),
                    "portal-less Wayland must name the missing backend"
                );
                assert!(handle.is_none() && rx.is_none());
                println!("portal matrix summary: structured unavailable — {reason}");
            }
            other => panic!(
                "a real Wayland session must report a structured Wayland state, got {other:?}"
            ),
        }
    }
}
