//! Runtime-side adapters of the runtime coordinator.
//!
//! The live loop, not the Slint thread, produces the accepted presentation:
//!
//! ```text
//! UI thread                          live-loop thread (runtime side)
//! ─────────                          ───────────────────────────────
//! RulesPublisher ──RulesSnapshot──►  RulesSnapshotSlot
//!   (immutable, rules revision)        │
//!                                      ▼
//!                  process_one_frame → AnalysisSnapshot (coordinator)
//!                                      │ evaluate against the rules snapshot
//!                                      ▼
//!                                Arc<Presentation> ──► OverlayPresenter
//!                                      │
//! LiveStateMailbox ◄─ call accepted ───┘  (same Arc, bounded, session-fenced)
//!   └► UI projection: current-item card, tooltip state, evaluation, frame
//!      numbers, frozen flag, renderer outcome, overlay status and text state,
//!      async OCR phases — one snapshot, one projection
//! ```
//!
//! The UI only projects what the loop accepted and evaluated; it is not
//! part of the evaluation round trip. Accepted-state updates travel
//! through a bounded "latest state" mailbox, and the remaining discrete
//! notifications through two bounded channels split by what a full buffer may
//! do to them (`super::RuntimeEventSink`), so neither a status flood nor a log
//! flood can grow memory or delay a control command.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use d4lc_core::text_measure::TextMeasure;
use d4lc_core::Rect;
use d4lc_gui::{CurrentItemAffixRowGeometryView, CurrentItemAffixView, CurrentItemNonAffixRowView};
use d4lc_overlay_x11::text::SystemTextMeasure;
use d4lc_overlay_x11::{OverlayFrame, OverlayStatus};
use d4lc_rules::{RuleMatchTrace, RuleOverlayDescriptor};
use d4lc_runtime::{
    AnalysisSnapshot, GameWindowState, InvalidationCause, RuntimeLifecycle, TooltipDebugBundle,
    TooltipState,
};

use crate::app_state::{
    AcceptedEvaluation, AcceptedSnapshotView, AppState, OcrPhaseView, OverlayDrawReport,
    OverlayTextState,
};

use super::RuntimeEvent;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Rules snapshot
// ---------------------------------------------------------------------------

/// An immutable snapshot of every input the rule evaluation reads, stamped
/// with a rules revision.
///
/// The rule projection (`app_state::project_overlay_descriptor_state`) is a
/// function of the app state; the snapshot freezes that state at publish time
/// so the live loop evaluates without touching the UI-owned `AppState`. The
/// current item is not part of the snapshot: the loop supplies the accepted
/// analysis at evaluation time.
#[derive(Debug)]
pub struct RulesSnapshot {
    revision: u64,
    inputs: AppState,
    /// Sizes the paragon panels from the overlay font.
    measure: Arc<dyn TextMeasure>,
    /// The board of the last evaluation; see
    /// [`ParagonDescriptorCache`](super::overlay::ParagonDescriptorCache).
    paragon_cache: super::overlay::ParagonDescriptorCache,
}

impl RulesSnapshot {
    /// Freeze the rule inputs of `state`, measuring text on the system
    /// overlay face.
    #[cfg(test)]
    pub fn capture(state: &AppState, revision: u64) -> Self {
        Self::capture_with(state, revision, Arc::new(SystemTextMeasure))
    }

    /// Freeze the rule inputs of `state`, measuring text with `measure`.
    pub fn capture_with(state: &AppState, revision: u64, measure: Arc<dyn TextMeasure>) -> Self {
        let mut inputs = state.clone();
        // UI-only history is not a rule input. The item fields the runtime
        // owns are replaced by the accepted analysis at evaluation time.
        inputs.logging = Default::default();
        inputs.overlay_descriptor_state = None;
        Self {
            revision,
            inputs,
            measure,
            paragon_cache: super::overlay::ParagonDescriptorCache::default(),
        }
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// `Settings.OverlayFontSize`: the size of every overlay text
    /// (`OverlayHandler.cs:884`, `:910`), bounded to the setting's range so a
    /// hand-edited `Settings.json` cannot ask for an absurd glyph size.
    pub fn overlay_font_px(&self) -> u32 {
        d4lc_config::bounded_overlay_font_size(self.inputs.shell.settings.overlay_font_size)
    }

    /// The overlay power switch of this snapshot (`Settings.OverlayEnabled`).
    pub fn overlay_enabled(&self) -> bool {
        self.inputs.shell.settings.overlay_enabled
    }

    /// The persisted `Settings.IsDebugInfoEnabled` bit:
    /// the live loop flips the OCR worker's debug-bundle gate from it.
    pub fn debug_info_enabled(&self) -> bool {
        self.inputs.shell.debug.debug_info_enabled
    }

    /// Evaluate the accepted analysis against these rules.
    ///
    /// `window_geometry` is the game window the overlay covers (the paragon
    /// board centers on it); `None` keeps the snapshot's own value.
    pub(super) fn evaluate(
        &self,
        analysis: &AnalysisSnapshot,
        window_geometry: Option<Rect>,
        paragon_navigation: Option<&super::paragon::ParagonNavigation>,
    ) -> RuleEvaluation {
        let mut scratch = self.inputs.clone();
        // Without the evaluation's traces no row claims a match; the rule
        // projection itself reads the parsed rows and their geometry.
        super::apply_runtime_event(&mut scratch, &current_item_event(analysis, &[]));
        if let Some(geometry) = window_geometry.filter(|rect| rect.width > 0 && rect.height > 0) {
            scratch.overlay_window_geometry = Some(geometry);
        }
        let mut state = crate::app_state::evaluate_overlay_descriptor_state(&scratch);
        if let (Some(navigation), Some(paragon)) = (
            paragon_navigation,
            state.as_mut().and_then(|state| state.paragon.as_mut()),
        ) {
            navigation.apply(&mut paragon.state);
        }
        RuleEvaluation {
            descriptors: super::convert_descriptors_with(
                state.as_ref(),
                &*self.measure,
                &self.paragon_cache,
            ),
            affix_traces: state
                .and_then(|state| state.rule)
                .map(|rule| rule.result.affix_traces)
                .unwrap_or_default(),
        }
    }
}

impl RulesSnapshot {
    /// [`Self::evaluate`] without paragon navigation, for tests outside the
    /// live loop.
    #[cfg(test)]
    pub(crate) fn evaluate_item(
        &self,
        analysis: &AnalysisSnapshot,
        window_geometry: Option<Rect>,
    ) -> RuleEvaluation {
        self.evaluate(analysis, window_geometry, None)
    }
}

/// What one evaluation of an accepted analysis against a rules snapshot
/// produced: the overlay descriptors and the per-row traces behind the
/// affix markers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleEvaluation {
    pub descriptors: Arc<[RuleOverlayDescriptor]>,
    pub affix_traces: Vec<RuleMatchTrace>,
}

/// The latest published rules snapshot, shared by the UI publisher and the
/// live loop(s). Cloning shares the slot.
#[derive(Debug, Clone, Default)]
pub struct RulesSnapshotSlot {
    current: Arc<Mutex<Option<Arc<RulesSnapshot>>>>,
}

impl RulesSnapshotSlot {
    pub fn publish(&self, snapshot: RulesSnapshot) {
        *lock(&self.current) = Some(Arc::new(snapshot));
    }

    pub fn current(&self) -> Option<Arc<RulesSnapshot>> {
        lock(&self.current).clone()
    }
}

/// The rule-relevant parts of the app state, compared on every UI tick to
/// decide whether a new rules snapshot must be published.
#[derive(Debug, Clone, PartialEq)]
struct RuleInputs {
    settings: d4lc_gui::SettingsPanelState,
    trade_imports: d4lc_gui::TradeImportsPanelState,
    affix_preset_editor: d4lc_gui::AffixPresetEditorState,
    build_import: crate::app_state::BuildImportState,
    downloaded_paragon_routes: Option<Vec<d4lc_rules::ImportedParagonRoute>>,
    overlay_window_geometry: Option<Rect>,
    /// Live debug-bundle gate. Not a rule input, but it
    /// travels the same snapshot so the loop sees the toggle without a second
    /// channel; the producer republishes on every flip.
    debug_info_enabled: bool,
}

impl RuleInputs {
    fn of(state: &AppState) -> Self {
        Self {
            settings: state.shell.settings.clone(),
            trade_imports: state.shell.trade_imports.clone(),
            affix_preset_editor: state.affix_preset_editor.clone(),
            build_import: state.build_import.clone(),
            downloaded_paragon_routes: state.downloaded_paragon_routes.clone(),
            overlay_window_geometry: state.overlay_window_geometry,
            debug_info_enabled: state.shell.debug.debug_info_enabled,
        }
    }

    fn matches(&self, state: &AppState) -> bool {
        self.settings == state.shell.settings
            && self.trade_imports == state.shell.trade_imports
            && self.affix_preset_editor == state.affix_preset_editor
            && self.build_import == state.build_import
            && self.downloaded_paragon_routes == state.downloaded_paragon_routes
            && self.overlay_window_geometry == state.overlay_window_geometry
            && self.debug_info_enabled == state.shell.debug.debug_info_enabled
    }
}

/// UI-side publisher: turns rule-setting and preset changes into new
/// immutable rules snapshots with increasing revisions.
///
/// The publisher compares only the app state's
/// `rules_input_revision` instead of deep-comparing (and cloning) every
/// rule input on every UI tick. Every mutating action bumps the revision
/// through [`crate::app_state::AppState::bump_rules_input_revision`]. The
/// preset editor's own revision (bumped by every editor method) is part of
/// the observed key too, so an editor edit is never missed even where a
/// caller forgot the app-side bump. The deep compare stays as a
/// `debug_assert` safety net that fires when inputs changed without either
/// revision moving.
#[derive(Debug)]
pub struct RulesPublisher {
    slot: RulesSnapshotSlot,
    measure: Arc<dyn TextMeasure>,
    published: Option<RuleInputs>,
    /// The app and preset-editor revisions the last publication observed;
    /// `None` before the first call, which always publishes.
    observed_revision: Option<(u64, u64)>,
    revision: u64,
}

impl RulesPublisher {
    pub fn new(slot: RulesSnapshotSlot) -> Self {
        Self::with_measure(slot, Arc::new(SystemTextMeasure))
    }

    /// A publisher whose snapshots measure text with `measure`.
    pub fn with_measure(slot: RulesSnapshotSlot, measure: Arc<dyn TextMeasure>) -> Self {
        Self {
            slot,
            measure,
            published: None,
            observed_revision: None,
            revision: 0,
        }
    }

    /// Publish a new snapshot when the rule inputs of `state` changed since
    /// the last publication (the first call always publishes). Returns the
    /// published revision.
    pub fn observe(&mut self, state: &AppState) -> Option<u64> {
        let observed = (
            state.rules_input_revision,
            state.affix_preset_editor.rules_revision,
        );
        if self.observed_revision == Some(observed) {
            // Safety net: an unchanged revision must mean unchanged inputs.
            debug_assert!(
                self.published
                    .as_ref()
                    .is_none_or(|published| published.matches(state)),
                "rule inputs changed without bumping the rules input revision"
            );
            return None;
        }
        self.observed_revision = Some(observed);
        self.published = Some(RuleInputs::of(state));
        self.revision = self.revision.saturating_add(1);
        self.slot.publish(RulesSnapshot::capture_with(
            state,
            self.revision,
            Arc::clone(&self.measure),
        ));
        Some(self.revision)
    }
}

// ---------------------------------------------------------------------------
// Accepted presentation and the UI mailbox
// ---------------------------------------------------------------------------

/// The accepted analysis plus its evaluation: what the overlay draws and the
/// UI projects — one shared value, never two computations.
#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    pub analysis: Arc<AnalysisSnapshot>,
    /// Revision of the rules snapshot the descriptors were evaluated
    /// against; `None` before the first rules snapshot arrived.
    pub rules_revision: Option<u64>,
    pub descriptors: Arc<[RuleOverlayDescriptor]>,
    /// Why each affix marker looks the way it does (one trace per evaluated
    /// row, per build in multi-build mode).
    pub affix_traces: Vec<RuleMatchTrace>,
}

impl Presentation {
    pub fn generation(&self) -> u64 {
        self.analysis.presentation_generation
    }
}

/// Per-frame debug facts of the live loop.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DebugStatus {
    pub frame_sequence: u64,
    /// Duration of one `process_one_frame` pass: capture, detection,
    /// crop-stability/scheduler work and result polling on the loop thread.
    /// It is the loop's own frame cost, not the user-visible latency — the
    /// asynchronous OCR work and the time until the overlay is visible are
    /// deliberately not part of it.
    pub hotpath_ms: u128,
    pub tooltip_found: bool,
    pub descriptor_count: usize,
    /// Accepted presentation generation this state carries.
    pub generation: u64,
    /// Frame the accepted analysis was read from (`AnalysisToken::
    /// source_frame_id`); `None` without an accepted analysis.
    pub source_frame: Option<u64>,
    /// Generation the overlay renderer last drew (`OverlayPresenter::
    /// last_drawn`); `None` before it drew one.
    pub drawn_generation: Option<u64>,
    /// What the presenter did with this state's frame; `None` without an
    /// overlay surface.
    pub draw: Option<PresentOutcome>,
    /// The presented frame is the frozen debug-capture lock image.
    pub frozen: bool,
    /// The asynchronous OCR phase of the same loop, or `None` in a loop
    /// without an OCR worker (the structured stub). These are the worker's
    /// own cumulative counters — the half of the pipeline `hotpath_ms` cannot
    /// include, reported instead of implied.
    pub ocr: Option<OcrPhaseStatus>,
    /// Percentiles of the live loop's per-tick stage-cost rings,
    /// or `None` before any tick was recorded.
    pub loop_timings: Option<d4lc_runtime::diagnostics::LoopTimingPercentiles>,
    /// The Wayland stream's live rates: negotiated rate,
    /// delivered and consumed fps, throttle skips. `None` for engines
    /// without a stream.
    pub capture_stream: Option<String>,
    /// The newest events of the live loop's bounded diagnostics ring, at most
    /// [`RECENT_DIAGNOSTICS`] and ordered newest first.
    /// Empty while the ring holds no event yet — never synthesized.
    pub recent_diagnostics: Vec<d4lc_runtime::DiagnosticsEvent>,
}

/// How many of the live loop's newest diagnostics events the Debug screen's
/// event list shows. The runtime's ring is its own bound;
/// this only caps the window the UI renders, so the list never grows.
pub const RECENT_DIAGNOSTICS: usize = 20;

/// The live phases of the asynchronous OCR worker.
///
/// Taken verbatim from the worker's own counters (`OcrWorkerStats`); the
/// per-item OCR latency and the time until the overlay is visible are not
/// measured per frame and are not invented here — they are named as absent
/// wherever a frame number is shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OcrPhaseStatus {
    /// Jobs whose result was produced.
    pub completed: u64,
    /// Jobs replaced by a newer job before they ran.
    pub dropped: u64,
    /// Jobs that ended in an error.
    pub failed: u64,
    /// A job finished and its result is waiting to be polled.
    pub pending: bool,
    /// A job is running right now.
    pub in_progress: bool,
    /// Cumulative OCR time of the completed jobs.
    pub total_ocr_ms: u64,
    /// Deepest queue the worker ever held.
    pub max_queue_depth: usize,
    pub last_error: Option<String>,
}

impl OcrPhaseStatus {
    /// The worker's own counters, verbatim.
    pub fn from_stats(stats: &d4lc_runtime::OcrWorkerStats) -> Self {
        Self {
            completed: stats.completed,
            dropped: stats.dropped,
            failed: stats.failed,
            pending: stats.pending,
            in_progress: stats.in_progress,
            total_ocr_ms: stats.total_ocr_ms,
            max_queue_depth: stats.max_queue_depth,
            last_error: stats.last_error.clone(),
        }
    }
}

/// The overlay side of one accepted snapshot: the started handle's own status
/// and what its bounded text path could draw.
///
/// Both travel inside [`LiveState`], so the Debug screen's overlay fields, the
/// item card and the frame numbers describe one accepted state instead of
/// independently updated fragments.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverlaySnapshot {
    /// The handle's status, or the loop's synthesized no-surface
    /// status. `None` before any surface reported.
    pub status: Option<OverlayStatus>,
    /// The handle's per-frame text-path diagnostics.
    pub text: OverlayTextState,
}

/// The newest state of one live loop, as the UI projects it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveState {
    /// Publication epoch of the loop that produced it.
    pub epoch: u64,
    pub lifecycle: RuntimeLifecycle,
    pub presentation: Arc<Presentation>,
    pub debug: DebugStatus,
    /// The same cycle's overlay status and text state.
    pub overlay: OverlaySnapshot,
    /// Live stage debug bundle of the shown accepted snapshot. Bounded
    /// latest-wins: it is replaced with every accepted snapshot
    /// and cleared on invalidation/NoTooltip. `None` both while DebugInfo is
    /// off and while an enabled loop has no accepted snapshot yet — the
    /// `debug_info_enabled` flag tells the two apart.
    pub debug_bundle: Option<Arc<TooltipDebugBundle>>,
    /// Whether DebugInfo is on for the loop that published this state.
    pub debug_info_enabled: bool,
    /// When the loop published this state.
    pub published_at: SystemTime,
}

#[derive(Debug, Default)]
struct MailboxInner {
    open_epoch: Option<u64>,
    latest: Option<LiveState>,
    replaced: u64,
    rejected: u64,
}

/// Bounded "latest state" mailbox between the live loop and the UI thread.
///
/// Holds at most one state: a newer publication replaces an unread older
/// one. Publications are fenced by an epoch the controller opens when it
/// starts a loop and closes when it stops it, so a stopped (or replaced)
/// loop can never publish into the UI again — not even while its threads
/// are still finishing.
#[derive(Debug, Clone, Default)]
pub struct LiveStateMailbox {
    inner: Arc<Mutex<MailboxInner>>,
    next_epoch: Arc<AtomicU64>,
}

impl LiveStateMailbox {
    /// Open a new publication epoch (closing any previous one) and return it.
    pub fn open_epoch(&self) -> u64 {
        let epoch = self
            .next_epoch
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let mut inner = lock(&self.inner);
        inner.open_epoch = Some(epoch);
        inner.latest = None;
        epoch
    }

    /// Close `epoch`: every later publication of it is rejected, and an
    /// unread state of it is dropped.
    pub fn close_epoch(&self, epoch: u64) {
        let mut inner = lock(&self.inner);
        if inner.open_epoch == Some(epoch) {
            inner.open_epoch = None;
        }
        if inner
            .latest
            .as_ref()
            .is_some_and(|state| state.epoch == epoch)
        {
            inner.latest = None;
        }
    }

    /// Whether `epoch` may still publish.
    pub fn is_open(&self, epoch: u64) -> bool {
        lock(&self.inner).open_epoch == Some(epoch)
    }

    /// Publish the newest state; `false` when its epoch is closed.
    pub fn publish(&self, state: LiveState) -> bool {
        let mut inner = lock(&self.inner);
        if inner.open_epoch != Some(state.epoch) {
            inner.rejected = inner.rejected.saturating_add(1);
            return false;
        }
        if inner.latest.is_some() {
            inner.replaced = inner.replaced.saturating_add(1);
        }
        inner.latest = Some(state);
        true
    }

    /// Take the newest unread state.
    pub fn take(&self) -> Option<LiveState> {
        lock(&self.inner).latest.take()
    }

    /// Publications rejected because their epoch was closed.
    #[cfg(test)]
    pub fn rejected(&self) -> u64 {
        lock(&self.inner).rejected
    }

    /// Unread states a newer publication replaced.
    #[cfg(test)]
    pub fn replaced(&self) -> u64 {
        lock(&self.inner).replaced
    }
}

// ---------------------------------------------------------------------------
// Control inputs
// ---------------------------------------------------------------------------

/// Coalescing control inbox from the app to the live loop: only the newest
/// value of each input matters, so it is bounded by construction and never
/// queues behind status traffic.
#[derive(Debug, Clone, Default)]
pub struct ControlInbox {
    inner: Arc<Mutex<PendingControls>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingControls {
    pub game_window: Option<GameWindowState>,
    /// The game window's newest on-screen geometry (moved or resized).
    pub window_geometry: Option<Rect>,
    pub overlay_enabled: Option<bool>,
}

impl ControlInbox {
    /// Game focus / minimize state (the game-window observer's input).
    pub fn report_game_window(&self, state: GameWindowState) {
        lock(&self.inner).game_window = Some(state);
    }

    /// Game window geometry (the game-window observer's move/resize input).
    pub fn report_window_geometry(&self, geometry: Rect) {
        lock(&self.inner).window_geometry = Some(geometry);
    }

    /// Whether anything is pending (tests wait on the observer thread).
    #[cfg(test)]
    pub fn drain_peek(&self) -> bool {
        *lock(&self.inner) != PendingControls::default()
    }

    /// Take everything reported since the last drain.
    pub fn drain(&self) -> PendingControls {
        std::mem::take(&mut *lock(&self.inner))
    }
}

/// Shared lifecycle of one live loop: written by the loop and its reaper,
/// read by the controller and the UI.
#[derive(Debug, Clone)]
pub struct LifecycleCell {
    inner: Arc<Mutex<RuntimeLifecycle>>,
}

impl Default for LifecycleCell {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RuntimeLifecycle::Starting)),
        }
    }
}

impl LifecycleCell {
    pub fn get(&self) -> RuntimeLifecycle {
        *lock(&self.inner)
    }

    pub fn set(&self, lifecycle: RuntimeLifecycle) {
        *lock(&self.inner) = lifecycle;
    }

    /// Move to `Stopping` unless the loop already ended.
    pub fn begin_stop(&self) {
        let mut current = lock(&self.inner);
        if current.accepts_work() {
            *current = RuntimeLifecycle::Stopping;
        }
    }

    /// Record the end of the loop thread. A thread that ended without
    /// reaching a terminal state (it panicked) is `Failed`, never `Running`.
    pub fn finish(&self, panicked: bool) {
        let mut current = lock(&self.inner);
        *current = match (*current, panicked) {
            (_, true) | (RuntimeLifecycle::Failed, _) => RuntimeLifecycle::Failed,
            _ => RuntimeLifecycle::Stopped,
        };
    }
}

/// Everything a live loop shares with its controller and the UI.
#[derive(Debug, Clone, Default)]
pub struct LiveLoopLinks {
    pub mailbox: LiveStateMailbox,
    pub rules: RulesSnapshotSlot,
    pub controls: ControlInbox,
}

impl LiveLoopLinks {
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// Overlay presentation with a monotonic generation
// ---------------------------------------------------------------------------

/// The drawing surface the presenter drives (the started overlay handle; a
/// recording surface in tests).
pub trait OverlaySurface {
    fn update(&mut self, frame: OverlayFrame) -> Result<(), String>;
    fn status(&self) -> &OverlayStatus;
    /// Wayland supplies surface-local pointer events; X11 uses the pipeline's cursor.
    fn poll_cursor_window_relative(&mut self) -> Option<d4lc_core::Point> {
        None
    }
    /// What the surface's shared bounded text path could not draw, or why it
    /// is disabled. Empty and clean while every laid-out text was
    /// drawn.
    fn text_diagnostics(&self) -> OverlayTextState;
    /// Follow the game window to a new on-screen geometry.
    fn set_window_geometry(&mut self, geometry: Rect) -> Result<(), String>;
    /// Raster/commit costs of the surface's last completed draw.
    /// Absent unless the backend measured them; the default surface reports
    /// nothing.
    fn draw_timings(&self) -> d4lc_overlay_x11::OverlayDrawTimings {
        d4lc_overlay_x11::OverlayDrawTimings::default()
    }
    /// Whether the surface skipped the last update because nothing
    /// observable changed. The default surface never skips.
    fn last_update_was_skipped(&self) -> bool {
        false
    }
    /// Apply the overlay menu settings to the running surface (debug UI
    /// .7): icon visibility + position and optionally the stored
    /// lock value. The default surface carries no menu.
    fn apply_menu_settings(
        &mut self,
        _show_overlay_icon: bool,
        _overlay_icon_pos_x: u32,
        _overlay_icon_pos_y: u32,
        _menu_locked: Option<bool>,
    ) {
    }
}

impl OverlaySurface for super::StartedOverlay {
    fn update(&mut self, frame: OverlayFrame) -> Result<(), String> {
        super::StartedOverlay::update(self, frame)
    }

    fn status(&self) -> &OverlayStatus {
        super::StartedOverlay::status(self)
    }

    fn poll_cursor_window_relative(&mut self) -> Option<d4lc_core::Point> {
        super::StartedOverlay::poll_cursor_window_relative(self)
    }

    fn text_diagnostics(&self) -> OverlayTextState {
        super::StartedOverlay::text_diagnostics(self)
    }

    fn set_window_geometry(&mut self, geometry: Rect) -> Result<(), String> {
        super::StartedOverlay::set_window_geometry(self, geometry)
    }

    fn draw_timings(&self) -> d4lc_overlay_x11::OverlayDrawTimings {
        super::StartedOverlay::draw_timings(self)
    }

    fn last_update_was_skipped(&self) -> bool {
        super::StartedOverlay::last_update_skipped(self)
    }

    fn apply_menu_settings(
        &mut self,
        show_overlay_icon: bool,
        overlay_icon_pos_x: u32,
        overlay_icon_pos_y: u32,
        menu_locked: Option<bool>,
    ) {
        super::StartedOverlay::apply_menu_settings(
            self,
            show_overlay_icon,
            overlay_icon_pos_x,
            overlay_icon_pos_y,
            menu_locked,
        )
    }
}

/// What the presenter did with one draw request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentOutcome {
    Drawn,
    /// The presentation is older than a confirmed clear; nothing was drawn.
    Refused,
    /// The surface skipped the draw: the render ops and the surface were
    /// unchanged, so the compositor still shows the previous identical
    /// frame. Never counted as `Drawn`.
    Unchanged,
    Failed(String),
}

/// Overlay presenter with a monotonic presentation generation.
///
/// Every draw names the presentation generation it was built from. Once a
/// presentation that carries an invalidation (an explicit empty analysis) is
/// confirmed drawn, it acts as a clear: nothing older may be drawn again. A
/// deliberately frozen debug image may outlive a soft clear (tooltip loss of
/// the live game), but never a hard one (focus lost, minimized, overlay off,
/// window lost, stop).
pub struct OverlayPresenter<S> {
    surface: S,
    soft_fence: u64,
    hard_fence: u64,
    last_drawn: Option<u64>,
}

impl<S: OverlaySurface> OverlayPresenter<S> {
    pub fn new(surface: S) -> Self {
        Self {
            surface,
            soft_fence: 0,
            hard_fence: 0,
            last_drawn: None,
        }
    }

    pub fn status(&self) -> &OverlayStatus {
        self.surface.status()
    }

    pub fn surface(&self) -> &S {
        &self.surface
    }

    #[cfg(feature = "real-runtime")]
    pub(crate) fn poll_cursor_window_relative(&mut self) -> Option<d4lc_core::Point> {
        self.surface.poll_cursor_window_relative()
    }

    /// Move the surface with the game window.
    pub fn set_window_geometry(&mut self, geometry: Rect) -> Result<(), String> {
        self.surface.set_window_geometry(geometry)
    }

    /// Apply the overlay menu settings to the running surface.
    pub fn apply_menu_settings(
        &mut self,
        show_overlay_icon: bool,
        overlay_icon_pos_x: u32,
        overlay_icon_pos_y: u32,
        menu_locked: Option<bool>,
    ) {
        self.surface.apply_menu_settings(
            show_overlay_icon,
            overlay_icon_pos_x,
            overlay_icon_pos_y,
            menu_locked,
        )
    }

    /// Generation of the last presentation actually drawn.
    pub const fn last_drawn(&self) -> Option<u64> {
        self.last_drawn
    }

    /// Draw `frame`, built from `presentation`. `frozen` marks the debug
    /// capture lock's frozen image.
    pub fn present(
        &mut self,
        frame: OverlayFrame,
        presentation: &Presentation,
        frozen: bool,
    ) -> PresentOutcome {
        let generation = presentation.generation();
        let fence = if frozen {
            self.hard_fence
        } else {
            self.soft_fence
        };
        if generation < fence {
            return PresentOutcome::Refused;
        }
        match self.surface.update(frame) {
            Ok(()) => {
                if self.surface.last_update_was_skipped() {
                    // The compositor still shows the previous identical
                    // frame; the drawn generation and the clear fences keep
                    // describing the frame that is actually on screen.
                    return PresentOutcome::Unchanged;
                }
                self.last_drawn = Some(generation);
                if let Some(cause) = presentation.analysis.invalidation {
                    if !presentation.analysis.has_item() {
                        self.soft_fence = self.soft_fence.max(generation);
                        if cause.is_hard() {
                            self.hard_fence = self.hard_fence.max(generation);
                        }
                    }
                }
                PresentOutcome::Drawn
            }
            Err(error) => PresentOutcome::Failed(error),
        }
    }

    /// Hide everything (stop): draws an empty frame and refuses every later
    /// presentation.
    pub fn clear_all(&mut self, sequence: u64) -> Result<(), String> {
        self.soft_fence = u64::MAX;
        self.hard_fence = u64::MAX;
        self.surface.update(OverlayFrame {
            sequence,
            tooltip_found: false,
            hotpath_ms: 0,
            cursor_window_relative: None,
            rule_descriptors: std::sync::Arc::from([]),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        })
    }
}

// ---------------------------------------------------------------------------
// UI projection of the accepted snapshot
// ---------------------------------------------------------------------------

/// Parse-status text of an analysis snapshot for the current-item card.
fn parse_status_text(analysis: &AnalysisSnapshot) -> String {
    if let Some(item) = &analysis.item {
        return item.parse_status.clone();
    }
    match (analysis.tooltip_state, analysis.invalidation) {
        (TooltipState::Invalidated, Some(cause)) => format!("Hidden: {}", cause_text(cause)),
        (TooltipState::Analyzing, _) => "OCR pending".to_string(),
        (TooltipState::Observing, _) | (TooltipState::Ready, _) => "Tooltip detected".to_string(),
        (TooltipState::NoTooltip, _) | (TooltipState::Invalidated, None) => {
            "No tooltip".to_string()
        }
    }
}

fn cause_text(cause: InvalidationCause) -> &'static str {
    match cause {
        InvalidationCause::TooltipLost => "tooltip lost",
        InvalidationCause::FocusLost => "game lost focus",
        InvalidationCause::Minimized => "game minimized",
        InvalidationCause::OverlayDisabled => "overlay off",
        InvalidationCause::WindowLost => "game window lost",
        InvalidationCause::CaptureFailed => "capture failed",
        InvalidationCause::WindowChanged => "game window changed",
        InvalidationCause::ConfigChanged => "settings changed",
        InvalidationCause::Stopped => "stopped",
    }
}

/// The explicit empty analysis of a stopped loop.
pub fn stopped_analysis() -> AnalysisSnapshot {
    AnalysisSnapshot {
        tooltip_state: TooltipState::Invalidated,
        invalidation: Some(InvalidationCause::Stopped),
        ..AnalysisSnapshot::empty(0)
    }
}

/// The current-item projection of an analysis snapshot with the evaluation
/// that produced it: the accepted item, or the explicit empty card. The UI
/// and the rule evaluation both consume exactly this event, so they can never
/// disagree about the item.
///
/// `affix_traces` is the evaluation of the very same snapshot, so a row's
/// `matched_by_rule` flag is the accepted evaluation's verdict on that row and
/// never an independent guess.
pub fn current_item_event(
    analysis: &AnalysisSnapshot,
    affix_traces: &[RuleMatchTrace],
) -> RuntimeEvent {
    let parse_status = parse_status_text(analysis);
    match &analysis.item {
        Some(item) => RuntimeEvent::CurrentItemUpdate {
            item_name: item.item_name.clone(),
            parse_status,
            item_type: item.item_type.clone(),
            base_type: item.base_type.clone(),
            item_power: item.item_power,
            affixes: item
                .affixes
                .iter()
                .map(|affix| CurrentItemAffixView {
                    row_id: affix.row_id,
                    affix_id: affix.affix_id.clone(),
                    label: affix.label.clone(),
                    value_text: affix.value_text.clone(),
                    source_text: affix.source_text.clone(),
                    is_greater: affix.is_greater,
                    is_implicit: affix.is_implicit,
                    is_tempered: affix.is_tempered,
                    matched_by_rule: row_matched_by_rule(affix_traces, affix.row_id),
                })
                .collect(),
            affix_row_geometry: item
                .affix_row_geometry
                .iter()
                .map(CurrentItemAffixRowGeometryView::from_parse_geometry)
                .collect(),
            non_affix_rows: item
                .non_affix_rows
                .iter()
                .filter_map(CurrentItemNonAffixRowView::from_parse_row)
                .collect(),
            tooltip_window_relative: analysis.tooltip_window_relative,
        },
        None => RuntimeEvent::CurrentItemUpdate {
            item_name: None,
            parse_status,
            item_type: None,
            base_type: None,
            item_power: None,
            affixes: Vec::new(),
            affix_row_geometry: Vec::new(),
            non_affix_rows: Vec::new(),
            tooltip_window_relative: None,
        },
    }
}

/// The current-item event of an evaluated presentation: the accepted item and
/// the rule verdicts of one snapshot.
pub fn item_event_for(presentation: &Presentation) -> RuntimeEvent {
    current_item_event(&presentation.analysis, presentation.affix_traces.as_slice())
}

/// Whether one parsed affix row carries a rule match in this evaluation.
///
/// In multi-build mode several traces share a row; the row counts as matched
/// when any of them matched — that build's color is what the overlay draws
/// for it.
fn row_matched_by_rule(traces: &[RuleMatchTrace], row_id: d4lc_core::AffixRowId) -> bool {
    traces
        .iter()
        .any(|trace| trace.row_id == Some(row_id) && trace.is_match())
}

/// The evaluation summary of an accepted presentation: the parsed rows the
/// evaluation saw, how many of them matched, and the markers it produced.
pub fn evaluation_summary(presentation: &Presentation) -> AcceptedEvaluation {
    let affixes = presentation
        .analysis
        .item
        .as_ref()
        .map(|item| item.affixes.as_slice())
        .unwrap_or(&[]);
    let matched_rows = affixes
        .iter()
        .filter(|affix| row_matched_by_rule(&presentation.affix_traces, affix.row_id))
        .count();
    AcceptedEvaluation {
        evaluated_rows: affixes.len(),
        matched_rows,
        markers: presentation.descriptors.len(),
    }
}

/// UI-side bookkeeping of the projected live state: the item card is
/// re-projected only when a new accepted presentation arrived.
#[derive(Debug, Default)]
pub struct LiveStateProjection {
    projected: Option<(u64, u64, Option<u64>)>,
}

impl LiveStateProjection {
    /// Forget what was projected (a loop stopped and reset the card).
    pub fn reset(&mut self) {
        self.projected = None;
    }

    /// Project one accepted snapshot into the UI-owned state.
    ///
    /// Every fragment of the display — the item card and its parse status, the
    /// freeze flag, the accepted generation and its source frame, the renderer
    /// outcome, the overlay status and text state, the evaluation — comes out
    /// of this one [`LiveState`]; nothing here reads anything else. The item
    /// card is re-projected only for a presentation this projection has not
    /// applied yet, so a flood of republications of the same generation costs
    /// one comparison.
    ///
    /// Returns whether the current item was re-projected (the caller re-derives
    /// the item-dependent surfaces from it).
    pub fn apply(&mut self, state: &mut AppState, live: &LiveState) -> bool {
        let key = (
            live.epoch,
            live.debug.generation,
            live.presentation.rules_revision,
        );
        let item_changed = self.projected != Some(key);
        if item_changed {
            self.projected = Some(key);
            super::apply_runtime_event(state, &item_event_for(&live.presentation));
        }
        state.runtime_status.debug_frame_sequence = live.debug.frame_sequence;
        state.runtime_status.debug_hotpath_ms = live.debug.hotpath_ms;
        state.runtime_status.debug_tooltip_found = live.debug.tooltip_found;
        state.runtime_status.loop_timings = live.debug.loop_timings.clone();
        state.runtime_status.capture_stream = live.debug.capture_stream.clone();
        state.runtime_status.ocr_phases = live.debug.ocr.as_ref().map(ocr_phase_view);
        state.runtime_status.overlay_status = live.overlay.status.clone();
        state.runtime_status.overlay_text = live.overlay.text.clone();
        // The live debug data. A new bundle replaces
        // the held one together with its generation and publish instant; a
        // state without one (the accepted snapshot was invalidated, or
        // DebugInfo is off) keeps the last bundle, marked as no longer
        // current — upstream never clears the Debug view's stage images
        // (`DebugViewModel.cs:466-500`). Only an `Arc` clone per new bundle.
        // The newest diagnostics window is bounded by the loop.
        match &live.debug_bundle {
            Some(bundle) => {
                let held = &mut state.runtime_status;
                if !held
                    .live_debug_bundle
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, bundle))
                {
                    held.live_debug_bundle = Some(Arc::clone(bundle));
                    held.live_debug_generation = live.debug.generation;
                    held.live_debug_published_at = Some(live.published_at);
                }
                held.live_debug_bundle_current = true;
            }
            None => state.runtime_status.live_debug_bundle_current = false,
        }
        state.runtime_status.live_debug_info_enabled = live.debug_info_enabled;
        state.runtime_status.recent_diagnostics = live.debug.recent_diagnostics.clone();
        state.runtime_status.accepted = Some(AcceptedSnapshotView {
            generation: live.debug.generation,
            source_frame: live.debug.source_frame,
            published_at: live.published_at,
            frozen: live.debug.frozen,
            drawn_generation: live.debug.drawn_generation,
            draw: live.debug.draw.as_ref().map(overlay_draw_report),
            draw_error: match &live.debug.draw {
                Some(PresentOutcome::Failed(error)) => Some(error.clone()),
                _ => None,
            },
            evaluation: evaluation_summary(&live.presentation),
        });
        item_changed
    }
}

/// The app-side view of the live OCR phases.
fn ocr_phase_view(phases: &OcrPhaseStatus) -> OcrPhaseView {
    OcrPhaseView {
        completed: phases.completed,
        dropped: phases.dropped,
        failed: phases.failed,
        pending: phases.pending,
        in_progress: phases.in_progress,
        total_ocr_ms: phases.total_ocr_ms,
        max_queue_depth: phases.max_queue_depth,
        last_error: phases.last_error.clone(),
    }
}

/// The app-side vocabulary of one presenter outcome.
fn overlay_draw_report(outcome: &PresentOutcome) -> OverlayDrawReport {
    match outcome {
        PresentOutcome::Drawn => OverlayDrawReport::Drawn,
        PresentOutcome::Refused => OverlayDrawReport::Refused,
        PresentOutcome::Unchanged => OverlayDrawReport::Unchanged,
        PresentOutcome::Failed(_) => OverlayDrawReport::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d4lc_rules::{
        ParsedAffixKind, RuleMatchTrace, RuleOverlayShape, RuleTraceEntry, RuleValueCheck,
    };

    /// A board built while the font cannot measure has no text panels and is
    /// not kept: the next evaluation builds it again.
    #[test]
    fn a_board_built_without_a_font_is_not_cached() {
        #[derive(Debug)]
        struct NoFace;
        impl TextMeasure for NoFace {
            fn measure(&self, _: &str, _: f32) -> Option<d4lc_core::text_measure::TextExtent> {
                None
            }
        }
        let snapshot = RulesSnapshot::capture_with(
            &crate::runtime_bridge::test_support::rules_state_with_descriptors(),
            1,
            Arc::new(NoFace),
        );
        let first = snapshot.evaluate(&AnalysisSnapshot::empty(0), None, None);
        let second = snapshot.evaluate(&AnalysisSnapshot::empty(0), None, None);
        assert_eq!(snapshot.paragon_cache.builds(), 2);
        assert!(!Arc::ptr_eq(&first.descriptors, &second.descriptors));
        assert!(
            first
                .descriptors
                .iter()
                .all(|descriptor| descriptor.text.is_none()),
            "no text panel without a font"
        );
    }

    /// The frame's text size follows the snapshot's setting, bounded.
    #[test]
    fn the_snapshot_bounds_the_overlay_font_size() {
        let mut state = AppState::default_startup();
        state.shell.settings.overlay_font_size = 4000;
        assert_eq!(RulesSnapshot::capture(&state, 1).overlay_font_px(), 50);
        state.shell.settings.overlay_font_size = 1;
        assert_eq!(RulesSnapshot::capture(&state, 1).overlay_font_px(), 10);
        state.shell.settings.overlay_font_size = 22;
        assert_eq!(RulesSnapshot::capture(&state, 1).overlay_font_px(), 22);
    }

    /// The paragon board is built once per distinct input: a new analysis,
    /// a repeated evaluation and every reuse of the shared list leave it be;
    /// a changed window rebuilds it.
    #[test]
    fn the_paragon_board_is_rebuilt_only_when_its_inputs_change() {
        let snapshot = RulesSnapshot::capture_with(
            &crate::runtime_bridge::test_support::rules_state_with_descriptors(),
            1,
            Arc::new(d4lc_core::text_measure::FixedTextMeasure::HALF_EM),
        );
        let first = snapshot.evaluate(&AnalysisSnapshot::empty(0), None, None);
        assert!(!first.descriptors.is_empty());
        assert_eq!(snapshot.paragon_cache.builds(), 1);

        let same = snapshot.evaluate(&AnalysisSnapshot::empty(0), None, None);
        let other_analysis = snapshot.evaluate(&AnalysisSnapshot::empty(7), None, None);
        assert_eq!(snapshot.paragon_cache.builds(), 1);
        assert!(Arc::ptr_eq(&first.descriptors, &same.descriptors));
        assert!(Arc::ptr_eq(&first.descriptors, &other_analysis.descriptors));

        let moved = snapshot.evaluate(
            &AnalysisSnapshot::empty(0),
            Some(Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            None,
        );
        assert_eq!(snapshot.paragon_cache.builds(), 2);
        assert!(!Arc::ptr_eq(&first.descriptors, &moved.descriptors));
    }

    /// One evaluation trace of `row`: a match when `matched`, the upstream
    /// red default rejection otherwise.
    fn trace(row: u32, build: Option<usize>, matched: bool) -> RuleMatchTrace {
        RuleMatchTrace {
            preset_name: "preset".to_string(),
            build_column: build,
            row_id: Some(d4lc_core::AffixRowId(row)),
            affix_id: format!("Affix_{row}"),
            item_type: "helm".to_string(),
            row_kind: ParsedAffixKind::Normal,
            pass: None,
            entry: matched.then(|| RuleTraceEntry {
                requirement_index: 0,
                affix_id: format!("Affix_{row}"),
                item_type: "helm".to_string(),
                color: "#FF0000".to_string(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            }),
            value_check: RuleValueCheck::NotEvaluated,
            color: "#FF0000".to_string(),
            shape: RuleOverlayShape::Rectangle,
            rejection: None,
        }
    }

    /// The card's per-row verdict is the accepted evaluation's own: a row is
    /// matched when any build's trace for it matched, and a row no trace
    /// covers is never claimed as matched.
    #[test]
    fn a_row_is_matched_when_any_build_trace_matched_it() {
        let traces = [
            trace(0, Some(0), false),
            trace(0, Some(1), true),
            trace(1, Some(0), false),
        ];

        assert!(row_matched_by_rule(&traces, d4lc_core::AffixRowId(0)));
        assert!(!row_matched_by_rule(&traces, d4lc_core::AffixRowId(1)));
        assert!(
            !row_matched_by_rule(&traces, d4lc_core::AffixRowId(2)),
            "a row outside the evaluation carries no match"
        );
        assert!(!row_matched_by_rule(&[], d4lc_core::AffixRowId(0)));
    }

    /// The publisher compares only the revision — the first
    /// observe publishes, an unchanged revision never re-publishes, and a
    /// bump through the one helper publishes exactly once.
    #[test]
    fn rules_publisher_observes_the_revision_not_the_inputs() {
        let slot = RulesSnapshotSlot::default();
        let mut publisher = RulesPublisher::new(slot.clone());
        let mut state = crate::app_state::AppState::default_startup();

        assert_eq!(
            publisher.observe(&state),
            Some(1),
            "the first call publishes"
        );
        assert_eq!(
            publisher.observe(&state),
            None,
            "an unchanged revision must not re-publish"
        );

        // A mutation through the one helper publishes exactly once, and the
        // snapshot the UI reads carries that revision.
        state.bump_rules_input_revision();
        assert_eq!(publisher.observe(&state), Some(2));
        assert_eq!(publisher.observe(&state), None);
        assert_eq!(slot.current().map(|snapshot| snapshot.revision()), Some(2));

        // A bumped revision with identical inputs still publishes (the
        // bump is a may-have-changed signal, not a change proof).
        state.bump_rules_input_revision();
        assert_eq!(publisher.observe(&state), Some(3));
    }

    /// The DebugInfo bit travels the rules snapshot:
    /// flipping it and bumping the input revision republishes it, so the live
    /// loop's OCR-worker gate follows the toggle without a runtime restart.
    #[test]
    fn rules_snapshot_carries_the_debug_info_bit() {
        let slot = RulesSnapshotSlot::default();
        let mut publisher = RulesPublisher::new(slot.clone());
        let mut state = crate::app_state::AppState::default_startup();
        assert!(
            !state.shell.debug.debug_info_enabled,
            "DebugInfo defaults to off"
        );
        assert_eq!(publisher.observe(&state), Some(1));
        assert_eq!(
            slot.current().map(|snapshot| snapshot.debug_info_enabled()),
            Some(false)
        );

        state.shell.debug.debug_info_enabled = true;
        state.bump_rules_input_revision();
        assert_eq!(publisher.observe(&state), Some(2));
        assert_eq!(
            slot.current().map(|snapshot| snapshot.debug_info_enabled()),
            Some(true)
        );
    }

    /// A preset-editor edit publishes through the editor's own revision even
    /// when the caller did not bump the app-side revision, and the snapshot
    /// carries the edited entry.
    #[test]
    fn rules_publisher_publishes_an_editor_edit_without_an_app_bump() {
        let slot = RulesSnapshotSlot::default();
        let mut publisher = RulesPublisher::new(slot.clone());
        let mut state = crate::app_state::AppState::default_startup();
        assert_eq!(publisher.observe(&state), Some(1));

        let revision = state.rules_input_revision;
        assert!(state.affix_preset_editor.create_preset("Edited").is_ok());
        assert_eq!(state.rules_input_revision, revision, "no app-side bump");
        assert_eq!(publisher.observe(&state), Some(2));
        assert_eq!(publisher.observe(&state), None);
    }

    /// The mutable accessors of rule inputs bump the revision themselves.
    #[test]
    fn mutable_rule_input_accessors_bump_the_revision() {
        let slot = RulesSnapshotSlot::default();
        let mut publisher = RulesPublisher::new(slot);
        let mut state = crate::app_state::AppState::default_startup();
        assert_eq!(publisher.observe(&state), Some(1));

        state.trade_imports_mut().selected_trade_item_id = Some("Trade_Amulet".to_string());
        assert_eq!(publisher.observe(&state), Some(2));

        state.settings_mut().tempered_detection_enabled =
            !state.shell.settings.tempered_detection_enabled;
        assert_eq!(publisher.observe(&state), Some(3));
        assert_eq!(publisher.observe(&state), None);
    }

    /// The projection carries the accepted snapshot's live debug bundle, the
    /// DebugInfo bit, its generation and the bounded diagnostics window into
    /// the app state — the Debug panels project from
    /// exactly these fields.
    #[test]
    fn the_projection_carries_the_live_bundle_and_diagnostics() {
        let stage = || d4lc_runtime::DebugImage {
            width: 2,
            height: 2,
            rgba: Arc::from(vec![0_u8; 16]),
        };
        let bundle = std::sync::Arc::new(TooltipDebugBundle {
            tooltip_crop: stage(),
            item_type_area: None,
            affix_locations: stage(),
            affix_areas: stage(),
            aspect_area: stage(),
            socket_locations: stage(),
            splitter_locations: stage(),
            segments: Vec::new(),
            lines: Vec::new(),
        });
        let live = LiveState {
            epoch: 1,
            lifecycle: RuntimeLifecycle::Running,
            presentation: Arc::new(Presentation {
                analysis: Arc::new(AnalysisSnapshot::empty(1)),
                rules_revision: None,
                descriptors: Arc::from([]),
                affix_traces: Vec::new(),
            }),
            debug: DebugStatus {
                generation: 5,
                recent_diagnostics: vec![d4lc_runtime::DiagnosticsEvent {
                    session_id: 1,
                    frame_id: 9,
                    tooltip_generation: 5,
                    config_revision: d4lc_runtime::Revision::Applied(1),
                    rules_revision: d4lc_runtime::Revision::Unknown,
                    phase: d4lc_runtime::DiagnosticsPhase::OcrSubmit,
                    at_micros: 1,
                    wall_unix_ms: 1_700_000_000_000,
                    duration_micros: None,
                    first_seen_at_micros: None,
                    outcome: d4lc_runtime::DiagnosticsOutcome::Ok,
                    detail: None,
                }],
                ..DebugStatus::default()
            },
            overlay: OverlaySnapshot::default(),
            debug_bundle: Some(Arc::clone(&bundle)),
            debug_info_enabled: true,
            published_at: std::time::SystemTime::UNIX_EPOCH,
        };

        let mut state = crate::app_state::AppState::default_startup();
        let mut projection = LiveStateProjection::default();
        projection.apply(&mut state, &live);

        let status = &state.runtime_status;
        assert_eq!(status.live_debug_generation, 5);
        assert!(status.live_debug_info_enabled);
        assert!(
            status
                .live_debug_bundle
                .as_ref()
                .is_some_and(|shown| Arc::ptr_eq(shown, &bundle)),
            "the exact accepted bundle reaches the app state"
        );
        assert_eq!(status.recent_diagnostics.len(), 1);
        assert_eq!(status.recent_diagnostics[0].frame_id, 9);
        assert!(status.live_debug_bundle_current);

        // The accepted snapshot is invalidated (tooltip or focus lost): the
        // next state carries no bundle and a newer generation. The bundle and
        // its own generation stay, marked as held over.
        let mut invalidated = live.clone();
        invalidated.debug_bundle = None;
        invalidated.debug.generation = 6;
        projection.apply(&mut state, &invalidated);
        let status = &state.runtime_status;
        assert!(
            status
                .live_debug_bundle
                .as_ref()
                .is_some_and(|shown| Arc::ptr_eq(shown, &bundle)),
            "an invalidation keeps the last bundle"
        );
        assert_eq!(status.live_debug_generation, 5);
        assert!(!status.live_debug_bundle_current);

        // A newer bundle replaces it together with its generation.
        let newer = Arc::new((*bundle).clone());
        let mut next = live.clone();
        next.debug_bundle = Some(Arc::clone(&newer));
        next.debug.generation = 8;
        projection.apply(&mut state, &next);
        let status = &state.runtime_status;
        assert!(status
            .live_debug_bundle
            .as_ref()
            .is_some_and(|shown| Arc::ptr_eq(shown, &newer)));
        assert_eq!(status.live_debug_generation, 8);
        assert!(status.live_debug_bundle_current);
    }
}
