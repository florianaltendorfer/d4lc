//! d4lc-runtime — latest-frame state, crop stability helpers, and OCR worker.
//!
//! The live runtime keeps only the newest captured frame.  This matches the
//! runtime pipeline rule that detector/OCR work must not build an unbounded
//! backlog when the game produces frames faster than downstream processing can
//! consume them.

use d4lc_core::coords::TooltipPlacement;
use d4lc_core::CaptureFrame;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

pub mod coordinator;
pub mod debug_bundle;
pub mod diagnostics;
#[cfg(test)]
mod e2e;
pub mod ocr_parity;
mod pipeline;
mod prepared;
mod reload;
mod session;

pub use coordinator::{
    read_rows_unchanged, AcceptedAnalysis, AnalysisSnapshot, AnalysisToken, GameWindowState,
    InvalidationCause, ObserveDecision, ObserveOutcome, ReadExtent, ResultDisposition,
    RuntimeCoordinator, RuntimeLifecycle, TooltipObservation, TooltipState,
};
/// The parse result an OCR result carries ([`TooltipTextSnapshot::item`]).
pub use d4lc_parse::{ItemTooltipParseResult, ItemTooltipParseStatus};
/// The live debug bundle of one accepted tooltip analysis.
pub use debug_bundle::{DebugImage, DebugSegmentImage, DebugTextLine, TooltipDebugBundle};
pub use diagnostics::{
    current_job_context, render_frame_chain, with_job_context_scope, DiagnosticsClock,
    DiagnosticsContext, DiagnosticsEvent, DiagnosticsHandle, DiagnosticsOutcome, DiagnosticsPhase,
    DiagnosticsStats, DiagnosticsVerbosity, DiscardReason, FrameTimeline, Revision,
    DEFAULT_EVENT_CAPACITY,
};
pub use ocr_parity::{
    load_template_gray_image, process_tooltip_ocr_parity, process_tooltip_ocr_parity_prepared,
    DetectedMarkerPlan, MarkerTemplate, PreparedTooltipOcr, SegmentResult, TooltipCropTrace,
    TooltipOcrExecution, TooltipOcrParityResult, TooltipOcrSegmentPreview, TEMPLATE_IMAGE_MAX_SIDE,
};
pub use pipeline::{
    process_one_frame, CurrentItemSnapshot, FrameStageTimings, FrameStatus, ProcessedFrameResult,
    RuntimePipelineError,
};
pub use prepared::{FramePacing, PacingSource, PreparedRuntime, SyntheticCursorSource};
pub use reload::{apply_pending_reload, ReloadOutcome, RuntimeReloadSignal};
pub use session::RuntimeSession;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestFrame<T> {
    pub sequence: u64,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestFrameSlot<T> {
    next_sequence: u64,
    dropped_frames: u64,
    latest: Option<LatestFrame<T>>,
}

impl<T> Default for LatestFrameSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LatestFrameSlot<T> {
    pub const fn new() -> Self {
        Self {
            next_sequence: 0,
            dropped_frames: 0,
            latest: None,
        }
    }

    /// Store a new frame and drop any unread older frame.
    pub fn push(&mut self, value: T) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        if self.latest.is_some() {
            self.dropped_frames = self.dropped_frames.saturating_add(1);
        }
        self.latest = Some(LatestFrame { sequence, value });
        sequence
    }

    /// Take the newest unread frame from the slot.
    pub fn take_latest(&mut self) -> Option<LatestFrame<T>> {
        self.latest.take()
    }

    pub const fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    pub const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn has_unread_frame(&self) -> bool {
        self.latest.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropStabilityStatus {
    pub hash: u64,
    pub changed: bool,
    pub stable_count: u32,
    pub should_schedule_ocr: bool,
}

/// The debug CLI's hash-change gate. The app does not use it: its live loop
/// decides when a tooltip is read through the placement-stability and
/// generation rules of [`RuntimeCoordinator::observe_tooltip`]. The CLI keeps
/// this simpler gate on purpose, because it only reports the status in its
/// session JSON (`crop_stability`) and never schedules OCR from it, so the
/// two can disagree about "changed" in a debug session. Moving the CLI to the
/// coordinator would change the recorded report shape and is not done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CropStabilityGate {
    last_hash: Option<u64>,
    stable_count: u32,
}

impl Default for CropStabilityGate {
    fn default() -> Self {
        Self::new()
    }
}

impl CropStabilityGate {
    pub const fn new() -> Self {
        Self {
            last_hash: None,
            stable_count: 0,
        }
    }

    /// Observe a crop hash.  OCR should only be scheduled when the crop changed.
    pub fn observe_hash(&mut self, hash: u64) -> CropStabilityStatus {
        let changed = self.last_hash != Some(hash);
        if changed {
            self.last_hash = Some(hash);
            self.stable_count = 1;
        } else {
            self.stable_count = self.stable_count.saturating_add(1);
        }

        CropStabilityStatus {
            hash,
            changed,
            stable_count: self.stable_count,
            should_schedule_ocr: changed,
        }
    }

    pub const fn last_hash(&self) -> Option<u64> {
        self.last_hash
    }
}

/// Deterministic FNV-1a hash over frame dimensions, stride and visible pixels.
///
/// The CLI live session exports it as the crop-stability hash, and recorded
/// sessions under `debug-output/` hold such values, so it stays FNV-1a.
pub fn hash_capture_frame(frame: &CaptureFrame) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    hash = fnv_update(hash, &frame.width.to_le_bytes());
    hash = fnv_update(hash, &frame.height.to_le_bytes());
    hash = fnv_update(hash, &(frame.stride as u64).to_le_bytes());

    let width_bytes = match usize::try_from(frame.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
    {
        Some(value) => value,
        None => return hash,
    };
    let height = match usize::try_from(frame.height) {
        Ok(value) => value,
        Err(_) => return hash,
    };

    for row in 0..height {
        let start = match row.checked_mul(frame.stride) {
            Some(value) => value,
            None => break,
        };
        let end = match start.checked_add(width_bytes) {
            Some(value) => value,
            None => break,
        };
        if let Some(bytes) = frame.pixels_bgra.get(start..end) {
            hash = fnv_update(hash, bytes);
        } else {
            break;
        }
    }

    hash
}

/// Content fingerprint over the visible pixels of the rows
/// `first_row..height` (plus the width and the number of hashed rows).
///
/// Stride padding is not part of the content: two buffers with the same
/// visible pixels and different row padding fingerprint the same.
///
/// Used as the [`TooltipImage`] content fingerprint (`first_row` 0). The
/// "already processed" comparison is noise-tolerant and compares pixels
/// instead (see [`coordinator::read_rows_unchanged`]).
///
/// Fingerprints are compared only within one process and are never persisted
/// or exported, so the function is free to change between builds. It hashes
/// 16 bytes per step (see `ContentHasher`); the exported crop-stability
/// hash stays FNV-1a ([`hash_capture_frame`]), whose values persisted debug
/// sessions contain.
pub fn hash_capture_rows(frame: &CaptureFrame, first_row: u32) -> u64 {
    let rows = frame.height.saturating_sub(first_row);
    let mut hasher = ContentHasher::new(u64::from(frame.width), u64::from(rows));
    let (Ok(width), Ok(first), Ok(height)) = (
        usize::try_from(frame.width),
        usize::try_from(first_row),
        usize::try_from(frame.height),
    ) else {
        return hasher.finish();
    };
    let Some(width_bytes) = width.checked_mul(4) else {
        return hasher.finish();
    };
    for row in first..height {
        let Some(start) = row.checked_mul(frame.stride) else {
            break;
        };
        let Some(end) = start.checked_add(width_bytes) else {
            break;
        };
        match frame.pixels_bgra.get(start..end) {
            Some(bytes) => hasher.write_row(bytes),
            None => break,
        }
    }
    hasher.finish()
}

/// Word-at-a-time content hash: every 16 bytes are folded into the state
/// with one 64×64→128-bit multiply (the wyhash mixing step), so the chain
/// costs one multiply per 16 bytes instead of one per byte as in FNV-1a.
/// Order-sensitive; a row's tail is zero-padded and its length mixed in.
struct ContentHasher {
    state: u64,
    rows: u64,
}

const CONTENT_HASH_K0: u64 = 0xa076_1d64_78bd_642f;
const CONTENT_HASH_K1: u64 = 0xe703_7ed1_a0b4_28db;
const CONTENT_HASH_K2: u64 = 0x8ebc_6af0_9c88_c6e3;

impl ContentHasher {
    fn new(width: u64, rows: u64) -> Self {
        Self {
            state: fold_multiply(width ^ CONTENT_HASH_K0, rows ^ CONTENT_HASH_K1),
            rows: 0,
        }
    }

    fn write_row(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<16>();
        let mut state = self.state;
        for word in words {
            let (low, high) = word.split_at(8);
            state = fold_multiply(
                le_u64(low) ^ CONTENT_HASH_K0,
                le_u64(high) ^ state ^ CONTENT_HASH_K1,
            );
        }
        let mut padded = [0u8; 16];
        padded[..tail.len()].copy_from_slice(tail);
        let (low, high) = padded.split_at(8);
        state = fold_multiply(
            le_u64(low) ^ CONTENT_HASH_K2,
            le_u64(high) ^ state ^ (tail.len() as u64),
        );
        self.state = state;
        self.rows += 1;
    }

    fn finish(&self) -> u64 {
        fold_multiply(self.state ^ CONTENT_HASH_K2, self.rows ^ CONTENT_HASH_K0)
    }
}

fn fold_multiply(a: u64, b: u64) -> u64 {
    let product = u128::from(a) * u128::from(b);
    (product as u64) ^ ((product >> 64) as u64)
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(bytes);
    u64::from_le_bytes(word)
}

// ---------------------------------------------------------------------------
// Async OCR worker — runs Tesseract outside the frame hotpath.
// ---------------------------------------------------------------------------

/// Capture-time facts of one tooltip image: which frame it was cut from,
/// when that frame was captured, and where the crop sits in window and
/// desktop space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooltipCaptureInfo {
    /// Sequence of the frame the detection ran on.
    pub frame_id: u64,
    /// Capture time of that frame on the session's diagnostics clock
    /// (`first_captured_at`).
    pub captured_at_micros: u64,
    /// The crop's rect in the frame plus the frame's window/desktop
    /// placement at capture time.
    pub placement: TooltipPlacement,
}

/// The immutable tooltip image one OCR job runs on.
///
/// The pixels are the crop the detection produced for this exact frame,
/// shared read-only: a later capture — X11 re-capture, the next Wayland
/// stream buffer, anything — cannot change what the job recognizes. The
/// content fingerprint is computed once over these same pixels, so two
/// crops with identical geometry but different content are different work.
#[derive(Debug, Clone)]
pub struct TooltipImage {
    pixels: Arc<CaptureFrame>,
    content_fingerprint: u64,
    capture: TooltipCaptureInfo,
}

impl TooltipImage {
    /// Wrap a detected crop. The fingerprint is taken here, over the pixels
    /// the job will carry.
    pub fn new(pixels: CaptureFrame, capture: TooltipCaptureInfo) -> Self {
        Self::from_shared(Arc::new(pixels), capture)
    }

    /// Wrap an already shared crop.
    pub fn from_shared(pixels: Arc<CaptureFrame>, capture: TooltipCaptureInfo) -> Self {
        let content_fingerprint = hash_capture_rows(&pixels, 0);
        Self {
            pixels,
            content_fingerprint,
            capture,
        }
    }

    pub fn pixels(&self) -> &CaptureFrame {
        &self.pixels
    }

    /// Shared handle on the pixels (cheap clone, same allocation).
    pub fn shared_pixels(&self) -> Arc<CaptureFrame> {
        Arc::clone(&self.pixels)
    }

    pub const fn content_fingerprint(&self) -> u64 {
        self.content_fingerprint
    }

    pub const fn capture(&self) -> &TooltipCaptureInfo {
        &self.capture
    }
}

/// A single OCR job: the tooltip image to recognize plus the observation
/// identity it was scheduled under.
///
/// The frame loop hands over the pixels its detection already cut out; the
/// worker never captures. Queueing a job moves an `Arc`, not the pixels.
#[derive(Debug, Clone)]
pub struct OcrJob {
    /// Geometry signature of the tooltip observation (crop rect, template).
    pub tooltip_signature: u64,
    /// Validity token the coordinator planned the job under; the worker
    /// stamps it onto the result.
    pub token: AnalysisToken,
    pub image: TooltipImage,
}

impl OcrJob {
    /// Frame the job's pixels were captured in.
    pub const fn frame_id(&self) -> u64 {
        self.image.capture.frame_id
    }

    pub const fn content_fingerprint(&self) -> u64 {
        self.image.content_fingerprint
    }
}

/// Preview for one upstream-parity OCR segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipTextSegmentSnapshot {
    pub index: usize,
    pub segment_kind: String,
    pub line_count: usize,
    pub raw_text_preview: String,
    pub converted_text_preview: String,
}

/// Normalized snapshot produced by the OCR worker for a single tooltip crop.
#[derive(Debug, Clone, PartialEq)]
pub struct TooltipTextSnapshot {
    /// Monotonic counter assigned by the worker (not the frame sequence).
    pub sequence: u64,
    pub source_frame_sequence: u64,
    pub tooltip_signature: u64,
    /// Content fingerprint of the pixels the result was recognized from
    /// ([`TooltipImage::content_fingerprint`]).
    pub content_fingerprint: u64,
    /// Validity token of the job this result belongs to (stamped by the
    /// worker from the job).
    pub token: AnalysisToken,
    /// First crop row that identifies the analyzed item: the first splitter
    /// below the item name, else the first recognized text line; `None` when
    /// the OCR path did not report its segment geometry (see
    /// [`coordinator::read_rows_unchanged`]).
    pub ocr_read_top: Option<u32>,
    /// Crop columns the recognized text lines cover; `None` compares the
    /// whole crop width (see [`coordinator::read_rows_unchanged`]).
    pub ocr_read_extent: Option<ReadExtent>,
    /// Runtime OCR path marker; live OCR must use the read-tooltip parity path.
    pub parity_path: String,
    /// Tesseract wall time (ms), summed across upstream-planned OCR segments.
    pub ocr_ms: u64,
    /// Crop/capture step time (ms), measured inside the worker.
    pub capture_ms: u64,
    /// Text conversion/parsing time (ms), measured inside the worker/parity path.
    pub conversion_ms: u64,
    /// Splitter detection time (ms), measured inside the read-tooltip parity path.
    pub splitter_detection_ms: Option<u64>,
    pub segment_count: usize,
    pub line_count: usize,
    pub raw_text: String,
    pub normalized_lines: Vec<String>,
    pub segments_preview: Vec<TooltipTextSegmentSnapshot>,
    pub title_preview: Option<String>,
    pub text_preview: String,
    pub item: Option<ItemTooltipParseResult>,
    pub error: Option<String>,
    /// Live stage debug bundle of this result, built by
    /// the worker only while `Settings.IsDebugInfoEnabled` is on. `None`
    /// with the bit off — no stage is allocated then.
    pub debug: Option<Arc<TooltipDebugBundle>>,
}

#[derive(Debug, Clone, Default)]
pub struct OcrWorkerStats {
    pub submitted: u64,
    pub completed: u64,
    pub dropped: u64,
    pub failed: u64,
    pub pending: bool,
    pub in_progress: bool,
    pub total_ocr_ms: u64,
    pub total_capture_ms: u64,
    pub total_conversion_ms: u64,
    pub total_splitter_detection_ms: u64,
    /// Highest observed in-flight job count (`pending + in_progress`) at any
    /// submit. The queue is single-slot (latest job wins), so the maximum is 2.
    pub max_queue_depth: usize,
    /// Wall time the worker thread spent executing jobs (submit-to-result OCR
    /// work, including the in-worker capture/crop step), in milliseconds.
    pub total_busy_ms: u64,
    /// Longest single job execution observed, in milliseconds.
    pub max_busy_ms: u64,
    pub last_error: Option<String>,
}

struct WorkerState {
    pending: Option<OcrJob>,
    in_progress: bool,
    submitted: u64,
    dropped: u64,
    completed: u64,
    failed: u64,
    total_ocr_ms: u64,
    total_capture_ms: u64,
    total_conversion_ms: u64,
    total_splitter_detection_ms: u64,
    max_queue_depth: usize,
    total_busy_ms: u64,
    max_busy_ms: u64,
    last_error: Option<String>,
    latest_result: Option<TooltipTextSnapshot>,
    next_sequence: u64,
    shutdown: bool,
    /// Diagnostics session the worker emits into. Default is a disabled
    /// context; the owning live session attaches the real one.
    diagnostics: DiagnosticsContext,
    /// Signal bumped after every stored result, so a loop that waits for
    /// the next frame adopts the result at once instead of one frame later.
    result_signal: Option<d4lc_capture::FrameSignal>,
}

/// Background OCR worker with a single-slot bounded queue.
///
/// When a new job is submitted while the previous pending job has not yet been
/// picked up by the worker thread, the pending job is replaced (counted as
/// dropped) so the worker always processes the *freshest* tooltip crop.
pub struct OcrWorker {
    shared: Arc<(Mutex<WorkerState>, Condvar)>,
    thread: Option<thread::JoinHandle<()>>,
    /// Live gate for the in-memory debug bundle. The
    /// job closure the caller supplied reads the same flag; this handle lets
    /// the live loop flip it when `Settings.IsDebugInfoEnabled` changes.
    /// Without a shared flag (plain [`OcrWorker::spawn`]) it stays off.
    debug_bundle: Arc<AtomicBool>,
}

impl OcrWorker {
    /// Spawn a worker thread that calls `ocr_fn` for each submitted job.
    ///
    /// `ocr_fn` must be `Send + 'static` because it runs on a background thread.
    /// It receives an `OcrJob` (the immutable tooltip image) and must return
    /// a `TooltipTextSnapshot` or an error string. The worker stamps the
    /// result's `sequence` and its job identity (frame, signature, content
    /// fingerprint) from the job itself, so a result always names the pixels
    /// it was recognized from.
    pub fn spawn<F>(ocr_fn: F) -> Self
    where
        F: Fn(OcrJob) -> Result<TooltipTextSnapshot, String> + Send + 'static,
    {
        Self::spawn_with_debug_flag(ocr_fn, Arc::new(AtomicBool::new(false)))
    }

    /// [`OcrWorker::spawn`] with the shared debug-bundle gate the job closure
    /// reads. The live loop flips the returned handle
    /// through [`OcrWorker::set_debug_bundle_enabled`]; with it off no bundle
    /// is built at all.
    pub fn spawn_with_debug_flag<F>(ocr_fn: F, debug_bundle: Arc<AtomicBool>) -> Self
    where
        F: Fn(OcrJob) -> Result<TooltipTextSnapshot, String> + Send + 'static,
    {
        let shared = Arc::new((
            Mutex::new(WorkerState {
                pending: None,
                in_progress: false,
                submitted: 0,
                dropped: 0,
                completed: 0,
                failed: 0,
                total_ocr_ms: 0,
                total_capture_ms: 0,
                total_conversion_ms: 0,
                total_splitter_detection_ms: 0,
                max_queue_depth: 0,
                total_busy_ms: 0,
                max_busy_ms: 0,
                last_error: None,
                latest_result: None,
                next_sequence: 0,
                shutdown: false,
                diagnostics: DiagnosticsContext::disabled(),
                result_signal: None,
            }),
            Condvar::new(),
        ));

        let shared_clone = Arc::clone(&shared);
        let thread_handle = thread::spawn(move || {
            let (lock, condvar) = &*shared_clone;
            loop {
                let (job, diagnostics) = {
                    let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    loop {
                        if state.shutdown {
                            return;
                        }
                        if let Some(job) = state.pending.take() {
                            state.in_progress = true;
                            break (job, state.diagnostics.clone());
                        }
                        state = condvar
                            .wait(state)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                };

                let source_frame_sequence = job.frame_id();
                let tooltip_signature = job.tooltip_signature;
                let content_fingerprint = job.content_fingerprint();
                let token = job.token;
                // `ocr_started_at`: stamped when the worker picks the
                // job up, on the same clock the frame loop uses.
                diagnostics.record_outcome(
                    source_frame_sequence,
                    DiagnosticsPhase::OcrStart,
                    DiagnosticsOutcome::Ok,
                );
                let busy_start = std::time::Instant::now();
                // The ambient job context lets the OCR parity path (which is
                // reached through a caller-owned worker closure) report
                // catalog degradation into this job's session record.
                let result =
                    with_job_context_scope(diagnostics.clone(), source_frame_sequence, || {
                        ocr_fn(job)
                    });
                let busy_ms = u64::try_from(busy_start.elapsed().as_millis()).unwrap_or(u64::MAX);
                let busy_micros = busy_ms.saturating_mul(1_000);

                // `ocr_finished_at`: the worker's own end of the job,
                // with the measured job duration attached.
                let mut finish = diagnostics
                    .begin(source_frame_sequence, DiagnosticsPhase::OcrFinish)
                    .with_duration_micros(busy_micros);
                match &result {
                    Ok(snapshot) => {
                        finish = diagnostics.attach_detail(finish, || {
                            format!(
                                "signature={tooltip_signature} segments={} lines={} ocr_ms={} capture_ms={} conversion_ms={} text={}",
                                snapshot.segment_count,
                                snapshot.line_count,
                                snapshot.ocr_ms,
                                snapshot.capture_ms,
                                snapshot.conversion_ms,
                                detail_preview(&snapshot.raw_text),
                            )
                        });
                    }
                    Err(error) => {
                        finish = finish.with_outcome(DiagnosticsOutcome::failed(error.clone()));
                    }
                }
                diagnostics.record(finish);

                let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.total_busy_ms = state.total_busy_ms.saturating_add(busy_ms);
                state.max_busy_ms = state.max_busy_ms.max(busy_ms);
                match result {
                    Ok(mut snapshot) => {
                        state.in_progress = false;
                        let seq = state.next_sequence;
                        state.next_sequence = state.next_sequence.saturating_add(1);
                        snapshot.sequence = seq;
                        snapshot.source_frame_sequence = source_frame_sequence;
                        snapshot.tooltip_signature = tooltip_signature;
                        snapshot.content_fingerprint = content_fingerprint;
                        snapshot.token = token;
                        state.total_ocr_ms = state.total_ocr_ms.saturating_add(snapshot.ocr_ms);
                        state.total_capture_ms =
                            state.total_capture_ms.saturating_add(snapshot.capture_ms);
                        state.total_conversion_ms = state
                            .total_conversion_ms
                            .saturating_add(snapshot.conversion_ms);
                        if let Some(splitter_detection_ms) = snapshot.splitter_detection_ms {
                            state.total_splitter_detection_ms = state
                                .total_splitter_detection_ms
                                .saturating_add(splitter_detection_ms);
                        }
                        state.completed = state.completed.saturating_add(1);
                        state.latest_result = Some(snapshot);
                    }
                    Err(error) => {
                        state.in_progress = false;
                        let seq = state.next_sequence;
                        state.next_sequence = state.next_sequence.saturating_add(1);
                        state.failed = state.failed.saturating_add(1);
                        state.last_error = Some(error.clone());
                        state.latest_result = Some(TooltipTextSnapshot {
                            sequence: seq,
                            source_frame_sequence,
                            tooltip_signature,
                            content_fingerprint,
                            token,
                            ocr_read_top: None,
                            ocr_read_extent: None,
                            parity_path: "read-tooltip".to_string(),
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
                            error: Some(error),
                            debug: None,
                        });
                    }
                }
                let result_signal = state.result_signal.clone();
                drop(state);
                if let Some(signal) = result_signal {
                    signal.bump();
                }
            }
        });

        Self {
            shared,
            thread: Some(thread_handle),
            debug_bundle,
        }
    }

    /// Whether the job closure builds the live debug bundle.
    pub fn debug_bundle_enabled(&self) -> bool {
        self.debug_bundle.load(Ordering::Relaxed)
    }

    /// Flip the shared debug-bundle gate the job closure reads.
    pub fn set_debug_bundle_enabled(&self, enabled: bool) {
        self.debug_bundle.store(enabled, Ordering::Relaxed);
    }

    /// Submit a job.  If a pending unprocessed job already exists it is
    /// replaced and counted as dropped.
    pub fn submit(&self, job: OcrJob) {
        let (lock, condvar) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut replaced: Option<(DiagnosticsContext, u64, u64)> = None;
        // The replaced job is taken out here, but the queue-depth accounting
        // below must still see that the pending slot was occupied.
        let had_pending = state.pending.is_some();
        if let Some(pending) = state.pending.take() {
            state.dropped = state.dropped.saturating_add(1);
            // The discarding seam that exists today: a fresher tooltip
            // observation replaces a job the worker has not picked up yet.
            replaced = Some((
                state.diagnostics.clone(),
                pending.frame_id(),
                pending.tooltip_signature,
            ));
        }
        let queue_depth = usize::from(had_pending) + usize::from(state.in_progress);
        state.max_queue_depth = state.max_queue_depth.max(queue_depth);
        state.pending = Some(job);
        state.submitted = state.submitted.saturating_add(1);
        condvar.notify_one();
        drop(state);

        if let Some((diagnostics, pending_frame, signature)) = replaced {
            let event = diagnostics
                .begin(pending_frame, DiagnosticsPhase::OcrSubmit)
                .with_outcome(DiagnosticsOutcome::discarded(DiscardReason::Superseded))
                .with_detail(format!(
                    "replaced by a fresher job (pending signature={signature})"
                ));
            diagnostics.record(event);
        }
    }

    /// Bump `signal` after every stored result (`None` detaches). The live
    /// loop passes the frame signal it waits on, so an OCR result is
    /// adopted the moment it exists, not on the next captured frame.
    pub fn set_result_signal(&self, signal: Option<d4lc_capture::FrameSignal>) {
        let (lock, _) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.result_signal = signal;
    }

    /// Attach (or replace) the diagnostics session this worker emits into.
    ///
    /// The live loop calls this right after preparation, and again after a
    /// settings hot reload swapped in a freshly prepared worker. Every
    /// emission before the attach goes to the disabled default context and is
    /// counted there, never mixed into another session.
    pub fn attach_diagnostics(&self, diagnostics: DiagnosticsContext) {
        let (lock, _) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.diagnostics = diagnostics;
    }

    /// The diagnostics session currently attached to this worker.
    pub fn diagnostics(&self) -> DiagnosticsContext {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .diagnostics
            .clone()
    }

    /// Take the latest completed result, if any.  Returns `None` when the
    /// worker has not finished a job since the last poll.
    pub fn poll_latest_result(&self) -> Option<TooltipTextSnapshot> {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .latest_result
            .take()
    }

    /// Returns true when a job is waiting to be processed by the worker thread.
    pub fn is_pending(&self) -> bool {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .is_some()
    }

    /// Returns true when a job is pending or the worker thread is currently
    /// processing OCR. The live loop uses this for backpressure so it does not
    /// re-submit the same stable tooltip while Tesseract is still running.
    pub fn is_busy(&self) -> bool {
        let (lock, _) = &*self.shared;
        let state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pending.is_some() || state.in_progress
    }

    /// Snapshot of current counters (submitted / completed / dropped / failed).
    pub fn stats(&self) -> OcrWorkerStats {
        let (lock, _) = &*self.shared;
        let state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        OcrWorkerStats {
            submitted: state.submitted,
            completed: state.completed,
            dropped: state.dropped,
            failed: state.failed,
            pending: state.pending.is_some(),
            in_progress: state.in_progress,
            total_ocr_ms: state.total_ocr_ms,
            total_capture_ms: state.total_capture_ms,
            total_conversion_ms: state.total_conversion_ms,
            total_splitter_detection_ms: state.total_splitter_detection_ms,
            max_queue_depth: state.max_queue_depth,
            total_busy_ms: state.total_busy_ms,
            max_busy_ms: state.max_busy_ms,
            last_error: state.last_error.clone(),
        }
    }

    /// Signal the worker to exit on the next idle iteration.
    pub fn shutdown(&self) {
        let (lock, condvar) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.shutdown = true;
        condvar.notify_one();
    }
}

impl Drop for OcrWorker {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Byte budget of one verbose OCR text detail payload. Detail diagnostics are
/// opt-in and bounded: the raw text is truncated with an explicit marker
/// instead of growing without limit.
const OCR_DETAIL_TEXT_LIMIT: usize = 512;

/// Truncate an OCR detail text to [`OCR_DETAIL_TEXT_LIMIT`] bytes on a char
/// boundary, marking the truncation explicitly.
fn detail_preview(text: &str) -> String {
    if text.len() <= OCR_DETAIL_TEXT_LIMIT {
        return text.to_string();
    }
    let mut end = OCR_DETAIL_TEXT_LIMIT;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated {} bytes]", &text[..end], text.len() - end)
}

fn fnv_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_frame_slot_drops_unread_old_frame() {
        let mut slot = LatestFrameSlot::new();
        assert_eq!(slot.push("old"), 0);
        assert_eq!(slot.push("new"), 1);
        assert_eq!(slot.dropped_frames(), 1);

        let latest = slot
            .take_latest()
            .unwrap_or_else(|| panic!("missing latest frame"));
        assert_eq!(latest.sequence, 1);
        assert_eq!(latest.value, "new");
        assert!(!slot.has_unread_frame());
    }

    #[test]
    fn latest_frame_slot_does_not_count_consumed_frames_as_dropped() {
        let mut slot = LatestFrameSlot::new();
        slot.push(10);
        assert_eq!(slot.take_latest().map(|frame| frame.value), Some(10));
        slot.push(20);

        assert_eq!(slot.dropped_frames(), 0);
        assert_eq!(slot.take_latest().map(|frame| frame.value), Some(20));
    }

    #[test]
    fn crop_stability_gate_schedules_only_changed_hashes() {
        let mut gate = CropStabilityGate::new();
        let first = gate.observe_hash(42);
        let second = gate.observe_hash(42);
        let third = gate.observe_hash(99);

        assert!(first.changed);
        assert!(first.should_schedule_ocr);
        assert_eq!(first.stable_count, 1);
        assert!(!second.changed);
        assert!(!second.should_schedule_ocr);
        assert_eq!(second.stable_count, 2);
        assert!(third.changed);
        assert!(third.should_schedule_ocr);
        assert_eq!(third.stable_count, 1);
    }

    #[test]
    fn hash_capture_frame_changes_when_pixels_change() {
        let mut frame = CaptureFrame {
            width: 1,
            height: 1,
            stride: 4,
            pixels_bgra: vec![0, 0, 0, 255],
        };
        let before = hash_capture_frame(&frame);
        frame.pixels_bgra[0] = 1;
        let after = hash_capture_frame(&frame);
        assert_ne!(before, after);
    }

    #[test]
    fn hash_capture_frame_keeps_its_exported_fnv_values() {
        // Recorded CLI live sessions hold these crop-stability hashes.
        let frame = CaptureFrame {
            width: 1,
            height: 1,
            stride: 4,
            pixels_bgra: vec![0, 0, 0, 255],
        };
        let mut expected = FNV_OFFSET_BASIS;
        for byte in [
            1u8, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255,
        ] {
            expected ^= u64::from(byte);
            expected = expected.wrapping_mul(FNV_PRIME);
        }
        assert_eq!(hash_capture_frame(&frame), expected);
    }

    /// A `width`×`height` frame of varied pixels with `padding` stride bytes
    /// per row, filled with `pad_byte`.
    fn content_frame(width: u32, height: u32, padding: usize, pad_byte: u8) -> CaptureFrame {
        let stride = width as usize * 4 + padding;
        let mut pixels_bgra = vec![pad_byte; stride * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize * 4 {
                pixels_bgra[y * stride + x] = ((x * 31 + y * 17) % 251) as u8;
            }
        }
        CaptureFrame {
            width,
            height,
            stride,
            pixels_bgra,
        }
    }

    fn set_pixel_byte(frame: &mut CaptureFrame, x: u32, y: u32, value: u8) {
        let index = y as usize * frame.stride + x as usize * 4 + 1;
        frame.pixels_bgra[index] = value;
    }

    #[test]
    fn content_hash_ignores_stride_padding() {
        // Widths whose rows are a multiple of 16 bytes and ones with a tail.
        for width in [4, 5, 7, 37] {
            let tight = content_frame(width, 9, 0, 0);
            let padded = content_frame(width, 9, 12, 0xEE);
            let other_padding = content_frame(width, 9, 3, 0x11);
            for first_row in [0, 4] {
                let expected = hash_capture_rows(&tight, first_row);
                assert_eq!(hash_capture_rows(&padded, first_row), expected);
                assert_eq!(hash_capture_rows(&other_padding, first_row), expected);
            }
        }
    }

    #[test]
    fn content_hash_sees_every_pixel_below_the_read_top_and_none_above() {
        let read_top = 3;
        for width in [4, 5, 37] {
            let base = content_frame(width, 8, 4, 0);
            let expected = hash_capture_rows(&base, read_top);
            for y in 0..8 {
                for x in 0..width {
                    let mut changed = base.clone();
                    set_pixel_byte(&mut changed, x, y, 255 - base.pixels_bgra[0]);
                    let hash = hash_capture_rows(&changed, read_top);
                    if y < read_top {
                        assert_eq!(hash, expected, "({x}, {y}) is above the read top");
                    } else {
                        assert_ne!(hash, expected, "({x}, {y}) is below the read top");
                    }
                }
            }
        }
    }

    #[test]
    fn content_hash_names_geometry_and_row_order() {
        let frame = content_frame(4, 6, 0, 0);
        // Same bytes, other shape.
        let reshaped = CaptureFrame {
            width: 8,
            height: 3,
            stride: 32,
            pixels_bgra: frame.pixels_bgra.clone(),
        };
        assert_ne!(
            hash_capture_rows(&frame, 0),
            hash_capture_rows(&reshaped, 0)
        );
        // Two rows swapped.
        let mut swapped = frame.clone();
        let (first, second) = swapped.pixels_bgra.split_at_mut(16);
        first.swap_with_slice(&mut second[..16]);
        assert_ne!(hash_capture_rows(&frame, 0), hash_capture_rows(&swapped, 0));
        // A tooltip image fingerprints its whole crop with the same hash.
        let image = TooltipImage::new(
            frame.clone(),
            TooltipCaptureInfo {
                frame_id: 0,
                captured_at_micros: 0,
                placement: TooltipPlacement::default(),
            },
        );
        assert_eq!(image.content_fingerprint(), hash_capture_rows(&frame, 0));
    }

    fn tiny_ocr_job(sequence: u64) -> OcrJob {
        OcrJob {
            tooltip_signature: sequence,
            token: AnalysisToken::default(),
            image: TooltipImage::new(
                CaptureFrame {
                    width: 1,
                    height: 1,
                    stride: 4,
                    pixels_bgra: vec![0, 0, 0, 255],
                },
                TooltipCaptureInfo {
                    frame_id: sequence,
                    captured_at_micros: 0,
                    placement: TooltipPlacement::default(),
                },
            ),
        }
    }

    /// The worker names every result after the job it ran, whatever the
    /// closure filled in: a result can never claim another frame's pixels.
    #[test]
    fn ocr_worker_stamps_the_job_identity_onto_the_result() {
        let worker = OcrWorker::spawn(|_job| Ok(mock_worker_snapshot()));
        let job = tiny_ocr_job(31);
        let fingerprint = job.content_fingerprint();
        worker.submit(job);

        let mut result = None;
        for _ in 0..500 {
            result = worker.poll_latest_result();
            if result.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let snapshot = result.unwrap_or_else(|| panic!("OCR worker did not produce a result"));
        assert_eq!(snapshot.source_frame_sequence, 31);
        assert_eq!(snapshot.tooltip_signature, 31);
        assert_eq!(snapshot.content_fingerprint, fingerprint);
        assert_ne!(fingerprint, 0);
    }

    #[test]
    fn ocr_worker_delivers_result_and_increments_counters() {
        let worker = OcrWorker::spawn(|job| {
            Ok(TooltipTextSnapshot {
                sequence: 0,
                source_frame_sequence: job.frame_id(),
                tooltip_signature: job.tooltip_signature,
                content_fingerprint: job.content_fingerprint(),
                token: Default::default(),
                ocr_read_top: None,
                ocr_read_extent: None,
                parity_path: "read-tooltip".to_string(),
                ocr_ms: 1,
                capture_ms: 0,
                conversion_ms: 0,
                splitter_detection_ms: Some(0),
                segment_count: 1,
                line_count: 1,
                raw_text: "hello".to_string(),
                normalized_lines: vec!["hello".to_string()],
                segments_preview: Vec::new(),
                title_preview: Some("hello".to_string()),
                text_preview: "hello".to_string(),
                item: None,
                error: None,
                debug: None,
            })
        });

        worker.submit(tiny_ocr_job(42));

        // Poll until the worker finishes (bounded retries).
        let mut result = None;
        for _ in 0..500 {
            result = worker.poll_latest_result();
            if result.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let snapshot = result.unwrap_or_else(|| panic!("OCR worker did not produce a result"));
        assert_eq!(snapshot.sequence, 0);
        assert_eq!(snapshot.source_frame_sequence, 42);
        assert_eq!(snapshot.raw_text, "hello");

        let stats = worker.stats();
        assert_eq!(stats.submitted, 1);
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.dropped, 0);
        assert_eq!(stats.failed, 0);
    }

    #[test]
    fn ocr_worker_drops_stale_pending_job_when_queue_full() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::sync::Arc;

        // Deterministic under any load: job 1 reports itself through the
        // channel and then blocks on a gate until the test thread has filled
        // the single pending slot (job 2) and replaced it (job 3). The drop
        // can therefore never race the worker finishing job 1 early, the way
        // it did under slow CI runners.
        let (sender, receiver) = mpsc::channel::<u64>();
        let gate = Arc::new(AtomicBool::new(false));
        let worker_gate = gate.clone();

        let worker = OcrWorker::spawn(move |job| {
            let _ = sender.send(job.frame_id());
            if job.frame_id() == 1 {
                while !worker_gate.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            Ok(TooltipTextSnapshot {
                sequence: 0,
                source_frame_sequence: job.frame_id(),
                tooltip_signature: job.tooltip_signature,
                content_fingerprint: job.content_fingerprint(),
                token: Default::default(),
                ocr_read_top: None,
                ocr_read_extent: None,
                parity_path: "read-tooltip".to_string(),
                ocr_ms: 0,
                capture_ms: 0,
                conversion_ms: 0,
                splitter_detection_ms: Some(0),
                segment_count: 0,
                line_count: 0,
                raw_text: "".to_string(),
                normalized_lines: vec![],
                segments_preview: Vec::new(),
                title_preview: None,
                text_preview: "".to_string(),
                item: None,
                error: None,
                debug: None,
            })
        });

        // Wait (bounded) until the worker has picked up job 1.
        worker.submit(tiny_ocr_job(1));
        let mut picked_up = false;
        for _ in 0..500 {
            if receiver.try_recv() == Ok(1) {
                picked_up = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(picked_up, "worker must pick up the first job");

        // While job 1 is held in progress by the gate: job 2 fills the single
        // pending slot and job 3 replaces it, which must be counted as
        // dropped. Only then is job 1 released.
        worker.submit(tiny_ocr_job(2));
        worker.submit(tiny_ocr_job(3));
        gate.store(true, Ordering::SeqCst);

        for _ in 0..500 {
            let stats = worker.stats();
            if stats.submitted >= 3 && stats.dropped >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let stats = worker.stats();
        assert_eq!(stats.submitted, 3);
        assert_eq!(stats.dropped, 1);
    }

    #[test]
    fn ocr_worker_exposes_failed_result_without_panicking() {
        let worker = OcrWorker::spawn(|_job| Err("ocr failed".to_string()));

        worker.submit(tiny_ocr_job(77));

        let mut result = None;
        for _ in 0..500 {
            result = worker.poll_latest_result();
            if result.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let snapshot = result.unwrap_or_else(|| panic!("OCR worker did not expose failure"));
        assert_eq!(snapshot.source_frame_sequence, 77);
        assert_eq!(snapshot.tooltip_signature, 77);
        assert_eq!(snapshot.error.as_deref(), Some("ocr failed"));
        assert_eq!(snapshot.parity_path, "read-tooltip");

        let stats = worker.stats();
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.last_error.as_deref(), Some("ocr failed"));
    }

    /// Diagnostics contract: the worker records its own start and
    /// finish for the job's frame, with the measured job duration and the
    /// failure reason when the job failed.
    #[test]
    fn ocr_worker_records_start_and_finish_with_the_job_frame_and_duration() {
        let handle = DiagnosticsHandle::new(32, DiagnosticsVerbosity::Summary);
        let diagnostics = DiagnosticsContext::new(
            5,
            DiagnosticsClock::new(),
            handle.clone(),
            Revision::Applied(2),
            Revision::Unknown,
        );
        let worker = OcrWorker::spawn(|_job| {
            std::thread::sleep(std::time::Duration::from_millis(5));
            Ok(mock_worker_snapshot())
        });
        worker.attach_diagnostics(diagnostics);
        worker.submit(tiny_ocr_job(12));

        for _ in 0..500 {
            if worker.stats().completed >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let events = handle.drain();
        let start = events
            .iter()
            .find(|event| event.phase == DiagnosticsPhase::OcrStart)
            .unwrap_or_else(|| panic!("no ocr_start event: {events:?}"));
        assert_eq!(start.frame_id, 12);
        assert_eq!(start.session_id, 5);
        assert_eq!(start.config_revision, Revision::Applied(2));

        let finish = events
            .iter()
            .find(|event| event.phase == DiagnosticsPhase::OcrFinish)
            .unwrap_or_else(|| panic!("no ocr_finish event: {events:?}"));
        assert_eq!(finish.frame_id, 12);
        assert_eq!(finish.outcome, DiagnosticsOutcome::Ok);
        assert!(
            finish
                .duration_micros
                .is_some_and(|duration| duration >= 5_000),
            "the finish event must carry the measured job duration, got {:?}",
            finish.duration_micros
        );
        assert!(start.at_micros <= finish.at_micros);
    }

    #[test]
    fn ocr_worker_records_the_failure_reason_on_finish() {
        let handle = DiagnosticsHandle::new(32, DiagnosticsVerbosity::Summary);
        let diagnostics = DiagnosticsContext::new(
            6,
            DiagnosticsClock::new(),
            handle.clone(),
            Revision::Unknown,
            Revision::Unknown,
        );
        let worker = OcrWorker::spawn(|_job| Err("ocr failed".to_string()));
        worker.attach_diagnostics(diagnostics);
        worker.submit(tiny_ocr_job(3));

        for _ in 0..500 {
            if worker.stats().failed >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let events = handle.drain();
        let finish = events
            .iter()
            .find(|event| event.phase == DiagnosticsPhase::OcrFinish)
            .unwrap_or_else(|| panic!("no ocr_finish event: {events:?}"));
        assert_eq!(
            finish.outcome,
            DiagnosticsOutcome::failed("ocr failed"),
            "the worker's own failure reason travels into the event"
        );
    }

    /// The discarding seam that exists today: a pending job replaced by a
    /// fresher observation is reported with the `Superseded` reason.
    #[test]
    fn ocr_worker_records_the_replaced_pending_job_as_superseded() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::sync::Arc;

        let (sender, receiver) = mpsc::channel::<u64>();
        let gate = Arc::new(AtomicBool::new(false));
        let worker_gate = gate.clone();
        let worker = OcrWorker::spawn(move |job| {
            let _ = sender.send(job.frame_id());
            if job.frame_id() == 1 {
                while !worker_gate.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            Ok(mock_worker_snapshot())
        });
        let handle = DiagnosticsHandle::new(32, DiagnosticsVerbosity::Summary);
        worker.attach_diagnostics(DiagnosticsContext::new(
            7,
            DiagnosticsClock::new(),
            handle.clone(),
            Revision::Unknown,
            Revision::Unknown,
        ));

        // Job 1 is picked up and held; job 2 fills the pending slot, job 3
        // replaces it (the counted drop) — that replacement is the event.
        worker.submit(tiny_ocr_job(1));
        let mut picked_up = false;
        for _ in 0..500 {
            if receiver.try_recv() == Ok(1) {
                picked_up = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(picked_up, "worker must pick up the first job");
        worker.submit(tiny_ocr_job(2));
        worker.submit(tiny_ocr_job(3));

        let mut superseded = None;
        for _ in 0..500 {
            let events = handle.drain();
            if let Some(event) = events.iter().find(|event| {
                event.phase == DiagnosticsPhase::OcrSubmit
                    && event.outcome == DiagnosticsOutcome::discarded(DiscardReason::Superseded)
            }) {
                superseded = Some(event.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        gate.store(true, Ordering::SeqCst);

        let event = superseded.unwrap_or_else(|| panic!("no superseded event was recorded"));
        assert_eq!(
            event.frame_id, 2,
            "the event names the replaced job's frame"
        );
        assert_eq!(worker.stats().dropped, 1);
    }

    #[test]
    fn ocr_worker_without_worker_does_not_start() {
        // Verify that the worker is only created when explicitly spawned, i.e.
        // there is no implicit global worker.  This is a compile-time guarantee
        // but we document it as a test for CI clarity.
        let stats = OcrWorkerStats::default();
        assert_eq!(stats.submitted, 0);
        assert_eq!(stats.completed, 0);
    }

    /// The live debug-bundle gate: the job closure reads
    /// the shared flag the loop flips, and the worker defaults to off.
    #[test]
    fn ocr_worker_debug_bundle_gate_is_shared_with_the_job_closure() {
        let flag = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_closure = Arc::clone(&observed);
        let job_flag = Arc::clone(&flag);
        let worker = OcrWorker::spawn_with_debug_flag(
            move |_job| {
                observed_closure
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(job_flag.load(Ordering::Relaxed));
                Ok(mock_worker_snapshot())
            },
            Arc::clone(&flag),
        );

        assert!(!worker.debug_bundle_enabled(), "defaults to off");
        worker.submit(tiny_ocr_job(1));
        wait_completed(&worker, 1);
        worker.set_debug_bundle_enabled(true);
        assert!(worker.debug_bundle_enabled());
        worker.submit(tiny_ocr_job(2));
        wait_completed(&worker, 2);

        assert_eq!(
            *observed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec![false, true],
            "the closure reads the live flag for every job"
        );
    }

    fn wait_completed(worker: &OcrWorker, completed: u64) {
        for _ in 0..500 {
            if worker.stats().completed >= completed {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("worker did not complete {completed} jobs");
    }

    #[test]
    fn ocr_worker_tracks_busy_time_and_queue_depth() {
        let worker = OcrWorker::spawn(|_job| {
            std::thread::sleep(std::time::Duration::from_millis(30));
            Ok(mock_worker_snapshot())
        });

        // Submit job 1 and wait until the worker thread has picked it up.
        worker.submit(tiny_ocr_job(1));
        let mut picked_up = false;
        for _ in 0..500 {
            if worker.stats().in_progress {
                picked_up = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(picked_up, "worker must pick up the first job");

        // While job 1 is in progress: job 2 fills the single pending slot and
        // job 3 replaces it (counted as dropped). Queue depth reaches 2.
        worker.submit(tiny_ocr_job(2));
        worker.submit(tiny_ocr_job(3));

        for _ in 0..500 {
            let stats = worker.stats();
            if stats.completed.saturating_add(stats.dropped) >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let stats = worker.stats();
        assert_eq!(stats.submitted, 3);
        assert_eq!(stats.dropped, 1);
        assert_eq!(stats.max_queue_depth, 2);
        assert!(
            stats.total_busy_ms >= 50,
            "worker busy time must cover the executed jobs, got {}",
            stats.total_busy_ms
        );
        assert!(stats.max_busy_ms >= 30);
    }

    fn mock_worker_snapshot() -> TooltipTextSnapshot {
        TooltipTextSnapshot {
            sequence: 0,
            source_frame_sequence: 0,
            tooltip_signature: 0,
            content_fingerprint: 0,
            token: Default::default(),
            ocr_read_top: None,
            ocr_read_extent: None,
            parity_path: "read-tooltip".to_string(),
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
}
