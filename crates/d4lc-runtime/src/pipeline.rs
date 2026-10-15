use std::time::Instant;

use d4lc_capture::{
    capture_reference_with_backend_and_request, read_cursor_state, CaptureRequest, CaptureSource,
};
use d4lc_core::InputMode;
use d4lc_detect::prepared::{detect_roi_frame_prepared, RoiFrame};
use d4lc_rules::OverlayFrame;

use crate::coordinator::{
    AnalysisSnapshot, InvalidationCause, ObserveDecision, ResultDisposition, TooltipObservation,
};
use crate::diagnostics::{DiagnosticsOutcome, DiagnosticsPhase, DiscardReason};
use crate::prepared::{CursorSource, FrameSource, PreparedRuntime};
use crate::session::{tooltip_ocr_signature, RuntimeSession, TooltipOcrSignatureInput};
use crate::{DiagnosticsContext, TooltipCaptureInfo, TooltipImage, TooltipTextSnapshot};
use d4lc_core::coords::{DesktopRect, FramePlacement, FrameRect, FrameScale, TooltipPlacement};
use d4lc_core::{AffixRowId, Point, Rect};
use d4lc_parse::{ItemTooltipAffixRowGeometry, ItemTooltipNonAffixRow, ItemTooltipParseStatus};
use std::sync::Arc;

/// Fatal error in the frame pipeline.
///
/// Only capture failures (no display, window gone) and unrecoverable
/// pipeline failures are fatal. Detection/OCR/parse failures are
/// non-fatal and expressed via `FrameStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimePipelineError {
    /// Capture backend unavailable or window/display went away.
    CaptureFailed(String),
    /// Unrecoverable pipeline failure (e.g., template match backend error).
    PipelineFailed(String),
}

impl std::fmt::Display for RuntimePipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CaptureFailed(msg) => write!(f, "capture failed: {msg}"),
            Self::PipelineFailed(msg) => write!(f, "pipeline failed: {msg}"),
        }
    }
}

/// Per-frame pipeline outcome — data, not control flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameStatus {
    /// No tooltip was detected in this frame.
    NoTooltip,
    /// A tooltip is visible but OCR has not yet produced a result.
    TooltipDetected,
    /// OCR work has been submitted but not yet completed.
    OcrPending,
    /// OCR job failed for this frame.
    OcrFailed { message: String },
    /// OCR text did not parse into a valid item.
    ParseFailed { message: String },
    /// Full pipeline succeeded — tooltip detected, OCR parsed, item available.
    Parsed,
    /// The frame source delivered no new frame since the previous one
    /// capture, detection and hashing were skipped and the
    /// previous frame's observation was re-fed to the coordinator — what
    /// re-capturing identical pixels would produce. Never a fresh capture.
    RepeatedFrame,
    /// The overlay is switched off: like upstream's capture handler while
    /// `IsEnabled` is false (`ScreenCaptureHandler.cs:246`), nothing was
    /// captured, detected or analyzed. Only the cursor was polled, so the
    /// overlay menu stays usable.
    OverlayDisabled,
}

impl FrameStatus {
    /// Human-readable parse status string for RuntimeEvent::CurrentItemUpdate.
    pub fn parse_status_string(&self) -> String {
        match self {
            Self::NoTooltip => "No tooltip".to_string(),
            Self::TooltipDetected => "Tooltip detected".to_string(),
            Self::OcrPending => "OCR pending".to_string(),
            Self::OcrFailed { message } => format!("OCR failed: {message}"),
            Self::ParseFailed { message } => format!("Parse failed: {message}"),
            Self::Parsed => "Parsed".to_string(),
            Self::RepeatedFrame => "Repeated frame (no new capture)".to_string(),
            Self::OverlayDisabled => "Overlay disabled (no capture)".to_string(),
        }
    }
}

/// Lightweight runtime-facing projection of one parsed affix row.
///
/// Carries only parse-derived facts (upstream `Tooltip.OcrResultAffixes`,
/// `TradeViewModel.HandleTooltipDataReadyMessage`); rule-match state is
/// not known on the runtime side and is applied by the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemAffixSnapshot {
    /// Tooltip-wide row identity ([`d4lc_parse::ItemTooltipAffix::row_id`]).
    pub row_id: AffixRowId,
    /// Matched affix id, when the fuzzy parse resolved one.
    pub affix_id: Option<String>,
    /// Display label (matched label, else the raw affix name).
    pub label: String,
    pub value_text: Option<String>,
    pub source_text: String,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
}

/// Lightweight runtime-facing projection of parsed item state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemSnapshot {
    /// Display name of the parsed item, if available.
    pub item_name: Option<String>,
    /// Parse status string (maps to FrameStatus.parse_status_string()).
    pub parse_status: String,
    /// Canonical item type id of the pinned catalog (`helm`, `chest`, ...),
    /// transported from the parse result unchanged (`ItemTooltipParseResult::
    /// item_type_id`). This is the only evaluation id; `None` when the parse
    /// did not resolve one and is never substituted.
    pub item_type: Option<String>,
    /// Display-side base type derivation ("Helm", "Two-Handed Sword") from the
    /// tooltip's rarity/base-type line. Display only, never an evaluation id.
    pub base_type: Option<String>,
    /// Parsed item power, if available.
    pub item_power: Option<u32>,
    /// Parsed affix rows in source order.
    pub affixes: Vec<CurrentItemAffixSnapshot>,
    /// Window-relative per-affix row geometry transported from the OCR
    /// parity path (see `d4lc_parse::ItemTooltipAffixRowGeometry`). One
    /// entry per affix whose OCR line carries geometry; empty otherwise.
    pub affix_row_geometry: Vec<ItemTooltipAffixRowGeometry>,
    /// Classified non-affix rows (aspect, unique aspect, rune, sigil)
    /// transported from the OCR parity path (see
    /// `d4lc_parse::ItemTooltipNonAffixRow`). The parse side classifies the
    /// rows; the runtime composes the window-relative row rects. Empty when
    /// the parse carried no non-affix rows.
    pub non_affix_rows: Vec<ItemTooltipNonAffixRow>,
}

/// Per-frame stage costs the live loop's per-tick breakdown builds on
/// Measured with the session's wall clock; the loop adds the
/// presentation and pacing stages on top.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStageTimings {
    pub capture_us: u64,
    pub detect_us: u64,
    pub detect_crop_us: u64,
    pub detect_gray_us: u64,
    pub detect_integral_us: u64,
    /// Template scan: `detect_us` minus the other detection splits,
    /// saturating.
    pub detect_scan_us: u64,
    /// Threads that ran the template scan; `None` when no scan ran this
    /// frame (repeated frame, or detection failed before the scan).
    pub detect_scan_threads: Option<usize>,
    /// Coordinator work: result adoption plus the observation decision.
    pub coordinator_us: u64,
}

/// Result of a single `process_one_frame()` call.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedFrameResult {
    /// Overlay frame for the overlay dispatch and DebugStatusUpdate.
    pub overlay_frame: OverlayFrame,
    /// Stage costs of this frame's pipeline pass.
    pub stage_us: FrameStageTimings,
    /// The accepted item of the current tooltip generation, if any — the
    /// same item [`ProcessedFrameResult::analysis`] carries. A result of an
    /// invalidated generation never appears here.
    pub current_item: Option<CurrentItemSnapshot>,
    /// Per-frame pipeline outcome.
    pub frame_status: FrameStatus,
    /// Detected tooltip rect in window space: this frame's detection
    /// (`TooltipDetectionTrace::final_tooltip_crop_rect`, the upstream
    /// `_currentScreenTooltip` crop clamped to the capture frame, a frame
    /// rect) converted once through the frame's placement. `None` when no
    /// tooltip was detected.
    pub tooltip_window_relative: Option<Rect>,
    /// The coordinator's accepted analysis state after this frame — the one
    /// snapshot the overlay and the UI both consume.
    pub analysis: Arc<AnalysisSnapshot>,
}

/// Pure function to determine the per-frame pipeline outcome from OCR result
/// and tooltip detection state.
///
/// Extracted for isolated testing — see tests below.
pub(crate) fn determine_frame_status(
    tooltip_found: bool,
    latest_ocr_result: Option<&TooltipTextSnapshot>,
    has_ocr_worker: bool,
) -> FrameStatus {
    if !tooltip_found {
        return FrameStatus::NoTooltip;
    }

    match latest_ocr_result {
        Some(result) if result.error.is_none() => match &result.item {
            Some(item) => match item.status {
                // Parsed and Partial both carry usable item data
                // (display_name, item_power, affixes, etc.). Downstream
                // consumers handle partial results correctly.
                ItemTooltipParseStatus::Parsed | ItemTooltipParseStatus::Partial => {
                    FrameStatus::Parsed
                }
                ItemTooltipParseStatus::NotItem => FrameStatus::ParseFailed {
                    message: "OCR text did not parse into an item".to_string(),
                },
                ItemTooltipParseStatus::Error => FrameStatus::ParseFailed {
                    message: "Parse error".to_string(),
                },
            },
            None => FrameStatus::TooltipDetected,
        },
        Some(result) => FrameStatus::OcrFailed {
            message: result
                .error
                .clone()
                .unwrap_or_else(|| "unknown OCR error".to_string()),
        },
        None if has_ocr_worker => FrameStatus::OcrPending,
        None => FrameStatus::TooltipDetected,
    }
}

/// Process one frame: capture → detect → stability → OCR poll/submit → parse → build result.
///
/// Only returns Err for fatal conditions (capture failure, unrecoverable pipeline error).
/// Detection/OCR/parse outcomes are expressed via `ProcessedFrameResult.frame_status`.
///
/// The caller:
/// - Owns the loop (start/stop, frame pacing)
/// - Maps the result to RuntimeEvents
/// - Supplies overlay descriptor state separately
///
/// `overlay_frame.hotpath_ms` measures the duration of this call only — the
/// capture/detect/scheduling/polling work on the loop thread. It is not the
/// user-visible latency: the asynchronous OCR work runs on the worker thread
/// and is reported separately through the diagnostics events
/// (`ocr_started_at` / `ocr_finished_at`), never folded into this number.
pub fn process_one_frame(
    session: &mut RuntimeSession,
    runtime: &PreparedRuntime,
) -> Result<ProcessedFrameResult, RuntimePipelineError> {
    let frame_start = Instant::now();
    let seq = session.sequence;
    let diagnostics = session.diagnostics.clone();

    // ── 1. Poll OCR worker for completed results ─────────────────
    // The adoption is coordinator work; its cost lands in the per-tick
    // breakdown.
    let adoption_start = Instant::now();
    adopt_completed_result(session, &diagnostics, seq);
    let adoption_us = micros(adoption_start.elapsed());

    // ── 2. Capture one frame ──────────────────────────────────────
    // A switched-off overlay captures nothing; the loop keeps running so
    // switching it back on is immediate.
    if !session.coordinator.overlay_enabled() {
        return Ok(process_disabled_frame(
            session,
            runtime,
            frame_start,
            adoption_us,
        ));
    }
    let capture_start = Instant::now();
    let captured = capture_frame(session, runtime, &diagnostics, seq)?;
    // `first_captured_at`: the wall time of the capture this frame
    // was built from.
    let capture_at = diagnostics.now_micros();
    let capture_micros = micros(capture_start.elapsed());
    let capture = match captured {
        FrameCapture::Fresh(capture) => capture,
        FrameCapture::NoNewFrame { last_seq } => {
            return Ok(process_repeated_frame(
                session,
                runtime,
                RepeatedFrame {
                    frame_start,
                    last_seq,
                    capture_at,
                    capture_micros,
                    adoption_us,
                },
            ));
        }
    };

    // ── 3. Poll the cursor source ─────────────────────────────────
    // One window-relative pointer observation per frame, right after
    // capture: the input of the overlay menu hover/lock state machine
    // (upstream `OverlayHandler.HandleMouseUpdatedMessage`). `None` only
    // when the source genuinely has no value this frame: poll failure, a
    // backend-managed pointer (Wayland surface-local events), or an
    // empty synthetic source.
    let cursor_window_relative = poll_cursor(runtime);

    // ── 3b. Window state from the capture ─────────────────────────
    let minimized = observe_window_state(session, runtime, &diagnostics, seq, &capture);

    // ── 4. Detect tooltip ─────────────────────────────────────────
    let detect_start = Instant::now();
    let detect_result = detect_tooltip(
        runtime,
        &diagnostics,
        seq,
        &capture,
        cursor_window_relative,
        minimized,
    )?;
    let detect_at = diagnostics.now_micros();
    let detect_micros = micros(detect_start.elapsed());
    let detect_timings = detect_result.as_ref().map(|result| result.timings);

    // ── 5. Observation → coordinator ──────────────────────────────
    // Every tooltip observation reaches the coordinator — stability and
    // already-processed content are its decisions. The whole block is
    // coordinator work for the per-tick breakdown, including the crop
    // fingerprint the observation carries.
    let observation_start = Instant::now();
    let frame_placement = FramePlacement {
        window_in_desktop: DesktopRect(capture.geometry),
        frame_origin_in_window: Point {
            x: capture.actual_capture_rect.x,
            y: capture.actual_capture_rect.y,
        },
        scale: FrameScale::from_milli(capture.scale.value_milli()),
    };
    let DetectedTooltip {
        found: tooltip_found,
        window_relative: tooltip_window_relative,
        observation: frame_observation,
    } = match detect_result {
        Some(detect_result) => observe_detected_tooltip(
            detect_result,
            session.ocr_worker.is_some(),
            seq,
            capture_at,
            frame_placement,
        ),
        None => DetectedTooltip {
            found: false,
            window_relative: None,
            observation: FrameObservation::Absent,
        },
    };
    let fed = feed_observation(session, &frame_observation);
    // The observation a repeated frame re-feeds, and the frame's
    // pixel buffer back to the Wayland stream's recycle pool.
    session.last_frame = Some(FrameReplay {
        geometry: capture.geometry,
        minimized,
        tooltip_found,
        tooltip_window_relative,
        observation: frame_observation,
    });
    #[cfg(feature = "wayland-portal")]
    if let FrameSource::Wayland(source) = &runtime.frame_source {
        source.recycle(capture.frame);
    }
    let observation_us = micros(observation_start.elapsed());
    let coordinator_us = adoption_us.saturating_add(observation_us);

    // ── 5b. Frame chain diagnostics ───────────────────────────────
    record_frame_chain(
        &diagnostics,
        FrameChainObservation {
            frame_id: seq,
            capture_at_micros: capture_at,
            capture_micros,
            detect_at_micros: detect_at,
            detect_micros,
            tooltip_found,
            tooltip_lost: fed.tooltip_lost,
            generation_changed: fed.generation_changed,
            scheduling: fed.scheduling,
            repeated_from: None,
        },
    );

    // ── 6. Accepted analysis → frame status and current item ──────
    // Only the accepted analysis of the current generation is reported: a
    // lost tooltip or an invalidated generation shows nothing.
    let analysis = session.coordinator.snapshot();
    let frame_status = determine_frame_status(
        tooltip_found,
        session.latest_ocr_result(),
        session.ocr_worker.is_some(),
    );
    let current_item = analysis.item.clone();

    // ── 7. Build overlay frame ────────────────────────────────────
    let hotpath_ms = duration_ms(frame_start.elapsed());
    let overlay_frame = OverlayFrame {
        sequence: seq,
        tooltip_found,
        hotpath_ms,
        cursor_window_relative,
        rule_descriptors: std::sync::Arc::from([]),
        text_font_px: d4lc_rules::DEFAULT_OVERLAY_FONT_PX, // evaluated by the caller from the analysis
    };

    session.sequence = session.sequence.wrapping_add(1);

    Ok(ProcessedFrameResult {
        overlay_frame,
        stage_us: stage_timings(
            capture_micros,
            detect_micros,
            detect_timings,
            coordinator_us,
        ),
        current_item,
        frame_status,
        tooltip_window_relative,
        analysis,
    })
}

/// Poll the OCR worker for a completed result and hand it to the coordinator.
///
/// The adoption seam: the coordinator alone decides whether a completed
/// result is taken over. The parse event belongs to the frame the job was
/// planned for; the adopting frame is named in the detail, so the split
/// between observation and adoption stays visible.
#[inline]
fn adopt_completed_result(
    session: &mut RuntimeSession,
    diagnostics: &DiagnosticsContext,
    seq: u64,
) {
    let completed = session
        .ocr_worker
        .as_ref()
        .and_then(|worker| worker.poll_latest_result());
    if let Some(result) = completed {
        let source_frame = result.source_frame_sequence;
        let detail = diagnostics.verbosity().records_detail().then(|| {
            format!(
                "ocr_result_sequence={} adopted_by_frame={seq} generation={} parity_path={} item_name={}",
                result.sequence,
                result.token.tooltip_generation,
                result.parity_path,
                result
                    .item
                    .as_ref()
                    .and_then(|item| item.display_name.clone())
                    .unwrap_or_else(|| "<none>".to_string()),
            )
        });
        let parse_outcome = parse_outcome_of(&result);
        let disposition = session.coordinator.on_result(result);
        let outcome = match disposition {
            ResultDisposition::Accepted | ResultDisposition::Failed => parse_outcome,
            ResultDisposition::Discarded(reason) => DiagnosticsOutcome::discarded(reason),
        };
        let mut event = diagnostics
            .begin(source_frame, DiagnosticsPhase::Parse)
            .with_outcome(outcome);
        let detail = match (detail, session.coordinator.take_content_diff()) {
            (Some(detail), Some(diff)) => Some(format!("{detail} {diff}")),
            (detail, diff) => detail.or(diff),
        };
        if let Some(detail) = detail {
            event = event.with_detail(detail);
        }
        diagnostics.record(event);
    }
}

/// Capture one frame from the runtime's frame source.
///
/// The frame-source seam: X11, synthetic (tests), and Wayland ScreenCast
/// portal sources share this single dispatch.
#[inline]
fn capture_frame(
    session: &mut RuntimeSession,
    runtime: &PreparedRuntime,
    diagnostics: &DiagnosticsContext,
    seq: u64,
) -> Result<FrameCapture, RuntimePipelineError> {
    match &runtime.frame_source {
        FrameSource::X11 => capture_reference_with_backend_and_request(
            &runtime.capture_reference,
            runtime.capture_backend,
            CaptureRequest::Full,
        )
        .map(FrameCapture::Fresh),
        FrameSource::Synthetic(source) => source
            .capture(CaptureRequest::Full)
            .map(FrameCapture::Fresh),
        #[cfg(feature = "wayland-portal")]
        FrameSource::Wayland(source) => {
            source
                .capture(CaptureRequest::Full)
                .map(|capture| match capture {
                    d4lc_capture::WaylandCapture::Frame(result) => FrameCapture::Fresh(result),
                    d4lc_capture::WaylandCapture::NoNewFrame { last_seq } => {
                        FrameCapture::NoNewFrame { last_seq }
                    }
                })
        }
        #[cfg(test)]
        FrameSource::Scripted(source) => source.capture(),
    }
    .map_err(|error| {
        // The fatal capture seam: this frame produced no capture at all.
        let event = diagnostics
            .begin(seq, DiagnosticsPhase::Capture)
            .with_outcome(DiagnosticsOutcome::discarded(DiscardReason::CaptureFailed))
            .with_detail(error.to_string());
        diagnostics.record(event);
        // A frame that cannot be captured invalidates at once.
        session
            .coordinator
            .invalidate(InvalidationCause::CaptureFailed);
        RuntimePipelineError::CaptureFailed(error)
    })
}

/// Feed the captured window state to the coordinator; returns whether the
/// window counts as minimized (no detection this frame).
///
/// A changed window geometry is a new window revision. A capture shorter
/// than 100 rows is upstream's "Diablo IV window is probably minimized": it
/// publishes an empty tooltip instead of searching the frame
/// (ScreenProcessHandler.cs:323-332). Only a captured game window can be
/// minimized — not a manual region or a synthetic source. The game-window
/// observer reports focus, minimize and close separately; the Wayland portal
/// stream has no proven window-state source.
#[inline]
fn observe_window_state(
    session: &mut RuntimeSession,
    runtime: &PreparedRuntime,
    diagnostics: &DiagnosticsContext,
    seq: u64,
    capture: &d4lc_capture::CaptureResult,
) -> bool {
    if session.coordinator.observe_window(capture.geometry) {
        record_invalidation(diagnostics, seq, InvalidationCause::WindowChanged);
    }
    let minimized = matches!(runtime.frame_source, FrameSource::X11)
        && matches!(runtime.capture_reference.source, CaptureSource::Window(_))
        && capture.frame.height < MINIMIZED_CAPTURE_HEIGHT;
    if let Some(cause) = session.coordinator.set_capture_minimized(minimized) {
        record_invalidation(diagnostics, seq, cause);
    }
    minimized
}

/// Detect the tooltip in the captured frame; `None` while the window is
/// minimized.
#[inline]
fn detect_tooltip(
    runtime: &PreparedRuntime,
    diagnostics: &DiagnosticsContext,
    seq: u64,
    capture: &d4lc_capture::CaptureResult,
    cursor_window_relative: Option<Point>,
    minimized: bool,
) -> Result<Option<d4lc_detect::prepared::DetectFrameResult>, RuntimePipelineError> {
    if minimized {
        Ok(None)
    } else {
        // Mouse mode narrows the search to upstream's cursor-centred
        // `TooltipWidth * 2.5` band over the bottom `ScanHeight` percent
        // (ScreenProcessHandler.cs:562-566). The capture stays the full
        // window: the search ROI is cropped out for matching only, and the
        // tooltip crop after a match still reads the full-window pixels
        // above the band, exactly like upstream's `currentScreenSource`.
        // Controller mode keeps the full-frame search, and a frame without
        // a cursor observation cannot form a band and searches the
        // full frame.
        let mut roi_frame = RoiFrame::full_window(&capture.frame);
        if let Some(band) = mouse_search_band(runtime, cursor_window_relative, &capture.frame) {
            roi_frame.planned_roi = Some(band);
            roi_frame.roi_role = d4lc_detect::RoiRole::MouseSearchBand;
        }
        detect_roi_frame_prepared(
            roi_frame,
            &runtime.detector,
            cursor_window_relative.map(|cursor| cursor.x).unwrap_or(0),
            None, // no trace output path
            None, // no crop output path
        )
        .map_err(|error| {
            let event = diagnostics
                .begin(seq, DiagnosticsPhase::Detect)
                .with_outcome(DiagnosticsOutcome::failed(error.clone()));
            diagnostics.record(event);
            RuntimePipelineError::PipelineFailed(error)
        })
        .map(Some)
    }
}

/// What one detected tooltip contributes to the frame.
struct DetectedTooltip {
    found: bool,
    window_relative: Option<Rect>,
    observation: FrameObservation,
}

/// Turn a detection into the observation the coordinator is fed.
///
/// The detection's own crop becomes the job's immutable image: the worker
/// recognizes exactly the pixels this frame detected, never a later capture.
/// A tooltip without an OCR path (no worker, no crop) is still an
/// observation: without a worker nothing can be analyzed.
#[inline]
fn observe_detected_tooltip(
    detect_result: d4lc_detect::prepared::DetectFrameResult,
    has_worker: bool,
    seq: u64,
    capture_at: u64,
    frame_placement: FramePlacement,
) -> DetectedTooltip {
    let tooltip_found = detect_result.selected.is_some();
    let crop_rect = detect_result.tooltip_detection.final_tooltip_crop_rect;
    // The detected tooltip in window space — converted from the frame
    // here, once, with the same placement the job's row geometry uses.
    let tooltip_window_relative =
        crop_rect.map(|rect| frame_placement.frame_to_window(FrameRect(rect)).rect());
    let observation = match (detect_result.crop, crop_rect, &detect_result.selected) {
        (Some(crop), Some(crop_rect), Some(selected)) => Some(TooltipObservation {
            signature: tooltip_ocr_signature(TooltipOcrSignatureInput {
                crop_rect,
                template_name: &selected.template_name,
                template_rect: selected.rect,
            }),
            image: TooltipImage::new(
                crop,
                TooltipCaptureInfo {
                    frame_id: seq,
                    captured_at_micros: capture_at,
                    placement: TooltipPlacement {
                        frame: frame_placement,
                        crop_in_frame: FrameRect(crop_rect),
                    },
                },
            ),
            tooltip_window_relative,
        }),
        _ => None,
    };
    let observation = match observation {
        Some(observation) if has_worker => FrameObservation::Tooltip(observation),
        // A tooltip without an OCR path (no worker, no crop) is still an
        // observation: without a worker nothing can be analyzed.
        Some(_) | None if tooltip_found => FrameObservation::TooltipWithoutOcr,
        _ => FrameObservation::Absent,
    };
    DetectedTooltip {
        found: tooltip_found,
        window_relative: tooltip_window_relative,
        observation,
    }
}

/// The stage costs of one processed frame; the detect split is zero when
/// no detection ran.
#[inline]
fn stage_timings(
    capture_micros: u64,
    detect_micros: u64,
    detect_timings: Option<d4lc_detect::prepared::DetectionTimings>,
    coordinator_us: u64,
) -> FrameStageTimings {
    FrameStageTimings {
        capture_us: capture_micros,
        detect_us: detect_micros,
        detect_crop_us: detect_timings
            .map(|timings| ms_to_us(timings.roi_crop_ms))
            .unwrap_or(0),
        detect_gray_us: detect_timings
            .map(|timings| ms_to_us(timings.grayscale_ms))
            .unwrap_or(0),
        detect_integral_us: detect_timings
            .map(|timings| ms_to_us(timings.integral_ms))
            .unwrap_or(0),
        detect_scan_us: detect_timings
            .map(|timings| ms_to_us(timings.match_ms).saturating_sub(ms_to_us(timings.integral_ms)))
            .unwrap_or(0),
        detect_scan_threads: detect_timings
            .map(|timings| timings.scan_threads)
            .filter(|threads| *threads > 0),
        coordinator_us,
    }
}

/// Microseconds of a milliseconds figure from `DetectionTimings`
/// (saturating; the detect split is only reported at millisecond
/// granularity).
fn ms_to_us(ms: u128) -> u64 {
    u64::try_from(ms).unwrap_or(u64::MAX).saturating_mul(1000)
}

/// Capture height below which upstream treats the game window as minimized
/// (`currentScreen.Height < 100`, ScreenProcessHandler.cs:323).
const MINIMIZED_CAPTURE_HEIGHT: u32 = 100;

/// The parse outcome a result carries (never re-derived).
fn parse_outcome_of(result: &TooltipTextSnapshot) -> DiagnosticsOutcome {
    match (&result.error, &result.item) {
        (Some(error), _) => DiagnosticsOutcome::failed(error.clone()),
        (None, None) => DiagnosticsOutcome::Ok,
        (None, Some(item)) => match item.status {
            ItemTooltipParseStatus::Parsed | ItemTooltipParseStatus::Partial => {
                DiagnosticsOutcome::Ok
            }
            ItemTooltipParseStatus::NotItem => {
                DiagnosticsOutcome::failed("OCR text did not parse into an item")
            }
            ItemTooltipParseStatus::Error => DiagnosticsOutcome::failed("Parse error"),
        },
    }
}

/// One frame source's capture outcome. Destructured at once
/// per frame, so boxing the large variant would only add an allocation.
#[allow(clippy::large_enum_variant)]
pub(crate) enum FrameCapture {
    /// A fresh capture.
    Fresh(d4lc_capture::CaptureResult),
    /// No new frame since the previous capture; `last_seq` names the
    /// newest delivered source frame. Only the Wayland stream (and the
    /// test script) can report it; X11 and synthetic sources always
    /// capture fresh.
    #[cfg_attr(not(any(test, feature = "wayland-portal")), allow(dead_code))]
    NoNewFrame { last_seq: u64 },
}

/// What one frame observed, in the shape the coordinator is fed (a repeated
/// frame re-feeds it).
#[derive(Debug, Clone)]
pub(crate) enum FrameObservation {
    /// A tooltip with an OCR path: the observation reaches the coordinator.
    Tooltip(TooltipObservation),
    /// A tooltip without an OCR path (no worker, no crop): nothing to feed.
    TooltipWithoutOcr,
    /// No tooltip (or no detection, for a minimized window).
    Absent,
}

/// The previous processed frame's window state and observation — exactly
/// what re-capturing its identical pixels would produce again. No pixels:
/// the tooltip crop is the observation's shared `Arc` image.
#[derive(Debug, Clone)]
pub(crate) struct FrameReplay {
    geometry: Rect,
    minimized: bool,
    tooltip_found: bool,
    tooltip_window_relative: Option<Rect>,
    observation: FrameObservation,
}

/// What feeding one observation to the coordinator did.
struct ObservationFed {
    scheduling: FrameScheduling,
    generation_changed: bool,
    tooltip_lost: bool,
}

/// Feed one frame observation to the coordinator (and publish its OCR job
/// when the coordinator decides to submit).
fn feed_observation(
    session: &mut RuntimeSession,
    observation: &FrameObservation,
) -> ObservationFed {
    match observation {
        FrameObservation::Tooltip(observation) => {
            let signature = observation.signature;
            let outcome = session
                .coordinator
                .observe_tooltip(observation.clone(), Instant::now());
            // `queued_at` is stamped *before* the job is
            // published: the worker picks the job up immediately, so a
            // timestamp read after the submit could land behind the
            // worker's own `ocr_started_at` and break the chain order.
            let queued_at_micros = session.diagnostics.now_micros();
            let decision = outcome.decision;
            let decision_name = decision.as_str();
            let superseded = decision.superseded();
            let submitted = matches!(decision, ObserveDecision::Submit(_));
            let mut first_seen_at_micros = None;
            if let (ObserveDecision::Submit(job), Some(worker)) =
                (decision, session.ocr_worker.as_ref())
            {
                first_seen_at_micros = job.token.first_seen_at_micros;
                worker.submit(job);
            }
            ObservationFed {
                scheduling: FrameScheduling::Decision(OcrSubmission {
                    decision_name,
                    superseded,
                    submitted,
                    signature,
                    queued_at_micros,
                    first_seen_at_micros,
                }),
                generation_changed: outcome.generation_changed,
                tooltip_lost: false,
            }
        }
        FrameObservation::TooltipWithoutOcr => ObservationFed {
            scheduling: FrameScheduling::Unavailable,
            generation_changed: false,
            tooltip_lost: false,
        },
        FrameObservation::Absent => ObservationFed {
            scheduling: FrameScheduling::Unavailable,
            generation_changed: false,
            tooltip_lost: session.coordinator.observe_absent(),
        },
    }
}

/// Poll the cursor source once per frame.
fn poll_cursor(runtime: &PreparedRuntime) -> Option<Point> {
    match &runtime.cursor_source {
        CursorSource::X11 => read_cursor_state(Some(runtime.capture_reference.geometry))
            .ok()
            .and_then(|cursor| cursor.window_relative),
        #[cfg(feature = "wayland-portal")]
        CursorSource::BackendManaged => None,
        CursorSource::Synthetic(source) => source.read(),
    }
}

/// The mouse-mode search band for this frame
/// (`ScreenProcessHandler.cs:562-566`, see
/// `d4lc_detect::upstream_mouse_search_roi`): the cursor-centred
/// `TooltipWidth * 2.5` column over the bottom `ScanHeight` percent of the
/// capture. `None` keeps the full-frame search — controller mode, a frame
/// without a cursor observation (the band cannot be centred), or a
/// degenerate band.
fn mouse_search_band(
    runtime: &PreparedRuntime,
    cursor_window_relative: Option<Point>,
    frame: &d4lc_core::CaptureFrame,
) -> Option<Rect> {
    if runtime.detector.mode != InputMode::Mouse {
        return None;
    }
    let cursor = cursor_window_relative?;
    d4lc_detect::upstream_mouse_search_roi(
        frame.width,
        frame.height,
        cursor.x,
        runtime.detector.tooltip_width,
        runtime.detector.scan_height_percent,
    )
}

/// The capture facts of a frame whose source delivered nothing new.
struct RepeatedFrame {
    frame_start: Instant,
    last_seq: u64,
    capture_at: u64,
    capture_micros: u64,
    adoption_us: u64,
}

/// A frame without a new capture. Detection and hashing are
/// skipped; the previous frame's window state and observation are re-fed
/// to the coordinator — what re-capturing identical pixels would produce —
/// so the stability counter and the tooltip-lost logic behave as with a
/// real repeat. A session with no previous frame (the first frame after a
/// runtime swap) has nothing to re-feed and observes nothing until the
/// source delivers a frame.
fn process_repeated_frame(
    session: &mut RuntimeSession,
    runtime: &PreparedRuntime,
    repeat: RepeatedFrame,
) -> ProcessedFrameResult {
    let seq = session.sequence;
    let diagnostics = session.diagnostics.clone();
    let cursor_window_relative = poll_cursor(runtime);

    let observation_start = Instant::now();
    let (tooltip_found, tooltip_window_relative, fed) = match session.last_frame.take() {
        Some(replay) => {
            if session.coordinator.observe_window(replay.geometry) {
                record_invalidation(&diagnostics, seq, InvalidationCause::WindowChanged);
            }
            if let Some(cause) = session.coordinator.set_capture_minimized(replay.minimized) {
                record_invalidation(&diagnostics, seq, cause);
            }
            let fed = feed_observation(session, &replay.observation);
            let facts = (replay.tooltip_found, replay.tooltip_window_relative, fed);
            session.last_frame = Some(replay);
            facts
        }
        None => (
            false,
            None,
            ObservationFed {
                scheduling: FrameScheduling::Unavailable,
                generation_changed: false,
                tooltip_lost: false,
            },
        ),
    };
    let observation_us = micros(observation_start.elapsed());

    record_frame_chain(
        &diagnostics,
        FrameChainObservation {
            frame_id: seq,
            capture_at_micros: repeat.capture_at,
            capture_micros: repeat.capture_micros,
            detect_at_micros: repeat.capture_at,
            detect_micros: 0,
            tooltip_found,
            tooltip_lost: fed.tooltip_lost,
            generation_changed: fed.generation_changed,
            scheduling: fed.scheduling,
            repeated_from: Some(repeat.last_seq),
        },
    );

    let analysis = session.coordinator.snapshot();
    let current_item = analysis.item.clone();
    let overlay_frame = OverlayFrame {
        sequence: seq,
        tooltip_found,
        hotpath_ms: duration_ms(repeat.frame_start.elapsed()),
        cursor_window_relative,
        rule_descriptors: std::sync::Arc::from([]),
        text_font_px: d4lc_rules::DEFAULT_OVERLAY_FONT_PX,
    };
    session.sequence = session.sequence.wrapping_add(1);

    ProcessedFrameResult {
        overlay_frame,
        stage_us: FrameStageTimings {
            capture_us: repeat.capture_micros,
            coordinator_us: repeat.adoption_us.saturating_add(observation_us),
            ..FrameStageTimings::default()
        },
        current_item,
        frame_status: FrameStatus::RepeatedFrame,
        tooltip_window_relative,
        analysis,
    }
}

/// A frame while the overlay is switched off: no capture, detection or
/// observation, only the cursor poll the overlay menu needs, and an empty
/// overlay frame.
fn process_disabled_frame(
    session: &mut RuntimeSession,
    runtime: &PreparedRuntime,
    frame_start: Instant,
    adoption_us: u64,
) -> ProcessedFrameResult {
    let seq = session.sequence;
    let cursor_window_relative = poll_cursor(runtime);
    let analysis = session.coordinator.snapshot();
    let current_item = analysis.item.clone();
    let overlay_frame = OverlayFrame {
        sequence: seq,
        tooltip_found: false,
        hotpath_ms: duration_ms(frame_start.elapsed()),
        cursor_window_relative,
        rule_descriptors: std::sync::Arc::from([]),
        text_font_px: d4lc_rules::DEFAULT_OVERLAY_FONT_PX,
    };
    session.sequence = session.sequence.wrapping_add(1);

    ProcessedFrameResult {
        overlay_frame,
        stage_us: FrameStageTimings {
            coordinator_us: adoption_us,
            ..FrameStageTimings::default()
        },
        current_item,
        frame_status: FrameStatus::OverlayDisabled,
        tooltip_window_relative: None,
        analysis,
    }
}

/// Record one invalidation transition with its discard reason.
fn record_invalidation(diagnostics: &DiagnosticsContext, frame_id: u64, cause: InvalidationCause) {
    if !diagnostics.is_enabled() {
        return;
    }
    let event = diagnostics
        .begin(frame_id, DiagnosticsPhase::StateChange)
        .with_outcome(DiagnosticsOutcome::discarded(cause.discard_reason()))
        .with_detail(format!("invalidated: {}", cause.as_str()));
    diagnostics.record(event);
}

/// Project one parsed tooltip into the runtime-facing current item snapshot.
///
/// The transported item type is the canonical catalog id of the parse
/// (`ItemTooltipParseResult::item_type_id`), never the display derivation of
/// the tooltip's rarity/base-type line: the rules compare the type exactly
/// against the canonical ids the imports carry. The display
/// derivation travels separately as [`CurrentItemSnapshot::base_type`], and
/// an item whose type did not resolve stays unknown.
pub(crate) fn current_item_snapshot(
    item: &d4lc_parse::ItemTooltipParseResult,
    parse_status: &str,
) -> CurrentItemSnapshot {
    CurrentItemSnapshot {
        item_name: item.display_name.clone(),
        parse_status: parse_status.to_string(),
        item_type: item.item_type_id.clone(),
        base_type: item.base_type.clone(),
        item_power: item.item_power,
        affixes: item
            .affixes
            .iter()
            .map(|affix| CurrentItemAffixSnapshot {
                row_id: affix.row_id,
                affix_id: affix.affix_id.clone(),
                label: affix
                    .affix_label
                    .clone()
                    .unwrap_or_else(|| affix.name.clone()),
                value_text: affix.value_text.clone(),
                source_text: affix.raw_text.clone(),
                is_greater: affix.is_greater_affix,
                is_implicit: affix.is_implicit,
                is_tempered: affix.is_tempered,
            })
            .collect(),
        affix_row_geometry: item.affix_row_geometry.clone(),
        non_affix_rows: item.non_affix_rows.clone(),
    }
}

/// One frame's coordinator decision, for the diagnostics frame chain.
struct OcrSubmission {
    decision_name: &'static str,
    superseded: bool,
    submitted: bool,
    signature: u64,
    /// Monotonic time the job was published (`queued_at`).
    queued_at_micros: u64,
    /// Capture time of the frame that opened the track
    /// (`first_seen_at`), from the published job's token.
    first_seen_at_micros: Option<u64>,
}

/// What this frame did about OCR for its tooltip observation.
enum FrameScheduling {
    /// No tooltip, or no OCR path was reachable for this observation (no
    /// worker, no crop rect).
    Unavailable,
    /// The coordinator decided.
    Decision(OcrSubmission),
}

/// One frame's observation, for the diagnostics frame chain.
struct FrameChainObservation {
    frame_id: u64,
    capture_at_micros: u64,
    capture_micros: u64,
    detect_at_micros: u64,
    detect_micros: u64,
    tooltip_found: bool,
    /// Whether this frame ended a visible tooltip.
    tooltip_lost: bool,
    /// Whether this frame started a new tooltip generation.
    generation_changed: bool,
    scheduling: FrameScheduling,
    /// The source delivered no new frame; the previous
    /// observation was re-fed. Carries the newest delivered stream frame.
    repeated_from: Option<u64>,
}

/// Record the frame-chain diagnostics of one frame.
///
/// Emission policy (bounded by the sinks, proportional to activity):
///
/// - A frame that carries a tooltip records `capture` + `detect`. The detect
///   outcome names the frame's disposition: `ok` for a new/covered
///   observation, or the `Superseded` discard when in-flight or already
///   processed work covers this exact observation.
/// - The frame on which a visible tooltip disappears records the `detect`
///   discard `TooltipLost` — the invalidation transition, not once per absent
///   frame.
/// - A published OCR job adds one `ocr_submit` event (`queued_at`); the worker
///   adds `ocr_start` / `ocr_finish`; the frame that adopts (or discards) a
///   completed result adds the `parse` event.
/// - Frames with no tooltip and no transition stay silent; their volume is
///   aggregated by the sinks instead of being recorded per frame.
fn record_frame_chain(diagnostics: &DiagnosticsContext, observation: FrameChainObservation) {
    let FrameChainObservation {
        frame_id,
        capture_at_micros,
        capture_micros,
        detect_at_micros,
        detect_micros,
        tooltip_found,
        tooltip_lost,
        generation_changed,
        scheduling,
        repeated_from,
    } = observation;

    // A repeated frame is always named, so it never reads as a fresh
    // capture.
    let repeat_detail = repeated_from.map(|last_seq| {
        format!(
            "repeated_frame: no new frame since stream frame {last_seq}; the previous \
             observation was re-fed"
        )
    });
    if !tooltip_found {
        if let Some(detail) = &repeat_detail {
            if diagnostics.is_enabled() {
                let mut event = diagnostics
                    .begin(frame_id, DiagnosticsPhase::Capture)
                    .with_duration_micros(capture_micros)
                    .with_detail(detail.clone());
                event.at_micros = capture_at_micros;
                diagnostics.record(event);
            }
        }
        if tooltip_lost {
            let mut event = diagnostics
                .begin(frame_id, DiagnosticsPhase::Detect)
                .with_duration_micros(detect_micros)
                .with_outcome(DiagnosticsOutcome::discarded(DiscardReason::TooltipLost))
                .with_detail("tooltip_found=false");
            event.at_micros = detect_at_micros;
            diagnostics.record(event);
        }
        return;
    }

    if !diagnostics.is_enabled() {
        return;
    }
    let mut capture_event = diagnostics
        .begin(frame_id, DiagnosticsPhase::Capture)
        .with_duration_micros(capture_micros);
    if let Some(detail) = repeat_detail {
        capture_event = capture_event.with_detail(detail);
    }
    capture_event.at_micros = capture_at_micros;
    diagnostics.record(capture_event);

    let detect_outcome = match &scheduling {
        FrameScheduling::Decision(submission) if submission.superseded => {
            DiagnosticsOutcome::discarded(DiscardReason::Superseded)
        }
        FrameScheduling::Unavailable | FrameScheduling::Decision(_) => DiagnosticsOutcome::Ok,
    };
    let mut detect_event = diagnostics
        .begin(frame_id, DiagnosticsPhase::Detect)
        .with_duration_micros(detect_micros)
        .with_outcome(detect_outcome);
    detect_event.at_micros = detect_at_micros;
    let detect_event = diagnostics.attach_detail(detect_event, || {
        let scheduling = match &scheduling {
            FrameScheduling::Unavailable => "no_ocr_path".to_string(),
            FrameScheduling::Decision(submission) => format!(
                "{}(signature={})",
                submission.decision_name, submission.signature
            ),
        };
        format!(
            "tooltip_found=true generation={} new_generation={generation_changed} scheduling={scheduling}",
            diagnostics.tooltip_generation()
        )
    });
    diagnostics.record(detect_event);

    if let FrameScheduling::Decision(submission) = &scheduling {
        if submission.submitted {
            let mut event = diagnostics.begin(frame_id, DiagnosticsPhase::OcrSubmit);
            event.at_micros = submission.queued_at_micros;
            event.first_seen_at_micros = submission.first_seen_at_micros;
            let detail = format!(
                "signature={} generation={}",
                submission.signature,
                diagnostics.tooltip_generation(),
            );
            let event = diagnostics.attach_detail(event, || detail);
            diagnostics.record(event);
        }
    }
}

/// Microseconds of a measured duration, saturating.
fn micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Milliseconds of a measured duration, rounded up.
fn duration_ms(duration: std::time::Duration) -> u128 {
    duration.as_micros().div_ceil(1000)
}

#[cfg(test)]
pub(crate) fn mock_ocr_snapshot() -> TooltipTextSnapshot {
    TooltipTextSnapshot {
        sequence: 0,
        source_frame_sequence: 0,
        tooltip_signature: 0,
        content_fingerprint: 0,
        token: Default::default(),
        ocr_read_top: None,
        ocr_read_extent: None,
        parity_path: String::new(),
        ocr_ms: 0,
        capture_ms: 0,
        conversion_ms: 0,
        splitter_detection_ms: None,
        segment_count: 0,
        line_count: 0,
        raw_text: String::new(),
        normalized_lines: Vec::new(),
        segments_preview: Vec::new(),
        title_preview: None,
        text_preview: String::new(),
        item: None,
        error: None,
        debug: None,
    }
}

#[cfg(test)]
pub(crate) fn mock_parse_result(
    status: ItemTooltipParseStatus,
) -> d4lc_parse::ItemTooltipParseResult {
    d4lc_parse::ItemTooltipParseResult {
        status,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagnosticsClock, DiagnosticsHandle, DiagnosticsVerbosity};
    use crate::prepared::PreparedRuntime;
    use crate::session::RuntimeSession;
    use crate::{Revision, TooltipTextSnapshot};
    use d4lc_capture::{CaptureBackend, CaptureReference, CaptureSource};
    use d4lc_core::{InputMode, Rect};
    use d4lc_detect::prepared::{PreparedDetector, ThresholdSettings};
    use d4lc_parse::ItemTooltipParseStatus;
    use std::time::Duration;

    fn dummy_runtime() -> PreparedRuntime {
        PreparedRuntime::new(
            CaptureBackend::X11Native,
            CaptureReference {
                source: CaptureSource::ManualRegion(Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                }),
                geometry: Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            },
            PreparedDetector {
                preset: "test".into(),
                mode: InputMode::Mouse,
                scan_height_percent: 50.0,
                thresholds: ThresholdSettings {
                    min: 0,
                    max: 255,
                    similarity_tooltip: 0.8,
                },
                tooltip_width: 300,
                templates: Vec::new(),
                scan: d4lc_detect::prepared::DetectScan::default(),
            },
        )
    }

    fn dummy_session() -> RuntimeSession {
        RuntimeSession::new(None, Duration::from_millis(250))
    }

    #[test]
    fn process_one_frame_capture_failure_returns_error() {
        let mut runtime = dummy_runtime();
        // A capture reference without pixels fails the capture step in every
        // environment, so the fatal-error path is asserted deterministically.
        // (A 1x1 dummy frame does not fail detection: the search region is
        // the captured frame, so it resolves to "no tooltip".)
        runtime.capture_reference = CaptureReference {
            source: CaptureSource::ManualRegion(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }),
            geometry: Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
        };
        let mut session = dummy_session();
        let result = process_one_frame(&mut session, &runtime);

        match &result {
            Err(RuntimePipelineError::CaptureFailed(msg)) => {
                assert!(!msg.is_empty(), "error message should not be empty");
            }
            other => panic!("expected capture failure, got: {other:?}"),
        }
    }

    /// The fatal capture seam records the discard with its concrete reason: a
    /// frame with no capture at all is not silently missing from the chain.
    #[test]
    fn process_one_frame_capture_failure_records_the_capture_failed_discard() {
        let mut runtime = dummy_runtime();
        runtime.capture_reference = CaptureReference {
            source: CaptureSource::ManualRegion(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }),
            geometry: Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
        };
        let handle = DiagnosticsHandle::new(16, DiagnosticsVerbosity::Summary);
        let mut session = RuntimeSession::with_diagnostics(
            None,
            Duration::from_millis(250),
            crate::DiagnosticsContext::new(
                3,
                DiagnosticsClock::new(),
                handle.clone(),
                Revision::Applied(1),
                Revision::Unknown,
            ),
        );

        assert!(process_one_frame(&mut session, &runtime).is_err());

        let events = handle.drain();
        assert_eq!(
            events.len(),
            1,
            "the failed frame records exactly one event"
        );
        let event = &events[0];
        assert_eq!(event.session_id, 3);
        assert_eq!(event.frame_id, 0);
        assert_eq!(event.phase, DiagnosticsPhase::Capture);
        assert_eq!(
            event.outcome,
            DiagnosticsOutcome::discarded(DiscardReason::CaptureFailed)
        );
        assert_eq!(event.config_revision, Revision::Applied(1));
        assert_eq!(event.rules_revision, Revision::Unknown);
        assert!(
            event
                .detail
                .as_deref()
                .is_some_and(|detail| !detail.is_empty()),
            "the discard must carry the capture reason"
        );
    }

    /// Without attached diagnostics the pipeline records nothing and behaves
    /// exactly as before — instrumentation is opt-in, never a behavior change.
    #[test]
    fn process_one_frame_without_diagnostics_records_nothing() {
        // A synthetic 1x1 frame keeps this test independent of a display
        // server: the verify gate runs the suite with DISPLAY unset.
        let runtime = dummy_runtime();
        let frame = d4lc_core::CaptureFrame {
            width: 1,
            height: 1,
            stride: 4,
            pixels_bgra: vec![0, 0, 0, 255],
        };
        let source = d4lc_capture::SyntheticCaptureFrameSource::new(
            runtime.capture_reference.clone(),
            frame,
        )
        .unwrap_or_else(|error| panic!("synthetic source: {error}"));
        let runtime = runtime.with_synthetic_capture_source(source);
        let mut session = dummy_session();
        let result = process_one_frame(&mut session, &runtime)
            .unwrap_or_else(|error| panic!("frame failed: {error}"));

        assert!(!session.diagnostics().is_enabled());
        assert_eq!(session.diagnostics().dropped_events(), 0);
        assert!(session.diagnostics().drain().is_empty());
        assert_eq!(result.frame_status, FrameStatus::NoTooltip);
    }

    #[test]
    fn frame_status_no_tooltip_returns_correct_string() {
        assert_eq!(FrameStatus::NoTooltip.parse_status_string(), "No tooltip");
    }

    #[test]
    fn frame_status_parsed_returns_correct_string() {
        assert_eq!(FrameStatus::Parsed.parse_status_string(), "Parsed");
    }

    #[test]
    fn frame_status_ocr_failed_returns_correct_string() {
        assert_eq!(
            FrameStatus::OcrFailed {
                message: "timeout".into()
            }
            .parse_status_string(),
            "OCR failed: timeout"
        );
    }

    #[test]
    fn frame_status_parse_failed_returns_correct_string() {
        assert_eq!(
            FrameStatus::ParseFailed {
                message: "garbage".into()
            }
            .parse_status_string(),
            "Parse failed: garbage"
        );
    }

    #[test]
    fn frame_status_tooltip_detected_returns_correct_string() {
        assert_eq!(
            FrameStatus::TooltipDetected.parse_status_string(),
            "Tooltip detected"
        );
    }

    #[test]
    fn frame_status_ocr_pending_returns_correct_string() {
        assert_eq!(FrameStatus::OcrPending.parse_status_string(), "OCR pending");
    }

    #[test]
    fn current_item_snapshot_holds_values() {
        let snap = CurrentItemSnapshot {
            item_name: Some("Helm".into()),
            parse_status: "Parsed".into(),
            item_type: Some("helm".into()),
            base_type: Some("Helm".into()),
            item_power: Some(925),
            affixes: vec![CurrentItemAffixSnapshot {
                row_id: AffixRowId(0),
                affix_id: Some("Affix_MaxLife".into()),
                label: "Maximum Life".into(),
                value_text: Some("+740".into()),
                source_text: "+740 Maximum Life".into(),
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
            }],
            affix_row_geometry: Vec::new(),
            non_affix_rows: Vec::new(),
        };
        assert_eq!(snap.item_name.as_deref(), Some("Helm"));
        assert_eq!(snap.parse_status, "Parsed");
        assert_eq!(snap.item_type.as_deref(), Some("helm"));
        assert_eq!(snap.base_type.as_deref(), Some("Helm"));
        assert_eq!(snap.item_power, Some(925));
        assert_eq!(snap.affixes.len(), 1);
        assert_eq!(snap.affixes[0].affix_id.as_deref(), Some("Affix_MaxLife"));

        let snap_none = CurrentItemSnapshot {
            item_name: None,
            parse_status: "No tooltip".into(),
            item_type: None,
            base_type: None,
            item_power: None,
            affixes: Vec::new(),
            affix_row_geometry: Vec::new(),
            non_affix_rows: Vec::new(),
        };
        assert!(snap_none.item_name.is_none());
        assert!(snap_none.item_type.is_none());
        assert!(snap_none.base_type.is_none());
        assert!(snap_none.affixes.is_empty());
    }

    /// The projection transports the canonical parse type id as
    /// the evaluation identity and keeps the display derivation in its own
    /// field; an unresolved type stays unknown instead of falling back to the
    /// display line.
    #[test]
    fn current_item_projection_carries_the_canonical_type_id() {
        let mut item = mock_parse_result(ItemTooltipParseStatus::Parsed);
        item.base_type = Some("Helm".into());
        item.item_type_id = Some("helm".into());

        let snapshot = current_item_snapshot(&item, "Parsed");
        assert_eq!(
            snapshot.item_type.as_deref(),
            Some("helm"),
            "the canonical catalog type id is the transported evaluation identity"
        );
        assert_eq!(
            snapshot.base_type.as_deref(),
            Some("Helm"),
            "the display derivation stays a display field"
        );

        let mut unresolved = mock_parse_result(ItemTooltipParseStatus::Parsed);
        unresolved.base_type = Some("Two-Handed Sword".into());
        let snapshot = current_item_snapshot(&unresolved, "Parsed");
        assert!(
            snapshot.item_type.is_none(),
            "an unresolved type must not be replaced by the display derivation"
        );
        assert_eq!(snapshot.base_type.as_deref(), Some("Two-Handed Sword"));
    }

    #[test]
    fn processed_frame_result_holds_fields() {
        let overlay = OverlayFrame {
            sequence: 1,
            tooltip_found: true,
            hotpath_ms: 42,
            cursor_window_relative: None,
            rule_descriptors: std::sync::Arc::from([]),
            text_font_px: d4lc_rules::DEFAULT_OVERLAY_FONT_PX,
        };
        let current_item = Some(CurrentItemSnapshot {
            item_name: Some("Helm".into()),
            parse_status: "Parsed".into(),
            item_type: Some("helm".into()),
            base_type: Some("Helm".into()),
            item_power: Some(925),
            affixes: Vec::new(),
            affix_row_geometry: Vec::new(),
            non_affix_rows: Vec::new(),
        });
        let result = ProcessedFrameResult {
            stage_us: Default::default(),
            overlay_frame: overlay,
            current_item,
            frame_status: FrameStatus::Parsed,
            tooltip_window_relative: Some(Rect {
                x: 12,
                y: 0,
                width: 300,
                height: 42,
            }),
            analysis: Arc::new(AnalysisSnapshot::empty(1)),
        };
        assert_eq!(result.overlay_frame.sequence, 1);
        assert!(result.overlay_frame.tooltip_found);
        assert_eq!(result.overlay_frame.hotpath_ms, 42);
        assert_eq!(
            result.tooltip_window_relative,
            Some(Rect {
                x: 12,
                y: 0,
                width: 300,
                height: 42,
            })
        );
        assert_eq!(
            result
                .current_item
                .as_ref()
                .and_then(|ci| ci.item_name.as_deref()),
            Some("Helm")
        );
        assert_eq!(
            result
                .current_item
                .as_ref()
                .and_then(|ci| ci.item_type.as_deref()),
            Some("helm")
        );
        assert_eq!(result.frame_status, FrameStatus::Parsed);
    }

    #[test]
    fn runtime_pipeline_error_display() {
        let err = RuntimePipelineError::CaptureFailed("no display".into());
        assert_eq!(err.to_string(), "capture failed: no display");

        let err2 = RuntimePipelineError::PipelineFailed("backend error".into());
        assert_eq!(err2.to_string(), "pipeline failed: backend error");
    }

    // ── Helpers for determine_frame_status tests ─────────────────

    // ── determine_frame_status tests ────────────────────────────

    #[test]
    fn test_determine_status_no_tooltip() {
        assert_eq!(
            determine_frame_status(false, None, true),
            FrameStatus::NoTooltip
        );
    }

    #[test]
    fn test_determine_status_ocr_pending() {
        assert_eq!(
            determine_frame_status(true, None, true),
            FrameStatus::OcrPending
        );
    }

    #[test]
    fn test_determine_status_tooltip_detected_no_worker() {
        assert_eq!(
            determine_frame_status(true, None, false),
            FrameStatus::TooltipDetected
        );
    }

    #[test]
    fn test_determine_status_tooltip_detected_item_none() {
        let snap = mock_ocr_snapshot(); // error=None, item=None
        assert_eq!(
            determine_frame_status(true, Some(&snap), false),
            FrameStatus::TooltipDetected
        );
    }

    #[test]
    fn test_determine_status_ocr_failed() {
        let snap = TooltipTextSnapshot {
            error: Some("timeout".into()),
            debug: None,
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(true, Some(&snap), false),
            FrameStatus::OcrFailed {
                message: "timeout".into()
            }
        );
    }

    #[test]
    fn test_determine_status_parsed() {
        let snap = TooltipTextSnapshot {
            error: None,
            debug: None,
            item: Some(mock_parse_result(ItemTooltipParseStatus::Parsed)),
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(true, Some(&snap), false),
            FrameStatus::Parsed
        );
    }

    #[test]
    fn test_determine_status_partial_is_parsed() {
        // Decision: Partial carries usable item data; downstream handles it.
        let snap = TooltipTextSnapshot {
            error: None,
            debug: None,
            item: Some(mock_parse_result(ItemTooltipParseStatus::Partial)),
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(true, Some(&snap), false),
            FrameStatus::Parsed
        );
    }

    #[test]
    fn test_determine_status_not_item_is_parse_failed() {
        let snap = TooltipTextSnapshot {
            error: None,
            debug: None,
            item: Some(mock_parse_result(ItemTooltipParseStatus::NotItem)),
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(true, Some(&snap), false),
            FrameStatus::ParseFailed {
                message: "OCR text did not parse into an item".into()
            }
        );
    }

    #[test]
    fn test_determine_status_error_is_parse_failed() {
        let snap = TooltipTextSnapshot {
            error: None,
            debug: None,
            item: Some(mock_parse_result(ItemTooltipParseStatus::Error)),
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(true, Some(&snap), false),
            FrameStatus::ParseFailed {
                message: "Parse error".into()
            }
        );
    }

    #[test]
    fn test_determine_status_ocr_error_distinct_from_parse_failure() {
        // OcrFailed and ParseFailed are different variants; verify the
        // function distinguishes them correctly.
        let ocr_snap = TooltipTextSnapshot {
            error: Some("timeout".into()),
            debug: None,
            item: None,
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(true, Some(&ocr_snap), false),
            FrameStatus::OcrFailed {
                message: "timeout".into()
            }
        );

        let parse_snap = TooltipTextSnapshot {
            error: None,
            debug: None,
            item: Some(mock_parse_result(ItemTooltipParseStatus::NotItem)),
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(true, Some(&parse_snap), false),
            FrameStatus::ParseFailed {
                message: "OCR text did not parse into an item".into()
            }
        );
    }

    #[test]
    fn test_determine_status_no_tooltip_ignores_ocr_result() {
        // Even with a fully parsed OCR result, tooltip_found=false
        // must produce NoTooltip.
        let snap = TooltipTextSnapshot {
            error: None,
            debug: None,
            item: Some(mock_parse_result(ItemTooltipParseStatus::Parsed)),
            ..mock_ocr_snapshot()
        };
        assert_eq!(
            determine_frame_status(false, Some(&snap), true),
            FrameStatus::NoTooltip
        );
    }
}
