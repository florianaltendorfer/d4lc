//! Structured runtime diagnostics — the event vocabulary, the bounded runtime
//! event buffer, and the frame-chain reconstruction.
//!
//! This module instruments the runtime; the analysis state itself belongs
//! to the runtime coordinator.
//!
//! ## What an event carries
//!
//! Every [`DiagnosticsEvent`] carries the session id, the frame id, the
//! tooltip generation, the applied configuration and rule-set revisions, the
//! phase, a monotonic timestamp plus the matching wall-clock time, an
//! optional duration, the outcome (ok / failed / discarded-with-reason) and an
//! optional short detail string.
//!
//! ## Default verbosity
//!
//! [`DiagnosticsVerbosity::Summary`] is the default: state transitions, errors,
//! the phase chain of a processed frame and the aggregate statistics. Verbose
//! OCR/image payloads are only attached in [`DiagnosticsVerbosity::Detail`]
//! (`detail` stays `None` in summary mode), so a build that never opts in
//! writes no detail payload at all.
//!
//! ## Boundedness
//!
//! [`DiagnosticsHandle`] is a fixed-capacity event buffer between the producer
//! threads (frame loop, OCR worker) and the draining consumer. Overflow drops
//! the oldest event and counts the drop — an event flood can never grow it.
//! A [`DiagnosticsHandle::disabled`] handle accepts nothing and counts every
//! emission as dropped.
//!
//! ## Timestamps
//!
//! Timestamps are monotonic microseconds since the [`DiagnosticsClock`] origin
//! of the session. The named time points that exist today
//! are reconstructed per frame by [`FrameTimeline`]; time points the runtime
//! cannot observe today (`tooltip_visible_at`, `overlay_visible_at`) are absent
//! fields, never fabricated values.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Capacity of the shared runtime event buffer.
///
/// The buffer sits between the frame loop / OCR worker and the draining
/// diagnostics sink. It is deliberately small: it is a hand-off buffer, not a
/// history.
pub const DEFAULT_EVENT_CAPACITY: usize = 4096;

/// Shared-slot marker for "no revision source exists in this runtime".
const REVISION_UNKNOWN: u64 = u64::MAX;

/// Why an observation, result or presentation was discarded.
///
/// Besides the pipeline's own reasons the vocabulary carries the
/// coordinator's invalidation causes: overlay deactivation, window
/// loss/change and the end of the session. `FocusLost` covers both a lost
/// game focus and a minimized game window, fed by the game-window observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    /// A tooltip that was visible is gone (the frame detected none).
    TooltipLost,
    /// The game window lost focus or was minimized.
    FocusLost,
    /// A newer observation or job superseded this one.
    Superseded,
    /// An applied configuration change invalidated this work.
    ConfigChanged,
    /// The frame or the OCR crop could not be captured.
    CaptureFailed,
    /// A required catalog / fuzzy index is unavailable.
    CatalogUnavailable,
    /// The overlay was turned off.
    OverlayDisabled,
    /// The game window is gone.
    WindowLost,
    /// The game window geometry changed under the work.
    WindowChanged,
    /// The analysis session stopped (or a newer session replaced it).
    SessionStopped,
}

impl DiscardReason {
    /// All variants in declaration order (stable indexing for statistics).
    pub const ALL: [Self; 10] = [
        Self::TooltipLost,
        Self::FocusLost,
        Self::Superseded,
        Self::ConfigChanged,
        Self::CaptureFailed,
        Self::CatalogUnavailable,
        Self::OverlayDisabled,
        Self::WindowLost,
        Self::WindowChanged,
        Self::SessionStopped,
    ];

    /// Stable short name for log lines and counters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TooltipLost => "TooltipLost",
            Self::FocusLost => "FocusLost",
            Self::Superseded => "Superseded",
            Self::ConfigChanged => "ConfigChanged",
            Self::CaptureFailed => "CaptureFailed",
            Self::CatalogUnavailable => "CatalogUnavailable",
            Self::OverlayDisabled => "OverlayDisabled",
            Self::WindowLost => "WindowLost",
            Self::WindowChanged => "WindowChanged",
            Self::SessionStopped => "SessionStopped",
        }
    }

    /// Index in [`DiscardReason::ALL`] for the per-reason counters.
    pub const fn index(self) -> usize {
        match self {
            Self::TooltipLost => 0,
            Self::FocusLost => 1,
            Self::Superseded => 2,
            Self::ConfigChanged => 3,
            Self::CaptureFailed => 4,
            Self::CatalogUnavailable => 5,
            Self::OverlayDisabled => 6,
            Self::WindowLost => 7,
            Self::WindowChanged => 8,
            Self::SessionStopped => 9,
        }
    }
}

/// The processing phase an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticsPhase {
    /// One capture of the game window.
    Capture,
    /// Tooltip detection on that capture.
    Detect,
    /// An OCR job was handed to the worker.
    OcrSubmit,
    /// The OCR worker started executing a job.
    OcrStart,
    /// The OCR worker finished (or failed) a job.
    OcrFinish,
    /// An OCR result was adopted / parsed for the frame.
    Parse,
    /// Rule evaluation for the presented frame.
    Evaluate,
    /// A presentation was submitted to the overlay backend.
    PresentationSubmit,
    /// The backend returned from the draw call.
    DrawCompleted,
    /// The first presentation that carries the accepted analysis was confirmed
    /// drawn (`overlay_committed_at`). The event duration is
    /// `committed − first_seen`.
    OverlayCommit,
    /// The first empty presentation after an invalidation was confirmed drawn
    /// (`hidden_at`). The event duration is `hidden − invalidated`.
    OverlayHidden,
    /// A runtime state transition (started, configuration applied, stopped).
    StateChange,
    /// A statistics record, not a per-frame observation.
    Aggregate,
}

impl DiagnosticsPhase {
    /// All variants in declaration order.
    pub const ALL: [Self; 13] = [
        Self::Capture,
        Self::Detect,
        Self::OcrSubmit,
        Self::OcrStart,
        Self::OcrFinish,
        Self::Parse,
        Self::Evaluate,
        Self::PresentationSubmit,
        Self::DrawCompleted,
        Self::OverlayCommit,
        Self::OverlayHidden,
        Self::StateChange,
        Self::Aggregate,
    ];

    /// Stable short name for log lines and counters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Detect => "detect",
            Self::OcrSubmit => "ocr_submit",
            Self::OcrStart => "ocr_start",
            Self::OcrFinish => "ocr_finish",
            Self::Parse => "parse",
            Self::Evaluate => "evaluate",
            Self::PresentationSubmit => "presentation_submit",
            Self::DrawCompleted => "draw_completed",
            Self::OverlayCommit => "overlay_commit",
            Self::OverlayHidden => "overlay_hidden",
            Self::StateChange => "state_change",
            Self::Aggregate => "aggregate",
        }
    }

    /// Index in [`DiagnosticsPhase::ALL`] for the per-phase counters.
    pub const fn index(self) -> usize {
        match self {
            Self::Capture => 0,
            Self::Detect => 1,
            Self::OcrSubmit => 2,
            Self::OcrStart => 3,
            Self::OcrFinish => 4,
            Self::Parse => 5,
            Self::Evaluate => 6,
            Self::PresentationSubmit => 7,
            Self::DrawCompleted => 8,
            Self::OverlayCommit => 9,
            Self::OverlayHidden => 10,
            Self::StateChange => 11,
            Self::Aggregate => 12,
        }
    }
}

/// Result of one phase observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticsOutcome {
    /// The phase completed successfully.
    Ok,
    /// The phase failed; the message is the structured reason already carried
    /// by the runtime (never a re-worded substitute).
    Failed { message: String },
    /// The phase was dropped on purpose, with the concrete reason.
    Discarded { reason: DiscardReason },
}

impl DiagnosticsOutcome {
    /// Build a failure outcome from an existing structured message.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
        }
    }

    /// Build a discard outcome.
    pub const fn discarded(reason: DiscardReason) -> Self {
        Self::Discarded { reason }
    }

    /// The discard reason, when this outcome is a discard.
    pub const fn discard_reason(&self) -> Option<DiscardReason> {
        match self {
            Self::Discarded { reason } => Some(*reason),
            Self::Ok | Self::Failed { .. } => None,
        }
    }

    /// Stable short name for log lines and counters.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed { .. } => "failed",
            Self::Discarded { .. } => "discarded",
        }
    }
}

/// A configuration or rule-set revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Revision {
    /// No revision source exists in this runtime. Recorded explicitly instead
    /// of inventing a plausible-looking number.
    #[default]
    Unknown,
    /// The nth revision this session observed as applied.
    Applied(u64),
}

impl Revision {
    /// The revision number, when one exists.
    pub const fn value(self) -> Option<u64> {
        match self {
            Self::Unknown => None,
            Self::Applied(value) => Some(value),
        }
    }

    /// Stable rendering for log lines ("unknown" / the number).
    pub fn as_str(self) -> String {
        match self {
            Self::Unknown => "unknown".to_string(),
            Self::Applied(value) => value.to_string(),
        }
    }

    const fn to_slot(self) -> u64 {
        match self {
            Self::Unknown => REVISION_UNKNOWN,
            Self::Applied(value) => value,
        }
    }

    const fn from_slot(slot: u64) -> Self {
        if slot == REVISION_UNKNOWN {
            Self::Unknown
        } else {
            Self::Applied(slot)
        }
    }
}

/// How much detail the diagnostics record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiagnosticsVerbosity {
    /// State transitions, errors, the per-frame phase chain and aggregates.
    #[default]
    Summary,
    /// Additionally attaches the verbose OCR/image payload (`detail`) so a
    /// detail sink can persist it — bounded by that sink.
    Detail,
}

impl DiagnosticsVerbosity {
    /// Map an explicit opt-in flag to the verbosity.
    pub const fn from_detail_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Detail
        } else {
            Self::Summary
        }
    }

    /// Whether verbose payloads are attached to events.
    pub const fn records_detail(self) -> bool {
        matches!(self, Self::Detail)
    }

    /// Stable short name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Detail => "detail",
        }
    }
}

/// Monotonic clock shared by every emitter of one diagnostics session.
///
/// Cloning shares the origin, so worker-thread events and frame-loop events
/// carry timestamps on the same timeline.
#[derive(Debug, Clone)]
pub struct DiagnosticsClock {
    origin: Arc<Instant>,
}

impl Default for DiagnosticsClock {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticsClock {
    /// Start a clock at "now".
    pub fn new() -> Self {
        Self {
            origin: Arc::new(Instant::now()),
        }
    }

    /// Monotonic microseconds since the clock origin.
    pub fn now_micros(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
}

/// Wall-clock milliseconds since the unix epoch.
///
/// Correlation only: it survives restarts, while ordering uses the monotonic
/// [`DiagnosticsClock`].
pub fn wall_unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// One structured runtime diagnostics event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsEvent {
    /// Analysis session the event belongs to.
    pub session_id: u64,
    /// Frame sequence of the session (monotonic within the session).
    pub frame_id: u64,
    /// Diagnostic tooltip-observation generation (see
    /// [`DiagnosticsContext::tooltip_generation`]).
    pub tooltip_generation: u64,
    /// Applied configuration revision.
    pub config_revision: Revision,
    /// Applied rule-set revision.
    pub rules_revision: Revision,
    pub phase: DiagnosticsPhase,
    /// Monotonic microseconds since the session clock origin.
    pub at_micros: u64,
    /// Wall-clock milliseconds since the unix epoch.
    pub wall_unix_ms: u64,
    /// Measured duration of the phase, when the phase measured one.
    pub duration_micros: Option<u64>,
    /// Capture time of the frame that opened the tooltip track this job
    /// belongs to (`first_seen_at`), on the session clock. Only the
    /// `ocr_submit` event of a published job carries it; every other event
    /// leaves it absent.
    pub first_seen_at_micros: Option<u64>,
    pub outcome: DiagnosticsOutcome,
    /// Short event context (named preset, signature, reason text). Verbose
    /// OCR/image payloads are only attached in [`DiagnosticsVerbosity::Detail`].
    pub detail: Option<String>,
}

impl DiagnosticsEvent {
    /// The discard reason carried by this event, if any.
    pub const fn discard_reason(&self) -> Option<DiscardReason> {
        self.outcome.discard_reason()
    }

    /// Set the measured phase duration in microseconds.
    #[must_use]
    pub fn with_duration_micros(mut self, duration_micros: u64) -> Self {
        self.duration_micros = Some(duration_micros);
        self
    }

    /// Attach the track's first-seen capture time (see
    /// [`DiagnosticsEvent::first_seen_at_micros`]).
    #[must_use]
    pub fn with_first_seen_at_micros(mut self, first_seen_at_micros: Option<u64>) -> Self {
        self.first_seen_at_micros = first_seen_at_micros;
        self
    }

    /// Replace the outcome.
    #[must_use]
    pub fn with_outcome(mut self, outcome: DiagnosticsOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    /// Attach short context. This is the summary-verbosity payload: use
    /// [`DiagnosticsContext::attach_detail`] for verbose payloads.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// One human-readable line (used by the frame-chain export and tests).
    pub fn one_line(&self) -> String {
        let mut line = format!(
            "session={} frame={} generation={} phase={} at={}us wall={}ms config_rev={} rules_rev={} outcome={}",
            self.session_id,
            self.frame_id,
            self.tooltip_generation,
            self.phase.as_str(),
            self.at_micros,
            self.wall_unix_ms,
            self.config_revision.as_str(),
            self.rules_revision.as_str(),
            self.outcome.as_str(),
        );
        match &self.outcome {
            DiagnosticsOutcome::Failed { message } => {
                line.push_str(&format!("({message})"));
            }
            DiagnosticsOutcome::Discarded { reason } => {
                line.push_str(&format!("({})", reason.as_str()));
            }
            DiagnosticsOutcome::Ok => {}
        }
        if let Some(duration) = self.duration_micros {
            line.push_str(&format!(" duration={duration}us"));
        }
        if let Some(first_seen) = self.first_seen_at_micros {
            line.push_str(&format!(" first_seen={first_seen}us"));
        }
        if let Some(detail) = &self.detail {
            line.push_str(&format!(" detail={detail}"));
        }
        line
    }
}

/// Shared revision + generation slots of one diagnostics session.
///
/// The slots are shared (not copied) so the OCR worker thread and the frame
/// loop always record the revisions currently applied to the running runtime.
#[derive(Debug)]
struct SharedSlots {
    config_revision: AtomicU64,
    rules_revision: AtomicU64,
    tooltip_generation: AtomicU64,
}

/// Handle over the bounded runtime event buffer.
#[derive(Debug, Clone)]
pub struct DiagnosticsHandle {
    inner: Arc<HandleInner>,
}

#[derive(Debug)]
struct HandleInner {
    capacity: usize,
    verbosity: DiagnosticsVerbosity,
    events: Mutex<VecDeque<DiagnosticsEvent>>,
    accepted: AtomicU64,
    dropped: AtomicU64,
}

impl DiagnosticsHandle {
    /// Create a handle with a fixed capacity and verbosity.
    pub fn new(capacity: usize, verbosity: DiagnosticsVerbosity) -> Self {
        Self {
            inner: Arc::new(HandleInner {
                capacity,
                verbosity,
                events: Mutex::new(VecDeque::new()),
                accepted: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
            }),
        }
    }

    /// A handle that accepts nothing (diagnostics switched off, CLI defaults).
    pub fn disabled() -> Self {
        Self::new(0, DiagnosticsVerbosity::Summary)
    }

    /// Store one event; returns whether it was accepted. A full buffer drops
    /// its oldest event and counts the drop.
    pub fn emit(&self, event: DiagnosticsEvent) -> bool {
        if self.inner.capacity == 0 {
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let mut events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut dropped_here = 0u64;
        while events.len() >= self.inner.capacity {
            events.pop_front();
            dropped_here = dropped_here.saturating_add(1);
        }
        events.push_back(event);
        drop(events);
        if dropped_here > 0 {
            self.inner
                .dropped
                .fetch_add(dropped_here, Ordering::Relaxed);
        }
        self.inner.accepted.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Take every buffered event, oldest first.
    pub fn drain(&self) -> Vec<DiagnosticsEvent> {
        let mut events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        events.drain(..).collect()
    }

    /// Events accepted into the buffer since creation.
    pub fn accepted_events(&self) -> u64 {
        self.inner.accepted.load(Ordering::Relaxed)
    }

    /// Events dropped because the buffer was full (or disabled).
    pub fn dropped_events(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Whether this handle stores events at all.
    pub fn is_enabled(&self) -> bool {
        self.inner.capacity > 0
    }

    /// Configured verbosity.
    pub fn verbosity(&self) -> DiagnosticsVerbosity {
        self.inner.verbosity
    }
}

/// Per-session diagnostics context handed to every emitter.
///
/// Cloning is cheap and shares the clock, the revisions and the event buffer,
/// so the OCR worker thread and the frame loop emit into one session record.
#[derive(Debug, Clone)]
pub struct DiagnosticsContext {
    session_id: u64,
    clock: DiagnosticsClock,
    handle: DiagnosticsHandle,
    slots: Arc<SharedSlots>,
}

impl DiagnosticsContext {
    /// Create the context of one analysis session.
    pub fn new(
        session_id: u64,
        clock: DiagnosticsClock,
        handle: DiagnosticsHandle,
        config_revision: Revision,
        rules_revision: Revision,
    ) -> Self {
        Self {
            session_id,
            clock,
            handle,
            slots: Arc::new(SharedSlots {
                config_revision: AtomicU64::new(config_revision.to_slot()),
                rules_revision: AtomicU64::new(rules_revision.to_slot()),
                tooltip_generation: AtomicU64::new(0),
            }),
        }
    }

    /// A context that records nothing (default for non-live callers/tests).
    pub fn disabled() -> Self {
        Self::new(
            next_session_id(),
            DiagnosticsClock::new(),
            DiagnosticsHandle::disabled(),
            Revision::Unknown,
            Revision::Unknown,
        )
    }

    /// Session id of this context.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Session clock.
    pub fn clock(&self) -> &DiagnosticsClock {
        &self.clock
    }

    /// The shared runtime event buffer.
    pub fn handle(&self) -> &DiagnosticsHandle {
        &self.handle
    }

    /// Configured verbosity.
    pub fn verbosity(&self) -> DiagnosticsVerbosity {
        self.handle.verbosity()
    }

    /// Whether this context records anything.
    pub fn is_enabled(&self) -> bool {
        self.handle.is_enabled()
    }

    /// Monotonic microseconds since the session clock origin.
    pub fn now_micros(&self) -> u64 {
        self.clock.now_micros()
    }

    /// Applied configuration revision.
    pub fn config_revision(&self) -> Revision {
        Revision::from_slot(self.slots.config_revision.load(Ordering::Relaxed))
    }

    /// Applied rule-set revision.
    pub fn rules_revision(&self) -> Revision {
        Revision::from_slot(self.slots.rules_revision.load(Ordering::Relaxed))
    }

    /// Tooltip generation the events are stamped with.
    ///
    /// The runtime coordinator mirrors its own tooltip generation (the
    /// `tooltip_generation` of its analysis tokens) into this slot, so every
    /// event names the generation that was current when it was recorded. `0`
    /// means no observation yet.
    pub fn tooltip_generation(&self) -> u64 {
        self.slots.tooltip_generation.load(Ordering::Relaxed)
    }

    /// Record a newly applied configuration revision.
    pub fn set_config_revision(&self, revision: Revision) {
        self.slots
            .config_revision
            .store(revision.to_slot(), Ordering::Relaxed);
    }

    /// Record a newly applied rule-set revision.
    pub fn set_rules_revision(&self, revision: Revision) {
        self.slots
            .rules_revision
            .store(revision.to_slot(), Ordering::Relaxed);
    }

    /// Mirror the coordinator's current tooltip generation.
    pub fn set_tooltip_generation(&self, generation: u64) {
        self.slots
            .tooltip_generation
            .store(generation, Ordering::Relaxed);
    }

    /// Bump the tooltip observation generation.
    pub fn bump_tooltip_generation(&self) -> u64 {
        self.slots
            .tooltip_generation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    /// Begin an event "now" with the session's revisions and generation.
    pub fn begin(&self, frame_id: u64, phase: DiagnosticsPhase) -> DiagnosticsEvent {
        DiagnosticsEvent {
            session_id: self.session_id,
            frame_id,
            tooltip_generation: self.tooltip_generation(),
            config_revision: self.config_revision(),
            rules_revision: self.rules_revision(),
            phase,
            at_micros: self.now_micros(),
            wall_unix_ms: wall_unix_ms_now(),
            duration_micros: None,
            first_seen_at_micros: None,
            outcome: DiagnosticsOutcome::Ok,
            detail: None,
        }
    }

    /// Attach a verbose payload — only when the detail verbosity is on. The
    /// payload closure only runs in detail mode, so a summary build never
    /// builds the payload string at all.
    #[must_use]
    pub fn attach_detail(
        &self,
        event: DiagnosticsEvent,
        detail: impl FnOnce() -> String,
    ) -> DiagnosticsEvent {
        if self.verbosity().records_detail() {
            event.with_detail(detail())
        } else {
            event
        }
    }

    /// Record one event into the shared buffer.
    pub fn record(&self, event: DiagnosticsEvent) -> bool {
        self.handle.emit(event)
    }

    /// Convenience: begin and record a one-shot event with an outcome.
    pub fn record_outcome(
        &self,
        frame_id: u64,
        phase: DiagnosticsPhase,
        outcome: DiagnosticsOutcome,
    ) -> bool {
        self.record(self.begin(frame_id, phase).with_outcome(outcome))
    }

    /// Take every event recorded since the last drain.
    pub fn drain(&self) -> Vec<DiagnosticsEvent> {
        self.handle.drain()
    }

    /// Events dropped by the bounded buffer.
    pub fn dropped_events(&self) -> u64 {
        self.handle.dropped_events()
    }
}

impl Default for DiagnosticsContext {
    fn default() -> Self {
        Self::disabled()
    }
}

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local monotonic session id.
pub fn next_session_id() -> u64 {
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

thread_local! {
    static JOB_CONTEXT: std::cell::RefCell<Option<JobDiagnostics>> =
        const { std::cell::RefCell::new(None) };
}

/// Ambient diagnostics context of the OCR job currently executed on this
/// thread: the session context plus the frame the job was planned for.
#[derive(Debug, Clone)]
pub struct JobDiagnostics {
    context: DiagnosticsContext,
    frame_id: u64,
}

impl JobDiagnostics {
    /// Build a job context.
    pub fn new(context: DiagnosticsContext, frame_id: u64) -> Self {
        Self { context, frame_id }
    }

    /// The session context.
    pub fn context(&self) -> &DiagnosticsContext {
        &self.context
    }

    /// The frame the job was planned for.
    pub fn frame_id(&self) -> u64 {
        self.frame_id
    }
}

/// Run `body` with the OCR job's diagnostics context installed.
///
/// The OCR worker wraps every job execution with this. Production call chains
/// between the worker and the OCR parity path include caller-owned code that
/// cannot take a diagnostics parameter (the app's worker closure), so the
/// context travels explicitly through the worker and is read back by the
/// parity path via [`current_job_context`]. Nothing else may read it, and the
/// context is always cleared afterwards.
pub fn with_job_context_scope<R>(
    context: DiagnosticsContext,
    frame_id: u64,
    body: impl FnOnce() -> R,
) -> R {
    JOB_CONTEXT.with(|slot| *slot.borrow_mut() = Some(JobDiagnostics::new(context, frame_id)));
    let result = body();
    JOB_CONTEXT.with(|slot| *slot.borrow_mut() = None);
    result
}

/// The ambient OCR-job diagnostics context, when the current thread is inside
/// [`with_job_context_scope`].
pub fn current_job_context() -> Option<JobDiagnostics> {
    JOB_CONTEXT.with(|slot| slot.borrow().clone())
}

/// The named phase time points of one frame.
///
/// Reconstructed from the frame's events; a time point the runtime cannot
/// observe today stays `None` (absent field, never a fabricated value).
/// `tooltip_visible_at` and `overlay_visible_at` are deliberately absent: they
/// need an external visibility measurement the runtime does not have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameTimeline {
    pub session_id: u64,
    pub frame_id: u64,
    /// First capture the tooltip was contained in.
    pub first_captured_at: Option<u64>,
    /// Capture time of the frame that opened the tooltip track this frame's
    /// job belongs to (`first_seen_at`). It can be older than
    /// `first_captured_at`: the track opens on the frame where the tooltip
    /// first appeared, while the job's pixels come from a later frame.
    pub first_seen_at: Option<u64>,
    /// A valid tooltip was located.
    pub detected_at: Option<u64>,
    /// The OCR job was published.
    pub queued_at: Option<u64>,
    /// The worker started the job.
    pub ocr_started_at: Option<u64>,
    /// The worker finished the job.
    pub ocr_finished_at: Option<u64>,
    /// Rule evaluation finished.
    pub evaluated_at: Option<u64>,
    /// A valid presentation was handed to the backend.
    pub presentation_submitted_at: Option<u64>,
    /// The backend returned from the draw call.
    pub draw_completed_at: Option<u64>,
    /// The first presentation carrying the accepted analysis was confirmed
    /// drawn (`overlay_committed_at`); the event's duration is
    /// `committed − first_seen`.
    pub overlay_committed_at: Option<u64>,
    /// The first empty presentation after an invalidation was confirmed drawn
    /// (`hidden_at`); the event's duration is `hidden − invalidated`.
    pub hidden_at: Option<u64>,
    /// Outcome of the frame's last recorded phase.
    pub outcome: Option<DiagnosticsOutcome>,
    /// First discard reason recorded for the frame.
    pub discard_reason: Option<DiscardReason>,
}

impl FrameTimeline {
    /// Reconstruct the timeline of `frame_id` from `events` (oldest first).
    /// Returns `None` when the events do not contain that frame.
    ///
    /// The frame's events are ordered by their monotonic timestamp, not by
    /// arrival: the frame loop and the OCR worker emit into one record from two
    /// threads, and a job's `ocr_start` can reach the buffer before the frame's
    /// own `ocr_submit`. The timestamps are what defines the chain.
    pub fn from_events(
        events: &[DiagnosticsEvent],
        session_id: u64,
        frame_id: u64,
    ) -> Option<Self> {
        let mut frame_events = frame_events_sorted(events, session_id, frame_id);
        if frame_events.is_empty() {
            return None;
        }
        frame_events.sort_by_key(|event| (event.at_micros, event.phase.index()));
        let mut timeline = FrameTimeline {
            session_id,
            frame_id,
            ..FrameTimeline::default()
        };
        for event in frame_events {
            match event.phase {
                DiagnosticsPhase::Capture => {
                    timeline.first_captured_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::OcrSubmit => {
                    timeline.queued_at.get_or_insert(event.at_micros);
                    if let Some(first_seen) = event.first_seen_at_micros {
                        timeline.first_seen_at.get_or_insert(first_seen);
                    }
                }
                DiagnosticsPhase::Detect => {
                    timeline.detected_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::OcrStart => {
                    timeline.ocr_started_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::OcrFinish => {
                    timeline.ocr_finished_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::Evaluate => {
                    timeline.evaluated_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::PresentationSubmit => {
                    timeline
                        .presentation_submitted_at
                        .get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::DrawCompleted => {
                    timeline.draw_completed_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::OverlayCommit => {
                    timeline.overlay_committed_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::OverlayHidden => {
                    timeline.hidden_at.get_or_insert(event.at_micros);
                }
                DiagnosticsPhase::Parse
                | DiagnosticsPhase::StateChange
                | DiagnosticsPhase::Aggregate => {}
            }
            if timeline.discard_reason.is_none() {
                timeline.discard_reason = event.discard_reason();
            }
            timeline.outcome = Some(event.outcome.clone());
        }
        Some(timeline)
    }

    /// Render the timeline as one line (export / verification).
    pub fn render(&self) -> String {
        let instant = |value: Option<u64>| match value {
            Some(value) => value.to_string(),
            None => "none".to_string(),
        };
        let outcome = match &self.outcome {
            Some(outcome) => outcome.as_str().to_string(),
            None => "none".to_string(),
        };
        let discard = match self.discard_reason {
            Some(reason) => reason.as_str(),
            None => "none",
        };
        format!(
            "session={} frame={} first_captured_at={} first_seen_at={} detected_at={} \
             queued_at={} ocr_started_at={} ocr_finished_at={} evaluated_at={} \
             presentation_submitted_at={} draw_completed_at={} overlay_committed_at={} \
             hidden_at={} outcome={} discard={}",
            self.session_id,
            self.frame_id,
            instant(self.first_captured_at),
            instant(self.first_seen_at),
            instant(self.detected_at),
            instant(self.queued_at),
            instant(self.ocr_started_at),
            instant(self.ocr_finished_at),
            instant(self.evaluated_at),
            instant(self.presentation_submitted_at),
            instant(self.draw_completed_at),
            instant(self.overlay_committed_at),
            instant(self.hidden_at),
            outcome,
            discard,
        )
    }
}

/// The events of one frame, in buffer order.
fn frame_events_sorted(
    events: &[DiagnosticsEvent],
    session_id: u64,
    frame_id: u64,
) -> Vec<&DiagnosticsEvent> {
    events
        .iter()
        .filter(|event| event.session_id == session_id && event.frame_id == frame_id)
        .collect()
}

/// Render the phase chain of one frame as one export line.
///
/// This is the "coherent frame chain" view: the ordered phases of a processed
/// frame with their outcomes (or the frame's discard reason), plus the named
/// time points of [`FrameTimeline`]. The order is the frames's monotonic time
/// order, so a fast OCR worker emitting before the frame loop wrote its own
/// submit event cannot reorder the chain.
pub fn render_frame_chain(
    events: &[DiagnosticsEvent],
    session_id: u64,
    frame_id: u64,
) -> Option<String> {
    let mut frame_events = frame_events_sorted(events, session_id, frame_id);
    if frame_events.is_empty() {
        return None;
    }
    frame_events.sort_by_key(|event| (event.at_micros, event.phase.index()));
    let chain: Vec<String> = frame_events
        .iter()
        .map(|event| match &event.outcome {
            DiagnosticsOutcome::Ok => format!("{}:ok", event.phase.as_str()),
            DiagnosticsOutcome::Failed { .. } => format!("{}:failed", event.phase.as_str()),
            DiagnosticsOutcome::Discarded { reason } => {
                format!("{}:discarded({})", event.phase.as_str(), reason.as_str())
            }
        })
        .collect();
    let timeline = FrameTimeline::from_events(events, session_id, frame_id)?;
    Some(format!(
        "{} chain={}",
        timeline.render(),
        chain.join(" -> ")
    ))
}

/// Aggregate statistics over recorded events (default verbosity).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticsStats {
    /// Events observed by the aggregate.
    pub recorded: u64,
    /// Successful phases.
    pub ok: u64,
    /// Failed phases.
    pub failed: u64,
    /// Discarded phases.
    pub discarded: u64,
    /// Discards per [`DiscardReason`], indexed by [`DiscardReason::index`].
    pub discarded_by_reason: [u64; DiscardReason::ALL.len()],
    /// Phases per [`DiagnosticsPhase`], indexed by [`DiagnosticsPhase::index`].
    pub phases: [u64; DiagnosticsPhase::ALL.len()],
    /// Events the bounded sinks had to drop.
    pub dropped: u64,
}

impl DiagnosticsStats {
    /// Fold one event into the aggregate.
    pub fn observe(&mut self, event: &DiagnosticsEvent) {
        self.recorded = self.recorded.saturating_add(1);
        self.phases[event.phase.index()] = self.phases[event.phase.index()].saturating_add(1);
        match &event.outcome {
            DiagnosticsOutcome::Ok => self.ok = self.ok.saturating_add(1),
            DiagnosticsOutcome::Failed { .. } => self.failed = self.failed.saturating_add(1),
            DiagnosticsOutcome::Discarded { reason } => {
                self.discarded = self.discarded.saturating_add(1);
                self.discarded_by_reason[reason.index()] =
                    self.discarded_by_reason[reason.index()].saturating_add(1);
            }
        }
    }

    /// Fold a batch of events (e.g. one drained buffer) into the aggregate.
    pub fn observe_all<'a>(&mut self, events: impl IntoIterator<Item = &'a DiagnosticsEvent>) {
        for event in events {
            self.observe(event);
        }
    }

    /// Render the aggregate as one key=value line.
    pub fn render(&self) -> String {
        let mut line = format!(
            "recorded={} ok={} failed={} discarded={} dropped={}",
            self.recorded, self.ok, self.failed, self.discarded, self.dropped
        );
        for phase in DiagnosticsPhase::ALL {
            line.push_str(&format!(
                " phase.{}={}",
                phase.as_str(),
                self.phases[phase.index()]
            ));
        }
        for reason in DiscardReason::ALL {
            line.push_str(&format!(
                " discard.{}={}",
                reason.as_str(),
                self.discarded_by_reason[reason.index()]
            ));
        }
        line
    }
}

/// Capacity of one per-tick timing ring (a bounded ring per field).
pub const LOOP_TIMING_RING_CAPACITY: usize = 512;

/// Stage costs of one live-loop tick, in microseconds.
///
/// Fields the loop cannot observe this tick stay `None` — never zero-filled:
/// a `None` raster or commit cost means the backend did not report one (no
/// draw, or a backend without draw timing), and the percentile export keeps
/// the field absent instead of showing a fake 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoopTickSample {
    pub capture_us: u64,
    pub detect_us: u64,
    /// Detection split: the full-frame crop into the search region.
    pub detect_crop_us: u64,
    /// Detection split: grayscale.
    pub detect_gray_us: u64,
    /// Detection split: squared integral image build.
    pub detect_integral_us: u64,
    /// Detection split: template scan (`detect_us` minus the other splits;
    /// saturating).
    pub detect_scan_us: u64,
    /// Threads that ran this tick's template scan; `None` when no scan ran.
    pub detect_scan_threads: Option<u64>,
    /// Coordinator work this tick: result adoption plus observation.
    pub coordinator_us: u64,
    pub present_us: u64,
    /// The backend-reported raster cost of the last draw.
    pub present_raster_us: Option<u64>,
    /// The backend-reported commit/roundtrip cost of the last draw.
    pub present_commit_us: Option<u64>,
    /// Time the loop spent waiting for the next tick.
    pub sleep_us: u64,
    /// Time between the start of this tick and the start of the previous one.
    pub period_us: u64,
}

/// One timing field's percentiles over its ring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FieldPercentiles {
    pub p50: Option<u64>,
    pub p95: Option<u64>,
    pub p99: Option<u64>,
}

impl FieldPercentiles {
    /// Nearest-rank percentiles of the observed values. Absent for a field
    /// with no observed value.
    fn of(values: impl Iterator<Item = Option<u64>>) -> Self {
        let mut observed: Vec<u64> = values.flatten().collect();
        if observed.is_empty() {
            return Self::default();
        }
        observed.sort_unstable();
        let percentile = |p: usize| -> Option<u64> {
            let rank = (p * observed.len()).div_ceil(100);
            observed.get(rank.saturating_sub(1)).copied()
        };
        Self {
            p50: percentile(50),
            p95: percentile(95),
            p99: percentile(99),
        }
    }
}

/// Per-tick timing rings with a p50/p95/p99 export per field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopTimingPercentiles {
    pub capture_us: FieldPercentiles,
    pub detect_us: FieldPercentiles,
    pub detect_crop_us: FieldPercentiles,
    pub detect_gray_us: FieldPercentiles,
    pub detect_integral_us: FieldPercentiles,
    pub detect_scan_us: FieldPercentiles,
    /// Fewest and most scan threads over the ring; `None` while no tick
    /// ran a scan.
    pub detect_scan_threads: Option<(u64, u64)>,
    pub coordinator_us: FieldPercentiles,
    pub present_us: FieldPercentiles,
    pub present_raster_us: FieldPercentiles,
    pub present_commit_us: FieldPercentiles,
    pub sleep_us: FieldPercentiles,
    pub period_us: FieldPercentiles,
}

impl LoopTimingPercentiles {
    fn of(rings: &LoopTimingRings) -> Self {
        let field = |ring: &TimingRing| FieldPercentiles::of(ring.values.iter().copied());
        Self {
            capture_us: field(&rings.capture_us),
            detect_us: field(&rings.detect_us),
            detect_crop_us: field(&rings.detect_crop_us),
            detect_gray_us: field(&rings.detect_gray_us),
            detect_integral_us: field(&rings.detect_integral_us),
            detect_scan_us: field(&rings.detect_scan_us),
            detect_scan_threads: rings.detect_scan_threads.values.iter().flatten().fold(
                None,
                |range: Option<(u64, u64)>, &threads| {
                    Some(range.map_or((threads, threads), |(fewest, most)| {
                        (fewest.min(threads), most.max(threads))
                    }))
                },
            ),
            coordinator_us: field(&rings.coordinator_us),
            present_us: field(&rings.present_us),
            present_raster_us: field(&rings.present_raster_us),
            present_commit_us: field(&rings.present_commit_us),
            sleep_us: field(&rings.sleep_us),
            period_us: field(&rings.period_us),
        }
    }

    /// Whether no field observed a value.
    pub fn is_empty(&self) -> bool {
        let measured = |field: &FieldPercentiles| field.p50.is_some();
        !measured(&self.capture_us)
            && !measured(&self.detect_us)
            && !measured(&self.detect_crop_us)
            && !measured(&self.detect_gray_us)
            && !measured(&self.detect_integral_us)
            && !measured(&self.detect_scan_us)
            && self.detect_scan_threads.is_none()
            && !measured(&self.coordinator_us)
            && !measured(&self.present_us)
            && !measured(&self.present_raster_us)
            && !measured(&self.present_commit_us)
            && !measured(&self.sleep_us)
            && !measured(&self.period_us)
    }

    /// The Debug-panel line: one `p50/p95/p99` triple per field, `n/a` while
    /// a field has no observed value. `scan_threads` is a count, not a time:
    /// the one value seen over the ring, `fewest-most` when it varied.
    pub fn render(&self) -> String {
        let field = |name: &str, percentiles: &FieldPercentiles| {
            format!(
                "{name}={}/{}/{}",
                render_percentile(percentiles.p50),
                render_percentile(percentiles.p95),
                render_percentile(percentiles.p99),
            )
        };
        [
            field("capture", &self.capture_us),
            field("detect", &self.detect_us),
            field("crop", &self.detect_crop_us),
            field("gray", &self.detect_gray_us),
            field("integral", &self.detect_integral_us),
            field("scan", &self.detect_scan_us),
            format!(
                "scan_threads={}",
                match self.detect_scan_threads {
                    None => "n/a".to_string(),
                    Some((fewest, most)) if fewest == most => fewest.to_string(),
                    Some((fewest, most)) => format!("{fewest}-{most}"),
                }
            ),
            field("coordinator", &self.coordinator_us),
            field("present", &self.present_us),
            field("raster", &self.present_raster_us),
            field("commit", &self.present_commit_us),
            field("sleep", &self.sleep_us),
            field("period", &self.period_us),
        ]
        .join(" ")
    }
}

fn render_percentile(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

/// One bounded per-tick ring.
#[derive(Debug, Default)]
struct TimingRing {
    values: VecDeque<Option<u64>>,
}

impl TimingRing {
    fn push(&mut self, value: Option<u64>) {
        if self.values.len() >= LOOP_TIMING_RING_CAPACITY {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }
}

/// Bounded rings of the per-tick stage costs, shared between the live loop
/// (records one [`LoopTickSample`] per tick) and the exporters (percentiles).
#[derive(Debug, Default)]
struct LoopTimingRings {
    capture_us: TimingRing,
    detect_us: TimingRing,
    detect_crop_us: TimingRing,
    detect_gray_us: TimingRing,
    detect_integral_us: TimingRing,
    detect_scan_us: TimingRing,
    detect_scan_threads: TimingRing,
    coordinator_us: TimingRing,
    present_us: TimingRing,
    present_raster_us: TimingRing,
    present_commit_us: TimingRing,
    sleep_us: TimingRing,
    period_us: TimingRing,
}

/// Shared handle over the per-tick timing rings. Cloning shares the rings.
#[derive(Debug, Clone, Default)]
pub struct LoopTimingRecorder {
    rings: Arc<Mutex<LoopTimingRings>>,
}

impl LoopTimingRecorder {
    fn rings(rings: &Mutex<LoopTimingRings>) -> std::sync::MutexGuard<'_, LoopTimingRings> {
        rings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record one tick's sample. Overflow drops the oldest sample.
    pub fn record(&self, sample: LoopTickSample) {
        let mut rings = Self::rings(&self.rings);
        rings.capture_us.push(Some(sample.capture_us));
        rings.detect_us.push(Some(sample.detect_us));
        rings.detect_crop_us.push(Some(sample.detect_crop_us));
        rings.detect_gray_us.push(Some(sample.detect_gray_us));
        rings
            .detect_integral_us
            .push(Some(sample.detect_integral_us));
        rings.detect_scan_us.push(Some(sample.detect_scan_us));
        rings.detect_scan_threads.push(sample.detect_scan_threads);
        rings.coordinator_us.push(Some(sample.coordinator_us));
        rings.present_us.push(Some(sample.present_us));
        rings.present_raster_us.push(sample.present_raster_us);
        rings.present_commit_us.push(sample.present_commit_us);
        rings.sleep_us.push(Some(sample.sleep_us));
        rings.period_us.push(Some(sample.period_us));
    }

    /// Percentiles over the recorded samples.
    pub fn percentiles(&self) -> LoopTimingPercentiles {
        LoopTimingPercentiles::of(&Self::rings(&self.rings))
    }

    /// Number of samples currently held (bounded by
    /// [`LOOP_TIMING_RING_CAPACITY`]).
    pub fn len(&self) -> usize {
        Self::rings(&self.rings).capture_us.values.len()
    }

    /// Whether no sample was recorded yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    fn context(capacity: usize) -> DiagnosticsContext {
        DiagnosticsContext::new(
            1,
            DiagnosticsClock::new(),
            DiagnosticsHandle::new(capacity, DiagnosticsVerbosity::Summary),
            Revision::Applied(3),
            Revision::Applied(7),
        )
    }

    #[test]
    fn handle_drops_oldest_beyond_capacity_and_counts_the_drops() {
        let handle = DiagnosticsHandle::new(4, DiagnosticsVerbosity::Summary);
        let context = DiagnosticsContext::new(
            1,
            DiagnosticsClock::new(),
            handle.clone(),
            Revision::Unknown,
            Revision::Unknown,
        );

        for frame_id in 0..10u64 {
            context.record_outcome(frame_id, DiagnosticsPhase::Capture, DiagnosticsOutcome::Ok);
        }

        let drained = handle.drain();
        assert_eq!(drained.len(), 4, "the buffer keeps exactly its capacity");
        // Oldest-first retention: the newest four frames survive.
        let frame_ids: Vec<u64> = drained.iter().map(|event| event.frame_id).collect();
        assert_eq!(frame_ids, vec![6, 7, 8, 9]);
        assert_eq!(handle.accepted_events(), 10);
        assert_eq!(handle.dropped_events(), 6);
    }

    #[test]
    fn disabled_handle_counts_every_event_as_dropped_and_stores_nothing() {
        let context = DiagnosticsContext::disabled();
        assert!(!context.is_enabled());

        for _ in 0..3 {
            assert!(!context.record_outcome(0, DiagnosticsPhase::Capture, DiagnosticsOutcome::Ok));
        }

        assert!(context.drain().is_empty());
        assert_eq!(context.dropped_events(), 3);
        // The disabled context is explicit about its unknown revisions.
        assert_eq!(context.config_revision(), Revision::Unknown);
        assert_eq!(context.rules_revision(), Revision::Unknown);
    }

    #[test]
    fn session_ids_are_process_monotonic() {
        let first = next_session_id();
        let second = next_session_id();
        assert!(second > first);
        assert!(DiagnosticsContext::disabled().session_id() > second);
    }

    #[test]
    fn revisions_and_generation_come_from_the_shared_slots() {
        let context = context(8);
        assert_eq!(context.config_revision(), Revision::Applied(3));
        assert_eq!(context.rules_revision(), Revision::Applied(7));
        assert_eq!(context.tooltip_generation(), 0);

        // A clone (the OCR worker's copy) observes the same revisions, so a
        // configuration change applied by the frame loop reaches worker events.
        let worker_copy = context.clone();
        context.set_config_revision(Revision::Applied(4));
        assert_eq!(worker_copy.config_revision(), Revision::Applied(4));

        assert_eq!(context.bump_tooltip_generation(), 1);
        assert_eq!(worker_copy.tooltip_generation(), 1);
    }

    #[test]
    fn detail_payload_is_attached_only_in_detail_verbosity() {
        let summary = context(8);
        let event = summary.begin(1, DiagnosticsPhase::OcrFinish);
        let event = summary.attach_detail(event, || "raw ocr text".to_string());
        assert!(event.detail.is_none());

        let detail = DiagnosticsContext::new(
            2,
            DiagnosticsClock::new(),
            DiagnosticsHandle::new(8, DiagnosticsVerbosity::Detail),
            Revision::Unknown,
            Revision::Unknown,
        );
        let event = detail.begin(1, DiagnosticsPhase::OcrFinish);
        let event = detail.attach_detail(event, || "raw ocr text".to_string());
        assert_eq!(event.detail.as_deref(), Some("raw ocr text"));

        // The payload closure only runs in detail mode: a summary context
        // never pays the payload construction.
        let mut built = false;
        let event = summary.begin(2, DiagnosticsPhase::OcrFinish);
        let _ = summary.attach_detail(event, || {
            built = true;
            "payload".to_string()
        });
        assert!(!built, "summary verbosity must not build detail payloads");
    }

    #[test]
    fn frame_timeline_maps_the_named_time_points_and_keeps_missing_ones_absent() {
        let context = context(64);
        let frame = 7u64;
        let mut events = Vec::new();
        let record = |phase: DiagnosticsPhase, at: u64, outcome: DiagnosticsOutcome| {
            let mut event = context.begin(frame, phase);
            event.at_micros = at;
            event.with_outcome(outcome)
        };
        events.push(record(
            DiagnosticsPhase::Capture,
            1_000,
            DiagnosticsOutcome::Ok,
        ));
        events.push(record(
            DiagnosticsPhase::Detect,
            1_500,
            DiagnosticsOutcome::Ok,
        ));
        events.push(record(
            DiagnosticsPhase::OcrSubmit,
            1_600,
            DiagnosticsOutcome::Ok,
        ));
        events.push(record(
            DiagnosticsPhase::OcrStart,
            2_000,
            DiagnosticsOutcome::Ok,
        ));
        events.push(record(
            DiagnosticsPhase::OcrFinish,
            9_000,
            DiagnosticsOutcome::Ok,
        ));
        events.push(record(
            DiagnosticsPhase::Evaluate,
            9_500,
            DiagnosticsOutcome::Ok,
        ));
        events.push(record(
            DiagnosticsPhase::PresentationSubmit,
            9_600,
            DiagnosticsOutcome::Ok,
        ));
        // No draw event in this chain: the time point stays absent.
        let timeline = FrameTimeline::from_events(&events, 1, frame)
            .unwrap_or_else(|| panic!("frame must be present"));
        assert_eq!(timeline.first_captured_at, Some(1_000));
        assert_eq!(timeline.detected_at, Some(1_500));
        assert_eq!(timeline.queued_at, Some(1_600));
        assert_eq!(timeline.ocr_started_at, Some(2_000));
        assert_eq!(timeline.ocr_finished_at, Some(9_000));
        assert_eq!(timeline.evaluated_at, Some(9_500));
        assert_eq!(timeline.presentation_submitted_at, Some(9_600));
        assert_eq!(timeline.draw_completed_at, None);
        assert_eq!(timeline.outcome, Some(DiagnosticsOutcome::Ok));
        assert_eq!(timeline.discard_reason, None);

        // Another session's events for the same frame id never leak in.
        assert!(FrameTimeline::from_events(&events, 2, frame).is_none());
        assert!(FrameTimeline::from_events(&events, 1, frame + 1).is_none());
    }

    /// The 0.2 time points: `first_seen_at` arrives on the job's submit
    /// event, the overlay commitment and hide on their own phase events.
    /// Fields the session did not observe stay absent — never zero-filled.
    #[test]
    fn frame_timeline_carries_first_seen_and_overlay_time_points() {
        let context = context(64);
        let frame = 9u64;
        let mut events = Vec::new();
        let record = |context: &DiagnosticsContext,
                      phase: DiagnosticsPhase,
                      at: u64,
                      outcome: DiagnosticsOutcome| {
            let mut event = context.begin(frame, phase);
            event.at_micros = at;
            event.with_outcome(outcome)
        };

        events.push(record(
            &context,
            DiagnosticsPhase::Capture,
            1_000,
            DiagnosticsOutcome::Ok,
        ));
        let mut submit = record(
            &context,
            DiagnosticsPhase::OcrSubmit,
            1_600,
            DiagnosticsOutcome::Ok,
        );
        submit.first_seen_at_micros = Some(500);
        events.push(submit);
        let mut commit = record(
            &context,
            DiagnosticsPhase::OverlayCommit,
            9_800,
            DiagnosticsOutcome::Ok,
        );
        commit.duration_micros = Some(9_300);
        events.push(commit);
        events.push(record(
            &context,
            DiagnosticsPhase::OverlayHidden,
            12_000,
            DiagnosticsOutcome::Ok,
        ));

        let timeline = FrameTimeline::from_events(&events, 1, frame)
            .unwrap_or_else(|| panic!("frame must be present"));
        assert_eq!(timeline.first_captured_at, Some(1_000));
        assert_eq!(timeline.first_seen_at, Some(500));
        assert_eq!(timeline.queued_at, Some(1_600));
        assert_eq!(timeline.overlay_committed_at, Some(9_800));
        assert_eq!(timeline.hidden_at, Some(12_000));
        // The commit event's duration is the committed − first_seen latency.
        assert_eq!(
            events
                .iter()
                .find(|event| event.phase == DiagnosticsPhase::OverlayCommit)
                .and_then(|event| event.duration_micros),
            Some(9_300)
        );

        let rendered = timeline.render();
        assert!(rendered.contains("first_seen_at=500"));
        assert!(rendered.contains("overlay_committed_at=9800"));
        assert!(rendered.contains("hidden_at=12000"));

        // An event set without the new time points leaves every one of them
        // absent (the same shape the older tests pin).
        let mut bare = Vec::new();
        let mut event = context.begin(frame + 1, DiagnosticsPhase::Capture);
        event.at_micros = 2_000;
        bare.push(event);
        let timeline = FrameTimeline::from_events(&bare, 1, frame + 1)
            .unwrap_or_else(|| panic!("frame must be present"));
        assert_eq!(timeline.first_seen_at, None);
        assert_eq!(timeline.overlay_committed_at, None);
        assert_eq!(timeline.hidden_at, None);
        assert!(timeline.render().contains("first_seen_at=none"));
    }

    /// Nearest-rank percentiles over the per-tick rings; fields never
    /// observed stay absent, and a ring never grows past its capacity.
    #[test]
    fn loop_timing_rings_export_percentiles_and_stay_bounded() {
        let recorder = LoopTimingRecorder::default();
        assert!(recorder.is_empty());
        assert!(recorder.percentiles().is_empty());

        for value in 1..=100u64 {
            recorder.record(LoopTickSample {
                capture_us: value,
                detect_us: value * 2,
                period_us: value,
                ..LoopTickSample::default()
            });
        }
        let percentiles = recorder.percentiles();
        assert_eq!(percentiles.capture_us.p50, Some(50));
        assert_eq!(percentiles.capture_us.p95, Some(95));
        assert_eq!(percentiles.capture_us.p99, Some(99));
        assert_eq!(percentiles.detect_us.p50, Some(100));
        assert_eq!(percentiles.period_us.p95, Some(95));
        // Never-measured fields (the backend-reported draw split) stay absent
        // instead of showing a fabricated 0.
        assert_eq!(percentiles.present_raster_us, FieldPercentiles::default());
        assert_eq!(percentiles.present_commit_us, FieldPercentiles::default());
        assert_eq!(recorder.len(), 100);

        let rendered = percentiles.render();
        assert!(rendered.contains("capture=50/95/99"));
        assert!(rendered.contains("detect=100/190/198"));
        assert!(rendered.contains("raster=n/a/n/a/n/a"));
        // No tick ran a scan: the thread count is absent, not 0.
        assert_eq!(percentiles.detect_scan_threads, None);
        assert!(rendered.contains("scan_threads=n/a"));

        recorder.record(LoopTickSample {
            detect_scan_threads: Some(4),
            ..LoopTickSample::default()
        });
        assert!(recorder.percentiles().render().contains("scan_threads=4 "));
        recorder.record(LoopTickSample {
            detect_scan_threads: Some(2),
            ..LoopTickSample::default()
        });
        assert!(recorder
            .percentiles()
            .render()
            .contains("scan_threads=2-4 "));

        for value in 0..(LOOP_TIMING_RING_CAPACITY as u64 + 50) {
            recorder.record(LoopTickSample {
                capture_us: value,
                ..LoopTickSample::default()
            });
        }
        assert_eq!(recorder.len(), LOOP_TIMING_RING_CAPACITY);
        let percentiles = recorder.percentiles();
        // The oldest samples fell out of the ring: every remaining value is
        // from the newest 512.
        assert!(
            percentiles
                .capture_us
                .p50
                .is_some_and(|p50| p50 >= LOOP_TIMING_RING_CAPACITY as u64 / 2),
            "p50 must come from the newest samples, got {percentiles:?}"
        );
    }

    #[test]
    fn frame_chain_renders_capture_to_discard_reason() {
        let context = context(64);
        let mut events = vec![
            context
                .begin(4, DiagnosticsPhase::Capture)
                .with_outcome(DiagnosticsOutcome::Ok),
            context
                .begin(4, DiagnosticsPhase::Detect)
                .with_outcome(DiagnosticsOutcome::discarded(DiscardReason::TooltipLost)),
        ];
        events[0].at_micros = 100;
        events[1].at_micros = 200;

        let chain =
            render_frame_chain(&events, 1, 4).unwrap_or_else(|| panic!("frame chain must render"));
        assert!(chain.contains("chain=capture:ok -> detect:discarded(TooltipLost)"));
        assert!(chain.contains("first_captured_at=100"));
        assert!(chain.contains("detected_at=200"));
        assert!(chain.contains("discard=TooltipLost"));
        assert!(render_frame_chain(&events, 1, 5).is_none());
    }

    #[test]
    fn statistics_count_phases_outcomes_and_discard_reasons() {
        let mut stats = DiagnosticsStats::default();
        let context = context(64);
        let events = vec![
            context
                .begin(1, DiagnosticsPhase::Capture)
                .with_outcome(DiagnosticsOutcome::Ok),
            context
                .begin(1, DiagnosticsPhase::Detect)
                .with_outcome(DiagnosticsOutcome::discarded(DiscardReason::TooltipLost)),
            context
                .begin(2, DiagnosticsPhase::Capture)
                .with_outcome(DiagnosticsOutcome::discarded(DiscardReason::CaptureFailed)),
            context
                .begin(2, DiagnosticsPhase::Parse)
                .with_outcome(DiagnosticsOutcome::failed("not an item")),
        ];
        stats.observe_all(&events);
        stats.dropped = 5;

        assert_eq!(stats.recorded, 4);
        assert_eq!(stats.ok, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.discarded, 2);
        assert_eq!(
            stats.discarded_by_reason[DiscardReason::TooltipLost.index()],
            1
        );
        assert_eq!(stats.phases[DiagnosticsPhase::Capture.index()], 2);
        let rendered = stats.render();
        assert!(rendered.contains("recorded=4"));
        assert!(rendered.contains("discard.TooltipLost=1"));
        assert!(rendered.contains("discard.CaptureFailed=1"));
        assert!(rendered.contains("dropped=5"));
    }

    #[test]
    fn event_one_line_carries_session_frame_generation_and_outcome() {
        let context = context(64);
        let event = context
            .begin(12, DiagnosticsPhase::OcrSubmit)
            .with_duration_micros(250)
            .with_outcome(DiagnosticsOutcome::discarded(DiscardReason::Superseded))
            .with_detail("signature=42");
        let line = event.one_line();

        assert!(line.contains("session=1"));
        assert!(line.contains("frame=12"));
        assert!(line.contains("phase=ocr_submit"));
        assert!(line.contains("outcome=discarded(Superseded)"));
        assert!(line.contains("duration=250us"));
        assert!(line.contains("detail=signature=42"));
        assert!(line.contains("config_rev=3"));
        assert!(line.contains("rules_rev=7"));
    }

    #[test]
    fn job_context_scope_is_set_with_the_job_frame_and_cleared() {
        assert!(current_job_context().is_none());
        let context = context(8);
        let observed = with_job_context_scope(context.clone(), 21, || {
            current_job_context().map(|job| (job.context().session_id(), job.frame_id()))
        });
        assert_eq!(observed, Some((1, 21)));
        assert!(
            current_job_context().is_none(),
            "the ambient context must not leak past the job"
        );
    }

    /// Instrumentation overhead at the emission primitive: what one emitter
    /// pays per event (timestamps + revision reads + bounded buffer push, with
    /// the detail closure attached the way a summary-mode emitter does it).
    /// Reported here; the guard is a coarse ceiling against pathological cost.
    #[test]
    fn emission_overhead_per_event_is_measured() {
        let context = context(4096);
        let iterations = 200_000u64;

        // Warm-up: allocator and buffer growth up to the capacity.
        for _ in 0..1_000 {
            let event = context.begin(1, DiagnosticsPhase::Capture);
            let event = context.attach_detail(event, || "payload".to_string());
            context.record(event);
        }
        context.drain();

        let started = Instant::now();
        for frame in 0..iterations {
            let event = context.begin(frame, DiagnosticsPhase::Capture);
            let event = context.attach_detail(event, || "payload".to_string());
            context.record(event);
        }
        let elapsed = started.elapsed();
        let per_event_ns = elapsed.as_nanos() / u128::from(iterations);
        println!(
            "diagnostics emission overhead: {per_event_ns} ns/event over {iterations} events \
             (summary verbosity; detail closure built per iteration)"
        );

        assert!(
            per_event_ns < 100_000,
            "per-event emission cost is implausibly high: {per_event_ns} ns"
        );
        let drained = context.drain();
        assert_eq!(drained.len(), 4096, "the bounded buffer keeps its capacity");
        assert_eq!(context.dropped_events(), iterations - 4096);
    }
}
