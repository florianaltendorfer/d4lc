//! Pure ScreenCast session state machine and restore-token rotation.
//!
//! The portal conversation (`CreateSession` → `SelectSources` → `Start` →
//! `OpenPipeWireRemote`) is driven here as a state machine over the
//! injectable [`ScreenCastPortalOps`] boundary. The real implementation
//! (`crate::wayland::portal`, behind the `wayland-portal` build feature)
//! wraps `ashpd`; tests inject a scripted portal so the ordering, the
//! advertised-modes discipline, and the single-use restore-token rotation
//! are unit-tested without any Wayland session.
//!
//! Portal rules encoded here (ScreenCast portal spec):
//!
//! - Never request a source type or cursor mode the backend does not
//!   advertise — the portal closes the session in response.
//! - `persist_mode` and `restore_token` need portal interface version >= 4;
//!   older backends must not be asked.
//! - Restore tokens are single-use: every successful `Start` hands out a
//!   fresh token that must replace the stored one, and a stored token that
//!   was just consumed without replacement must be dropped. A failed
//!   `Start` consumed the stored token just the same — the caller rotates
//!   it to `Clear` ([`failed_restore_token_rotation`]) so the next attempt
//!   shows a fresh dialog instead of hammering a dead token.
//! - The `Start`-returned node id must be a real PipeWire node; the
//!   `0` / `PW_ID_ANY` sentinels are refused at the boundary instead of
//!   reaching `pw_stream_connect` as a dead target.
//! - Portal sessions are exclusive per process
//!   ([`PortalSessionGate`]): a second session sharing the persisted
//!   restore token tears down the first stream's target on the compositor
//!   side.

use std::fmt;
use std::os::fd::OwnedFd;

use super::WaylandCaptureError;

/// Portal source types (bitmask): Monitor = 1, Window = 2, Virtual = 4.
pub const SOURCE_TYPE_MONITOR: u32 = 1;
pub const SOURCE_TYPE_WINDOW: u32 = 2;
pub const SOURCE_TYPE_VIRTUAL: u32 = 4;

/// Portal cursor modes (bitmask): Hidden = 1, Embedded = 2, Metadata = 4.
pub const CURSOR_MODE_HIDDEN: u32 = 1;
pub const CURSOR_MODE_EMBEDDED: u32 = 2;
pub const CURSOR_MODE_METADATA: u32 = 4;

/// ScreenCast portal interface version that introduced `persist_mode` and
/// `restore_token`; older backends refuse both.
pub const PORTAL_PERSIST_INTERFACE_VERSION: u32 = 4;

/// PipeWire's `PW_ID_ANY` sentinel. The portal must hand out the concrete
/// node id of the selected stream; either sentinel can never be connected.
pub const PW_ID_ANY: u32 = u32::MAX;

/// What the portal advertises for the current session bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalCapabilities {
    /// Advertised source types (bitmask of `SOURCE_TYPE_*`).
    pub source_types: u32,
    /// Advertised cursor modes (bitmask of `CURSOR_MODE_*`).
    pub cursor_modes: u32,
    /// ScreenCast portal interface version.
    pub version: u32,
}

/// Persist-mode choice sent with `SelectSources` (interface version >= 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistModeChoice {
    /// Do not persist (portal default).
    DoNot,
    /// Persist while the application runs.
    Application,
    /// Persist until explicitly revoked — the mode d4lc requests so the
    /// compositor keeps honoring the restore token across restarts.
    ExplicitlyRevoked,
}

/// The source/cursor selection the state machine computes from the
/// advertised capabilities. `restore_token` travels with `SelectSources`
/// per the portal spec (it is not a `Start` argument).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSelection {
    pub source_types: u32,
    pub cursor_mode: u32,
    pub persist_mode: Option<PersistModeChoice>,
    pub multiple: bool,
    pub restore_token: Option<String>,
}

/// What the caller asks the state machine to establish.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRequest {
    /// Requested source types (bitmask); intersected with the advertised
    /// set before anything is sent to the portal.
    pub source_types: u32,
    /// Allow selecting multiple sources in the compositor dialog.
    pub multiple: bool,
    /// The stored restore token from the previous successful session, if
    /// any. Only forwarded to portals that support restore (version >= 4).
    pub restore_token: Option<String>,
}

/// Outcome of a successful portal `Start`: the first selected stream plus
/// the fresh (single-use) restore token, if the portal issued one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartOutcome {
    pub node_id: u32,
    /// Reported size of the first stream, when the portal provides one.
    pub stream_size: Option<(i32, i32)>,
    /// Reported global position of the first stream, when the portal
    /// provides one (multi-monitor logical coordinates).
    pub stream_position: Option<(i32, i32)>,
    pub restore_token: Option<String>,
}

/// Everything the frame source needs from an established portal session.
#[derive(Debug)]
pub struct EstablishedSession {
    /// PipeWire node id of the first selected stream.
    pub node_id: u32,
    /// PipeWire remote file descriptor for `pw_context_connect_fd`.
    pub pipe_wire_fd: OwnedFd,
    /// Cursor mode the session was established with (already intersected
    /// with the advertised modes).
    pub negotiated_cursor_mode: u32,
    /// Fresh restore token from `Start`, when the portal issued one.
    pub new_restore_token: Option<String>,
    /// Reported size of the first stream, if any.
    pub stream_size: Option<(i32, i32)>,
    /// Reported global position of the first stream, if any.
    pub stream_position: Option<(i32, i32)>,
}

/// The portal operations the state machine drives. The real
/// implementation performs the DBus calls via `ashpd`; tests script it.
pub trait ScreenCastPortalOps {
    fn capabilities(&mut self) -> Result<PortalCapabilities, WaylandCaptureError>;
    fn create_session(&mut self) -> Result<(), WaylandCaptureError>;
    fn select_sources(&mut self, selection: SourceSelection) -> Result<(), WaylandCaptureError>;
    fn start(&mut self) -> Result<StartOutcome, WaylandCaptureError>;
    fn open_pipe_wire_remote(&mut self) -> Result<OwnedFd, WaylandCaptureError>;
    /// Close the portal session (best effort; used on failure teardown).
    fn close_session(&mut self);
}

/// Session state machine progress. Advancement happens only through
/// [`SessionStateMachine::establish`]; every step requires the previous
/// state so a partial session can never skip the advertised-modes
/// discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No portal session exists yet.
    Idle,
    /// `CreateSession` succeeded; the session handle exists.
    Created,
    /// `SelectSources` succeeded (allowed once per session).
    SourcesSelected,
    /// `Start` succeeded; the user completed the compositor dialog.
    Started,
    /// `OpenPipeWireRemote` succeeded; the stream fd is usable.
    Active,
}

/// Drives one portal session through its mandatory step order.
pub struct SessionStateMachine<'a, P: ScreenCastPortalOps> {
    portal: &'a mut P,
    state: SessionState,
}

impl<'a, P: ScreenCastPortalOps> SessionStateMachine<'a, P> {
    pub fn new(portal: &'a mut P) -> Self {
        Self {
            portal,
            state: SessionState::Idle,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Establish a full portal session: query capabilities, intersect the
    /// request with what is advertised, then run
    /// CreateSession → SelectSources → Start → OpenPipeWireRemote in
    /// order. Any failure after `CreateSession` closes the portal session
    /// before returning, so no half-open session is leaked.
    ///
    /// # Errors
    ///
    /// Returns a structured [`WaylandCaptureError`] naming the missing
    /// piece: an unreachable portal, an empty source-type intersection,
    /// unusable cursor modes, or a failed session step.
    pub fn establish(
        &mut self,
        request: &SessionRequest,
    ) -> Result<EstablishedSession, WaylandCaptureError> {
        if self.state != SessionState::Idle {
            return Err(WaylandCaptureError::InvalidSessionState {
                expected: "idle (fresh state machine per session)",
            });
        }

        let capabilities = self.portal.capabilities()?;
        let source_types = intersect_source_types(request.source_types, capabilities.source_types)?;
        let cursor_mode = resolve_cursor_mode(capabilities.cursor_modes)?;
        let persist_mode = resolve_persist_mode(capabilities.version);
        let restore_token = if capabilities.version >= PORTAL_PERSIST_INTERFACE_VERSION {
            request.restore_token.clone()
        } else {
            None
        };

        self.portal.create_session()?;
        self.state = SessionState::Created;

        if let Err(error) = self.portal.select_sources(SourceSelection {
            source_types,
            cursor_mode,
            persist_mode,
            multiple: request.multiple,
            restore_token,
        }) {
            self.abandon();
            return Err(error);
        }
        self.state = SessionState::SourcesSelected;

        let outcome = match self.portal.start() {
            Ok(outcome) => outcome,
            Err(error) => {
                self.abandon();
                return Err(error);
            }
        };
        self.state = SessionState::Started;

        // The node id flows straight into `pw_stream_connect`; feeding it a
        // sentinel would only surface later as a PipeWire remote
        // "target not found" error. Refuse it at the portal boundary.
        if let Err(error) = validate_stream_node_id(outcome.node_id) {
            self.abandon();
            return Err(error);
        }

        let pipe_wire_fd = match self.portal.open_pipe_wire_remote() {
            Ok(fd) => fd,
            Err(error) => {
                self.abandon();
                return Err(error);
            }
        };
        self.state = SessionState::Active;

        Ok(EstablishedSession {
            node_id: outcome.node_id,
            pipe_wire_fd,
            negotiated_cursor_mode: cursor_mode,
            new_restore_token: outcome.restore_token,
            stream_size: outcome.stream_size,
            stream_position: outcome.stream_position,
        })
    }

    /// Close the portal session and reset to `Idle` after a mid-session
    /// failure — never leave a half-open session behind.
    fn abandon(&mut self) {
        self.portal.close_session();
        self.state = SessionState::Idle;
    }
}

/// Intersect the requested source types with what the portal advertises.
///
/// # Errors
///
/// Returns `NoSupportedSourceType` naming both bitmasks when the
/// intersection is empty — requesting an unadvertised type would close
/// the session.
pub fn intersect_source_types(requested: u32, advertised: u32) -> Result<u32, WaylandCaptureError> {
    let supported = requested & advertised;
    if supported == 0 {
        return Err(WaylandCaptureError::NoSupportedSourceType {
            requested,
            advertised,
        });
    }
    Ok(supported)
}

/// Pick the best advertised cursor mode: Metadata (4) when offered so
/// cursor presence can be reported, else Embedded (2), else the
/// always-safe Hidden (1).
///
/// # Errors
///
/// Returns `NoSupportedCursorMode` when the portal advertises none of the
/// known modes — requesting an unadvertised mode would close the session.
pub fn resolve_cursor_mode(advertised: u32) -> Result<u32, WaylandCaptureError> {
    for candidate in [
        CURSOR_MODE_METADATA,
        CURSOR_MODE_EMBEDDED,
        CURSOR_MODE_HIDDEN,
    ] {
        if advertised & candidate != 0 {
            return Ok(candidate);
        }
    }
    Err(WaylandCaptureError::NoSupportedCursorMode { advertised })
}

/// `persist_mode` requires portal interface version >= 4; older backends
/// must not be asked (they close the session). Version >= 4 requests
/// `ExplicitlyRevoked` so the stored token stays valid across restarts.
pub fn resolve_persist_mode(portal_version: u32) -> Option<PersistModeChoice> {
    if portal_version >= PORTAL_PERSIST_INTERFACE_VERSION {
        Some(PersistModeChoice::ExplicitlyRevoked)
    } else {
        None
    }
}

/// Validate the stream node id the portal returned with `Start` before it
/// feeds `pw_stream_connect`: a zero id or the `PW_ID_ANY` sentinel has no
/// PipeWire node behind it and would only fail later as a remote
/// "target not found" error with no compositor screencast node in sight.
///
/// # Errors
///
/// Returns [`WaylandCaptureError::InvalidStreamTarget`] for a sentinel id.
pub fn validate_stream_node_id(node_id: u32) -> Result<(), WaylandCaptureError> {
    if node_id == 0 || node_id == PW_ID_ANY {
        return Err(WaylandCaptureError::InvalidStreamTarget { node_id });
    }
    Ok(())
}

/// The persistence effect of rotating to a fresh token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreTokenUpdate {
    /// Nothing changed; the stored value stays.
    Keep,
    /// Persist this fresh token (replaces any stored value).
    Store(String),
    /// Drop the stored token: it was single-use, was just consumed, and
    /// the portal did not replace it. Keeping it would poison the next
    /// start with an already-consumed token.
    Clear,
}

/// Single-use restore-token rotation state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreTokenState {
    current: Option<String>,
}

impl RestoreTokenState {
    pub fn new(stored: Option<String>) -> Self {
        Self { current: stored }
    }

    /// The token a restore-capable portal may be handed.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Rotate after one session outcome. Tokens are single-use: every
    /// successful session must end up storing the fresh token, and a
    /// consumed token without replacement must be cleared.
    pub fn rotate(&mut self, fresh: Option<String>) -> RestoreTokenUpdate {
        match fresh {
            Some(token) => {
                if self.current.as_deref() == Some(token.as_str()) {
                    RestoreTokenUpdate::Keep
                } else {
                    self.current = Some(token.clone());
                    RestoreTokenUpdate::Store(token)
                }
            }
            None => {
                if self.current.take().is_some() {
                    RestoreTokenUpdate::Clear
                } else {
                    RestoreTokenUpdate::Keep
                }
            }
        }
    }
}

impl fmt::Display for PersistModeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DoNot => write!(f, "do-not"),
            Self::Application => write!(f, "application"),
            Self::ExplicitlyRevoked => write!(f, "explicitly-revoked"),
        }
    }
}

/// Pure single-holder gate behind the process-wide portal-session slot.
///
/// ScreenCast portal sessions are exclusive per process: concurrent
/// sessions sharing the persisted restore token tear down each other's
/// stream target on the compositor side (observed live as
/// `xdg-desktop-portal-kde: Stream error: target not found`, with no
/// screencast node left afterwards). A holder acquires a generation token
/// and keeps it for the whole lifetime of its frame source — the real
/// slot (`source.rs`) releases it only after the source's portal session
/// is closed and its PipeWire pump thread is torn down, so a new
/// `WaylandFrameSource::connect` can never overlap a previous one.
#[derive(Debug, Default)]
pub struct PortalSessionGate {
    /// Generation of the current holder, if any.
    holder: Option<u64>,
    next_generation: u64,
}

impl PortalSessionGate {
    /// Try to take the exclusive session slot. The returned generation
    /// identifies this holder for [`PortalSessionGate::release`].
    ///
    /// # Errors
    ///
    /// Returns [`WaylandCaptureError::SessionConflict`] while another
    /// holder has not released the slot yet.
    pub fn try_acquire(&mut self) -> Result<u64, WaylandCaptureError> {
        if self.holder.is_some() {
            return Err(WaylandCaptureError::SessionConflict {
                detail: "a previous connect holds the portal-session slot".to_string(),
            });
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        self.holder = Some(generation);
        Ok(generation)
    }

    /// Release the slot. A generation that is not the current holder's is
    /// ignored — releasing a slot you do not hold must never free someone
    /// else's session. Returns whether this release freed the slot.
    pub fn release(&mut self, generation: u64) -> bool {
        if self.holder == Some(generation) {
            self.holder = None;
            true
        } else {
            false
        }
    }

    /// Whether a holder currently occupies the slot.
    pub fn is_held(&self) -> bool {
        self.holder.is_some()
    }
}

/// The restore-token rotation a FAILED establishment attempt leaves the
/// persisted value with.
///
/// Restore tokens are single-use and are validated and consumed by the
/// portal at `Start`: a failure at or after `Start`
/// ([`WaylandCaptureError::restore_attempted`]) burned the stored token,
/// and retrying the same value would hammer a consumed token instead of
/// showing a fresh compositor dialog — so it rotates to
/// [`RestoreTokenUpdate::Clear`]. A failure before `Start` never reached
/// the portal with the token and keeps the stored value
/// ([`RestoreTokenUpdate::Keep`]). With nothing stored the update is a
/// no-op either way.
pub fn failed_restore_token_rotation(
    stored_token: Option<String>,
    failure: &WaylandCaptureError,
) -> RestoreTokenUpdate {
    if failure.restore_attempted() {
        RestoreTokenState::new(stored_token).rotate(None)
    } else {
        RestoreTokenUpdate::Keep
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use std::cell::Cell;

    // ── Advertised-modes discipline ──────────────────────────────

    #[test]
    fn source_types_intersect_with_the_advertised_set_and_name_both_on_empty() {
        let requested = SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW;
        assert_eq!(
            intersect_source_types(requested, SOURCE_TYPE_MONITOR).unwrap(),
            SOURCE_TYPE_MONITOR,
            "only the advertised part may be requested"
        );
        assert_eq!(
            intersect_source_types(requested, requested).unwrap(),
            requested
        );

        let Err(error) = intersect_source_types(requested, SOURCE_TYPE_VIRTUAL) else {
            panic!("empty intersection must be refused");
        };
        match error {
            WaylandCaptureError::NoSupportedSourceType {
                requested,
                advertised,
            } => {
                assert_eq!(requested, SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW);
                assert_eq!(advertised, SOURCE_TYPE_VIRTUAL);
            }
            other => panic!("wrong error variant: {other:?}"),
        }
    }

    #[test]
    fn cursor_mode_prefers_metadata_then_embedded_then_hidden_and_refuses_unknown() {
        let all = CURSOR_MODE_HIDDEN | CURSOR_MODE_EMBEDDED | CURSOR_MODE_METADATA;
        assert_eq!(resolve_cursor_mode(all).unwrap(), CURSOR_MODE_METADATA);
        assert_eq!(
            resolve_cursor_mode(CURSOR_MODE_HIDDEN | CURSOR_MODE_EMBEDDED).unwrap(),
            CURSOR_MODE_EMBEDDED
        );
        assert_eq!(
            resolve_cursor_mode(CURSOR_MODE_HIDDEN).unwrap(),
            CURSOR_MODE_HIDDEN
        );

        let Err(error) = resolve_cursor_mode(0) else {
            panic!("no advertised mode must be refused");
        };
        assert!(
            error.to_string().contains("cursor"),
            "structured error must name the missing piece: {error}"
        );
    }

    #[test]
    fn persist_mode_is_version_gated() {
        // Interface version 4 introduced persist_mode/restore_token;
        // older backends must not be asked at all.
        assert_eq!(resolve_persist_mode(1), None);
        assert_eq!(resolve_persist_mode(3), None);
        assert_eq!(
            resolve_persist_mode(PORTAL_PERSIST_INTERFACE_VERSION),
            Some(PersistModeChoice::ExplicitlyRevoked)
        );
        assert_eq!(
            resolve_persist_mode(5),
            Some(PersistModeChoice::ExplicitlyRevoked)
        );
    }

    // ── Single-use restore-token rotation ────────────────────────

    #[test]
    fn restore_token_rotation_is_single_use() {
        let mut tokens = RestoreTokenState::new(Some("used-on-first-run".to_string()));

        // A successful session replaces the stored token: the old one was
        // consumed by this Start and must never survive.
        let update = tokens.rotate(Some("fresh-token".to_string()));
        assert_eq!(update, RestoreTokenUpdate::Store("fresh-token".to_string()));
        assert_eq!(tokens.current(), Some("fresh-token"));

        // A successful session without a fresh token consumed the stored
        // one — keeping it would poison the next start, so it is cleared.
        let update = tokens.rotate(None);
        assert_eq!(update, RestoreTokenUpdate::Clear);
        assert_eq!(tokens.current(), None);

        // Nothing stored and nothing issued: nothing to persist.
        let update = tokens.rotate(None);
        assert_eq!(update, RestoreTokenUpdate::Keep);

        // First successful session from a fresh install stores its token.
        let update = tokens.rotate(Some("first-ever".to_string()));
        assert_eq!(update, RestoreTokenUpdate::Store("first-ever".to_string()));
    }

    #[test]
    fn failed_restore_token_rotation_clears_only_an_attempted_restore() {
        // A Start failure consumed the stored single-use token: it must be
        // cleared so the next attempt performs a fresh dialog instead of
        // hammering the dead token.
        let start_failed = WaylandCaptureError::SessionFailed {
            step: "Start",
            reason: "portal request failed or was cancelled".to_string(),
        };
        assert_eq!(
            failed_restore_token_rotation(Some("stored".to_string()), &start_failed),
            RestoreTokenUpdate::Clear
        );

        // A later failure (Start succeeded, the stream never came up)
        // burned the token the same way — as does a dead Start target.
        for failure in [
            WaylandCaptureError::PipeWireStreamFailed {
                reason: "no frame".to_string(),
            },
            WaylandCaptureError::PipeWireRemoteFailed {
                reason: "remote refused".to_string(),
            },
            WaylandCaptureError::InvalidStreamTarget { node_id: 0 },
        ] {
            assert_eq!(
                failed_restore_token_rotation(Some("stored".to_string()), &failure),
                RestoreTokenUpdate::Clear,
                "a failure at or after Start must clear the token: {failure}"
            );
        }

        // A failure before Start never reached the portal with the token:
        // the stored grant stays.
        for failure in [
            WaylandCaptureError::PortalUnavailable {
                reason: "no xdg-desktop-portal on the session bus".to_string(),
            },
            WaylandCaptureError::SessionFailed {
                step: "SelectSources",
                reason: "scripted failure".to_string(),
            },
            WaylandCaptureError::SessionConflict {
                detail: "held".to_string(),
            },
        ] {
            assert_eq!(
                failed_restore_token_rotation(Some("stored".to_string()), &failure),
                RestoreTokenUpdate::Keep,
                "a failure before Start must keep the token: {failure}"
            );
        }

        // Nothing stored: the failed rotation is a no-op either way.
        assert_eq!(
            failed_restore_token_rotation(None, &start_failed),
            RestoreTokenUpdate::Keep
        );
    }

    #[test]
    fn portal_session_gate_serializes_holders_until_release() {
        let mut gate = PortalSessionGate::default();
        assert!(!gate.is_held());

        let first = gate.try_acquire().expect("a free slot must be acquirable");
        assert!(gate.is_held());

        let Err(error) = gate.try_acquire() else {
            panic!("a held slot must refuse a second holder");
        };
        assert!(
            matches!(error, WaylandCaptureError::SessionConflict { .. }),
            "the second holder must be refused: {error}"
        );
        assert!(
            error.to_string().contains("ScreenCast session"),
            "the conflict must name the session exclusivity: {error}"
        );

        // Releasing a generation you do not hold must not free the slot.
        assert!(
            !gate.release(first.wrapping_add(1)),
            "a stale release must not free the holder's slot"
        );
        assert!(gate.is_held());

        assert!(gate.release(first));
        assert!(!gate.is_held());
        let second = gate
            .try_acquire()
            .expect("a released slot must be acquirable again");
        assert_ne!(second, first, "each holder must observe its own generation");
    }

    #[test]
    fn stream_node_id_validation_refuses_sentinels() {
        assert_eq!(validate_stream_node_id(77), Ok(()));
        for sentinel in [0, PW_ID_ANY] {
            let Err(error) = validate_stream_node_id(sentinel) else {
                panic!("node id {sentinel} must be refused");
            };
            match error {
                WaylandCaptureError::InvalidStreamTarget { node_id } => {
                    assert_eq!(node_id, sentinel);
                }
                other => panic!("wrong error variant: {other:?}"),
            }
        }
    }

    // ── State machine over a scripted portal ─────────────────────

    /// Scripted fake portal: records every call and replays the
    /// capabilities it was constructed with. The scripted `select_sources`
    /// asserts the advertised-modes discipline at the boundary itself.
    struct ScriptedPortal {
        capabilities: Result<PortalCapabilities, WaylandCaptureError>,
        seen_selection: Option<SourceSelection>,
        start_response: StartOutcome,
        fail_at: Option<&'static str>,
        close_calls: Cell<u32>,
    }

    impl ScriptedPortal {
        fn healthy(capabilities: PortalCapabilities) -> Self {
            Self {
                capabilities: Ok(capabilities),
                seen_selection: None,
                start_response: StartOutcome {
                    node_id: 77,
                    stream_size: Some((1920, 1080)),
                    stream_position: Some((0, 0)),
                    restore_token: Some("fresh-from-start".to_string()),
                },
                fail_at: None,
                close_calls: Cell::new(0),
            }
        }
    }

    impl ScreenCastPortalOps for ScriptedPortal {
        fn capabilities(&mut self) -> Result<PortalCapabilities, WaylandCaptureError> {
            self.capabilities.clone()
        }

        fn create_session(&mut self) -> Result<(), WaylandCaptureError> {
            self.step_should_fail("create_session")
        }

        fn select_sources(
            &mut self,
            selection: SourceSelection,
        ) -> Result<(), WaylandCaptureError> {
            self.seen_selection = Some(selection.clone());
            // The portal itself would close the session when asked for an
            // unadvertised mode; the state machine must never let it
            // happen.
            assert!(
                selection.source_types & self.capabilities.as_ref().unwrap().source_types
                    == selection.source_types,
                "requested unadvertised source types: {selection:?}"
            );
            assert!(
                selection.cursor_mode & self.capabilities.as_ref().unwrap().cursor_modes
                    == selection.cursor_mode,
                "requested unadvertised cursor mode: {selection:?}"
            );
            self.step_should_fail("select_sources")
        }

        fn start(&mut self) -> Result<StartOutcome, WaylandCaptureError> {
            self.step_should_fail("start")?;
            Ok(self.start_response.clone())
        }

        fn open_pipe_wire_remote(&mut self) -> Result<OwnedFd, WaylandCaptureError> {
            self.step_should_fail("open_pipe_wire_remote")?;
            // The state machine only moves an owned fd through, so any
            // valid owned fd proves the contract.
            let null = std::fs::File::open("/dev/null").map_err(|error| {
                WaylandCaptureError::PipeWireRemoteFailed {
                    reason: format!("test fixture could not open /dev/null: {error}"),
                }
            })?;
            Ok(null.into())
        }

        fn close_session(&mut self) {
            self.close_calls.set(self.close_calls.get() + 1);
        }
    }

    impl ScriptedPortal {
        fn step_should_fail(&mut self, step: &'static str) -> Result<(), WaylandCaptureError> {
            if self.fail_at == Some(step) {
                return Err(WaylandCaptureError::SessionFailed {
                    step,
                    reason: "scripted failure".to_string(),
                });
            }
            Ok(())
        }
    }

    fn monitor_capabilities() -> PortalCapabilities {
        PortalCapabilities {
            source_types: SOURCE_TYPE_MONITOR,
            cursor_modes: CURSOR_MODE_METADATA | CURSOR_MODE_HIDDEN,
            version: 4,
        }
    }

    fn request() -> SessionRequest {
        SessionRequest {
            source_types: SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW,
            multiple: false,
            restore_token: Some("stored-token".to_string()),
        }
    }

    #[test]
    fn state_machine_establishes_a_session_in_order_with_intersected_modes() {
        let mut portal = ScriptedPortal::healthy(monitor_capabilities());

        let established = {
            let mut machine = SessionStateMachine::new(&mut portal);
            let established = machine.establish(&request()).unwrap();
            assert_eq!(machine.state(), SessionState::Active);
            established
        };

        // Window was requested but the fake portal only advertises
        // Monitor: the selection must carry exactly the intersection.
        let selection = portal.seen_selection.expect("select_sources must run");
        assert_eq!(selection.source_types, SOURCE_TYPE_MONITOR);
        assert_eq!(
            selection.cursor_mode, CURSOR_MODE_METADATA,
            "the best advertised cursor mode must be requested"
        );
        assert_eq!(
            selection.persist_mode,
            Some(PersistModeChoice::ExplicitlyRevoked),
            "a version-4 portal must be asked to persist until revoked"
        );
        assert_eq!(
            selection.restore_token.as_deref(),
            Some("stored-token"),
            "a stored token must travel with SelectSources on a restore-capable portal"
        );
        assert_eq!(established.node_id, 77);
        assert_eq!(established.negotiated_cursor_mode, CURSOR_MODE_METADATA);
        assert_eq!(portal.close_calls.get(), 0);
    }

    #[test]
    fn state_machine_hides_restore_token_from_old_portals() {
        let mut capabilities = monitor_capabilities();
        capabilities.version = 3;
        let mut portal = ScriptedPortal::healthy(capabilities);

        {
            let mut machine = SessionStateMachine::new(&mut portal);
            machine.establish(&request()).unwrap();
        }

        let selection = portal.seen_selection.expect("select_sources must run");
        assert_eq!(
            selection.restore_token, None,
            "a portal older than interface version 4 must never be handed a restore token"
        );
        assert_eq!(selection.persist_mode, None);
    }

    #[test]
    fn state_machine_closes_the_session_on_mid_session_failure_and_resets() {
        for failing_step in ["select_sources", "start", "open_pipe_wire_remote"] {
            let mut portal = ScriptedPortal::healthy(monitor_capabilities());
            portal.fail_at = Some(failing_step);

            let error = {
                let mut machine = SessionStateMachine::new(&mut portal);
                let Err(error) = machine.establish(&request()) else {
                    panic!("the scripted step must fail");
                };
                assert_eq!(
                    machine.state(),
                    SessionState::Idle,
                    "a failed session must reset instead of leaking a half-open session"
                );
                error
            };

            match error {
                WaylandCaptureError::SessionFailed { step, .. } => assert_eq!(step, failing_step),
                other => panic!("wrong error for {failing_step}: {other:?}"),
            }
            assert_eq!(
                portal.close_calls.get(),
                1,
                "the portal session must be closed after a {failing_step} failure"
            );
        }
    }

    #[test]
    fn state_machine_propagates_portal_unavailability_before_any_session_exists() {
        let mut portal = ScriptedPortal {
            capabilities: Err(WaylandCaptureError::PortalUnavailable {
                reason: "no xdg-desktop-portal on the session bus".to_string(),
            }),
            ..ScriptedPortal::healthy(monitor_capabilities())
        };

        let error = {
            let mut machine = SessionStateMachine::new(&mut portal);
            let Err(error) = machine.establish(&request()) else {
                panic!("unavailable portal must fail the establishment");
            };
            assert_eq!(machine.state(), SessionState::Idle);
            error
        };

        assert!(
            error.to_string().contains("portal"),
            "the structured error must name the missing piece: {error}"
        );
        assert_eq!(
            portal.close_calls.get(),
            0,
            "nothing to close when no session was created"
        );
    }

    #[test]
    fn state_machine_refuses_reuse_and_exposes_the_fresh_token() {
        let mut portal = ScriptedPortal::healthy(monitor_capabilities());
        let mut machine = SessionStateMachine::new(&mut portal);

        let established = machine.establish(&request()).unwrap();
        assert_eq!(
            established.new_restore_token.as_deref(),
            Some("fresh-from-start"),
            "the fresh single-use token must reach the caller for rotation"
        );

        // One state machine drives exactly one session.
        let Err(error) = machine.establish(&request()) else {
            panic!("re-establishing on an active machine must be refused");
        };
        assert!(
            error.to_string().contains("idle"),
            "structured state error must name the expectation: {error}"
        );
    }

    #[test]
    fn state_machine_refuses_a_sentinel_stream_node_id_and_closes_the_session() {
        for node_id in [0, PW_ID_ANY] {
            let mut portal = ScriptedPortal::healthy(monitor_capabilities());
            portal.start_response.node_id = node_id;

            let error = {
                let mut machine = SessionStateMachine::new(&mut portal);
                let Err(error) = machine.establish(&request()) else {
                    panic!("a sentinel node id must fail the establishment");
                };
                assert_eq!(
                    machine.state(),
                    SessionState::Idle,
                    "a dead target must reset the machine instead of leaking the session"
                );
                error
            };

            assert!(
                error.to_string().contains("node id"),
                "the error must name the invalid target: {error}"
            );
            match error {
                WaylandCaptureError::InvalidStreamTarget { node_id: returned } => {
                    assert_eq!(returned, node_id)
                }
                other => panic!("wrong error for node id {node_id}: {other:?}"),
            }
            assert_eq!(
                portal.close_calls.get(),
                1,
                "a Start that returned a dead target must still close the session"
            );
        }
    }
}
