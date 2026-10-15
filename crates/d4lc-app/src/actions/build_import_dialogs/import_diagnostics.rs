//! Per-import diagnostics: one JSON line per finished, failed or cancelled
//! build import, with the measured phases.
//!
//! The line goes to stderr and is appended to
//! `<diagnostics dir>/build-import.jsonl` (the runtime diagnostics
//! directory, `D4LC_DIAGNOSTICS_DIR` override included). The file is
//! bounded: past [`LOG_MAX_BYTES`] it is rotated to `build-import.1.jsonl`
//! (one generation kept). Only public build URLs / ids, provider names,
//! error messages and durations are written — no cookies, tokens or
//! browser profile data.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::actions::build_download::{ImportError, PhaseTiming};

/// Rotation bound of the import diagnostics file.
const LOG_MAX_BYTES: u64 = 1024 * 1024;
const LOG_FILE: &str = "build-import.jsonl";
const LOG_FILE_ROTATED: &str = "build-import.1.jsonl";

/// One import job's diagnostics record.
pub(super) struct ImportRecord<'a> {
    pub job_id: u64,
    pub provider: &'a str,
    pub subject: &'a str,
    /// The stored build's name, or the failure (`Cancelled` included).
    pub result: Result<&'a str, &'a ImportError>,
    pub phases: &'a [PhaseTiming],
    /// Worker completion → build list refreshed on the UI thread.
    pub ui_finish: Option<Duration>,
    /// Add click → job finished on the UI thread.
    pub total: Duration,
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

/// The JSON line of `record`.
pub(super) fn import_record_json(record: &ImportRecord<'_>, unix_ms: u128) -> serde_json::Value {
    let mut phases = serde_json::Map::new();
    for timing in record.phases {
        // A phase measured twice (e.g. two page loads after a bot check)
        // is summed.
        let total = phases
            .get(timing.phase)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
            + millis(timing.duration);
        phases.insert(timing.phase.to_string(), serde_json::json!(total));
    }
    if let Some(ui_finish) = record.ui_finish {
        phases.insert(
            "ui_finish".to_string(),
            serde_json::json!(millis(ui_finish)),
        );
    }
    let (outcome, kind, message, name) = match record.result {
        Ok(name) => ("stored", None, None, Some(name)),
        Err(error) if error.kind == crate::actions::build_download::ImportErrorKind::Cancelled => {
            ("cancelled", Some(error.kind.code()), None, None)
        }
        Err(error) => (
            "failed",
            Some(error.kind.code()),
            Some(error.message.as_str()),
            None,
        ),
    };
    serde_json::json!({
        "event": "build_import",
        "unix_ms": unix_ms,
        "job_id": record.job_id,
        "provider": record.provider,
        "subject": record.subject,
        "outcome": outcome,
        "error_kind": kind,
        "error": message,
        "build_name": name,
        "phases_ms": phases,
        "total_ms": millis(record.total),
    })
}

/// Write `record` to stderr and the bounded diagnostics file. A failing
/// file write is reported on stderr only: diagnostics never fail an import.
pub(super) fn record_import(record: &ImportRecord<'_>) {
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let line = import_record_json(record, unix_ms).to_string();
    eprintln!("[d4lc] import diagnostics: {line}");
    let dir = crate::diagnostics::resolve_diagnostics_dir(
        &crate::actions::settings::resolve_config_root(),
    );
    if let Err(error) = append_line(&dir, &line) {
        eprintln!(
            "[d4lc] import diagnostics: cannot write {}: {error}",
            dir.join(LOG_FILE).display()
        );
    }
}

fn append_line(dir: &Path, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() >= LOG_MAX_BYTES) {
        std::fs::rename(&path, dir.join(LOG_FILE_ROTATED))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::actions::build_download::ImportErrorKind;

    #[test]
    fn the_record_names_the_job_its_outcome_and_every_phase() {
        let phases = [
            PhaseTiming {
                phase: "browser_launch",
                duration: Duration::from_millis(420),
            },
            PhaseTiming {
                phase: "page_load",
                duration: Duration::from_millis(1000),
            },
            PhaseTiming {
                phase: "page_load",
                duration: Duration::from_millis(500),
            },
        ];
        let error = ImportError::new(ImportErrorKind::Timeout, "no planner data");
        let value = import_record_json(
            &ImportRecord {
                job_id: 7,
                provider: "D2Core",
                subject: "https://www.d2core.com/d4/planner?bd=23tb",
                result: Err(&error),
                phases: &phases,
                ui_finish: Some(Duration::from_millis(12)),
                total: Duration::from_millis(2100),
            },
            1,
        );
        assert_eq!(value["job_id"], 7);
        assert_eq!(value["outcome"], "failed");
        assert_eq!(value["error_kind"], "timeout");
        assert_eq!(value["error"], "no planner data");
        assert_eq!(value["phases_ms"]["browser_launch"], 420.0);
        assert_eq!(
            value["phases_ms"]["page_load"], 1500.0,
            "repeated phases sum"
        );
        assert_eq!(value["phases_ms"]["ui_finish"], 12.0);
        assert_eq!(value["total_ms"], 2100.0);

        let cancelled = ImportError::cancelled();
        let value = import_record_json(
            &ImportRecord {
                job_id: 8,
                provider: "Maxroll",
                subject: "x",
                result: Err(&cancelled),
                phases: &[],
                ui_finish: None,
                total: Duration::ZERO,
            },
            1,
        );
        assert_eq!(value["outcome"], "cancelled");
        assert!(value["error"].is_null());
    }

    #[test]
    fn the_file_rotates_at_its_bound() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(LOG_FILE),
            vec![b'x'; usize::try_from(LOG_MAX_BYTES).unwrap()],
        )
        .unwrap();
        append_line(dir.path(), "{\"a\":1}").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(LOG_FILE)).unwrap(),
            "{\"a\":1}\n"
        );
        assert!(dir.path().join(LOG_FILE_ROTATED).is_file());
    }
}
