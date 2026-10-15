use d4lc_overlay_x11::OverlayStatus;

// ---------------------------------------------------------------------------
// The accepted snapshot — one projection of the runtime's live state.
// ---------------------------------------------------------------------------

/// What the overlay renderer did with the frame of an accepted snapshot.
///
/// The live loop reports this together with the accepted generation, so the UI
/// can state whether the renderer drew the very snapshot the UI shows instead
/// of assuming that it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayDrawReport {
    /// The frame was submitted and the backend returned.
    Drawn,
    /// The presentation was older than a confirmed clear: nothing was drawn.
    Refused,
    /// The surface skipped the draw: nothing observable changed, so the
    /// compositor still shows the previous identical frame.
    Unchanged,
    /// The backend refused the frame; `draw_error` names the reason.
    Failed,
}

impl OverlayDrawReport {
    /// The Debug screen's one-word rendering.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Drawn => "drawn",
            Self::Refused => "refused (older than a confirmed clear)",
            Self::Unchanged => "unchanged (draw skipped)",
            Self::Failed => "failed",
        }
    }
}

/// The overlay's own text-path state: the resolved font assignment or
/// the exact reason text is disabled, plus the characters the surface flagged.
///
/// A copy of the started handle's `TextDiagnostics` in the app's own
/// vocabulary, so no UI type depends on one backend's type. The flagged
/// codepoints and the clipped-character count accumulate over the surface's
/// frames; the font assignment or the disabled reason is the surface's static
/// text state and travels in its `OverlayStatus.limitation`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayTextState {
    /// Set when the assigned font could not be resolved: the exact reason.
    /// Text is then not drawn at all — no fallback face is substituted.
    pub font_unavailable: Option<String>,
    /// Distinct codepoints the assigned font has no glyph for.
    pub unsupported_codepoints: Vec<char>,
    /// Source characters that did not fit their text area, summed over the
    /// surface's layouts.
    pub clipped_chars: u64,
}

impl OverlayTextState {
    /// What the surface flagged, or `None` while nothing was flagged.
    pub fn flagged_text(&self) -> Option<String> {
        let mut flagged = Vec::new();
        if let Some(reason) = &self.font_unavailable {
            flagged.push(format!("text disabled: {reason}"));
        }
        if !self.unsupported_codepoints.is_empty() {
            let codepoints = self
                .unsupported_codepoints
                .iter()
                .map(|codepoint| format!("U+{:04X}", u32::from(*codepoint)))
                .collect::<Vec<_>>()
                .join(" ");
            flagged.push(format!(
                "{} unsupported codepoint(s): {codepoints}",
                self.unsupported_codepoints.len()
            ));
        }
        if self.clipped_chars > 0 {
            flagged.push(format!("{} clipped character(s)", self.clipped_chars));
        }
        (!flagged.is_empty()).then(|| flagged.join("; "))
    }
}

/// The live phases of the asynchronous OCR work.
///
/// A copy of the worker's own `OcrWorkerStats` in the app's vocabulary, so no
/// UI type depends on the runtime crate — the same boundary the accepted
/// snapshot's view keeps.
///
/// The frame pass (`RuntimeStatus::debug_hotpath_ms`) cannot include the OCR
/// work: it runs on the worker's own thread and a result is adopted frames
/// later. These are the worker's cumulative counters, so the async half of the
/// pipeline is visible as itself instead of being read into the frame number.
/// Per-item OCR latency and time-to-visible are not measured per frame and are
/// named as absent wherever the frame numbers are shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OcrPhaseView {
    /// Jobs whose result was produced.
    pub completed: u64,
    /// Jobs replaced by a newer job before they ran (the worker's own
    /// supersede rule).
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

impl OcrPhaseView {
    /// Average duration of one completed OCR pass, or `None` before the first
    /// one finished. Derived from the worker's own two counters, never
    /// synthesized.
    pub fn average_pass_ms(&self) -> Option<u64> {
        (self.completed > 0).then(|| self.total_ocr_ms / self.completed)
    }
}

/// The rule evaluation of the accepted snapshot: what the evaluation did with
/// the item rows of the very frame the UI shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AcceptedEvaluation {
    /// Affix rows the accepted item carried into the evaluation.
    pub evaluated_rows: usize,
    /// Rows whose rule trace carries an entry (a match).
    pub matched_rows: usize,
    /// Overlay descriptors the accepted snapshot produced.
    pub markers: usize,
}

/// The UI facts of ONE accepted snapshot.
///
/// Every field comes from the single `LiveState` the runtime publishes
/// (`runtime_bridge::live_state`): the generation, the frame the accepted
/// analysis was read from, when the loop published it, whether the presented
/// frame is the frozen debug-capture image, and what the overlay renderer did
/// with that same generation. The UI projects them together — never as
/// independently updated fragments.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedSnapshotView {
    /// Generation of the accepted presentation the UI shows.
    pub generation: u64,
    /// Frame the accepted analysis was read from; `None` without an accepted
    /// analysis (no analysis token).
    pub source_frame: Option<u64>,
    /// When the loop published this state.
    pub published_at: std::time::SystemTime,
    /// The presented frame is the frozen debug-capture lock image.
    pub frozen: bool,
    /// Generation the overlay renderer last drew; `None` before it drew one.
    pub drawn_generation: Option<u64>,
    /// What the presenter did with this snapshot's frame; `None` without an
    /// overlay surface.
    pub draw: Option<OverlayDrawReport>,
    /// Backend error of a failed draw.
    pub draw_error: Option<String>,
    pub evaluation: AcceptedEvaluation,
}

impl AcceptedSnapshotView {
    /// The Debug screen's generation line: the accepted generation and the
    /// frame the accepted analysis was read from.
    pub fn generation_text(&self) -> String {
        match self.source_frame {
            Some(frame) => format!("{} (source frame {frame})", self.generation),
            None => format!("{} (no accepted frame)", self.generation),
        }
    }

    /// Whether the renderer drew the very generation the UI shows.
    pub fn renderer_drew_accepted(&self) -> bool {
        self.drawn_generation == Some(self.generation)
    }

    /// The Debug screen's renderer line: the drawn generation and the
    /// presenter's outcome for the accepted snapshot.
    pub fn renderer_text(&self) -> String {
        match (self.draw, self.drawn_generation) {
            (Some(report), Some(generation)) => {
                let matching = if self.renderer_drew_accepted() {
                    " = accepted generation".to_string()
                } else {
                    format!(" (accepted generation {})", self.generation)
                };
                match &self.draw_error {
                    Some(error) => format!("{} {generation}: {error}", report.as_str()),
                    None => format!("{} {generation}{matching}", report.as_str()),
                }
            }
            (Some(report), None) => match &self.draw_error {
                Some(error) => format!("{}: {error}", report.as_str()),
                None => report.as_str().to_string(),
            },
            (None, _) => "no overlay surface".to_string(),
        }
    }

    /// The Debug screen's frame-provenance line: live, or explicitly frozen.
    pub fn frame_state_text(&self) -> String {
        if self.frozen {
            return format!(
                "Frozen (debug capture lock): {} stays displayed while the game keeps running; \
                 release the lock to follow the live analysis",
                self.source_frame.map_or_else(
                    || "the last accepted analysis".to_string(),
                    |frame| format!("frame {frame}")
                )
            );
        }
        "Live (following the running loop)".to_string()
    }

    /// The Debug screen's evaluation line.
    pub fn evaluation_text(&self) -> String {
        format!(
            "{} of {} affix row(s) matched, {} overlay marker(s)",
            self.evaluation.matched_rows, self.evaluation.evaluated_rows, self.evaluation.markers
        )
    }
}

// ---------------------------------------------------------------------------
// RuntimeStatus — live-loop runtime state visible to the UI.
// ---------------------------------------------------------------------------

/// Live-loop runtime status tracked in `AppState` and projected for Slint.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeStatus {
    /// Whether the live loop is currently started.
    pub is_running: bool,
    /// Whether the started loop is still preparing its runtime (portal
    /// dialog, first stream frame): it is stoppable but processes no frame.
    pub is_preparing: bool,
    /// Human-readable status message (e.g. "Stopped", "Running", error text).
    pub status_message: String,
    /// Name of the most recently parsed item (if any).
    pub current_item_name: Option<String>,
    /// Parse status of the most recently processed item.
    pub current_item_parse_status: String,
    /// Monotonic frame sequence counter.
    pub debug_frame_sequence: u64,
    /// Last-cycle hotpath duration in milliseconds.
    pub debug_hotpath_ms: u128,
    /// Percentiles of the live loop's per-tick stage-cost rings,
    /// or `None` before the loop reported any tick.
    pub loop_timings: Option<d4lc_runtime::diagnostics::LoopTimingPercentiles>,
    /// The live Wayland stream-rate line, appended to the
    /// Capture engine row; `None` for engines without a stream.
    pub capture_stream: Option<String>,
    /// Whether the last capture cycle found a tooltip.
    pub debug_tooltip_found: bool,
    /// Whether `ToggleDebugLockScreencapture` is currently engaged.
    ///
    /// Toggled by the `ToggleDebugLockScreencapture` hotkey. Session state,
    /// like upstream's `ScreenCaptureHandler.IsScreencaptureLocked` (never
    /// persisted). The live loop reads it every cycle
    /// (`runtime_bridge::DebugCaptureLock`) and, while it is set, stops
    /// following the game: the overlay and the current-item card keep
    /// showing the latest screencapture (upstream
    /// `ScreenCaptureHandler.cs:246-264`).
    pub debug_lock_screencapture: bool,
    /// Most-recent Debug screen action the user attempted, with
    /// status. Populated by the debug action handlers and
    /// the `ToggleDebugLockScreencapture` hotkey so the UI and
    /// log surface can observe what the user did without a
    /// live runtime bridge.
    pub last_debug_action: Option<DebugActionRecord>,
    /// The live loop's last published overlay status, or `None` before the
    /// loop has reported. It travels inside the accepted snapshot
    /// (`LiveState::overlay`), so the Debug screen's "Overlay:" field always
    /// describes the same accepted state as the item card and the frame
    /// numbers beside it; once a report exists the field must never
    /// fall back to a static claim. The status vocabulary is the
    /// handle's own `d4lc_overlay_x11::OverlayStatus` — no second status
    /// channel.
    pub overlay_status: Option<OverlayStatus>,
    /// The overlay's own text-path state of the same published snapshot
    /// the flagged codepoints and clipped characters, paired with the
    /// font assignment or disabled reason of `overlay_status.limitation`.
    pub overlay_text: OverlayTextState,
    /// The accepted snapshot's projections (generation, source frame,
    /// publication time, freeze, renderer outcome, evaluation), or `None`
    /// before the live loop published its first snapshot.
    pub accepted: Option<AcceptedSnapshotView>,
    /// The async OCR phases of the same published snapshot, or `None`
    /// in a loop without an OCR worker.
    pub ocr_phases: Option<OcrPhaseView>,
    /// The game-window observer's last capability report
    /// (`RuntimeEvent::GameWindowObservation`), or `None` before the live
    /// loop reported one.
    pub game_window_status: Option<String>,
    /// Desired versus applied settings of the running runtime.
    pub settings: crate::settings_consumers::SettingsApplication,
    /// The debug bundle of the last accepted snapshot that carried one:
    /// latest-wins, replaced by every newer bundle and
    /// cleared only when the loop stops. A published state without a bundle
    /// (tooltip lost, focus lost, DebugInfo off) does not clear it — upstream
    /// pushes each stage image when it is produced and never clears the
    /// Debug view's image properties (`DebugViewModel.cs:466-500`), so the
    /// last image stays visible. [`Self::live_debug_bundle_current`] says
    /// whether the newest published state still shows it.
    pub live_debug_bundle: Option<std::sync::Arc<d4lc_runtime::TooltipDebugBundle>>,
    /// Whether the newest published live state carries
    /// [`Self::live_debug_bundle`] (live) or the bundle is held over from an
    /// earlier accepted snapshot (labelled as such).
    pub live_debug_bundle_current: bool,
    /// Whether DebugInfo is on for the loop that published the last accepted
    /// snapshot.
    pub live_debug_info_enabled: bool,
    /// Generation of the accepted snapshot [`Self::live_debug_bundle`] came
    /// from (the panels label it "live, generation N").
    pub live_debug_generation: u64,
    /// When that snapshot was published.
    pub live_debug_published_at: Option<std::time::SystemTime>,
    /// The newest events of the live loop's bounded diagnostics ring, bounded
    /// to `live_state::RECENT_DIAGNOSTICS` (phase, outcome, reason,
    /// generation). The ring is the runtime's own; this is only the
    /// bounded newest-first window the Debug screen's event list renders.
    pub recent_diagnostics: Vec<d4lc_runtime::DiagnosticsEvent>,
    /// Provenance of the stored debug run the panels fall back to when no
    /// live bundle exists: the newest export bundle's name and capture
    /// instant, or `None` without one.
    pub stored_debug_run: Option<StoredDebugRun>,
}

/// The stored debug run the debug panels fall back to.
///
/// The nine stage panels and the timing table read the newest export bundle
/// under `debug-output/`; the panels name it (name + capture instant) so a
/// "stored run, HH:MM:SS" line is the run that really produced the image,
/// never a bare "timings" next to live numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDebugRun {
    /// Directory name of the newest `export-<unix seconds>` bundle.
    pub bundle_name: String,
    /// Capture instant read from the bundle's own name suffix, `None` when
    /// the name carries no readable suffix (the age is then not claimed).
    pub captured_at: Option<std::time::SystemTime>,
}

/// Stable name for a debug-screen action. Mirrors the
/// `DebugViewModel` command set plus the `ToggleDebugLockScreencapture`
/// hotkey (which routes through the same `DebugViewModel`-derived
/// state path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugAction {
    TakeScreenshot,
    ExportDebugImages,
    ReloadPresetImages,
    ResetPerformanceResults,
    ToggleDebugInfo,
    ToggleTopMost,
    ToggleDebugLockScreencapture,
    ToggleTuningPrismHints,
    ExportCrashLog,
    /// Open the debug-output artifact folder with the desktop file
    /// handler — the port's equivalent of upstream's
    /// `Process.Start("explorer.exe", …)` after an export
    /// (`DebugViewModel.cs:693-696`).
    OpenArtifactsFolder,
}

impl DebugAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TakeScreenshot => "take_screenshot",
            Self::ExportDebugImages => "export_debug_images",
            Self::ReloadPresetImages => "reload_preset_images",
            Self::ResetPerformanceResults => "reset_performance_results",
            Self::ToggleDebugInfo => "toggle_debug_info",
            Self::ToggleTopMost => "toggle_topmost",
            Self::ToggleDebugLockScreencapture => "toggle_debug_lock_screencapture",
            Self::ToggleTuningPrismHints => "toggle_tuning_prism_hints",
            Self::ExportCrashLog => "export_crash_log",
            Self::OpenArtifactsFolder => "open_artifacts_folder",
        }
    }
}

/// Outcome of a debug-screen action invocation. When the capture,
/// preset or overlay backend is not reachable the status is `Blocked`
/// and `reason` names the missing piece. `Applied` means the action mutated real app-shell
/// state and (where applicable) the persisted path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugActionRecord {
    pub action: DebugAction,
    pub status: DebugActionStatus,
    pub reason: String,
    pub monotonic: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugActionStatus {
    /// Action produced real, observable app-shell state change.
    Applied,
    /// Action was attempted but cannot complete inside the
    /// allowed app-shell surface (e.g. screenshot needs a live
    /// capture pipeline that lives in the runtime crate).
    Blocked,
    /// The action entered the handler but the live capture /
    /// file-write step returned an error (e.g. no X11 display,
    /// no matching Diablo window, PNG encoding failure, disk
    /// write failure). Distinct from `Blocked` so the Logging
    /// screen can tell the user "we tried, here's why it did not
    /// land" versus "we never even compiled the attempt".
    Failed,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            is_running: false,
            is_preparing: false,
            status_message: "Stopped".to_string(),
            current_item_name: None,
            current_item_parse_status: "not_item".to_string(),
            debug_frame_sequence: 0,
            debug_hotpath_ms: 0,
            loop_timings: None,
            capture_stream: None,
            debug_tooltip_found: false,
            debug_lock_screencapture: false,
            last_debug_action: None,
            overlay_status: None,
            overlay_text: OverlayTextState::default(),
            accepted: None,
            ocr_phases: None,
            game_window_status: None,
            settings: crate::settings_consumers::SettingsApplication::default(),
            live_debug_bundle: None,
            live_debug_bundle_current: false,
            live_debug_info_enabled: false,
            live_debug_generation: 0,
            live_debug_published_at: None,
            recent_diagnostics: Vec::new(),
            stored_debug_run: None,
        }
    }
}
