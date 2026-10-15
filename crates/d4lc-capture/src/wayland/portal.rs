//! Real ScreenCast portal boundary backed by `ashpd` (zbus).
//!
//! Implements the pure [`ScreenCastPortalOps`] boundary so the session
//! state machine drives actual DBus portal calls. All DBus conversation
//! is bridged into this synchronous crate with `zbus::block_on`, which
//! matches the workspace's async-io zbus flavor (see the crate docs on
//! why the tokio flavor is off the table).

use std::os::fd::OwnedFd;

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use enumflags2::BitFlags;
use zbus::block_on;

use super::session::{
    PersistModeChoice, PortalCapabilities, ScreenCastPortalOps, SourceSelection, StartOutcome,
    CURSOR_MODE_EMBEDDED, CURSOR_MODE_HIDDEN, CURSOR_MODE_METADATA, SOURCE_TYPE_MONITOR,
    SOURCE_TYPE_VIRTUAL, SOURCE_TYPE_WINDOW,
};
use super::WaylandCaptureError;

/// The `ashpd` ScreenCast portal. One instance drives one session; the
/// state machine owns the step order.
pub(crate) struct AshpdScreenCastPortal {
    screencast: Screencast,
    session: Option<Session<Screencast>>,
}

impl AshpdScreenCastPortal {
    /// Connect to the portal service. Fails with
    /// [`WaylandCaptureError::PortalUnavailable`] when the session bus or
    /// the `org.freedesktop.portal.ScreenCast` interface is unreachable —
    /// the structured "missing piece" for builds on systems without a
    /// portal.
    pub(crate) fn new() -> Result<Self, WaylandCaptureError> {
        let screencast = block_on(Screencast::new()).map_err(|error| {
            WaylandCaptureError::PortalUnavailable {
                reason: format!(
                    "org.freedesktop.portal.ScreenCast is unreachable on the session bus: {error}"
                ),
            }
        })?;
        Ok(Self {
            screencast,
            session: None,
        })
    }

    /// Take the established session out of the adapter so the frame
    /// source owns (and later closes) it.
    pub(crate) fn take_session(&mut self) -> Option<Session<Screencast>> {
        self.session.take()
    }
}

impl ScreenCastPortalOps for AshpdScreenCastPortal {
    fn capabilities(&mut self) -> Result<PortalCapabilities, WaylandCaptureError> {
        let source_types = block_on(self.screencast.available_source_types()).map_err(|error| {
            WaylandCaptureError::SessionFailed {
                step: "AvailableSourceTypes",
                reason: error.to_string(),
            }
        })?;
        let cursor_modes = block_on(self.screencast.available_cursor_modes()).map_err(|error| {
            WaylandCaptureError::SessionFailed {
                step: "AvailableCursorModes",
                reason: error.to_string(),
            }
        })?;
        Ok(PortalCapabilities {
            source_types: source_types_to_mask(source_types),
            cursor_modes: cursor_modes_to_mask(cursor_modes),
            version: self.screencast.version(),
        })
    }

    fn create_session(&mut self) -> Result<(), WaylandCaptureError> {
        let session =
            block_on(self.screencast.create_session(Default::default())).map_err(|error| {
                WaylandCaptureError::SessionFailed {
                    step: "CreateSession",
                    reason: error.to_string(),
                }
            })?;
        self.session = Some(session);
        Ok(())
    }

    fn select_sources(&mut self, selection: SourceSelection) -> Result<(), WaylandCaptureError> {
        let Some(session) = self.session.as_ref() else {
            return Err(WaylandCaptureError::InvalidSessionState {
                expected: "a created session before SelectSources",
            });
        };
        let mut options = SelectSourcesOptions::default()
            .set_sources(source_type_flags(selection.source_types))
            .set_cursor_mode(cursor_mode_flag(selection.cursor_mode))
            .set_multiple(selection.multiple);
        if let Some(persist_mode) = selection.persist_mode {
            options = options.set_persist_mode(persist_mode_flag(persist_mode));
        }
        if let Some(token) = selection.restore_token.as_deref() {
            options = options.set_restore_token(Some(token));
        }
        block_on(self.screencast.select_sources(session, options)).map_err(|error| {
            WaylandCaptureError::SessionFailed {
                step: "SelectSources",
                reason: error.to_string(),
            }
        })?;
        Ok(())
    }

    fn start(&mut self) -> Result<StartOutcome, WaylandCaptureError> {
        let Some(session) = self.session.as_ref() else {
            return Err(WaylandCaptureError::InvalidSessionState {
                expected: "a configured session before Start",
            });
        };
        // No window identifier: d4lc has no Wayland window of its own to
        // associate with the consent dialog.
        let response = block_on(self.screencast.start(session, None, Default::default()))
            .map_err(|error| WaylandCaptureError::SessionFailed {
                step: "Start",
                reason: error.to_string(),
            })?
            .response()
            .map_err(|error| WaylandCaptureError::SessionFailed {
                step: "Start",
                reason: format!("portal request failed or was cancelled: {error}"),
            })?;

        let streams = response.streams();
        let first = streams
            .first()
            .ok_or_else(|| WaylandCaptureError::SessionFailed {
                step: "Start",
                reason: "the user selection contains no screencast stream".to_string(),
            })?;
        Ok(StartOutcome {
            node_id: first.pipe_wire_node_id(),
            stream_size: first.size(),
            stream_position: first.position(),
            restore_token: response.restore_token().map(str::to_string),
        })
    }

    fn open_pipe_wire_remote(&mut self) -> Result<OwnedFd, WaylandCaptureError> {
        let Some(session) = self.session.as_ref() else {
            return Err(WaylandCaptureError::InvalidSessionState {
                expected: "a started session before OpenPipeWireRemote",
            });
        };
        block_on(
            self.screencast
                .open_pipe_wire_remote(session, Default::default()),
        )
        .map_err(|error| WaylandCaptureError::PipeWireRemoteFailed {
            reason: error.to_string(),
        })
    }

    fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            // Best effort: a failed close only means the portal cleans
            // the session up on its own; it must not mask the original
            // failure.
            let _ = block_on(session.close());
        }
    }
}

fn source_types_to_mask(flags: BitFlags<SourceType>) -> u32 {
    let mut mask = 0;
    if flags.contains(SourceType::Monitor) {
        mask |= SOURCE_TYPE_MONITOR;
    }
    if flags.contains(SourceType::Window) {
        mask |= SOURCE_TYPE_WINDOW;
    }
    if flags.contains(SourceType::Virtual) {
        mask |= SOURCE_TYPE_VIRTUAL;
    }
    mask
}

fn source_type_flags(mask: u32) -> BitFlags<SourceType> {
    let mut flags = BitFlags::empty();
    if mask & SOURCE_TYPE_MONITOR != 0 {
        flags |= SourceType::Monitor;
    }
    if mask & SOURCE_TYPE_WINDOW != 0 {
        flags |= SourceType::Window;
    }
    if mask & SOURCE_TYPE_VIRTUAL != 0 {
        flags |= SourceType::Virtual;
    }
    flags
}

fn cursor_modes_to_mask(flags: BitFlags<CursorMode>) -> u32 {
    let mut mask = 0;
    if flags.contains(CursorMode::Hidden) {
        mask |= CURSOR_MODE_HIDDEN;
    }
    if flags.contains(CursorMode::Embedded) {
        mask |= CURSOR_MODE_EMBEDDED;
    }
    if flags.contains(CursorMode::Metadata) {
        mask |= CURSOR_MODE_METADATA;
    }
    mask
}

/// The session is established with exactly one cursor mode (the state
/// machine already picked the best advertised one).
fn cursor_mode_flag(mask: u32) -> CursorMode {
    if mask & CURSOR_MODE_METADATA != 0 {
        CursorMode::Metadata
    } else if mask & CURSOR_MODE_EMBEDDED != 0 {
        CursorMode::Embedded
    } else {
        CursorMode::Hidden
    }
}

fn persist_mode_flag(choice: PersistModeChoice) -> PersistMode {
    match choice {
        PersistModeChoice::DoNot => PersistMode::DoNot,
        PersistModeChoice::Application => PersistMode::Application,
        PersistModeChoice::ExplicitlyRevoked => PersistMode::ExplicitlyRevoked,
    }
}
