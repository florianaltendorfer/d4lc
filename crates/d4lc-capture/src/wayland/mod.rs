//! Wayland capture via the `org.freedesktop.portal.ScreenCast` portal and
//! PipeWire.
//!
//! Layout:
//!
//! - [`session`](crate::wayland::session) (pure): the portal session state machine, the
//!   advertised-modes discipline, and the single-use restore-token
//!   rotation behind an injectable portal boundary. Compiled in every
//!   build and unit-tested without a Wayland session.
//! - [`frame`](crate::wayland::frame) (pure): the BGRx/BGRA SHM → `CaptureFrame` (BGRA)
//!   conversion with per-frame cost accounting. Compiled in every build.
//! - `portal` (behind the `wayland-portal` build feature): the real
//!   `ashpd` portal implementation of the session boundary.
//! - `pipewire_stream` (same feature): the PipeWire stream pump thread
//!   that negotiates the SHM format and feeds the conversion.
//! - `source` (same feature): `WaylandFrameSource`, the frame source
//!   the runtime dispatch injects for a Wayland display backend.
//!
//! The pure modules compile without the `ashpd`/`pipewire` dependencies
//! so default X11 builds never link libpipewire.
//!
//! Window selection happens in the
//! compositor dialog (title/pid discovery has no Wayland equivalent), a
//! monitor stream plus in-process cropping covers ROI requests, and the
//! cursor state is reported unavailable when the negotiated mode is
//! Hidden instead of pretending.

pub mod frame;
pub mod session;

#[cfg(feature = "wayland-portal")]
mod pipewire_stream;
#[cfg(feature = "wayland-portal")]
mod portal;
#[cfg(feature = "wayland-portal")]
mod source;

#[cfg(feature = "wayland-portal")]
pub use source::{
    WaylandCapture, WaylandCursorStatus, WaylandFrameSource, WaylandFrameSourceConfig,
};

pub use frame::{
    convert_into_capture_frame, convert_to_capture_frame, plan_capture, CapturePlan,
    FrameBufferPool, FrameConversionStats, LatestFrameSlot, NegotiatedRate, PortalFrameFormat,
    RateMeter, RawPortalFrame, SlotTake, StreamRateStatus, UnmappedBufferCounter,
    FRAME_BUFFER_POOL_CAPACITY, MAX_STREAM_FPS, STREAM_TARGET_INTERVAL, UNMAPPED_BUFFER_LIMIT,
};
pub use session::{
    failed_restore_token_rotation, intersect_source_types, resolve_cursor_mode,
    resolve_persist_mode, validate_stream_node_id, PortalCapabilities, PortalSessionGate,
    RestoreTokenState, RestoreTokenUpdate, SessionRequest, SessionState, SessionStateMachine,
    SourceSelection, StartOutcome, PW_ID_ANY,
};

/// Structured failure of the Wayland capture path. Every variant names
/// the missing or failed piece so callers can surface it instead of
/// silently falling back to X11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaylandCaptureError {
    /// The ScreenCast portal is unreachable (no `xdg-desktop-portal`
    /// service, or the session bus is unavailable).
    PortalUnavailable { reason: String },
    /// The portal advertises none of the requested source types.
    NoSupportedSourceType { requested: u32, advertised: u32 },
    /// The portal advertises none of the known cursor modes.
    NoSupportedCursorMode { advertised: u32 },
    /// A portal session step failed (`CreateSession`, `SelectSources`,
    /// `Start`) or the user dismissed the compositor dialog.
    SessionFailed { step: &'static str, reason: String },
    /// `OpenPipeWireRemote` failed.
    PipeWireRemoteFailed { reason: String },
    /// The PipeWire stream could not be set up or delivered frames.
    PipeWireStreamFailed { reason: String },
    /// The portal returned a PipeWire stream target that can never be
    /// connected (a zero or `PW_ID_ANY` sentinel node id) — feeding it to
    /// PipeWire would only surface as a remote "target not found" error.
    InvalidStreamTarget { node_id: u32 },
    /// The process already holds a live ScreenCast portal session. Portal
    /// sessions are exclusive per process: a second session sharing the
    /// persisted restore token tears down the first stream's target
    /// (observed as `xdg-desktop-portal-kde: Stream error: target not
    /// found`).
    SessionConflict { detail: String },
    /// The compositor delivered only buffers d4lc cannot map:
    /// `consecutive` unmapped buffers in a row, the last of data type
    /// `data_type` (DMA-BUF when the compositor ignored the mappable-memory
    /// request). DMA-BUF import is not implemented — a gap.
    MappableMemoryUnavailable { consecutive: u32, data_type: String },
    /// The state machine was used out of order (internal contract).
    InvalidSessionState { expected: &'static str },
}

impl WaylandCaptureError {
    /// The human-facing piece this error names, for logs and the
    /// runtime status surface.
    pub fn missing_piece(&self) -> &'static str {
        match self {
            Self::PortalUnavailable { .. } => "ScreenCast portal (xdg-desktop-portal)",
            Self::NoSupportedSourceType { .. } => "advertised source types",
            Self::NoSupportedCursorMode { .. } => "advertised cursor modes",
            Self::SessionFailed { .. } => "portal session",
            Self::PipeWireRemoteFailed { .. } => "PipeWire remote",
            Self::PipeWireStreamFailed { .. } => "PipeWire stream",
            Self::InvalidStreamTarget { .. } => "a valid PipeWire stream node id",
            Self::SessionConflict { .. } => "an exclusive ScreenCast session slot",
            Self::MappableMemoryUnavailable { .. } => {
                "mappable (MemFd/MemPtr) stream buffers; DMA-BUF import is not supported"
            }
            Self::InvalidSessionState { .. } => "session state machine",
        }
    }

    /// Whether the failed attempt reached `Start` — the step at which the
    /// portal validates and consumes a restore token. The caller uses this
    /// to decide the failed-attempt token rotation
    /// ([`crate::wayland::session::failed_restore_token_rotation`]): a failure
    /// before `Start` leaves the stored token untouched (the portal never
    /// saw it), while a failure at or after `Start` consumed it, and
    /// retrying the same value would hammer a dead token.
    pub fn restore_attempted(&self) -> bool {
        match self {
            Self::SessionFailed { step, .. } => {
                matches!(*step, "Start" | "OpenPipeWireRemote")
            }
            Self::PipeWireRemoteFailed { .. }
            | Self::PipeWireStreamFailed { .. }
            | Self::InvalidStreamTarget { .. }
            | Self::MappableMemoryUnavailable { .. } => true,
            Self::PortalUnavailable { .. }
            | Self::NoSupportedSourceType { .. }
            | Self::NoSupportedCursorMode { .. }
            | Self::SessionConflict { .. }
            | Self::InvalidSessionState { .. } => false,
        }
    }
}

impl std::fmt::Display for WaylandCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortalUnavailable { reason } => {
                write!(f, "wayland ScreenCast portal unavailable: {reason}")
            }
            Self::NoSupportedSourceType { requested, advertised } => write!(
                f,
                "portal advertises none of the requested source types (requested 0x{requested:x}, advertised 0x{advertised:x})"
            ),
            Self::NoSupportedCursorMode { advertised } => write!(
                f,
                "portal advertises no usable cursor mode (advertised 0x{advertised:x})"
            ),
            Self::SessionFailed { step, reason } => {
                write!(f, "portal session step {step} failed: {reason}")
            }
            Self::PipeWireRemoteFailed { reason } => {
                write!(f, "PipeWire remote could not be opened: {reason}")
            }
            Self::PipeWireStreamFailed { reason } => {
                write!(f, "PipeWire stream failed: {reason}")
            }
            Self::InvalidStreamTarget { node_id } => write!(
                f,
                "the portal returned an invalid PipeWire stream target (node id {node_id}); \
                 no ScreenCast stream can be connected to it"
            ),
            Self::SessionConflict { detail } => write!(
                f,
                "another ScreenCast session is active in this process ({detail}); \
                 the previous capture source must be fully closed before a new \
                 portal session can start"
            ),
            Self::MappableMemoryUnavailable {
                consecutive,
                data_type,
            } => write!(
                f,
                "the compositor delivered {consecutive} consecutive stream buffers d4lc cannot \
                 map (last data type {data_type}) although mappable MemFd/MemPtr memory was \
                 requested; DMA-BUF import is not supported by d4lc (known gap)"
            ),
            Self::InvalidSessionState { expected } => {
                write!(f, "session state machine used out of order; expected {expected}")
            }
        }
    }
}

impl std::error::Error for WaylandCaptureError {}
