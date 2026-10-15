//! App-owned bounded diagnostics sinks.
//!
//! The runtime crate emits structured events into a bounded, drop-counting
//! buffer ([`d4lc_runtime::DiagnosticsHandle`]). This module is the consumer
//! side and owns every durable sink:
//!
//! ```text
//! frame loop / OCR worker ──► DiagnosticsHandle (bounded, drop-counting)
//!                                     │ drain once per frame
//!                                     ▼
//!                          AppDiagnostics ──► UI ring      (2000 events, drop-oldest)
//!                                         ├─► log file    (5 files × 10 MiB, rotation)
//!                                         └─► detail dir  (opt-in, bounded file count)
//! ```
//!
//! Every sink is bounded and counts what it had to drop, so an event flood
//! cannot grow memory, disk usage or per-frame file counts. Detail artifacts
//! (the verbose OCR payload) are only written when the detail verbosity was
//! explicitly opted into; with the default verbosity the detail directory is
//! never created.
//!
//! Log location: `<config_root>/logs/` (the app's own configuration
//! directory), overridable with `D4LC_DIAGNOSTICS_DIR` for tests — the same
//! override pattern `crash_report` uses for `D4LC_CRASH_DIR`.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

#[cfg(test)]
use d4lc_runtime::diagnostics::DiagnosticsContext;
use d4lc_runtime::diagnostics::{
    DiagnosticsEvent, DiagnosticsHandle, DiagnosticsOutcome, DiagnosticsPhase, DiagnosticsStats,
    DiagnosticsVerbosity,
};

/// UI ring capacity: the newest events the UI can show without unbounded
/// growth.
pub const UI_RING_CAPACITY: usize = 2000;

/// Maximum size of one diagnostics log file before rotation.
pub const LOG_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Number of rotated diagnostics log files kept.
pub const LOG_FILE_COUNT: usize = 5;

/// Maximum number of detail artifact files per session (opt-in detail mode).
pub const DETAIL_ARTIFACT_LIMIT: usize = 64;

/// Environment override for the diagnostics directory (tests, development).
pub const DIAGNOSTICS_DIR_ENV_VAR: &str = "D4LC_DIAGNOSTICS_DIR";

/// Environment switch for the opt-in detail verbosity. Any non-empty value
/// other than `0` enables it.
pub const DIAGNOSTICS_DETAIL_ENV_VAR: &str = "D4LC_DIAGNOSTICS_DETAIL";

/// Log subdirectory below the config root.
pub const LOG_DIR_NAME: &str = "logs";

/// Detail artifact subdirectory below the diagnostics directory.
pub const DETAIL_DIR_NAME: &str = "detail";

/// Prefix / extension of a diagnostics log file (`diagnostics-000.jsonl`).
const LOG_FILE_PREFIX: &str = "diagnostics-";
const LOG_FILE_SUFFIX: &str = ".jsonl";

/// Detail artifact file prefix (`ocr-0000.txt`).
const DETAIL_FILE_PREFIX: &str = "ocr-";

/// The verbosity the process was started with.
pub fn verbosity_from_env() -> DiagnosticsVerbosity {
    let enabled = std::env::var(DIAGNOSTICS_DETAIL_ENV_VAR)
        .map(|value| !value.trim().is_empty() && value.trim() != "0")
        .unwrap_or(false);
    DiagnosticsVerbosity::from_detail_enabled(enabled)
}

/// Default diagnostics directory: `<config_root>/logs`.
pub fn default_diagnostics_dir(config_root: &Path) -> PathBuf {
    config_root.join(LOG_DIR_NAME)
}

/// Resolve the diagnostics directory: `D4LC_DIAGNOSTICS_DIR` wins (tests and
/// development), otherwise `<config_root>/logs`.
pub fn resolve_diagnostics_dir(config_root: &Path) -> PathBuf {
    if let Ok(override_dir) = std::env::var(DIAGNOSTICS_DIR_ENV_VAR) {
        if !override_dir.trim().is_empty() {
            return PathBuf::from(override_dir);
        }
    }
    default_diagnostics_dir(config_root)
}

/// Sink bounds. Production uses the documented constants; tests shrink them to
/// prove the bounds without writing tens of megabytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticsLimits {
    pub ring_capacity: usize,
    pub file_bytes: u64,
    pub file_count: usize,
    pub detail_artifact_limit: usize,
}

impl Default for DiagnosticsLimits {
    fn default() -> Self {
        Self {
            ring_capacity: UI_RING_CAPACITY,
            file_bytes: LOG_FILE_BYTES,
            file_count: LOG_FILE_COUNT,
            detail_artifact_limit: DETAIL_ARTIFACT_LIMIT,
        }
    }
}

/// Events each bounded sink had to drop.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticsDrops {
    /// Dropped by the runtime buffer before the app could drain it.
    pub runtime: u64,
    /// Dropped by the UI ring (drop-oldest).
    pub ring: u64,
    /// Not written to the log file (sink unavailable or write failure).
    pub file: u64,
    /// Not written as detail artifacts (count bound reached).
    pub detail: u64,
}

#[cfg(test)]
impl DiagnosticsDrops {
    /// Total of every sink.
    #[cfg(test)]
    pub const fn total(self) -> u64 {
        self.runtime
            .saturating_add(self.ring)
            .saturating_add(self.file)
            .saturating_add(self.detail)
    }
}

/// State of the bounded file sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSinkStatus {
    /// Writing into `<dir>` with rotation.
    Active { path: PathBuf },
    /// Could not be opened; the reason is reported instead of being swallowed.
    Unavailable { reason: String },
}

impl FileSinkStatus {
    /// Whether the file sink writes.
    #[cfg(test)]
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

struct ActiveFile {
    path: PathBuf,
    writer: BufWriter<File>,
    bytes: u64,
}

/// One existing log file of the rotation set.
struct LogFileMeta {
    index: usize,
    size: u64,
    modified: std::time::SystemTime,
}

/// Bounded, rotating JSONL log sink.
struct FileSink {
    dir: PathBuf,
    limits: DiagnosticsLimits,
    active: Option<ActiveFile>,
    next_index: usize,
    dropped: u64,
    status: FileSinkStatus,
}

impl FileSink {
    fn open(dir: PathBuf, limits: DiagnosticsLimits) -> Self {
        match Self::open_active(&dir, limits) {
            Ok((active, next_index)) => {
                let path = active.path.clone();
                Self {
                    dir,
                    limits,
                    active: Some(active),
                    next_index,
                    dropped: 0,
                    status: FileSinkStatus::Active { path },
                }
            }
            Err(reason) => Self {
                dir,
                limits,
                active: None,
                next_index: 0,
                dropped: 0,
                status: FileSinkStatus::Unavailable { reason },
            },
        }
    }

    /// Continue the most recently written file when it still has room,
    /// otherwise start the next rotation slot (truncating the oldest one), so
    /// a restart keeps the history that still fits instead of wiping a slot.
    fn open_active(dir: &Path, limits: DiagnosticsLimits) -> Result<(ActiveFile, usize), String> {
        fs::create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
        let existing = list_log_files(dir);
        let newest = existing
            .iter()
            .max_by_key(|file| (file.modified, file.index));
        let (index, append) = match newest {
            Some(file) if file.size < limits.file_bytes => (file.index, true),
            Some(file) => ((file.index + 1) % limits.file_count.max(1), false),
            None => (0, false),
        };
        let path = log_path(dir, index);
        let file = if append {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| format!("open {}: {error}", path.display()))?
        } else {
            File::create(&path).map_err(|error| format!("create {}: {error}", path.display()))?
        };
        let bytes = if append {
            fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0)
        } else {
            0
        };
        Ok((
            ActiveFile {
                path,
                writer: BufWriter::new(file),
                bytes,
            },
            (index + 1) % limits.file_count.max(1),
        ))
    }

    fn record(&mut self, line: &str) {
        let Some(active) = self.active.as_mut() else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        let line_bytes = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if active.bytes > 0 && active.bytes.saturating_add(line_bytes) > self.limits.file_bytes {
            // Rotation: the next file in order is truncated, so the retained
            // window stays bounded at `file_count * file_bytes`.
            let next_index = self.next_index;
            match Self::rotate(&self.dir, &mut self.active, next_index, &self.limits) {
                Ok(()) => {
                    self.next_index = (next_index + 1) % self.limits.file_count.max(1);
                    if let Some(active) = self.active.as_ref() {
                        self.status = FileSinkStatus::Active {
                            path: active.path.clone(),
                        };
                    }
                }
                Err(reason) => {
                    self.status = FileSinkStatus::Unavailable { reason };
                    self.dropped = self.dropped.saturating_add(1);
                    return;
                }
            }
        }
        let Some(active) = self.active.as_mut() else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        if active.writer.write_all(line.as_bytes()).is_err() {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        active.bytes = active.bytes.saturating_add(line_bytes);
    }

    fn rotate(
        dir: &Path,
        active: &mut Option<ActiveFile>,
        index: usize,
        limits: &DiagnosticsLimits,
    ) -> Result<(), String> {
        if let Some(mut current) = active.take() {
            current
                .writer
                .flush()
                .map_err(|error| format!("flush {}: {error}", current.path.display()))?;
        }
        let path = log_path(dir, index % limits.file_count.max(1));
        let file =
            File::create(&path).map_err(|error| format!("create {}: {error}", path.display()))?;
        *active = Some(ActiveFile {
            path,
            writer: BufWriter::new(file),
            bytes: 0,
        });
        Ok(())
    }

    fn flush(&mut self) {
        if let Some(active) = self.active.as_mut() {
            let _ = active.writer.flush();
        }
    }

    fn status(&self) -> &FileSinkStatus {
        &self.status
    }
}

/// The existing `diagnostics-<index>.jsonl` files in `dir`.
fn list_log_files(dir: &Path) -> Vec<LogFileMeta> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let index = log_file_index(&name)?;
            let metadata = entry.metadata().ok()?;
            Some(LogFileMeta {
                index,
                size: metadata.len(),
                modified: metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
            })
        })
        .collect()
}

/// Parse the rotation index out of a log file name; `None` for foreign files.
fn log_file_index(name: &str) -> Option<usize> {
    let digits = name
        .strip_prefix(LOG_FILE_PREFIX)?
        .strip_suffix(LOG_FILE_SUFFIX)?;
    if digits.len() != 3 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Path of one rotation slot.
fn log_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("{LOG_FILE_PREFIX}{index:03}{LOG_FILE_SUFFIX}"))
}

/// Bounded detail artifact sink (opt-in verbosity only).
struct DetailSink {
    dir: PathBuf,
    verbosity: DiagnosticsVerbosity,
    limit: usize,
    written: usize,
    dropped: u64,
}

impl DetailSink {
    fn new(diagnostics_dir: &Path, verbosity: DiagnosticsVerbosity, limit: usize) -> Self {
        Self {
            dir: diagnostics_dir.join(DETAIL_DIR_NAME),
            verbosity,
            limit,
            written: 0,
            dropped: 0,
        }
    }

    /// Write one detail artifact for an OCR-detail event, when detail
    /// diagnostics are enabled and the file bound is not reached. With the
    /// default verbosity nothing is written and the directory is not created.
    fn record(&mut self, event: &DiagnosticsEvent) -> bool {
        if !self.verbosity.records_detail() {
            return false;
        }
        let Some(detail) = event.detail.as_deref() else {
            return false;
        };
        if !matches!(
            event.phase,
            DiagnosticsPhase::OcrStart | DiagnosticsPhase::OcrFinish | DiagnosticsPhase::Parse
        ) {
            return false;
        }
        if self.written >= self.limit {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        if fs::create_dir_all(&self.dir).is_err() {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let path = self
            .dir
            .join(format!("{DETAIL_FILE_PREFIX}{:04}.txt", self.written));
        let body = format!("{}\ndetail: {detail}\n", event.one_line());
        if fs::write(&path, body).is_err() {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.written = self.written.saturating_add(1);
        true
    }
}

/// The app's diagnostics record: bounded ring, bounded log file and the opt-in
/// bounded detail artifacts, plus the aggregate statistics.
pub struct AppDiagnostics {
    dir: PathBuf,
    verbosity: DiagnosticsVerbosity,
    limits: DiagnosticsLimits,
    handle: DiagnosticsHandle,
    ring: VecDeque<DiagnosticsEvent>,
    ring_dropped: u64,
    file: FileSink,
    detail: DetailSink,
    stats: DiagnosticsStats,
    runtime_dropped: u64,
    /// Per-tick timing percentiles, set by the live loop before
    /// the aggregate is written. `None` keeps the aggregate unchanged.
    loop_timings: Option<String>,
}

impl AppDiagnostics {
    /// Start the app diagnostics for a live runtime rooted at `config_root`.
    pub fn start(config_root: &Path, verbosity: DiagnosticsVerbosity) -> Self {
        Self::with_limits(
            resolve_diagnostics_dir(config_root),
            verbosity,
            DiagnosticsLimits::default(),
        )
    }

    /// Start with explicit bounds (tests shrink them).
    pub fn with_limits(
        dir: PathBuf,
        verbosity: DiagnosticsVerbosity,
        limits: DiagnosticsLimits,
    ) -> Self {
        let file = FileSink::open(dir.clone(), limits);
        let detail = DetailSink::new(&dir, verbosity, limits.detail_artifact_limit);
        Self {
            dir,
            verbosity,
            limits,
            handle: DiagnosticsHandle::new(d4lc_runtime::DEFAULT_EVENT_CAPACITY, verbosity),
            ring: VecDeque::new(),
            ring_dropped: 0,
            file,
            detail,
            stats: DiagnosticsStats::default(),
            runtime_dropped: 0,
            loop_timings: None,
        }
    }

    /// The runtime event buffer this record drains. Producers (the runtime
    /// session and its OCR worker) emit into this handle.
    pub fn handle(&self) -> &DiagnosticsHandle {
        &self.handle
    }

    /// Configured verbosity.
    pub const fn verbosity(&self) -> DiagnosticsVerbosity {
        self.verbosity
    }

    /// Diagnostics directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// State of the bounded file sink (names the reason when it is down).
    pub fn file_sink_status(&self) -> &FileSinkStatus {
        self.file.status()
    }

    /// Record one event into every bounded sink.
    pub fn record(&mut self, event: DiagnosticsEvent) {
        self.stats.observe(&event);
        self.detail.record(&event);
        self.file.record(&render_event_line(&event));
        if self.ring.len() >= self.limits.ring_capacity {
            self.ring.pop_front();
            self.ring_dropped = self.ring_dropped.saturating_add(1);
        }
        self.ring.push_back(event);
    }

    /// Drain the runtime buffer into the app sinks. Returns the event count.
    pub fn drain_runtime(&mut self, handle: &DiagnosticsHandle) -> usize {
        let events = handle.drain();
        self.runtime_dropped = handle.dropped_events();
        for event in &events {
            self.record(event.clone());
        }
        events.len()
    }

    /// The bounded UI ring, oldest first.
    pub fn ring(&self) -> &VecDeque<DiagnosticsEvent> {
        &self.ring
    }

    /// Aggregate statistics of the recorded events.
    #[cfg(test)]
    pub fn stats(&self) -> &DiagnosticsStats {
        &self.stats
    }

    /// Every sink's drop counters.
    #[cfg(test)]
    pub fn dropped(&self) -> DiagnosticsDrops {
        DiagnosticsDrops {
            runtime: self.runtime_dropped,
            ring: self.ring_dropped,
            file: self.file.dropped,
            detail: self.detail.dropped,
        }
    }

    /// Number of detail artifact files written so far.
    #[cfg(test)]
    pub const fn detail_artifacts_written(&self) -> usize {
        self.detail.written
    }

    /// Export line of one frame's chain, reconstructed from the recorded
    /// events (the ring holds the newest events; older frames fall out of the
    /// window, which is what "bounded" means).
    #[cfg(test)]
    pub fn frame_chain(&self, session_id: u64, frame_id: u64) -> Option<String> {
        let events: Vec<DiagnosticsEvent> = self.ring.iter().cloned().collect();
        d4lc_runtime::diagnostics::render_frame_chain(&events, session_id, frame_id)
    }

    /// Set the per-tick timing percentile line exported with the
    /// aggregate record. `None` (the default) keeps the aggregate unchanged.
    pub fn set_loop_timings(&mut self, loop_timings: Option<String>) {
        self.loop_timings = loop_timings;
    }

    /// Flush the log file and write the session's aggregate record.
    ///
    /// The aggregate is the default-verbosity statistics record: per-phase and
    /// per-discard-reason counts plus every sink's drop counters.
    pub fn finish(&mut self, reason: &str) {
        let record = AggregateRecord {
            record: "aggregate",
            reason,
            stats: self.stats.render(),
            dropped_runtime: self.runtime_dropped,
            dropped_ring: self.ring_dropped,
            dropped_file: self.file.dropped,
            dropped_detail: self.detail.dropped,
            loop_timings_us_p50_p95_p99: self.loop_timings.as_deref(),
        };
        self.file.record(&json_line(&record));
        self.file.flush();
    }
}

/// The session's aggregate statistics record.
#[derive(Serialize)]
struct AggregateRecord<'a> {
    record: &'static str,
    reason: &'a str,
    stats: String,
    dropped_runtime: u64,
    dropped_ring: u64,
    dropped_file: u64,
    dropped_detail: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    loop_timings_us_p50_p95_p99: Option<&'a str>,
}

/// One event as a JSONL record.
#[derive(Serialize)]
struct EventRecord<'a> {
    record: &'static str,
    session: u64,
    frame: u64,
    generation: u64,
    config_rev: String,
    rules_rev: String,
    phase: &'static str,
    at_us: u64,
    wall_unix_ms: u64,
    duration_us: Option<u64>,
    outcome: &'static str,
    reason: Option<&'static str>,
    message: Option<&'a str>,
    detail: Option<&'a str>,
}

/// Render one event as a single JSONL record.
fn render_event_line(event: &DiagnosticsEvent) -> String {
    let (outcome, reason, message) = match &event.outcome {
        DiagnosticsOutcome::Ok => ("ok", None, None),
        DiagnosticsOutcome::Failed { message } => ("failed", None, Some(message.as_str())),
        DiagnosticsOutcome::Discarded { reason } => ("discarded", Some(reason.as_str()), None),
    };
    json_line(&EventRecord {
        record: "event",
        session: event.session_id,
        frame: event.frame_id,
        generation: event.tooltip_generation,
        config_rev: event.config_revision.as_str(),
        rules_rev: event.rules_revision.as_str(),
        phase: event.phase.as_str(),
        at_us: event.at_micros,
        wall_unix_ms: event.wall_unix_ms,
        duration_us: event.duration_micros,
        outcome,
        reason,
        message,
        detail: event.detail.as_deref(),
    })
}

/// `record` as one JSON line. The records hold only strings and integers,
/// so serialization cannot fail.
fn json_line(record: &impl Serialize) -> String {
    let mut line = serde_json::to_string(record).unwrap_or_default();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use d4lc_runtime::diagnostics::{
        DiagnosticsClock, DiagnosticsOutcome, DiscardReason, Revision,
    };

    /// Test directories live under the workspace target directory (never the
    /// real user home); every test cleans up after itself.
    fn test_dir(label: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-diagnostics-dirs")
            .join(format!("{label}-{}", std::process::id()))
    }

    fn fixture(label: &str, limits: DiagnosticsLimits) -> (AppDiagnostics, DiagnosticsClock) {
        let dir = test_dir(label);
        let _ = fs::remove_dir_all(&dir);
        let diagnostics = AppDiagnostics::with_limits(dir, DiagnosticsVerbosity::Summary, limits);
        assert!(
            diagnostics.file_sink_status().is_active(),
            "file sink must open: {:?}",
            diagnostics.file_sink_status()
        );
        (diagnostics, DiagnosticsClock::new())
    }

    fn event(
        clock: &DiagnosticsClock,
        session: u64,
        frame: u64,
        phase: DiagnosticsPhase,
        outcome: DiagnosticsOutcome,
    ) -> DiagnosticsEvent {
        DiagnosticsEvent {
            session_id: session,
            frame_id: frame,
            tooltip_generation: 1,
            config_revision: Revision::Applied(1),
            rules_revision: Revision::Unknown,
            phase,
            at_micros: clock.now_micros(),
            // Fixed wall time: the record is asserted on content, not on when
            // the test ran.
            wall_unix_ms: 1_700_000_000_000,
            duration_micros: None,
            first_seen_at_micros: None,
            outcome,
            detail: None,
        }
    }

    #[test]
    fn event_lines_are_escaped_json_records() {
        let clock = DiagnosticsClock::new();
        let line = render_event_line(&event(
            &clock,
            3,
            7,
            DiagnosticsPhase::Detect,
            DiagnosticsOutcome::Failed {
                message: "bad \"frame\"\n".to_string(),
            },
        ));
        assert!(line.ends_with('\n'));
        let record: serde_json::Value =
            serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(record["record"], "event");
        assert_eq!(record["frame"], 7);
        assert_eq!(record["outcome"], "failed");
        assert_eq!(record["message"], "bad \"frame\"\n");
        assert!(record["reason"].is_null());
    }

    fn log_files(dir: &Path) -> Vec<(String, u64)> {
        let mut files: Vec<(String, u64)> = match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .file_type()
                        .map(|kind| kind.is_file())
                        .unwrap_or(false)
                })
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    log_file_index(&name)?;
                    let size = entry.metadata().ok()?.len();
                    Some((name, size))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort_unstable();
        files
    }

    fn artifact_count(dir: &Path) -> usize {
        match fs::read_dir(dir.join(DETAIL_DIR_NAME)) {
            Ok(entries) => entries.filter_map(|entry| entry.ok()).count(),
            Err(_) => 0,
        }
    }

    /// Bounds contract: a synthetic event/error flood
    /// grows neither the ring nor the file set beyond their bounds, and every
    /// dropped event is counted.
    #[test]
    fn event_flood_keeps_ring_and_files_bounded_and_counts_drops() {
        let limits = DiagnosticsLimits {
            ring_capacity: 64,
            file_bytes: 2048,
            file_count: 3,
            detail_artifact_limit: 4,
        };
        let (mut diagnostics, clock) = fixture("flood-bounds", limits);
        let dir = diagnostics.dir().to_path_buf();

        for frame in 0..2000u64 {
            let outcome = if frame % 2 == 0 {
                DiagnosticsOutcome::Ok
            } else {
                DiagnosticsOutcome::discarded(DiscardReason::Superseded)
            };
            for phase in [DiagnosticsPhase::Capture, DiagnosticsPhase::Detect] {
                diagnostics.record(event(&clock, 1, frame, phase, outcome.clone()));
            }
        }

        assert_eq!(
            diagnostics.ring().len(),
            limits.ring_capacity,
            "the ring keeps exactly its capacity"
        );
        // Oldest-first retention: the newest frames survive a flood.
        let newest_frame = diagnostics
            .ring()
            .back()
            .map(|event| event.frame_id)
            .unwrap_or(u64::MAX);
        assert_eq!(newest_frame, 1999);

        let files = log_files(&dir);
        assert_eq!(
            files.len(),
            limits.file_count,
            "rotation keeps exactly the configured file count: {files:?}"
        );
        for (name, size) in &files {
            assert!(
                *size <= limits.file_bytes,
                "{name} exceeds the per-file bound: {size} bytes"
            );
        }
        let total: u64 = files.iter().map(|(_, size)| *size).sum();
        assert!(
            total <= limits.file_bytes * u64::try_from(limits.file_count).unwrap_or(1),
            "total log usage must stay bounded, got {total} bytes"
        );

        let dropped = diagnostics.dropped();
        assert!(
            dropped.ring > 0,
            "the ring must report the events it dropped"
        );
        assert_eq!(
            diagnostics.stats().recorded,
            4000,
            "every event is counted by the aggregate, dropped or not"
        );
        assert_eq!(
            diagnostics.stats().discarded_by_reason[DiscardReason::Superseded.index()],
            2000
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The runtime buffer in front of the app is bounded too: a producer that
    /// outruns the drain loses oldest events and counts them instead of
    /// growing.
    #[test]
    fn runtime_buffer_overflow_is_counted_not_grown() {
        let limits = DiagnosticsLimits {
            ring_capacity: 16,
            file_bytes: 1024 * 1024,
            file_count: 1,
            detail_artifact_limit: 1,
        };
        let (mut diagnostics, _clock) = fixture("runtime-buffer", limits);
        let dir = diagnostics.dir().to_path_buf();

        let handle = DiagnosticsHandle::new(4, DiagnosticsVerbosity::Summary);
        let context = DiagnosticsContext::new(
            9,
            DiagnosticsClock::new(),
            handle.clone(),
            Revision::Applied(1),
            Revision::Unknown,
        );
        for frame in 0..50u64 {
            context.record_outcome(frame, DiagnosticsPhase::Capture, DiagnosticsOutcome::Ok);
        }
        assert_eq!(handle.dropped_events(), 46);

        let drained = diagnostics.drain_runtime(&handle);
        assert_eq!(drained, 4, "only the retained events arrive");
        assert_eq!(diagnostics.dropped().runtime, 46);
        assert_eq!(diagnostics.dropped().total(), 46);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Detail diagnostics are opt-in: with the default
    /// verbosity nothing is written into the detail directory, not even for
    /// events that carry a detail payload.
    #[test]
    fn detail_disabled_writes_no_artifact_files() {
        let limits = DiagnosticsLimits {
            detail_artifact_limit: 4,
            ..DiagnosticsLimits::default()
        };
        let (mut diagnostics, clock) = fixture("detail-off", limits);
        let dir = diagnostics.dir().to_path_buf();

        for frame in 0..20u64 {
            let mut ocr = event(
                &clock,
                1,
                frame,
                DiagnosticsPhase::OcrFinish,
                DiagnosticsOutcome::Ok,
            );
            ocr.detail = Some("segments=3 lines=12 text=Synthetic Helm".to_string());
            diagnostics.record(ocr);
        }

        assert_eq!(diagnostics.detail_artifacts_written(), 0);
        assert_eq!(diagnostics.dropped().detail, 0);
        assert_eq!(artifact_count(&dir), 0, "no detail files may exist");
        assert!(
            !dir.join(DETAIL_DIR_NAME).exists(),
            "the detail directory must not even be created"
        );
        // The bounded log still records the events themselves.
        assert!(!log_files(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// With detail diagnostics enabled the artifacts are written but bounded:
    /// the limit is a hard ceiling and every suppressed artifact is counted.
    #[test]
    fn detail_enabled_writes_bounded_artifacts_and_counts_the_drops() {
        let limits = DiagnosticsLimits {
            detail_artifact_limit: 3,
            ..DiagnosticsLimits::default()
        };
        let dir = test_dir("detail-on");
        let _ = fs::remove_dir_all(&dir);
        let mut diagnostics =
            AppDiagnostics::with_limits(dir.clone(), DiagnosticsVerbosity::Detail, limits);
        let clock = DiagnosticsClock::new();

        for frame in 0..10u64 {
            let mut ocr = event(
                &clock,
                1,
                frame,
                DiagnosticsPhase::OcrFinish,
                DiagnosticsOutcome::Ok,
            );
            ocr.detail = Some(format!("segments=3 lines=12 frame={frame}"));
            diagnostics.record(ocr);
        }

        assert_eq!(diagnostics.detail_artifacts_written(), 3);
        assert_eq!(artifact_count(&dir), 3);
        assert_eq!(diagnostics.dropped().detail, 7);
        let _ = fs::remove_dir_all(&dir);
    }

    /// An unusable log directory is reported and counted, never silently
    /// swallowed: the events survive in the ring.
    #[test]
    fn unavailable_file_sink_is_reported_and_counted() {
        let dir = test_dir("sink-unavailable");
        let _ = fs::remove_dir_all(&dir);
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| panic!("fixture: {error}"));
        }
        // A file where the directory should be: every open below it fails.
        fs::write(&dir, "not a directory").unwrap_or_else(|error| panic!("fixture: {error}"));

        let mut diagnostics = AppDiagnostics::with_limits(
            dir.join("logs"),
            DiagnosticsVerbosity::Summary,
            DiagnosticsLimits::default(),
        );

        match diagnostics.file_sink_status() {
            FileSinkStatus::Unavailable { reason } => assert!(!reason.is_empty()),
            other => panic!("expected an unavailable sink, got {other:?}"),
        }

        let clock = DiagnosticsClock::new();
        for frame in 0..5u64 {
            diagnostics.record(event(
                &clock,
                1,
                frame,
                DiagnosticsPhase::Capture,
                DiagnosticsOutcome::Ok,
            ));
        }

        assert_eq!(diagnostics.dropped().file, 5);
        assert_eq!(diagnostics.ring().len(), 5, "the ring still records");
        diagnostics.finish("stopped");
        // The fixture is a file at the directory's place, so the cleanup
        // removes a file here (not a directory).
        let _ = fs::remove_file(&dir);
    }

    /// Chain contract: one processed frame's chain —
    /// capture → detect → submit → OCR start/finish → parse — is reconstructed
    /// from the recorded events, and a discarded frame keeps its reason.
    #[test]
    fn recorded_events_reconstruct_a_frame_chain() {
        let (mut diagnostics, clock) = fixture("frame-chain", DiagnosticsLimits::default());
        let dir = diagnostics.dir().to_path_buf();
        let handle = diagnostics.handle().clone();
        let session = 4;
        let context = DiagnosticsContext::new(
            session,
            clock.clone(),
            handle.clone(),
            Revision::Applied(2),
            Revision::Applied(7),
        );

        // Frame 5: the observation chain including the worker's start/finish
        // and the adoption parse, deliberately recorded in the order the two
        // threads can produce (worker first).
        let mut events = vec![
            context.begin(5, DiagnosticsPhase::OcrFinish),
            context.begin(5, DiagnosticsPhase::OcrStart),
            context.begin(5, DiagnosticsPhase::Capture),
            context.begin(5, DiagnosticsPhase::Detect),
            context.begin(5, DiagnosticsPhase::OcrSubmit),
            context.begin(5, DiagnosticsPhase::Parse),
        ];
        // Monotonic timestamps in chain order; the buffer order above is not.
        let stamp = |event: &mut DiagnosticsEvent, at: u64| event.at_micros = at;
        stamp(&mut events[0], 5_000);
        stamp(&mut events[1], 4_000);
        stamp(&mut events[2], 1_000);
        stamp(&mut events[3], 2_000);
        stamp(&mut events[4], 3_000);
        stamp(&mut events[5], 6_000);
        for event in events {
            context.record(event);
        }

        // Frame 6: the tooltip was lost.
        let mut lost = context.begin(6, DiagnosticsPhase::Detect);
        lost.at_micros = 7_000;
        let lost = lost.with_outcome(DiagnosticsOutcome::discarded(DiscardReason::TooltipLost));
        context.record(lost);

        assert_eq!(diagnostics.drain_runtime(&handle), 7);

        let chain = diagnostics
            .frame_chain(session, 5)
            .unwrap_or_else(|| panic!("frame 5 chain missing"));
        assert!(
            chain.contains(
                "chain=capture:ok -> detect:ok -> ocr_submit:ok -> ocr_start:ok -> \
                 ocr_finish:ok -> parse:ok"
            ),
            "unexpected chain: {chain}"
        );
        assert!(chain.contains("first_captured_at=1000"));
        assert!(chain.contains("draw_completed_at=none"));
        // The applied revisions travel on the events themselves.
        assert_eq!(
            diagnostics
                .ring()
                .front()
                .map(|event| (event.config_revision, event.rules_revision)),
            Some((Revision::Applied(2), Revision::Applied(7)))
        );

        let lost_chain = diagnostics
            .frame_chain(session, 6)
            .unwrap_or_else(|| panic!("frame 6 chain missing"));
        assert!(lost_chain.contains("chain=detect:discarded(TooltipLost)"));
        assert!(lost_chain.contains("discard=TooltipLost"));
        assert!(diagnostics.frame_chain(session, 99).is_none());

        // The aggregate is written on finish and carries the drop counters.
        diagnostics.finish("stopped");
        let content = fs::read_to_string(log_path(&dir, 0)).unwrap_or_default();
        assert!(content.contains("\"record\":\"aggregate\""));
        assert!(content.contains("discard.TooltipLost=1"));
        assert!(content.contains("\"reason\":\"stopped\""));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The production bounds are the documented budget, not test values.
    #[test]
    fn production_limits_match_the_documented_budget() {
        let limits = DiagnosticsLimits::default();
        assert_eq!(limits.ring_capacity, 2000);
        assert_eq!(limits.file_bytes, 10 * 1024 * 1024);
        assert_eq!(limits.file_count, 5);
    }

    /// The feature-gated state matrix of the detail switch: absent, disabled
    /// and enabled.
    #[test]
    fn detail_verbosity_follows_the_environment_switch() {
        let previous = std::env::var(DIAGNOSTICS_DETAIL_ENV_VAR).ok();
        std::env::remove_var(DIAGNOSTICS_DETAIL_ENV_VAR);
        assert_eq!(verbosity_from_env(), DiagnosticsVerbosity::Summary);
        std::env::set_var(DIAGNOSTICS_DETAIL_ENV_VAR, "0");
        assert_eq!(verbosity_from_env(), DiagnosticsVerbosity::Summary);
        std::env::set_var(DIAGNOSTICS_DETAIL_ENV_VAR, "1");
        assert_eq!(verbosity_from_env(), DiagnosticsVerbosity::Detail);
        match previous {
            Some(value) => std::env::set_var(DIAGNOSTICS_DETAIL_ENV_VAR, value),
            None => std::env::remove_var(DIAGNOSTICS_DETAIL_ENV_VAR),
        }
    }

    /// Instrumentation overhead: measure the per-event cost of the bounded
    /// record path (ring + log line + aggregate) and report it. The assertion
    /// is a coarse ceiling that catches a pathological regression, not a
    /// performance claim.
    #[test]
    fn record_overhead_per_event_stays_bounded() {
        let (mut diagnostics, clock) = fixture("overhead", DiagnosticsLimits::default());
        let dir = diagnostics.dir().to_path_buf();
        let iterations = 20_000u64;

        let started = std::time::Instant::now();
        for frame in 0..iterations {
            diagnostics.record(event(
                &clock,
                1,
                frame,
                DiagnosticsPhase::Capture,
                DiagnosticsOutcome::Ok,
            ));
        }
        let elapsed = started.elapsed();
        let per_event_ns = elapsed.as_nanos() / u128::from(iterations);
        println!(
            "diagnostics record overhead: {per_event_ns} ns/event over {iterations} events \
             (ring + log line + aggregate)"
        );

        assert!(
            per_event_ns < 200_000,
            "per-event record cost is implausibly high: {per_event_ns} ns"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
