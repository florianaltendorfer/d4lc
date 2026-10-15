//! X11 global hotkeys: passive key grabs on the root window and the
//! background event loop that delivers them.

use std::sync::Arc;

use super::backend::{
    HotkeyBackendCapability, HotkeyEvent, HotkeyRegistration, HotkeyRegistrationStatus,
    HotkeyRequest,
};
#[cfg(feature = "x11rb-backend")]
use super::binding::{named_key_to_keysym, parse_binding, to_x11_modmask_bits};

/// Returns `Some((capability, reason, registrations))` when the
/// X11 path ran (success or per-binding failure), or `None`
/// when the runtime path produced no result (currently unused
/// — kept for symmetry with future async / event-loop wiring).
type X11Result = (
    HotkeyBackendCapability,
    Option<String>,
    Vec<HotkeyRegistration>,
);

pub(super) fn try_register_x11(requests: &[HotkeyRequest]) -> Result<X11Result, String> {
    // 1. Detect Wayland explicitly so the failure mode is
    //    obvious in the log — passive X11 grabs will not work
    //    on a Wayland session regardless of `DISPLAY`.
    if let Ok(session) = std::env::var("XDG_SESSION_TYPE") {
        if session.eq_ignore_ascii_case("wayland") {
            let reason =
                "XDG_SESSION_TYPE=wayland — X11 passive grabs are not effective on Wayland"
                    .to_string();
            return Ok((
                HotkeyBackendCapability::X11Unavailable,
                Some(reason.clone()),
                unavailable_registrations(requests, &reason),
            ));
        }
    }

    // 2. Require DISPLAY. Without it, the X11 socket does not
    //    exist and `RustConnection::connect` would still try
    //    and fail; this fast-path avoids the noisy IO error.
    if std::env::var_os("DISPLAY").is_none() {
        let reason = "DISPLAY environment variable is not set".to_string();
        return Ok((
            HotkeyBackendCapability::X11Unavailable,
            Some(reason.clone()),
            unavailable_registrations(requests, &reason),
        ));
    }

    // 3. Real X11 attempt.
    #[cfg(feature = "x11rb-backend")]
    {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{ConnectionExt, GrabMode, Keycode, ModMask};
        use x11rb::rust_connection::RustConnection;

        let (conn, screen_num) = match RustConnection::connect(None) {
            Ok(pair) => pair,
            Err(e) => {
                let reason = format!("X11 connect failed: {e}");
                return Ok((
                    HotkeyBackendCapability::X11Unavailable,
                    Some(reason.clone()),
                    unavailable_registrations(requests, &reason),
                ));
            }
        };

        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;

        // Fetch the keyboard mapping so we can resolve keysyms
        // → keycodes. `get_keyboard_mapping` is the standard
        // X11 way; the reply is a per-keycode list of keysyms
        // (typically 4 per keycode for groups/levels).
        let mapping = match conn
            .get_keyboard_mapping(Keycode::from(min_keycode), max_keycode - min_keycode + 1)
        {
            Ok(cookie) => match cookie.reply() {
                Ok(reply) => reply,
                Err(e) => {
                    let reason = format!("get_keyboard_mapping failed: {e}");
                    return Ok((
                        HotkeyBackendCapability::X11Unavailable,
                        Some(reason.clone()),
                        unavailable_registrations(requests, &reason),
                    ));
                }
            },
            Err(e) => {
                let reason = format!("get_keyboard_mapping send failed: {e}");
                return Ok((
                    HotkeyBackendCapability::X11Unavailable,
                    Some(reason.clone()),
                    unavailable_registrations(requests, &reason),
                ));
            }
        };

        // We grab on the root window with `owner_events=false`
        // and `GrabMode::Async` (per upstream
        // `MainWindowViewModel.cs:353-439` semantics: a
        // passive grab that does not interfere with focus).
        let mut registrations = Vec::with_capacity(requests.len());
        for req in requests {
            if !req.enabled {
                registrations.push(HotkeyRegistration {
                    name: req.name.clone(),
                    binding: req.binding.clone(),
                    status: HotkeyRegistrationStatus::Disabled,
                });
                continue;
            }
            let parsed = match parse_binding(&req.binding) {
                Ok(p) => p,
                Err(e) => {
                    registrations.push(HotkeyRegistration {
                        name: req.name.clone(),
                        binding: req.binding.clone(),
                        status: HotkeyRegistrationStatus::ParseError {
                            detail: e.to_string(),
                        },
                    });
                    continue;
                }
            };
            let keysym = named_key_to_keysym(parsed.key);
            let keycode = match find_keycode_for_keysym(&mapping, min_keycode, keysym) {
                Some(kc) => kc,
                None => {
                    registrations.push(HotkeyRegistration {
                        name: req.name.clone(),
                        binding: req.binding.clone(),
                        status: HotkeyRegistrationStatus::ParseError {
                            detail: format!(
                                "no keycode for keysym {:#x} (key {:?}) on the current keyboard mapping",
                                keysym, parsed.key
                            ),
                        },
                    });
                    continue;
                }
            };
            let modmask_bits = to_x11_modmask_bits(parsed.modifiers);
            let mut status = HotkeyRegistrationStatus::Registered;
            for variant in lock_mask_variants(modmask_bits) {
                let grab = conn.grab_key(
                    false,
                    root,
                    ModMask::from(variant),
                    Keycode::from(keycode),
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                );
                let result = match grab {
                    Ok(cookie) => match cookie.check() {
                        Ok(()) => continue,
                        Err(e) => classify_x11_error(e),
                    },
                    Err(e) => HotkeyRegistrationStatus::Unavailable {
                        reason: format!("grab_key send failed: {e}"),
                    },
                };
                status = result;
                break;
            }
            registrations.push(HotkeyRegistration {
                name: req.name.clone(),
                binding: req.binding.clone(),
                status,
            });
        }

        Ok((HotkeyBackendCapability::X11Available, None, registrations))
    }

    #[cfg(not(feature = "x11rb-backend"))]
    {
        let reason = "x11rb feature is not enabled in this build".to_string();
        Ok((
            HotkeyBackendCapability::X11Unavailable,
            Some(reason.clone()),
            unavailable_registrations(requests, &reason),
        ))
    }
}

fn unavailable_registrations(requests: &[HotkeyRequest], reason: &str) -> Vec<HotkeyRegistration> {
    requests
        .iter()
        .map(|r| {
            let status = if r.enabled {
                HotkeyRegistrationStatus::Unavailable {
                    reason: reason.to_string(),
                }
            } else {
                HotkeyRegistrationStatus::Disabled
            };
            HotkeyRegistration {
                name: r.name.clone(),
                binding: r.binding.clone(),
                status,
            }
        })
        .collect()
}

/// CapsLock (`Lock`) and NumLock (`Mod2`, where every common keymap puts
/// `Num_Lock`). The key event state carries them whenever they are on, and
/// a passive grab only matches its exact modifier mask.
#[cfg_attr(not(feature = "x11rb-backend"), allow(dead_code))]
const LOCK_MODIFIER_BITS: u16 = (1 << 1) | (1 << 4);

/// The masks a binding is grabbed with: its own, plus each combination of
/// the lock modifiers it does not name, so the hotkey fires whether CapsLock
/// or NumLock is on.
#[cfg_attr(not(feature = "x11rb-backend"), allow(dead_code))]
fn lock_mask_variants(modmask_bits: u16) -> Vec<u16> {
    let free = LOCK_MODIFIER_BITS & !modmask_bits;
    let mut variants = vec![modmask_bits];
    for lock in [1u16 << 1, 1 << 4] {
        if free & lock != 0 {
            let with_lock: Vec<u16> = variants.iter().map(|mask| mask | lock).collect();
            variants.extend(with_lock);
        }
    }
    variants
}

#[cfg(feature = "x11rb-backend")]
fn find_keycode_for_keysym(
    mapping: &x11rb::protocol::xproto::GetKeyboardMappingReply,
    min_keycode: u8,
    keysym: u32,
) -> Option<u8> {
    // `keysyms_per_keycode` is the stride; iterate each keycode
    // and check every entry for a match. The first match wins;
    // this is the standard X11 lookup strategy. The first two
    // entries are level 1 (no modifier) and level 2 (Shift),
    // which is what the upstream defaults target.
    let stride = mapping.keysyms_per_keycode as usize;
    if stride == 0 {
        return None;
    }
    for (idx, raw) in mapping.keysyms.iter().enumerate() {
        if *raw == keysym {
            return Some(min_keycode + (idx / stride) as u8);
        }
    }
    None
}

#[cfg(feature = "x11rb-backend")]
fn classify_x11_error(err: x11rb::errors::ReplyError) -> HotkeyRegistrationStatus {
    use x11rb::errors::ReplyError;
    use x11rb::protocol::ErrorKind;
    use x11rb::x11_utils::X11Error;
    match err {
        ReplyError::X11Error(X11Error {
            error_kind: ErrorKind::Access,
            ..
        }) => HotkeyRegistrationStatus::Conflict {
            detail: "X11 BadAccess: another client already owns this key combination".to_string(),
        },
        ReplyError::X11Error(X11Error {
            error_kind: ErrorKind::Value,
            ..
        }) => HotkeyRegistrationStatus::ParseError {
            detail: "X11 BadValue: keycode or modifier mask is out of range".to_string(),
        },
        ReplyError::X11Error(X11Error {
            error_kind: ErrorKind::Window,
            ..
        }) => HotkeyRegistrationStatus::Unavailable {
            reason: "X11 BadWindow: the root window is not reachable".to_string(),
        },
        ReplyError::X11Error(X11Error { error_kind, .. }) => {
            HotkeyRegistrationStatus::Unavailable {
                reason: format!("X11 protocol error: {error_kind:?}"),
            }
        }
        ReplyError::ConnectionError(e) => HotkeyRegistrationStatus::Unavailable {
            reason: format!("X11 transport error: {e}"),
        },
    }
}

/// One row of the in-process event-loop lookup table. The
/// thread loops over the `Vec<EventLoopEntry>` to translate a
/// keycode + modifier mask into an action name.
#[cfg_attr(not(feature = "x11rb-backend"), allow(dead_code))]
#[derive(Debug, Clone)]
pub(super) struct EventLoopEntry {
    name: String,
    keycode: u8,
    modmask_bits: u16,
}

#[cfg(feature = "x11rb-backend")]
pub(super) fn build_event_lookup_table(requests: &[HotkeyRequest]) -> Vec<EventLoopEntry> {
    // 1. Same env-var short-circuits as `try_register_x11`:
    //    Wayland or missing DISPLAY => no event loop.
    if let Ok(session) = std::env::var("XDG_SESSION_TYPE") {
        if session.eq_ignore_ascii_case("wayland") {
            return Vec::new();
        }
    }
    if std::env::var_os("DISPLAY").is_none() {
        return Vec::new();
    }

    // 2. Connect, fetch the keyboard mapping, resolve
    //    keycodes / modmasks for every *enabled* request. The
    //    probe path in `try_register_x11` already does the
    //    same dance; the duplication is intentional — the
    //    event loop must own its own `RustConnection` so the
    //    probe's connection can drop without taking the grab
    //    with it. The cost is a single second `connect` +
    //    `get_keyboard_mapping` round-trip at startup.
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, Keycode};
    use x11rb::rust_connection::RustConnection;

    let (conn, _screen_num) = match RustConnection::connect(None) {
        Ok(pair) => pair,
        Err(_) => return Vec::new(),
    };
    let setup = conn.setup();
    let min_keycode = setup.min_keycode;
    let max_keycode = setup.max_keycode;

    let mapping = match conn
        .get_keyboard_mapping(Keycode::from(min_keycode), max_keycode - min_keycode + 1)
    {
        Ok(cookie) => cookie.reply().ok(),
        Err(_) => return Vec::new(),
    };
    let Some(mapping) = mapping else {
        return Vec::new();
    };

    // Hold the connection alive for the duration of the
    // table build. The *event-loop* thread below opens its
    // own connection.
    drop(conn);

    let mut out = Vec::with_capacity(requests.len());
    for req in requests {
        if !req.enabled {
            continue;
        }
        let Ok(parsed) = parse_binding(&req.binding) else {
            continue;
        };
        let keysym = named_key_to_keysym(parsed.key);
        let Some(keycode) = find_keycode_for_keysym(&mapping, min_keycode, keysym) else {
            continue;
        };
        let modmask_bits = to_x11_modmask_bits(parsed.modifiers);
        out.push(EventLoopEntry {
            name: req.name.clone(),
            keycode,
            modmask_bits,
        });
    }
    out
}

#[cfg(not(feature = "x11rb-backend"))]
pub(super) fn build_event_lookup_table(_requests: &[HotkeyRequest]) -> Vec<EventLoopEntry> {
    Vec::new()
}

/// X11 event-loop poll cadence.
///
/// The loop re-checks the shared shutdown flag at most this long
/// after it is set, on a completely idle connection too — this is
/// what keeps [`HotkeyBackendHandle::stop`](super::HotkeyBackendHandle::stop)
/// bounded on the UI thread. It is also the worst-case delivery delay for a real
/// hotkey press, an order of magnitude below the 50 ms UI drain
/// timer.
pub(super) const HOTKEY_EVENT_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(25);

/// Waiting step of the X11 event loop: hand back the next pending
/// event, or `Ok(None)` once the shared `shutdown` flag is observed
/// (the loop must then exit).
///
/// `poll_for_event()` never blocks, so an idle connection still lets
/// the loop see the flag — the reason a bounded `stop()` is possible
/// without a self-pipe wakeup. The flag is checked *before* every
/// poll, so a pending event cannot delay a stop. Between idle polls
/// the step sleeps one [`HOTKEY_EVENT_POLL_INTERVAL`].
///
/// The poll is a closure so this step — the part that has to keep
/// the loop wakeable — is unit-testable without an X connection.
#[cfg(feature = "x11rb-backend")]
pub(super) fn poll_until_event_or_shutdown<E>(
    shutdown: &std::sync::atomic::AtomicBool,
    mut poll: impl FnMut() -> Result<Option<E>, x11rb::errors::ConnectionError>,
) -> Result<Option<E>, x11rb::errors::ConnectionError> {
    loop {
        if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        match poll()? {
            Some(event) => return Ok(Some(event)),
            None => std::thread::sleep(HOTKEY_EVENT_POLL_INTERVAL),
        }
    }
}

/// Background-thread body: own a `RustConnection`, wait for
/// key events, dispatch matches over the channel. Exits when
/// the shared `shutdown` flag flips to `true` (observed within
/// one [`HOTKEY_EVENT_POLL_INTERVAL`], no X event needed) or
/// when the connection drops (which is the X11 way of saying
/// the server went away or our grab was lost).
#[cfg(feature = "x11rb-backend")]
pub(super) fn run_event_loop(
    table: Arc<Vec<EventLoopEntry>>,
    tx: std::sync::mpsc::Sender<HotkeyEvent>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;
    use x11rb::rust_connection::RustConnection;

    // (Re)open a connection dedicated to the event loop.
    // The probe connection in `try_register_x11` is gone by
    // now, so we need our own to keep the grab alive.
    let (conn, _screen_num) = match RustConnection::connect(None) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[d4lc] hotkey: event loop could not connect to X11 ({e}); shutting down");
            return;
        }
    };

    // Re-register the grabs on this fresh connection. The
    // X11 server releases a grab when the connection that
    // registered it drops, so a new connection must redo
    // every `grab_key`. The `init_hotkeys_from` probe
    // already validated that the server *can* accept each
    // grab; we expect the same result here. If a grab
    // fails, the matching `EventLoopEntry` is left in the
    // table anyway — the dispatch loop will simply never
    // match it (the keycode + modmask pair still matches
    // an incoming event, but the server is no longer
    // sending it to us).
    use x11rb::protocol::xproto::{GrabMode, Keycode, ModMask};
    let setup = conn.setup();
    let root = setup.roots[_screen_num].root;
    for entry in table.iter() {
        // Checked re-grab: a silent fire-and-forget grab made a lost
        // grab undiagnosable. Failures
        // are logged so a hotkey that will never fire stays visible in
        // the logs instead of faking success.
        for variant in lock_mask_variants(entry.modmask_bits) {
            let result = match conn.grab_key(
                false,
                root,
                ModMask::from(variant),
                Keycode::from(entry.keycode),
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            ) {
                Ok(cookie) => cookie.check(),
                Err(e) => Err(x11rb::errors::ReplyError::ConnectionError(e)),
            };
            if let Err(e) = result {
                eprintln!(
                    "[d4lc] hotkey: event-loop re-grab failed for keycode {} mask {:#06x}: {e:?} (this hotkey will not fire with that lock state)",
                    entry.keycode, variant
                );
            }
        }
    }

    // Drain pending replies so the server has acknowledged
    // our grabs. `poll_for_event` reads pending packets
    // non-blockingly, so the first loop iterations pick the
    // replies up.
    let _ = conn.flush();

    // Main event loop. `wait_for_event` would block until a
    // matching event arrives — indefinitely on an idle server,
    // which is exactly what made the handle's `stop()` (the UI
    // thread's re-registration seam) wait for the next X event.
    // The waiting step therefore polls the connection and
    // re-checks the shared `shutdown` flag every
    // `HOTKEY_EVENT_POLL_INTERVAL`, so a stop is observed on an
    // idle server within that interval. The same interval is the
    // added latency for a real hotkey press — well under the
    // 50ms `drain_runtime_events` timer that delivers the event.
    loop {
        let event = match poll_until_event_or_shutdown(&shutdown, || conn.poll_for_event()) {
            Ok(Some(event)) => event,
            // The shared flag was observed: exit without
            // waiting for an event that may never come.
            Ok(None) => break,
            Err(e) => {
                // Transport error — connection dropped,
                // server restarted, etc. Exit the loop; the
                // next call to `start_hotkey_backend` (or a
                // app restart) will rebuild the grab.
                eprintln!("[d4lc] hotkey: event loop connection error ({e}); exiting");
                break;
            }
        };

        // We only care about `KeyPress` events.
        // `x11rb::protocol::Event::KeyPress`
        // carries `(u8, u16)` for (keycode, state).
        use x11rb::protocol::Event;
        if let Event::KeyPress(keypress) = event {
            let state = keypress.state;
            // The state field includes the modifier
            // bits the user is currently holding,
            // plus CapsLock / NumLock when they are on.
            // The lock bits a binding does not name are
            // ignored (every lock variant was grabbed),
            // the rest must match the binding exactly.
            for entry in table.iter() {
                let ignored = LOCK_MODIFIER_BITS & !entry.modmask_bits;
                if entry.keycode == keypress.detail
                    && (state.bits() & !ignored) == entry.modmask_bits
                {
                    // Send — ignore the result; a closed
                    // receiver means the UI thread has
                    // dropped the handle and we should
                    // exit (the flag check above ends the
                    // loop on the next iteration).
                    if tx.send(HotkeyEvent::new(entry.name.clone())).is_err() {
                        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            }
        }
        // Non-key events (configure, expose, …) are ignored —
        // the live overlay window / the root window are not our
        // problem here.
    }

    // Best-effort ungrab: walk the table and call
    // `ungrab_key` for each registered binding. Failures
    // are silent — the connection is about to drop, and
    // dropping it is the OS-level ungrab X11 understands.
    for entry in table.iter() {
        for variant in lock_mask_variants(entry.modmask_bits) {
            let _ = conn.ungrab_key(Keycode::from(entry.keycode), root, ModMask::from(variant));
        }
    }
    let _ = conn.flush();
    // `conn` drops at end of scope, freeing the socket.
}

#[cfg(not(feature = "x11rb-backend"))]
pub(super) fn run_event_loop(
    _table: Arc<Vec<EventLoopEntry>>,
    _tx: std::sync::mpsc::Sender<HotkeyEvent>,
    _shutdown: Arc<std::sync::atomic::AtomicBool>,
) {
    // x11rb feature is disabled in this build — there is no
    // event loop to drive. `start_hotkey_backend` already
    // returned `(status, None, None)` for this case, but
    // keep the function symbol stable for the no-op build.
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::hotkeys::backend::{init_hotkeys_from, requests_from_settings};
    #[cfg(feature = "x11rb-backend")]
    use crate::hotkeys::backend::{start_hotkey_backend, HOTKEY_STOP_JOIN_TIMEOUT};

    #[test]
    fn a_binding_is_grabbed_for_every_caps_and_num_lock_state() {
        let control = 1u16 << 2;
        assert_eq!(
            lock_mask_variants(control),
            vec![control, control | 2, control | 16, control | 18]
        );
        // A lock the binding names itself is not varied.
        assert_eq!(
            lock_mask_variants(control | 2),
            vec![control | 2, control | 18]
        );
    }

    // -- Real X11 session end-to-end tests --------------------------
    //
    // The tests above prove parser / status / dispatch logic without an
    // X server. The opt-in tests below prove the *production* X11
    // backend on a real desktop session:
    //
    //   1. `hotkey_real_session_default_bindings_matrix` runs the pure
    //      registration probe for the seven upstream default bindings
    //      against the live X server and records the structured
    //      per-binding matrix. Conflicts with desktop-environment
    //      shortcuts are expected on some sessions and must surface as
    //      `Conflict`, never as fake success.
    //   2. `hotkey_real_session_x11_grab_and_dispatch_end_to_end` starts
    //      the persistent backend, fires each binding with `xdotool`
    //      (XTEST — server-side synthetic input that follows the normal
    //      grab routing, unlike `SendEvent`), and dispatches the
    //      received events through the production `drain_hotkey_events`
    //      into real `AppState` mutations and persisted settings. If a
    //      default combo is conflicted on the session (as `Ctrl+F10`
    //      is on the reference KDE/KWin machine), the settings-backed
    //      configurability path is exercised instead: a free rebind
    //      candidate is probed and used, mirroring what a user does in
    //      the HotkeyConfig dialog.

    /// Opt-in evidence test: run the pure registration probe for the
    /// seven upstream default bindings against the real X server and
    /// record the structured per-binding matrix. No key synthesis, no
    /// background thread.
    #[test]
    #[ignore = "real X11 session: opt-in; run with `cargo test -p d4lc-app hotkey_real_session -- --ignored --nocapture`"]
    fn hotkey_real_session_default_bindings_matrix() {
        if std::env::var_os("DISPLAY").is_none() {
            panic!("the defaults matrix requires a real X11 DISPLAY");
        }
        println!(
            "session: XDG_SESSION_TYPE={:?} DISPLAY={:?}",
            std::env::var("XDG_SESSION_TYPE").ok(),
            std::env::var("DISPLAY").ok()
        );

        let mut settings = d4lc_config::AppSettings::default();
        for binding in &mut settings.hotkey_config.bindings {
            binding.enabled = true;
        }
        // Pure probe: `init_hotkeys_from` grabs on a temporary
        // connection and drops it; no event-loop thread is started, so
        // the grabs die with the probe connection.
        let status = init_hotkeys_from(&requests_from_settings(&settings));
        assert_eq!(
            status.capability,
            HotkeyBackendCapability::X11Available,
            "real X11 session must report X11Available (reason: {:?})",
            status.unavailable_reason
        );
        assert_eq!(status.registrations.len(), 7);
        println!("defaults matrix:");
        for row in &status.registrations {
            println!("  {:30} {:8} {:?}", row.name, row.binding, row.status);
        }
        let registered = status
            .registrations
            .iter()
            .filter(|row| matches!(row.status, HotkeyRegistrationStatus::Registered))
            .count();
        assert!(
            registered >= 1,
            "at least one upstream default binding must register on a sane X11 desktop; \
             a session that owns all seven combos is an environment blocker"
        );
        println!(
            "defaults matrix summary: {registered}/7 upstream defaults registered; \
             the rest are structured Conflict/ParseError rows above"
        );
    }

    /// Opt-in diagnostic: isolate raw XTEST -> passive-grab delivery on
    /// the real X server, with no production wiring involved. One
    /// connection grabs `Ctrl+F5` on the root window in all four
    /// lock-variant modifier masks (`Control`, `Control+Lock`,
    /// `Control+Mod2`, `Control+Lock+Mod2` — the way xbindkeys-class
    /// tools grab), `xdotool` synthesizes the combo, and a reader
    /// thread reports the raw `KeyPress` the server delivers back.
    ///
    /// The delivered event's modifier state reveals whether active
    /// NumLock (`Mod2`) / CapsLock (`Lock`) bits were present during
    /// the synthesized press. The production backend grabs the same four
    /// lock variants of every binding, so an active lock modifier does not
    /// stop a hotkey from firing.
    #[cfg(feature = "x11rb-backend")]
    #[test]
    #[ignore = "real X11 session diagnostic: opt-in; run with `cargo test -p d4lc-app hotkey_real_session -- --ignored --nocapture`"]
    fn hotkey_real_session_xtest_grab_delivery_diagnostic() {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{ConnectionExt, GrabMode, Keycode, ModMask};
        use x11rb::rust_connection::RustConnection;

        if std::env::var_os("DISPLAY").is_none() {
            panic!("diagnostic requires a real X11 DISPLAY");
        }

        let (conn, screen) = RustConnection::connect(None).expect("X11 connect");
        let setup = conn.setup();
        let root = setup.roots[screen].root;
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let mapping = conn
            .get_keyboard_mapping(Keycode::from(min_keycode), max_keycode - min_keycode + 1)
            .expect("send get_keyboard_mapping")
            .reply()
            .expect("get_keyboard_mapping reply");
        let f5 = find_keycode_for_keysym(&mapping, min_keycode, 0xffc2).expect("F5 keycode");
        println!(
            "diagnostic: F5 keysym 0xffc2 resolved to keycode {f5} (xmodmap expectation on this host: 71)"
        );

        // Grab all four lock variants of Ctrl+F5 on this connection;
        // the reader thread owns the connection from here on (the
        // grabs die with the process).
        let control = 1u16 << 2;
        let lock = 1u16 << 1;
        let mod2 = 1u16 << 4;
        let variants = [
            control,
            control | lock,
            control | mod2,
            control | lock | mod2,
        ];
        for bits in variants {
            conn.grab_key(
                false,
                root,
                ModMask::from(bits),
                Keycode::from(f5),
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .expect("send grab_key")
            .check()
            .expect("variant grab must not conflict (Ctrl+F5 is free on this host)");
        }
        conn.flush().expect("flush");

        let (tx, rx) = std::sync::mpsc::channel::<Result<(u8, u16), String>>();
        let reader = std::thread::spawn(move || loop {
            match conn.wait_for_event() {
                Ok(x11rb::protocol::Event::KeyPress(kp)) => {
                    let _ = tx.send(Ok((kp.detail, kp.state.bits())));
                    // `conn` intentionally leaks in this thread: the
                    // grab is released when the test process exits.
                    break;
                }
                Ok(other) => {
                    let _ = tx.send(Err(format!("non-KeyPress event: {other:?}")));
                    // Keep reading: the KeyPress may follow other
                    // unrelated events.
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("connection error: {e}")));
                    break;
                }
            }
        });

        let out = std::process::Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+F5"])
            .output()
            .expect("spawn xdotool");
        println!("diagnostic: xdotool status {:?}", out.status);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!(
                    "diagnostic: TIMEOUT — no KeyPress delivered to the grabbing client within 5s; \
                     XTEST-driven passive-grab delivery does not work in this environment"
                );
            }
            match rx.recv_timeout(remaining) {
                Ok(Ok((detail, state))) => {
                    println!(
                        "diagnostic: received KeyPress detail={detail} state={state:#018b} \
                         (expect detail={f5}; Lock=0x2 Control=0x4 Mod2=0x10)"
                    );
                    assert_eq!(detail, f5, "KeyPress detail must be the F5 keycode");
                    assert_eq!(
                        state & control,
                        control,
                        "ControlMask must be set in the delivered event state"
                    );
                    if state & mod2 != 0 {
                        println!(
                            "diagnostic: NumLock (Mod2) was ACTIVE during the synthesized press — \
                             xdotool --clearmodifiers did not clear NumLock on this xdotool build, \
                             so an exact-Control grab alone would NOT have fired"
                        );
                    }
                    if state & lock != 0 {
                        println!(
                            "diagnostic: CapsLock (Lock) was ACTIVE during the synthesized press"
                        );
                    }
                    println!(
                        "diagnostic: PASS — this X server routes XTEST input through root passive grabs"
                    );
                    break;
                }
                Ok(Err(noise)) => {
                    println!("diagnostic: {noise}");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    panic!(
                        "diagnostic: TIMEOUT — no KeyPress delivered to the grabbing client within 5s; \
                         XTEST-driven passive-grab delivery does not work in this environment"
                    );
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("diagnostic: reader thread ended without delivering a KeyPress");
                }
            }
        }
        let _ = reader; // reader thread exits with the process by design
    }

    /// Opt-in diagnostic: verify that the production event-loop thread
    /// actually holds its grabs after `start_hotkey_backend`, by
    /// re-grabbing the same combination from a second connection. A
    /// second grab that fails with `BadAccess` proves the loop's grab
    /// is live; a second grab that succeeds proves the production
    /// thread silently lost its grabs (the loop's grabs are
    /// fire-and-forget).
    #[cfg(feature = "x11rb-backend")]
    #[test]
    #[ignore = "real X11 session diagnostic: opt-in; run with `cargo test -p d4lc-app hotkey_real_session -- --ignored --nocapture`"]
    fn hotkey_real_session_grab_liveness_check() {
        use x11rb::connection::Connection;
        use x11rb::errors::ReplyError;
        use x11rb::protocol::xproto::{ConnectionExt, GrabMode, Keycode, ModMask};
        use x11rb::protocol::ErrorKind;
        use x11rb::rust_connection::RustConnection;
        use x11rb::x11_utils::X11Error;

        if std::env::var_os("DISPLAY").is_none() {
            panic!("liveness diagnostic requires a real X11 DISPLAY");
        }

        // One enabled binding only: the default SwitchPreset Ctrl+F5.
        // D4LC_LIVENESS_SIX=1 switches to the full default-binding e2e
        // set (with the static rebinds).
        let mut settings = d4lc_config::AppSettings::default();
        if std::env::var("D4LC_LIVENESS_SIX").is_ok() {
            for binding in &mut settings.hotkey_config.bindings {
                binding.enabled = true;
            }
            let rebinds: &[(&str, &str)] = &[
                ("TakeScreenshot", "Shift+F5"),
                ("ToggleController", "Alt+F5"),
                ("ToggleOverlay", "Ctrl+Shift+F5"),
                ("ToggleDebugLockScreencapture", "Ctrl+8"),
            ];
            for (name, combo) in rebinds {
                settings
                    .hotkey_config
                    .find_by_name_mut(name)
                    .expect("known action name")
                    .binding = (*combo).to_string();
            }
        } else {
            settings
                .hotkey_config
                .find_by_name_mut("SwitchPreset")
                .expect("SwitchPreset default binding")
                .enabled = true;
        }

        let (status, handle, rx) = start_hotkey_backend(&settings);
        assert_eq!(status.capability, HotkeyBackendCapability::X11Available);
        assert_eq!(
            status.registrations.len(),
            7,
            "the default config always carries seven rows"
        );
        assert_eq!(
            status.registrations[0].status,
            HotkeyRegistrationStatus::Registered,
            "the probe must register SwitchPreset"
        );
        println!(
            "liveness: probe registered SwitchPreset (Ctrl+F5); the other six rows are disabled"
        );

        // Give the event-loop thread time to connect and re-grab.
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Second connection: attempt the identical grab.
        let (conn2, screen) = RustConnection::connect(None).expect("second X11 connect");
        let setup = conn2.setup();
        let root = setup.roots[screen].root;
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let mapping = conn2
            .get_keyboard_mapping(Keycode::from(min_keycode), max_keycode - min_keycode + 1)
            .expect("send get_keyboard_mapping")
            .reply()
            .expect("get_keyboard_mapping reply");
        let f5 = find_keycode_for_keysym(&mapping, min_keycode, 0xffc2).expect("F5 keycode");
        let duplicate = conn2
            .grab_key(
                false,
                root,
                ModMask::from(1u16 << 2),
                Keycode::from(f5),
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .expect("send duplicate grab_key")
            .check();
        match duplicate {
            Err(ReplyError::X11Error(X11Error {
                error_kind: ErrorKind::Access,
                ..
            })) => {
                println!(
                    "liveness: LIVE — the production event-loop thread holds the grab \
                     (duplicate grab from a second connection got BadAccess)"
                );
            }
            Ok(()) => {
                println!(
                    "liveness: NOT LIVE — the duplicate grab succeeded, so the production \
                     event-loop thread holds NO grab (its fire-and-forget grabs were silently lost)"
                );
                // Release the duplicate grab so the rest of the test
                // (and the production loop) is not confused by it.
                let _ = conn2.ungrab_key(Keycode::from(f5), root, ModMask::from(1u16 << 2));
                let _ = conn2.flush();
            }
            Err(other) => {
                panic!("liveness: duplicate grab failed unexpectedly: {other:?}");
            }
        }
        drop(conn2);

        // Regardless of liveness, try the full path once. NumLock is
        // disabled first (verified) so the exact-Control grab can fire.
        {
            let _guard = NumLockOffGuard::engage();
            xdotool_key("ctrl+F5").expect("XTEST synthesis");
            let event_rx = rx.expect("receiver");
            match event_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(event) => println!("liveness: event received on the channel: {}", event.name),
                Err(_) => println!("liveness: NO event received within 5s"),
            }

            // Deterministic, bounded teardown of the production loop: the
            // receiver is still open, so only the shutdown flag can end
            // the loop — the re-registration stop must not wait for the
            // next X event on an idle session.
            let backend = handle
                .expect("the probe registered SwitchPreset, so the backend thread must be running");
            let started = std::time::Instant::now();
            backend.stop();
            let elapsed = started.elapsed();
            println!(
                "liveness: stop() returned in {elapsed:?} (bound {HOTKEY_STOP_JOIN_TIMEOUT:?})"
            );
            assert!(
                elapsed < HOTKEY_STOP_JOIN_TIMEOUT,
                "the production backend did not stop within the bound on a live session: \
                 {elapsed:?}"
            );
            drop(event_rx);
        }
        let _ = xdotool_key("ctrl+F5");
    }

    /// Opt-in diagnostic: replay the production startup sequence
    /// (registration probe -> lookup table -> background thread
    /// with its own connection, fire-and-forget grabs) with
    /// **every** received event logged, and observe whether
    /// constructing a Slint `AppWindow` changes what the event loop
    /// receives. The replica thread keeps the blocking
    /// `wait_for_event` read on purpose — the production loop now
    /// polls (see `poll_until_event_or_shutdown`) and its thread is
    /// never joined here.
    #[cfg(feature = "x11rb-backend")]
    #[test]
    #[ignore = "real X11 session diagnostic: opt-in; run with `cargo test -p d4lc-app hotkey_real_session -- --ignored --nocapture`"]
    fn hotkey_real_session_replica_event_loop_check() {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{ConnectionExt, GrabMode, Keycode, ModMask};
        use x11rb::rust_connection::RustConnection;

        if std::env::var_os("DISPLAY").is_none() {
            panic!("replica diagnostic requires a real X11 DISPLAY");
        }

        let mut settings = d4lc_config::AppSettings::default();
        settings
            .hotkey_config
            .find_by_name_mut("SwitchPreset")
            .expect("SwitchPreset default binding")
            .enabled = true;
        let mut use_e2e_binding_set = std::env::var("D4LC_REPLICA_E2E_BINDINGS").is_ok();
        let requests = if use_e2e_binding_set {
            // Mirror the e2e binding set exactly: all default bindings
            // enabled with the same rebinds (Ctrl+F10 conflicts on this
            // session).
            println!(
                "replica: using the full e2e binding set (all default bindings enabled with rebinds)"
            );
            let mut settings = d4lc_config::AppSettings::default();
            for binding in &mut settings.hotkey_config.bindings {
                binding.enabled = true;
            }
            let rebinds: &[(&str, &str)] = &[
                ("TakeScreenshot", "Shift+F5"),
                ("ToggleController", "Alt+F5"),
                ("ToggleOverlay", "Ctrl+Shift+F5"),
                ("ToggleDebugLockScreencapture", "Ctrl+8"),
            ];
            for (name, combo) in rebinds {
                settings
                    .hotkey_config
                    .find_by_name_mut(name)
                    .expect("known action")
                    .binding = (*combo).to_string();
            }
            requests_from_settings(&settings)
        } else {
            requests_from_settings(&settings)
        };
        let _ = &mut use_e2e_binding_set;

        // Production step 1: registration probe on a temporary connection.
        let status = init_hotkeys_from(&requests);
        assert_eq!(
            status.registrations[0].status,
            HotkeyRegistrationStatus::Registered,
            "probe must register SwitchPreset"
        );
        println!("replica: probe registered SwitchPreset (Ctrl+F5)");

        // Production step 2: lookup table on a second temporary connection.
        let table = Arc::new(build_event_lookup_table(&requests));
        assert!(!table.is_empty(), "lookup table must resolve Ctrl+F5");
        println!("replica: lookup table = {table:?}");

        // Production step 3: background thread with its own connection,
        // fire-and-forget grabs (but reporting their result here),
        // wait_for_event loop with every event logged.
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let thread = std::thread::spawn(move || {
            let (conn, screen) = match RustConnection::connect(None) {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = tx.send(format!("connect error: {e}"));
                    return;
                }
            };
            let setup = conn.setup();
            let root = setup.roots[screen].root;
            // Grab exactly the table entries (production behavior) but
            // CHECK each one and report it.
            for entry in table.iter() {
                let result = match conn.grab_key(
                    false,
                    root,
                    ModMask::from(entry.modmask_bits),
                    Keycode::from(entry.keycode),
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                ) {
                    Ok(cookie) => cookie.check(),
                    Err(e) => Err(x11rb::errors::ReplyError::ConnectionError(e)),
                };
                if let Err(e) = result {
                    let _ = tx.send(format!(
                        "grab_key FAILED (keycode {} mask {:#06x}): {e:?}",
                        entry.keycode, entry.modmask_bits
                    ));
                } else {
                    let _ = tx.send(format!(
                        "grab_key ok (keycode {} mask {:#06x})",
                        entry.keycode, entry.modmask_bits
                    ));
                }
            }
            let _ = conn.flush();
            loop {
                match conn.wait_for_event() {
                    Ok(x11rb::protocol::Event::KeyPress(kp)) => {
                        let _ = tx.send(format!(
                            "KeyPress detail={} state={:#06x}",
                            kp.detail,
                            kp.state.bits()
                        ));
                    }
                    Ok(other) => {
                        let _ = tx.send(format!("other event: {other:?}"));
                    }
                    Err(e) => {
                        let _ = tx.send(format!("connection error: {e}"));
                        break;
                    }
                }
            }
            // `conn` drops here; the process exit also reaps this thread.
        });

        // Give the loop time to connect + grab.
        std::thread::sleep(std::time::Duration::from_millis(500));
        // Drain the grab-status messages.
        while let Ok(msg) = rx.try_recv() {
            println!("replica: {msg}");
        }

        let _guard = NumLockOffGuard::engage();

        // Phase A: no Slint window yet. Drain *everything* the loop
        // reports for a full window (KeyPress events may queue behind
        // MappingNotify noise).
        xdotool_key("ctrl+F5").expect("XTEST ctrl+F5 (phase A)");
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    println!("replica phase A (no AppWindow): drain window elapsed");
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(msg) => println!("replica phase A (no AppWindow): {msg}"),
                    Err(_) => {
                        println!("replica phase A (no AppWindow): channel quiet");
                        break;
                    }
                }
            }
        }

        // Phase B: construct the Slint window, then fire again and
        // drain the same way.
        let app = crate::slint_generated::AppWindow::new();
        println!(
            "replica: AppWindow::new() -> {}",
            if app.is_ok() { "ok" } else { "failed" }
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
        xdotool_key("ctrl+F5").expect("XTEST ctrl+F5 (phase B)");
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    println!("replica phase B (AppWindow present): drain window elapsed");
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(msg) => println!("replica phase B (AppWindow present): {msg}"),
                    Err(_) => {
                        println!("replica phase B (AppWindow present): channel quiet");
                        break;
                    }
                }
            }
        }

        // No join: the loop thread is only reaped with the process
        // (mirrors the production teardown contract).
        let _ = thread;
    }

    /// Run one XTEST key synthesis through `xdotool`.
    ///
    /// `--clearmodifiers` temporarily releases held modifiers and the
    /// CapsLock / NumLock toggles, sends the combo, then restores them.
    /// This is required because X11 passive grabs match the exact
    /// modifier mask: a grab registered for `Control` does not fire
    /// while NumLock adds the `Mod2` bit to the event state.
    #[cfg(feature = "x11rb-backend")]
    fn xdotool_key(combo: &str) -> Result<(), String> {
        let out = std::process::Command::new("xdotool")
            .args(["key", "--clearmodifiers", combo])
            .output()
            .map_err(|e| format!("spawn xdotool failed: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "xdotool key {combo} failed (status {:?}): {} {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ))
        }
    }

    /// Read the current NumLock indicator state via `xset q`.
    /// Best-effort: on any failure this reports `false` and the run
    /// proceeds with whatever state the session has.
    #[cfg(feature = "x11rb-backend")]
    fn numlock_is_on() -> bool {
        let Ok(out) = std::process::Command::new("xset").arg("q").output() else {
            return false;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(idx) = line.find("Num Lock:") {
                let rest = line[idx + "Num Lock:".len()..].trim_start();
                return rest.starts_with("on");
            }
        }
        false
    }

    /// RAII guard for the synthesis run: X11 passive grabs match the
    /// exact modifier mask, and this reference session runs with
    /// NumLock on (Mod2), which prevents every exact-Control grab from
    /// firing (proven by the diagnostic test). Temporarily toggle
    /// NumLock off for the run and restore the original state on drop —
    /// including on test panics.
    #[cfg(feature = "x11rb-backend")]
    struct NumLockOffGuard {
        restore: bool,
    }

    #[cfg(feature = "x11rb-backend")]
    impl NumLockOffGuard {
        fn engage() -> Self {
            let was_on = numlock_is_on();
            if was_on {
                // Toggle with verification: an XTEST press of
                // `Num_Lock` flips the server-side lock state, but
                // verify rather than assume it took effect.
                for _ in 0..3 {
                    xdotool_key("Num_Lock").expect("toggle NumLock off for the synthesis run");
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    if !numlock_is_on() {
                        break;
                    }
                }
                assert!(
                    !numlock_is_on(),
                    "could not disable NumLock for the synthesis run; \
                     exact-modifier grabs would not fire"
                );
                println!("numlock: toggled off for the synthesis run (was on; verified off)");
            }
            Self { restore: was_on }
        }
    }

    #[cfg(feature = "x11rb-backend")]
    impl Drop for NumLockOffGuard {
        fn drop(&mut self) {
            if self.restore {
                let _ = xdotool_key("Num_Lock");
                println!("numlock: restored to its original on state");
            }
        }
    }
}
