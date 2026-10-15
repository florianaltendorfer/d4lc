//! Runtime coordinator — the single owner of the analysis state.
//!
//! One state machine decides everything about the analysis state. The
//! coordinator decides, and only it decides, whether a
//! tooltip observation starts OCR, whether an OCR result is accepted, and when
//! the accepted analysis is invalidated. The frame pipeline, the OCR worker and
//! the app are adapters around it: they feed observations, results and control
//! inputs in, and read the accepted [`AnalysisSnapshot`] out.
//!
//! ## Tooltip states
//!
//! ```text
//! NoTooltip ──observe──► Observing ──stable──► Analyzing ──accept──► Ready
//!     ▲                     ▲  ▲                   │                   │
//!     │                     │  └──failed/superseded┘   new content ────┤
//!     └──── Invalidated ◄───┴── tooltip lost / focus lost / minimized /
//!                               overlay off / window lost / config change / stop
//! ```
//!
//! Every tooltip observation reaches the coordinator. Two questions
//! are answered separately:
//!
//! - **Stability**: the tooltip placement (crop rect + matched template) was
//!   observed on [`REQUIRED_STABLE_FRAMES`] consecutive frames.
//! - **Already processed content**: the pixels the accepted OCR result was
//!   read from are unchanged (see [`read_rows_unchanged`] for why this
//!   compares only the OCR-read rows, and why it ignores render noise).
//!
//! ## Validity tokens
//!
//! Every OCR job carries an [`AnalysisToken`]. A result is accepted
//! only when its session and tooltip generation are still current, its
//! configuration and window revisions still match, and it is the job the
//! coordinator is waiting for. Anything else is discarded with the concrete
//! [`DiscardReason`]. A pure rule-set change never discards an OCR result: the
//! accepted analysis stays valid and is re-evaluated against the new rules by
//! the consumer, without new OCR.
//!
//! ## Invalidation
//!
//! Tooltip loss, focus loss, minimization, overlay deactivation, window loss,
//! configuration changes and stop invalidate immediately — they bump the
//! tooltip generation and drop the accepted analysis on the spot, without
//! waiting for OCR. A late result of an invalidated generation can therefore
//! never replace the item or re-show the overlay.
//!
//! ## Bounded work
//!
//! At most one job of the current generation is outstanding: a steady tooltip
//! never queues a second job while its first one runs. A new placement may
//! queue its job behind the running one of an older generation, which the
//! worker's single replaceable pending slot bounds to one running plus one
//! waiting job.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use d4lc_core::Rect;

use crate::diagnostics::{DiagnosticsContext, DiscardReason, Revision};
use crate::pipeline::{current_item_snapshot, determine_frame_status, CurrentItemSnapshot};
use crate::{OcrJob, TooltipImage, TooltipTextSnapshot};

/// Consecutive frames a tooltip placement must be observed on before OCR
/// starts (the former scheduler's stability requirement, kept unchanged).
pub const REQUIRED_STABLE_FRAMES: u32 = 2;

/// Largest per-channel difference between two captures of unchanged tooltip
/// content (see [`read_rows_unchanged`]). The live capture moved single
/// pixels by one level on "Temerity", and by up to 25 levels beside the
/// wrapped aspect text of the chest armor "Runic Mail of Diabolical Armor";
/// OCR-relevant text, dim value ranges included, differs from the tooltip
/// background by well over 80 levels.
pub const CONTENT_NOISE_DELTA: u8 = 48;

/// Where an OCR result read text below its read top: the columns
/// `left..right` and the rows down to `bottom` (exclusive), in the
/// coordinates of the input crop (see [`read_rows_unchanged`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadExtent {
    pub left: u32,
    pub right: u32,
    pub bottom: u32,
}

impl ReadExtent {
    /// The byte range of the read columns within one row of a `width`
    /// pixels wide frame; the whole row when the extent is unknown.
    fn row_bytes(extent: Option<Self>, width: usize) -> std::ops::Range<usize> {
        let (left, right) = extent.map_or((0, width), |extent| {
            let right = usize::try_from(extent.right).unwrap_or(width).min(width);
            (usize::try_from(extent.left).unwrap_or(0).min(right), right)
        });
        left * 4..right * 4
    }

    /// The end of the compared rows in a `height` rows tall frame; the crop
    /// bottom when the extent is unknown.
    fn row_end(extent: Option<Self>, height: usize) -> usize {
        extent.map_or(height, |extent| {
            usize::try_from(extent.bottom).map_or(height, |bottom| bottom.min(height))
        })
    }
}

/// Number of ended generations whose end reason is remembered for the
/// discard reason of their late results.
const ENDED_GENERATION_MEMORY: usize = 32;

/// Tooltip analysis state of the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipState {
    /// No tooltip is observed.
    NoTooltip,
    /// A tooltip is observed but not yet stable (or its content just changed).
    Observing,
    /// An OCR job for the current generation is outstanding.
    Analyzing,
    /// The current generation has an accepted analysis.
    Ready,
    /// The analysis was invalidated (focus lost, overlay off, stop, …) and no
    /// new observation is accepted until the cause is lifted.
    Invalidated,
}

impl TooltipState {
    /// Stable short name for log lines.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoTooltip => "no_tooltip",
            Self::Observing => "observing",
            Self::Analyzing => "analyzing",
            Self::Ready => "ready",
            Self::Invalidated => "invalidated",
        }
    }
}

/// Lifecycle of a runtime session, separate from the tooltip state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLifecycle {
    /// The session is being prepared; no frame was processed yet.
    Starting,
    /// Frames are processed.
    Running,
    /// Stop was requested: the analysis is invalidated and publications are
    /// blocked, but worker threads (native OCR) may still be finishing.
    Stopping,
    /// Every thread of the session ended.
    Stopped,
    /// The session ended on an error or a panicked thread.
    Failed,
}

impl RuntimeLifecycle {
    /// Stable short name for log lines and the UI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    /// Whether the session still accepts observations and results.
    pub const fn accepts_work(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// Why the coordinator invalidated its analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationCause {
    /// The frame no longer carries the tooltip.
    TooltipLost,
    /// The game window lost the input focus.
    FocusLost,
    /// The game window is minimized.
    Minimized,
    /// The overlay was turned off.
    OverlayDisabled,
    /// The game window is gone.
    WindowLost,
    /// The frame could not be captured.
    CaptureFailed,
    /// The game window geometry changed.
    WindowChanged,
    /// An applied configuration change requires a new OCR context.
    ConfigChanged,
    /// The session stops.
    Stopped,
}

impl InvalidationCause {
    /// The diagnostics discard reason of work this cause invalidated.
    pub const fn discard_reason(self) -> DiscardReason {
        match self {
            Self::TooltipLost => DiscardReason::TooltipLost,
            Self::FocusLost | Self::Minimized => DiscardReason::FocusLost,
            Self::OverlayDisabled => DiscardReason::OverlayDisabled,
            Self::WindowLost => DiscardReason::WindowLost,
            Self::CaptureFailed => DiscardReason::CaptureFailed,
            Self::WindowChanged => DiscardReason::WindowChanged,
            Self::ConfigChanged => DiscardReason::ConfigChanged,
            Self::Stopped => DiscardReason::SessionStopped,
        }
    }

    /// Whether this cause hides the overlay regardless of a deliberately
    /// frozen debug image (focus and deactivation locks apply to a
    /// frozen image too). Tooltip loss, window-geometry and configuration
    /// changes are content changes of the live analysis instead.
    pub const fn is_hard(self) -> bool {
        match self {
            Self::FocusLost
            | Self::Minimized
            | Self::OverlayDisabled
            | Self::WindowLost
            | Self::CaptureFailed
            | Self::Stopped => true,
            Self::TooltipLost | Self::WindowChanged | Self::ConfigChanged => false,
        }
    }

    /// Stable short name for log lines.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TooltipLost => "tooltip_lost",
            Self::FocusLost => "focus_lost",
            Self::Minimized => "minimized",
            Self::OverlayDisabled => "overlay_disabled",
            Self::WindowLost => "window_lost",
            Self::CaptureFailed => "capture_failed",
            Self::WindowChanged => "window_changed",
            Self::ConfigChanged => "config_changed",
            Self::Stopped => "stopped",
        }
    }
}

/// Focus and visibility of the game window, as the game-window observer
/// reports it (X11: `_NET_ACTIVE_WINDOW`, `_NET_WM_STATE_HIDDEN`, map state
/// and window destruction). The capture path keeps its own minimized guess
/// (a capture shorter than 100 rows) apart from this report, see
/// [`RuntimeCoordinator::set_capture_minimized`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameWindowState {
    pub focused: bool,
    pub minimized: bool,
    /// The game window was destroyed.
    pub closed: bool,
}

impl Default for GameWindowState {
    fn default() -> Self {
        Self {
            focused: true,
            minimized: false,
            closed: false,
        }
    }
}

/// The validity key of one OCR job (`AnalysisToken`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnalysisToken {
    /// Analysis session the job belongs to.
    pub session_id: u64,
    /// Tooltip generation the job was planned for.
    pub tooltip_generation: u64,
    /// Frame the job's pixels were captured in.
    pub source_frame_id: u64,
    /// Capture time of the frame that opened the tooltip track this job was
    /// planned under (`first_seen_at`), on the session clock. `None`
    /// for a hand-built token that never went through the coordinator.
    pub first_seen_at_micros: Option<u64>,
    /// Fingerprint of the job's pixels ([`TooltipImage::content_fingerprint`]).
    pub content_fingerprint: u64,
    /// Applied configuration revision at planning time.
    pub config_revision: Revision,
    /// Applied rule-set revision at planning time (informational: a rule
    /// change re-evaluates, it never discards).
    pub rules_revision: Revision,
    /// Game-window revision at planning time.
    pub window_revision: u64,
}

/// One tooltip observation of one frame.
#[derive(Debug, Clone)]
pub struct TooltipObservation {
    /// Placement signature (crop rect + matched template).
    pub signature: u64,
    /// The immutable crop of this frame.
    pub image: TooltipImage,
    /// The crop rect in window space.
    pub tooltip_window_relative: Option<Rect>,
}

/// What the coordinator decided about one tooltip observation.
#[derive(Debug, Clone)]
pub enum ObserveDecision {
    /// The observation is not analyzed: the session stopped or an
    /// invalidation cause (focus lost, minimized, overlay off) is active.
    Suppressed(InvalidationCause),
    /// The placement has not been stable for [`REQUIRED_STABLE_FRAMES`] yet.
    WaitingForStability,
    /// The content at a stable placement changed; the new generation waits
    /// one frame for the new content to settle.
    ContentChanged,
    /// The accepted analysis already covers these pixels.
    AlreadyProcessed,
    /// A job of the current generation is outstanding.
    InFlight,
    /// The per-placement retry interval has not elapsed.
    RetryInterval,
    /// Publish this job.
    Submit(OcrJob),
}

impl ObserveDecision {
    /// Stable short name (diagnostics detail text).
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Suppressed(_) => "suppressed",
            Self::WaitingForStability => "waiting_for_stability",
            Self::ContentChanged => "content_changed",
            Self::AlreadyProcessed => "already_processed",
            Self::InFlight => "in_flight",
            Self::RetryInterval => "retry_interval",
            Self::Submit(_) => "submitted",
        }
    }

    /// Whether in-flight or completed work covers this observation.
    pub const fn superseded(&self) -> bool {
        matches!(
            self,
            Self::AlreadyProcessed | Self::InFlight | Self::RetryInterval
        )
    }
}

/// Outcome of one observation: the decision plus whether it started a new
/// tooltip generation.
#[derive(Debug, Clone)]
pub struct ObserveOutcome {
    pub decision: ObserveDecision,
    pub generation_changed: bool,
}

/// What the coordinator did with one OCR result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultDisposition {
    /// The result is the accepted analysis of the current generation.
    Accepted,
    /// The job failed; the generation stays current and is retried after the
    /// retry interval.
    Failed,
    /// The result is not adopted.
    Discarded(DiscardReason),
}

/// The accepted analysis of the current generation.
#[derive(Debug, Clone)]
pub struct AcceptedAnalysis {
    pub token: AnalysisToken,
    pub result: Arc<TooltipTextSnapshot>,
    /// The analyzed crop rect in window space.
    pub tooltip_window_relative: Option<Rect>,
}

/// The accepted analysis state the overlay and the UI both consume.
///
/// One snapshot per presentation generation: every change of the accepted
/// analysis (acceptance, invalidation, new content) produces a new snapshot
/// with a higher `presentation_generation`. Absence is an explicit snapshot
/// with no item, never a missing update.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisSnapshot {
    pub session_id: u64,
    /// Monotonic within the session.
    pub presentation_generation: u64,
    pub tooltip_state: TooltipState,
    pub tooltip_generation: u64,
    /// Token of the accepted job; `None` without an accepted analysis.
    pub token: Option<AnalysisToken>,
    /// The accepted item; `None` without an accepted analysis.
    pub item: Option<CurrentItemSnapshot>,
    /// Window-space rect of the analyzed tooltip.
    pub tooltip_window_relative: Option<Rect>,
    /// The cause of the latest invalidation, when the snapshot is empty
    /// because of one.
    pub invalidation: Option<InvalidationCause>,
    /// Session-clock time of that invalidation (the anchor of the
    /// `hidden − invalidated` duration). `None` without an invalidation.
    pub invalidated_at_micros: Option<u64>,
    /// Live stage debug bundle of the accepted analysis.
    /// Present only while `Settings.IsDebugInfoEnabled` is on and this
    /// snapshot carries an accepted result; cleared with the analysis on
    /// every invalidation/empty snapshot, so a superseded generation can
    /// never leave a stale bundle visible.
    pub debug_bundle: Option<Arc<crate::debug_bundle::TooltipDebugBundle>>,
}

impl AnalysisSnapshot {
    /// The explicit "nothing accepted" snapshot of a session.
    pub fn empty(session_id: u64) -> Self {
        Self {
            session_id,
            presentation_generation: 0,
            tooltip_state: TooltipState::NoTooltip,
            tooltip_generation: 0,
            token: None,
            item: None,
            tooltip_window_relative: None,
            invalidation: None,
            invalidated_at_micros: None,
            debug_bundle: None,
        }
    }

    /// Whether the snapshot carries an accepted item.
    pub fn has_item(&self) -> bool {
        self.item.is_some()
    }
}

/// The tracked tooltip placement.
#[derive(Debug, Clone)]
struct Track {
    signature: u64,
    stable_frames: u32,
    latest: TooltipImage,
    tooltip_window_relative: Option<Rect>,
    last_submit_at: Option<Instant>,
    /// Capture time of the frame that opened this track
    /// (`first_seen_at`).
    first_seen_at_micros: u64,
}

/// The outstanding job of the current generation.
#[derive(Debug, Clone)]
struct InFlight {
    token: AnalysisToken,
    image: TooltipImage,
    tooltip_window_relative: Option<Rect>,
}

/// The pixels the accepted analysis was read from.
#[derive(Debug, Clone)]
struct Processed {
    read_top: u32,
    read_extent: Option<ReadExtent>,
    pixels: Arc<d4lc_core::CaptureFrame>,
}

/// The single owner of the analysis state of one runtime session.
#[derive(Debug)]
pub struct RuntimeCoordinator {
    diagnostics: DiagnosticsContext,
    lifecycle: RuntimeLifecycle,
    tooltip_state: TooltipState,
    generation: u64,
    ended: VecDeque<(u64, DiscardReason)>,
    window_revision: u64,
    window_geometry: Option<Rect>,
    applied_config_revision: Revision,
    game_window: GameWindowState,
    capture_minimized: bool,
    overlay_enabled: bool,
    retry_interval: Duration,
    track: Option<Track>,
    in_flight: Option<InFlight>,
    processed: Option<Processed>,
    accepted: Option<AcceptedAnalysis>,
    last_invalidation: Option<InvalidationCause>,
    last_invalidation_at: Option<u64>,
    presentation_generation: u64,
    snapshot: Arc<AnalysisSnapshot>,
    submitted_jobs: u64,
    last_content_diff: Option<String>,
}

impl RuntimeCoordinator {
    /// A running coordinator for the session `diagnostics` describes.
    pub fn new(diagnostics: DiagnosticsContext, retry_interval: Duration) -> Self {
        let session_id = diagnostics.session_id();
        let applied_config_revision = diagnostics.config_revision();
        diagnostics.set_tooltip_generation(0);
        Self {
            diagnostics,
            lifecycle: RuntimeLifecycle::Running,
            tooltip_state: TooltipState::NoTooltip,
            generation: 0,
            ended: VecDeque::with_capacity(ENDED_GENERATION_MEMORY),
            window_revision: 0,
            window_geometry: None,
            applied_config_revision,
            game_window: GameWindowState::default(),
            capture_minimized: false,
            overlay_enabled: true,
            retry_interval,
            track: None,
            in_flight: None,
            processed: None,
            accepted: None,
            last_invalidation: None,
            last_invalidation_at: None,
            presentation_generation: 0,
            snapshot: Arc::new(AnalysisSnapshot::empty(session_id)),
            submitted_jobs: 0,
            last_content_diff: None,
        }
    }

    /// Replace the diagnostics session (the live loop attaches its own after
    /// preparation). The coordinator's session is the diagnostics session.
    pub fn attach_diagnostics(&mut self, diagnostics: DiagnosticsContext) {
        diagnostics.set_tooltip_generation(self.generation);
        self.applied_config_revision = diagnostics.config_revision();
        self.diagnostics = diagnostics;
        self.snapshot = Arc::new(self.build_snapshot());
    }

    /// Change the retry interval (settings hot reload).
    pub fn set_retry_interval(&mut self, retry_interval: Duration) {
        self.retry_interval = retry_interval;
    }

    pub fn session_id(&self) -> u64 {
        self.diagnostics.session_id()
    }

    pub const fn lifecycle(&self) -> RuntimeLifecycle {
        self.lifecycle
    }

    pub const fn tooltip_state(&self) -> TooltipState {
        self.tooltip_state
    }

    pub const fn tooltip_generation(&self) -> u64 {
        self.generation
    }

    pub const fn window_revision(&self) -> u64 {
        self.window_revision
    }

    pub const fn presentation_generation(&self) -> u64 {
        self.presentation_generation
    }

    /// Jobs this coordinator published.
    pub const fn submitted_jobs(&self) -> u64 {
        self.submitted_jobs
    }

    /// Whether a job of the current generation is outstanding.
    pub const fn has_job_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    pub fn accepted(&self) -> Option<&AcceptedAnalysis> {
        self.accepted.as_ref()
    }

    /// The accepted analysis state (shared, cheap to clone).
    pub fn snapshot(&self) -> Arc<AnalysisSnapshot> {
        Arc::clone(&self.snapshot)
    }

    /// The effective game-window state: the observer's report, minimized
    /// also when the capture path sees a minimized window.
    pub const fn game_window(&self) -> GameWindowState {
        GameWindowState {
            minimized: self.game_window.minimized || self.capture_minimized,
            ..self.game_window
        }
    }

    /// The game-window state as the observer last reported it.
    pub const fn observed_game_window(&self) -> GameWindowState {
        self.game_window
    }

    /// Whether the capture path currently sees a minimized window.
    pub const fn capture_minimized(&self) -> bool {
        self.capture_minimized
    }

    pub const fn overlay_enabled(&self) -> bool {
        self.overlay_enabled
    }

    // ── Control inputs ────────────────────────────────────────────────

    /// Game focus / minimize / close input of the game-window observer.
    /// Losing focus, minimizing or closing the window invalidates
    /// immediately, whatever OCR is doing.
    pub fn set_game_window(&mut self, state: GameWindowState) -> Option<InvalidationCause> {
        self.game_window = state;
        self.apply_window_state()
    }

    /// The capture path's own minimized guess (upstream: a capture shorter
    /// than 100 rows, ScreenProcessHandler.cs:323-332). Kept apart from the
    /// observer's report so neither source overrides the other: the window
    /// counts as minimized while either says so.
    pub fn set_capture_minimized(&mut self, minimized: bool) -> Option<InvalidationCause> {
        if self.capture_minimized == minimized {
            return None;
        }
        self.capture_minimized = minimized;
        self.apply_window_state()
    }

    fn apply_window_state(&mut self) -> Option<InvalidationCause> {
        let state = self.game_window();
        let cause = if state.closed {
            Some(InvalidationCause::WindowLost)
        } else if state.minimized {
            Some(InvalidationCause::Minimized)
        } else if !state.focused {
            Some(InvalidationCause::FocusLost)
        } else {
            None
        };
        if let Some(cause) = cause {
            self.invalidate(cause);
        } else if self.tooltip_state == TooltipState::Invalidated && self.blocking_cause().is_none()
        {
            self.tooltip_state = TooltipState::NoTooltip;
        }
        cause
    }

    /// Overlay on/off input. Turning the overlay off invalidates immediately.
    pub fn set_overlay_enabled(&mut self, enabled: bool) -> Option<InvalidationCause> {
        self.overlay_enabled = enabled;
        if enabled {
            if self.tooltip_state == TooltipState::Invalidated && self.blocking_cause().is_none() {
                self.tooltip_state = TooltipState::NoTooltip;
            }
            None
        } else {
            self.invalidate(InvalidationCause::OverlayDisabled);
            Some(InvalidationCause::OverlayDisabled)
        }
    }

    /// The captured window geometry of this frame. A changed geometry is a
    /// new window revision; analyses of the old geometry are invalid.
    pub fn observe_window(&mut self, geometry: Rect) -> bool {
        match self.window_geometry {
            Some(previous) if previous == geometry => false,
            Some(_) => {
                self.window_geometry = Some(geometry);
                self.window_revision = self.window_revision.saturating_add(1);
                self.invalidate(InvalidationCause::WindowChanged);
                true
            }
            None => {
                self.window_geometry = Some(geometry);
                false
            }
        }
    }

    /// Begin the stop: invalidate, block every later result and observation.
    pub fn begin_stop(&mut self) {
        if self.lifecycle.accepts_work() {
            self.invalidate(InvalidationCause::Stopped);
            self.lifecycle = RuntimeLifecycle::Stopping;
        }
    }

    /// Record how the session ended (after its threads were released).
    pub fn finish(&mut self, failed: bool) {
        if self.lifecycle.accepts_work() {
            self.invalidate(InvalidationCause::Stopped);
        }
        self.lifecycle = if failed {
            RuntimeLifecycle::Failed
        } else {
            RuntimeLifecycle::Stopped
        };
    }

    /// Invalidate the analysis now: new generation, no accepted analysis, no
    /// outstanding job of the current generation. Independent of OCR.
    pub fn invalidate(&mut self, cause: InvalidationCause) {
        self.track = None;
        self.start_generation(cause.discard_reason());
        self.last_invalidation = Some(cause);
        self.last_invalidation_at = Some(self.diagnostics.now_micros());
        self.tooltip_state = if cause == InvalidationCause::TooltipLost
            || cause == InvalidationCause::WindowChanged
            || cause == InvalidationCause::ConfigChanged
        {
            TooltipState::NoTooltip
        } else {
            TooltipState::Invalidated
        };
        self.publish_snapshot();
    }

    // ── Frame observations ────────────────────────────────────────────

    /// A frame without a tooltip. Returns whether this ended a visible
    /// tooltip (the `TooltipLost` transition).
    pub fn observe_absent(&mut self) -> bool {
        self.sync_config_revision();
        if self.track.is_none() {
            return false;
        }
        self.invalidate(InvalidationCause::TooltipLost);
        true
    }

    /// A frame with a tooltip.
    pub fn observe_tooltip(
        &mut self,
        observation: TooltipObservation,
        now: Instant,
    ) -> ObserveOutcome {
        self.sync_config_revision();
        if !self.lifecycle.accepts_work() {
            return self.outcome(
                ObserveDecision::Suppressed(InvalidationCause::Stopped),
                false,
            );
        }
        if let Some(cause) = self.blocking_cause() {
            return self.outcome(ObserveDecision::Suppressed(cause), false);
        }

        let same_place = self
            .track
            .as_ref()
            .is_some_and(|track| track.signature == observation.signature);
        if !same_place {
            // A new episode or a moved tooltip: a new generation. Movement of
            // provably unchanged content could keep the analysis, but the row geometry of the accepted result is tied
            // to the old placement, so it is re-analyzed instead.
            self.start_generation(DiscardReason::Superseded);
            let first_seen_at_micros = observation.image.capture().captured_at_micros;
            self.track = Some(Track {
                signature: observation.signature,
                stable_frames: 1,
                latest: observation.image,
                tooltip_window_relative: observation.tooltip_window_relative,
                last_submit_at: None,
                first_seen_at_micros,
            });
            self.last_invalidation = None;
            self.last_invalidation_at = None;
            self.tooltip_state = TooltipState::Observing;
            self.publish_snapshot();
            return self.outcome(ObserveDecision::WaitingForStability, true);
        }

        let Some(track) = self.track.as_mut() else {
            return self.outcome(ObserveDecision::WaitingForStability, false);
        };
        track.stable_frames = track.stable_frames.saturating_add(1);
        track.latest = observation.image;
        track.tooltip_window_relative = observation.tooltip_window_relative;
        if track.stable_frames < REQUIRED_STABLE_FRAMES {
            return self.outcome(ObserveDecision::WaitingForStability, false);
        }

        if let Some(processed) = &self.processed {
            if read_rows_unchanged(
                &processed.pixels,
                track.latest.pixels(),
                processed.read_top,
                processed.read_extent,
            ) {
                return self.outcome(ObserveDecision::AlreadyProcessed, false);
            }
            // New content at the same place: a new generation. The
            // accepted analysis described the previous item and is dropped.
            track.stable_frames = 1;
            self.start_generation(DiscardReason::Superseded);
            self.tooltip_state = TooltipState::Observing;
            self.last_invalidation = None;
            self.last_invalidation_at = None;
            self.publish_snapshot();
            return self.outcome(ObserveDecision::ContentChanged, true);
        }

        if self.in_flight.is_some() {
            return self.outcome(ObserveDecision::InFlight, false);
        }

        // One pacing rule per placement: a retry after a failed job, and a
        // new content generation at a placement that was just submitted,
        // wait for the retry interval.
        let paced = track
            .last_submit_at
            .is_some_and(|at| now.saturating_duration_since(at) < self.retry_interval);
        if paced {
            return self.outcome(ObserveDecision::RetryInterval, false);
        }

        track.last_submit_at = Some(now);
        let image = track.latest.clone();
        let tooltip_window_relative = track.tooltip_window_relative;
        let signature = track.signature;
        let first_seen_at_micros = Some(track.first_seen_at_micros);
        let token = AnalysisToken {
            session_id: self.session_id(),
            tooltip_generation: self.generation,
            source_frame_id: image.capture().frame_id,
            first_seen_at_micros,
            content_fingerprint: image.content_fingerprint(),
            config_revision: self.diagnostics.config_revision(),
            rules_revision: self.diagnostics.rules_revision(),
            window_revision: self.window_revision,
        };
        self.in_flight = Some(InFlight {
            token,
            image: image.clone(),
            tooltip_window_relative,
        });
        self.tooltip_state = TooltipState::Analyzing;
        self.submitted_jobs = self.submitted_jobs.saturating_add(1);
        self.outcome(
            ObserveDecision::Submit(OcrJob {
                tooltip_signature: signature,
                token,
                image,
            }),
            false,
        )
    }

    // ── OCR results ───────────────────────────────────────────────────

    /// The pixel difference behind the last result discarded because the
    /// shown content changed while OCR ran (taken once).
    pub fn take_content_diff(&mut self) -> Option<String> {
        self.last_content_diff.take()
    }

    /// Decide about one OCR result. Only the result of the outstanding job of
    /// the current generation, with matching session and revisions, is
    /// adopted.
    pub fn on_result(&mut self, result: TooltipTextSnapshot) -> ResultDisposition {
        self.sync_config_revision();
        let token = result.token;
        if token.session_id != self.session_id() || !self.lifecycle.accepts_work() {
            return ResultDisposition::Discarded(DiscardReason::SessionStopped);
        }
        if token.tooltip_generation != self.generation {
            return ResultDisposition::Discarded(self.ended_reason(token.tooltip_generation));
        }
        let matches_in_flight = self
            .in_flight
            .as_ref()
            .is_some_and(|in_flight| in_flight.token == token);
        if !matches_in_flight {
            return ResultDisposition::Discarded(DiscardReason::Superseded);
        }
        if token.config_revision != self.diagnostics.config_revision() {
            return ResultDisposition::Discarded(DiscardReason::ConfigChanged);
        }
        if token.window_revision != self.window_revision {
            return ResultDisposition::Discarded(DiscardReason::WindowChanged);
        }
        let Some(in_flight) = self.in_flight.take() else {
            return ResultDisposition::Discarded(DiscardReason::Superseded);
        };

        if result.error.is_some() {
            // The generation stays current; the per-placement retry interval
            // paces the next attempt on the still visible tooltip.
            self.tooltip_state = TooltipState::Observing;
            return ResultDisposition::Failed;
        }

        let read_top = result.ocr_read_top.unwrap_or(0);
        let read_extent = result.ocr_read_extent;
        // The screen may have changed while OCR ran: a result whose read
        // pixels no longer match the latest observation describes an item
        // that is gone. It is superseded, and the new content starts over.
        let still_shown = self.track.as_ref().is_some_and(|track| {
            read_rows_unchanged(
                in_flight.image.pixels(),
                track.latest.pixels(),
                read_top,
                read_extent,
            )
        });
        if !still_shown {
            self.last_content_diff = self.track.as_ref().map(|track| {
                describe_row_difference(
                    in_flight.image.pixels(),
                    track.latest.pixels(),
                    read_top,
                    read_extent,
                )
            });
            if let Some(track) = self.track.as_mut() {
                track.stable_frames = 1;
            }
            self.start_generation(DiscardReason::Superseded);
            self.tooltip_state = TooltipState::Observing;
            self.publish_snapshot();
            return ResultDisposition::Discarded(DiscardReason::Superseded);
        }

        self.processed = Some(Processed {
            read_top,
            read_extent,
            pixels: in_flight.image.shared_pixels(),
        });
        self.accepted = Some(AcceptedAnalysis {
            token,
            result: Arc::new(result),
            tooltip_window_relative: in_flight.tooltip_window_relative,
        });
        self.tooltip_state = TooltipState::Ready;
        self.last_invalidation = None;
        self.last_invalidation_at = None;
        self.publish_snapshot();
        ResultDisposition::Accepted
    }

    // ── Internals ─────────────────────────────────────────────────────

    fn outcome(&self, decision: ObserveDecision, generation_changed: bool) -> ObserveOutcome {
        ObserveOutcome {
            decision,
            generation_changed,
        }
    }

    /// The invalidation cause that currently blocks new analyses.
    fn blocking_cause(&self) -> Option<InvalidationCause> {
        let window = self.game_window();
        if !self.overlay_enabled {
            Some(InvalidationCause::OverlayDisabled)
        } else if window.closed {
            Some(InvalidationCause::WindowLost)
        } else if window.minimized {
            Some(InvalidationCause::Minimized)
        } else if !window.focused {
            Some(InvalidationCause::FocusLost)
        } else {
            None
        }
    }

    /// An applied configuration change (settings hot reload) requires a new
    /// OCR context: everything analyzed under the old configuration ends.
    fn sync_config_revision(&mut self) {
        let applied = self.diagnostics.config_revision();
        if applied != self.applied_config_revision {
            self.applied_config_revision = applied;
            self.invalidate(InvalidationCause::ConfigChanged);
        }
    }

    /// End the current generation with `reason` and start the next one.
    fn start_generation(&mut self, reason: DiscardReason) {
        if self.ended.len() == ENDED_GENERATION_MEMORY {
            self.ended.pop_front();
        }
        self.ended.push_back((self.generation, reason));
        self.generation = self.generation.saturating_add(1);
        self.diagnostics.set_tooltip_generation(self.generation);
        self.in_flight = None;
        self.processed = None;
        self.accepted = None;
    }

    /// Why an older generation ended (for the discard of its late result).
    fn ended_reason(&self, generation: u64) -> DiscardReason {
        self.ended
            .iter()
            .find(|(ended, _)| *ended == generation)
            .map(|(_, reason)| *reason)
            .unwrap_or(DiscardReason::Superseded)
    }

    fn publish_snapshot(&mut self) {
        self.presentation_generation = self.presentation_generation.saturating_add(1);
        self.snapshot = Arc::new(self.build_snapshot());
    }

    fn build_snapshot(&self) -> AnalysisSnapshot {
        let (token, item, rect, debug_bundle) = match &self.accepted {
            Some(accepted) => {
                let status = determine_frame_status(true, Some(&accepted.result), true);
                let item = accepted
                    .result
                    .item
                    .as_ref()
                    .map(|item| current_item_snapshot(item, &status.parse_status_string()));
                (
                    Some(accepted.token),
                    item,
                    accepted.tooltip_window_relative,
                    accepted.result.debug.clone(),
                )
            }
            None => (None, None, None, None),
        };
        AnalysisSnapshot {
            session_id: self.session_id(),
            presentation_generation: self.presentation_generation,
            tooltip_state: self.tooltip_state,
            tooltip_generation: self.generation,
            token,
            item,
            tooltip_window_relative: rect,
            invalidation: self.last_invalidation,
            invalidated_at_micros: self.last_invalidation_at,
            debug_bundle,
        }
    }
}

/// What differs between two crops in the compared region (the rows from
/// `read_top` down, within `extent` when known): the diagnostics detail of a
/// result superseded by changed content, so a live log shows where the
/// tooltip changed or whether the frames only jitter. Only pixels beyond
/// [`CONTENT_NOISE_DELTA`] count, as in [`read_rows_unchanged`].
pub fn describe_row_difference(
    before: &d4lc_core::CaptureFrame,
    after: &d4lc_core::CaptureFrame,
    read_top: u32,
    extent: Option<ReadExtent>,
) -> String {
    if before.width != after.width || before.height != after.height {
        return format!(
            "crop size changed {}x{} -> {}x{} read_top={read_top}",
            before.width, before.height, after.width, after.height
        );
    }
    let width = usize::try_from(before.width).unwrap_or(0);
    let top = usize::try_from(read_top.min(before.height)).unwrap_or(0);
    let height = ReadExtent::row_end(extent, usize::try_from(before.height).unwrap_or(0));
    let bytes = ReadExtent::row_bytes(extent, width);
    let (left, right) = (bytes.start / 4, bytes.end / 4);
    let (mut pixels, mut max_delta) = (0_usize, 0_u8);
    let (mut first_row, mut last_row) = (None, 0_usize);
    let (mut first_col, mut last_col) = (usize::MAX, 0_usize);
    for row in top..height {
        let (Some(a), Some(b)) = (
            before
                .pixels_bgra
                .get(row * before.stride + bytes.start..row * before.stride + bytes.end),
            after
                .pixels_bgra
                .get(row * after.stride + bytes.start..row * after.stride + bytes.end),
        ) else {
            break;
        };
        for (col, (pa, pb)) in a
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .enumerate()
        {
            let delta = pa
                .iter()
                .zip(pb)
                .take(3)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            if delta > CONTENT_NOISE_DELTA {
                pixels += 1;
                max_delta = max_delta.max(delta);
                first_row.get_or_insert(row);
                last_row = row;
                first_col = first_col.min(left + col);
                last_col = last_col.max(left + col);
            }
        }
    }
    match first_row {
        Some(first) => format!(
            "read rows changed: {pixels} px, max channel delta {max_delta}, rows {first}..={last_row} of {top}..{height}, columns {first_col}..={last_col} of {left}..{right}"
        ),
        None => format!("read rows identical ({top}..{height}, columns {left}..{right})"),
    }
}

/// Whether `after` still shows the tooltip content an OCR result read from
/// `before`.
///
/// The detection crop is upstream's `_currentScreenTooltip`
/// (`ScreenProcessHandler.FindTooltips`, ScreenProcessHandler.cs:625-636):
/// `Rectangle(marker.X + offsetX, 0, TooltipWidth, offsetY + marker.Y)` — a
/// column from the *top of the screen* down to the tooltip marker. The tooltip
/// panel only fills the lower part of that column; above it is live game
/// scene (character, effects, inventory paper doll), which animates on
/// practically every frame. A comparison over the whole crop therefore
/// reported a steadily shown tooltip as new content on every frame and
/// re-ran OCR once per retry interval (every 250 ms).
///
/// The compared rows start at [`TooltipTextSnapshot::ocr_read_top`]: the
/// first splitter below the item name, else the first recognized text line.
/// Scene rows above the tooltip and the animated header of legendary, unique
/// and mythic tooltips (which carries the name and type lines) change on
/// every frame, so comparing them rejected every result on those items.
///
/// The compared columns come from [`TooltipTextSnapshot::ocr_read_extent`]:
/// the horizontal extent of the recognized text lines. The crop is
/// `TooltipWidth` wide from the marker on, and the stored `TooltipWidth` can
/// exceed the tooltip (the upstream default is 500, the 1080p presets use
/// 375): the columns right of the tooltip are then live scene or the
/// inventory panel. A live Wayland session superseded every
/// result on them (thousands of pixels changed on every row of the read
/// region), so no item ever reached the overlay.
///
/// The compared rows end at the extent's bottom: the last recognized text
/// line. The crop reaches down to the tooltip marker, and in controller mode
/// the rows between the tooltip text and the marker hold the key hints
/// ("Take", "Move to another tab"), whose button glyphs animate. A live
/// Wayland session superseded the result each time they pulsed; the overlay
/// was dropped and redrawn about twice a second.
///
/// The region is compared pixel by pixel, and a pixel counts as changed when
/// one color channel moved by more than [`CONTENT_NOISE_DELTA`]:
///
/// - a steady tooltip under or beside an animated scene or header is
///   processed once. A steady tooltip is not bit-stable: the live
///   Wayland capture of the unique pants "Temerity" showed single pixels
///   moving by one level about once a second, and a bit-exact comparison
///   dropped the shown overlay and re-ran OCR each time (the overlay
///   flickered); when the noise came faster than OCR, no result was ever
///   adopted. On "Runic Mail of Diabolical Armor" one to four pixels at the
///   left edge of the wrapped aspect lines moved by 13 to 25 levels about
///   twice a second, and a noise level of 12 still let the overlay flicker;
/// - a different item at the same geometry differs in the text below the
///   header — its affix lines and values, which also follow item power — and
///   is new content. Text on the tooltip background differs by far
///   more than the noise level, so a single changed glyph still counts. A
///   perceptual hash was rejected: a tolerance large enough to absorb
///   animation also absorbs a changed digit.
///
/// Without a known read region the whole crop is compared, which is
/// conservative.
pub fn read_rows_unchanged(
    before: &d4lc_core::CaptureFrame,
    after: &d4lc_core::CaptureFrame,
    read_top: u32,
    extent: Option<ReadExtent>,
) -> bool {
    if before.width != after.width || before.height != after.height {
        return false;
    }
    let (Ok(width), Ok(top), Ok(height)) = (
        usize::try_from(before.width),
        usize::try_from(read_top.min(before.height)),
        usize::try_from(before.height),
    ) else {
        return false;
    };
    let height = ReadExtent::row_end(extent, height);
    let bytes = ReadExtent::row_bytes(extent, width);
    (top..height).all(|row| {
        let (Some(a), Some(b)) = (
            row.checked_mul(before.stride).and_then(|start| {
                before
                    .pixels_bgra
                    .get(start.checked_add(bytes.start)?..start.checked_add(bytes.end)?)
            }),
            row.checked_mul(after.stride).and_then(|start| {
                after
                    .pixels_bgra
                    .get(start.checked_add(bytes.start)?..start.checked_add(bytes.end)?)
            }),
        ) else {
            return false;
        };
        // Fast path: identical rows are the common case.
        a == b
            || a.as_chunks::<4>()
                .0
                .iter()
                .zip(b.as_chunks::<4>().0)
                .all(|(pa, pb)| {
                    pa.iter()
                        .zip(pb)
                        .take(3)
                        .all(|(x, y)| x.abs_diff(*y) <= CONTENT_NOISE_DELTA)
                })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TooltipCaptureInfo;
    use d4lc_core::coords::{DesktopRect, FramePlacement, FrameRect, FrameScale, TooltipPlacement};
    use d4lc_core::Point;

    fn coordinator() -> RuntimeCoordinator {
        RuntimeCoordinator::new(DiagnosticsContext::disabled(), Duration::from_millis(250))
    }

    /// A tooltip crop captured in frame 41 at session-clock 12_345 µs.
    fn observed_image() -> TooltipImage {
        TooltipImage::new(
            bgra_frame(16, 8, 7),
            TooltipCaptureInfo {
                frame_id: 41,
                captured_at_micros: 12_345,
                placement: TooltipPlacement {
                    frame: FramePlacement {
                        window_in_desktop: DesktopRect(rect(0, 0)),
                        frame_origin_in_window: Point { x: 0, y: 0 },
                        scale: FrameScale::from_milli(1000),
                    },
                    crop_in_frame: FrameRect(Rect {
                        x: 0,
                        y: 0,
                        width: 16,
                        height: 8,
                    }),
                },
            },
        )
    }

    /// The published job's token carries the capture time of the frame that
    /// opened the track (`first_seen_at`), and an invalidation
    /// stamps the snapshot with its time until the track reopens.
    #[test]
    fn published_job_token_carries_first_seen_and_invalidation_time() {
        let mut coordinator = coordinator();
        let observation = |image| TooltipObservation {
            signature: 1,
            image,
            tooltip_window_relative: None,
        };
        assert!(matches!(
            coordinator
                .observe_tooltip(observation(observed_image()), Instant::now())
                .decision,
            ObserveDecision::WaitingForStability
        ));
        let outcome = coordinator.observe_tooltip(observation(observed_image()), Instant::now());
        match outcome.decision {
            ObserveDecision::Submit(job) => {
                assert_eq!(job.token.first_seen_at_micros, Some(12_345));
            }
            decision => panic!("expected a submitted job, got {decision:?}"),
        }

        // The invalidation records when it happened; reopening the track
        // clears the timestamp again.
        coordinator.invalidate(InvalidationCause::FocusLost);
        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.invalidation, Some(InvalidationCause::FocusLost));
        assert!(
            snapshot.invalidated_at_micros.is_some(),
            "the invalidation must be stamped"
        );
        let outcome = coordinator.observe_tooltip(observation(observed_image()), Instant::now());
        assert!(matches!(
            outcome.decision,
            ObserveDecision::WaitingForStability
        ));
        assert!(outcome.generation_changed, "a new track opens");
        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.invalidation, None);
        assert_eq!(snapshot.invalidated_at_micros, None);
    }

    fn rect(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            width: 1920,
            height: 1080,
        }
    }

    fn bgra_frame(width: u32, height: u32, fill: u8) -> d4lc_core::CaptureFrame {
        d4lc_core::CaptureFrame {
            width,
            height,
            stride: usize::try_from(width).unwrap_or(0) * 4,
            pixels_bgra: vec![fill; usize::try_from(width * height).unwrap_or(0) * 4],
        }
    }

    /// A minimal live debug bundle for the carrier tests.
    fn tiny_debug_bundle() -> Arc<crate::debug_bundle::TooltipDebugBundle> {
        let image = || crate::debug_bundle::DebugImage {
            width: 1,
            height: 1,
            rgba: Arc::from(vec![255_u8, 0, 0, 255]),
        };
        Arc::new(crate::debug_bundle::TooltipDebugBundle {
            tooltip_crop: image(),
            item_type_area: None,
            affix_locations: image(),
            affix_areas: image(),
            aspect_area: image(),
            socket_locations: image(),
            splitter_locations: image(),
            segments: Vec::new(),
            lines: Vec::new(),
        })
    }

    /// The accepted snapshot carries the OCR result's live debug bundle, an
    /// accepted result without one stays empty (`DebugInfo` off), and an
    /// invalidation clears a previously shown bundle so a superseded
    /// generation is never left visible.
    #[test]
    fn accepted_snapshot_carries_the_debug_bundle_and_invalidation_clears_it() {
        let mut coordinator = coordinator();
        let now = Instant::now();
        let observation = || TooltipObservation {
            signature: 7,
            image: TooltipImage::new(bgra_frame(16, 8, 7), *observed_image().capture()),
            tooltip_window_relative: None,
        };
        let submit = |coordinator: &mut RuntimeCoordinator| {
            coordinator.observe_tooltip(observation(), now);
            match coordinator.observe_tooltip(observation(), now).decision {
                ObserveDecision::Submit(job) => job,
                decision => panic!("second stable frame submits, got {decision:?}"),
            }
        };

        // DebugInfo off: an accepted result carries no bundle.
        let job = submit(&mut coordinator);
        let mut result = crate::pipeline::mock_ocr_snapshot();
        result.token = job.token;
        result.ocr_read_top = Some(2);
        assert_eq!(coordinator.on_result(result), ResultDisposition::Accepted);
        assert!(
            coordinator.snapshot().debug_bundle.is_none(),
            "without a bundle on the result the accepted snapshot stays empty"
        );

        // DebugInfo on: the next accepted result's bundle rides the snapshot.
        coordinator.invalidate(InvalidationCause::FocusLost);
        assert!(
            coordinator.snapshot().debug_bundle.is_none(),
            "the invalidation clears the previous bundle"
        );
        let job = submit(&mut coordinator);
        let mut result = crate::pipeline::mock_ocr_snapshot();
        result.token = job.token;
        result.ocr_read_top = Some(2);
        result.debug = Some(tiny_debug_bundle());
        assert_eq!(coordinator.on_result(result), ResultDisposition::Accepted);
        let snapshot = coordinator.snapshot();
        assert_eq!(
            snapshot
                .debug_bundle
                .as_deref()
                .map(|bundle| bundle.tooltip_crop.width),
            Some(1),
            "the accepted snapshot carries the result's live bundle"
        );

        // A later invalidation removes it again.
        coordinator.invalidate(InvalidationCause::FocusLost);
        assert!(coordinator.snapshot().debug_bundle.is_none());
    }

    /// The superseded-result detail names the changed rows below `read_top`
    /// and ignores changes above it.
    #[test]
    fn row_difference_reports_changed_rows_below_read_top() {
        let before = bgra_frame(4, 6, 10);
        let mut after = before.clone();
        after.pixels_bgra[0] = 200; // row 0, above read_top
        after.pixels_bgra[4 * 4 * 3 + 4] = 110; // row 3, pixel 1
        after.pixels_bgra[4 * 4 * 4 + 12] = 35; // row 4, pixel 3: noise
        assert_eq!(
            describe_row_difference(&before, &after, 2, None),
            "read rows changed: 1 px, max channel delta 100, rows 3..=3 of 2..6, columns 1..=1 of 0..4"
        );
        assert_eq!(
            describe_row_difference(&before, &before, 2, None),
            "read rows identical (2..6, columns 0..4)"
        );
        assert_eq!(
            describe_row_difference(
                &before,
                &after,
                2,
                Some(ReadExtent {
                    left: 2,
                    right: 9,
                    bottom: 9
                })
            ),
            "read rows identical (2..6, columns 2..4)"
        );
        assert_eq!(
            describe_row_difference(
                &before,
                &after,
                2,
                Some(ReadExtent {
                    left: 0,
                    right: 4,
                    bottom: 3
                })
            ),
            "read rows identical (2..3, columns 0..4)"
        );
        assert!(
            describe_row_difference(&before, &bgra_frame(4, 5, 10), 0, None)
                .starts_with("crop size changed 4x6 -> 4x5")
        );
    }

    /// Columns outside the recognized text (scene beside a tooltip narrower
    /// than `TooltipWidth`) and rows below it (the animated controller key
    /// hints above the marker) do not decide the content; a change inside
    /// the text does.
    #[test]
    fn read_rows_compare_only_the_read_extent() {
        let before = bgra_frame(4, 6, 10);
        let text = Some(ReadExtent {
            left: 0,
            right: 2,
            bottom: 5,
        });
        let mut scene = before.clone();
        scene.pixels_bgra[4 * 4 * 3 + 12] = 200; // row 3, pixel 3
        assert!(read_rows_unchanged(&before, &scene, 2, text));
        assert!(!read_rows_unchanged(&before, &scene, 2, None));

        let mut hint = before.clone();
        hint.pixels_bgra[4 * 4 * 5 + 4] = 200; // row 5, pixel 1
        assert!(read_rows_unchanged(&before, &hint, 2, text));
        assert!(!read_rows_unchanged(&before, &hint, 2, None));

        let mut glyph = before.clone();
        glyph.pixels_bgra[4 * 4 * 3 + 4] = 200; // row 3, pixel 1
        assert!(!read_rows_unchanged(&before, &glyph, 2, text));
    }

    /// Render noise below the read top or within [`CONTENT_NOISE_DELTA`] keeps
    /// the content; a text-level change, a stride change or a size change
    /// does not.
    #[test]
    fn read_rows_ignore_render_noise_but_not_text_changes() {
        let before = bgra_frame(4, 6, 10);
        let mut noise = before.clone();
        noise.pixels_bgra[4 * 4 * 3 + 4] = 11; // one level, as measured live
        noise.pixels_bgra[4 * 4 * 4 + 4] = 35; // 25 levels, as measured live
        noise.pixels_bgra[4 * 4 * 5] = 10 + CONTENT_NOISE_DELTA;
        noise.pixels_bgra[1] = 250; // above read_top
        assert!(read_rows_unchanged(&before, &noise, 2, None));

        let mut glyph = before.clone();
        glyph.pixels_bgra[4 * 4 * 4 + 8 + 2] = 10 + CONTENT_NOISE_DELTA + 1;
        assert!(!read_rows_unchanged(&before, &glyph, 2, None));
        assert!(!read_rows_unchanged(
            &before,
            &bgra_frame(4, 5, 10),
            0,
            None
        ));

        let mut padded = before.clone();
        padded.stride = 20;
        padded.pixels_bgra = before
            .pixels_bgra
            .chunks(16)
            .flat_map(|row| row.iter().copied().chain([0xEE; 4]))
            .collect();
        assert!(read_rows_unchanged(&before, &padded, 0, None));
    }

    /// A steady tooltip whose read rows jitter by render noise stays
    /// processed: no new generation, no second OCR job.
    #[test]
    fn render_noise_on_a_processed_tooltip_keeps_the_analysis() {
        let mut coordinator = coordinator();
        let now = Instant::now();
        let observation = |fill_jitter: u8| TooltipObservation {
            signature: 7,
            image: TooltipImage::new(
                {
                    let mut frame = bgra_frame(16, 8, 7);
                    frame.pixels_bgra[16 * 4 * 5] = 7 + fill_jitter;
                    frame
                },
                *observed_image().capture(),
            ),
            tooltip_window_relative: None,
        };
        coordinator.observe_tooltip(observation(0), now);
        let ObserveDecision::Submit(job) =
            coordinator.observe_tooltip(observation(0), now).decision
        else {
            panic!("second stable frame submits");
        };
        let generation = job.token.tooltip_generation;
        let mut result = crate::pipeline::mock_ocr_snapshot();
        result.token = job.token;
        result.ocr_read_top = Some(2);
        assert_eq!(coordinator.on_result(result), ResultDisposition::Accepted);

        let outcome = coordinator.observe_tooltip(observation(1), now);
        assert!(matches!(
            outcome.decision,
            ObserveDecision::AlreadyProcessed
        ));
        assert!(!outcome.generation_changed);
        assert_eq!(coordinator.snapshot().tooltip_generation, generation);

        let outcome = coordinator.observe_tooltip(observation(200), now);
        assert!(matches!(outcome.decision, ObserveDecision::ContentChanged));
    }

    /// The observer's report and the capture path's minimized guess are two
    /// sources: neither may lift the other's block.
    #[test]
    fn capture_minimized_and_observer_state_do_not_override_each_other() {
        let mut coordinator = coordinator();
        let hidden = GameWindowState {
            minimized: true,
            ..GameWindowState::default()
        };
        assert_eq!(
            coordinator.set_game_window(hidden),
            Some(InvalidationCause::Minimized)
        );
        // A full-height capture does not un-minimize what the observer saw.
        assert_eq!(coordinator.set_capture_minimized(false), None);
        assert!(coordinator.game_window().minimized);
        assert_eq!(coordinator.tooltip_state(), TooltipState::Invalidated);

        // The observer restores the window; now the capture guess alone
        // blocks, and lifting it releases the block.
        assert_eq!(
            coordinator.set_game_window(GameWindowState::default()),
            None
        );
        assert_eq!(coordinator.tooltip_state(), TooltipState::NoTooltip);
        assert_eq!(
            coordinator.set_capture_minimized(true),
            Some(InvalidationCause::Minimized)
        );
        assert!(!coordinator.observed_game_window().minimized);
        assert_eq!(coordinator.set_capture_minimized(false), None);
        assert_eq!(coordinator.tooltip_state(), TooltipState::NoTooltip);
    }

    #[test]
    fn a_closed_game_window_invalidates_as_window_lost() {
        let mut coordinator = coordinator();
        let generation = coordinator.tooltip_generation();
        let cause = coordinator.set_game_window(GameWindowState {
            closed: true,
            ..GameWindowState::default()
        });
        assert_eq!(cause, Some(InvalidationCause::WindowLost));
        assert!(coordinator.tooltip_generation() > generation);
        assert_eq!(
            coordinator.snapshot().invalidation,
            Some(InvalidationCause::WindowLost)
        );
        assert_eq!(coordinator.tooltip_state(), TooltipState::Invalidated);
    }

    #[test]
    fn a_moved_window_is_a_new_window_revision() {
        let mut coordinator = coordinator();
        assert!(!coordinator.observe_window(rect(0, 0)));
        assert_eq!(coordinator.window_revision(), 0);
        assert!(!coordinator.observe_window(rect(0, 0)));
        assert!(coordinator.observe_window(rect(-1920, 40)));
        assert_eq!(coordinator.window_revision(), 1);
        assert_eq!(
            coordinator.snapshot().invalidation,
            Some(InvalidationCause::WindowChanged)
        );
    }
}
