//! Local crash-log writer and panic hook.
//!
//! A panic hook installed by the app entrypoint writes one bounded,
//! self-describing crash log per panic into a local directory
//! (`~/.local/share/d4lc/crashes/` by default, XDG-respected). Crash
//! logs carry the app version, the panic message + location, a
//! settings snapshot restricted to non-personal diagnostic fields,
//! and a backtrace. Logs survive restarts and are pruned to a
//! bounded count (oldest removed first) after every write.
//!
//! ## Diagnostics are local-only
//!
//! "Diagnostics export" is strictly a local copy performed on an
//! explicit user action (the Debug screen's crash-log export), gated
//! by an opt-in settings toggle (default off), so the user can
//! attach a crash log to a GitHub issue they open manually. This
//! module contains no network code and the application never
//! uploads anything.
//!
//! ## Test isolation
//!
//! `resolve_crash_dir()` honors the `D4LC_CRASH_DIR` environment
//! variable (same override pattern as `D4LC_CONFIG_ROOT`) so tests
//! never write into the real user home.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use d4lc_config::AppSettings;

/// Environment override for the crash-log directory (tests /
/// development). Mirrors the `D4LC_CONFIG_ROOT` pattern.
pub const CRASH_DIR_ENV_VAR: &str = "D4LC_CRASH_DIR";

/// Bounded retention: at most this many crash logs are kept in the
/// crash directory. After every write, files beyond this count are
/// pruned oldest-first (file names embed a zero-padded unix
/// timestamp, so lexicographic order is chronological).
pub const MAX_RETAINED_CRASH_LOGS: usize = 10;

/// Prefix + extension of a crash-log file name. Export/read paths
/// only accept names matching `crash-*.log` so a hostile name can
/// never escape the crash directory.
pub const CRASH_LOG_PREFIX: &str = "crash-";
pub const CRASH_LOG_SUFFIX: &str = ".log";

/// The payload of one crash log. [`write_crash_log`] renders it and
/// persists it; keep this pure so tests can build synthetic reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    pub app_version: String,
    pub timestamp_unix_secs: u64,
    pub timestamp_subsec_nanos: u32,
    pub panic_message: String,
    pub panic_location: Option<String>,
    pub backtrace: String,
    /// Pre-rendered settings snapshot restricted to non-personal
    /// diagnostic fields (see [`settings_snapshot`]).
    pub settings_snapshot: String,
}

/// Default crash-log directory: `<data_dir>/d4lc/crashes`
/// (XDG: `~/.local/share/d4lc/crashes`). `None` when the platform
/// cannot resolve a data/home directory.
pub fn default_crash_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|base| base.join("d4lc").join("crashes"))
}

/// Resolve the crash-log directory.
///
/// Priority:
/// 1. `D4LC_CRASH_DIR` environment variable (for tests / development)
/// 2. [`default_crash_dir`]
pub fn resolve_crash_dir() -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var(CRASH_DIR_ENV_VAR) {
        if !override_dir.trim().is_empty() {
            return Some(PathBuf::from(override_dir));
        }
    }
    default_crash_dir()
}

/// Render the crash-report settings snapshot.
///
/// Deliberately restricted to diagnostic fields with no personal
/// data: backend/profile selections, mode toggles, and numeric
/// tuning. Window geometry, capture regions, controller images,
/// hotkey bindings, and file-system paths are never included.
pub fn settings_snapshot(settings: &AppSettings) -> String {
    let mut out = String::new();
    let push = |out: &mut String, key: &str, value: String| {
        out.push_str(key);
        out.push('=');
        out.push_str(&value);
        out.push('\n');
    };
    push(
        &mut out,
        "selected_system_preset",
        settings.selected_system_preset.clone(),
    );
    push(
        &mut out,
        "selected_affix_language",
        settings.selected_affix_language.clone(),
    );
    push(
        &mut out,
        "capture_backend",
        settings.capture_backend.clone(),
    );
    push(
        &mut out,
        "controller_mode",
        settings.controller_mode.to_string(),
    );
    push(&mut out, "debug_mode", settings.debug_mode.to_string());
    push(
        &mut out,
        "overlay_enabled",
        settings.overlay_enabled.to_string(),
    );
    push(&mut out, "is_top_most", settings.is_top_most.to_string());
    push(
        &mut out,
        "scan_height_percent",
        settings.scan_height_percent.to_string(),
    );
    push(
        &mut out,
        "tooltip_width",
        settings.tooltip_width.to_string(),
    );
    push(
        &mut out,
        "tooltip_max_height",
        settings.tooltip_max_height.to_string(),
    );
    push(
        &mut out,
        "threshold_min",
        settings.threshold_min.to_string(),
    );
    push(
        &mut out,
        "threshold_max",
        settings.threshold_max.to_string(),
    );
    out
}

/// Build a [`CrashReport`] from a panic hook invocation. Never panics:
/// unparseable payloads fall back to a placeholder string.
fn report_from_panic(
    info: &std::panic::PanicHookInfo<'_>,
    app_version: &str,
    settings_snapshot: String,
) -> CrashReport {
    let payload = info.payload();
    let panic_message = if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_string()
    };
    let (secs, nanos) = unix_timestamp();
    CrashReport {
        app_version: app_version.to_string(),
        timestamp_unix_secs: secs,
        timestamp_subsec_nanos: nanos,
        panic_message,
        panic_location: info.location().map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        }),
        backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        settings_snapshot,
    }
}

/// Current unix timestamp as `(secs, subsec_nanos)`. Clock failures
/// degrade to `(0, 0)` — a panic hook must never panic.
fn unix_timestamp() -> (u64, u32) {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => (duration.as_secs(), duration.subsec_nanos()),
        Err(_) => (0, 0),
    }
}

/// Pure writer: render `report` into one crash-log file inside `dir`
/// and prune the directory to [`MAX_RETAINED_CRASH_LOGS`] files.
///
/// The file name embeds the zero-padded timestamp
/// (`crash-<secs>-<nanos>.log`) so repeated crashes never overwrite
/// each other and lexicographic order is chronological. The directory
/// is created on demand. Retention pruning is best-effort: a failed
/// prune never fails the write of the log that just landed.
pub fn write_crash_log(dir: &Path, report: &CrashReport) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let file_name = crash_log_file_name(report.timestamp_unix_secs, report.timestamp_subsec_nanos);
    let path = dir.join(&file_name);
    fs::write(&path, render_crash_report(report))?;
    let _ = prune_crash_logs(dir, MAX_RETAINED_CRASH_LOGS);
    Ok(path)
}

/// Zero-padded, sortable crash-log file name:
/// `crash-<secs:010>-<nanos:09>.log`.
fn crash_log_file_name(secs: u64, subsec_nanos: u32) -> String {
    format!("{CRASH_LOG_PREFIX}{secs:010}-{subsec_nanos:09}{CRASH_LOG_SUFFIX}")
}

/// Render the full crash-log text. One stable, plain-text shape so
/// the file is directly attachable to a GitHub issue.
fn render_crash_report(report: &CrashReport) -> String {
    let mut out = String::new();
    out.push_str("d4lc crash log\n");
    out.push_str(&format!("app_version={}\n", report.app_version));
    out.push_str(&format!(
        "timestamp_unix={}.{:09}\n",
        report.timestamp_unix_secs, report.timestamp_subsec_nanos
    ));
    out.push_str(&format!("panic_message={}\n", report.panic_message));
    out.push_str(&format!(
        "panic_location={}\n",
        report.panic_location.as_deref().unwrap_or("<unknown>")
    ));
    out.push_str("settings_snapshot (no personal data):\n");
    out.push_str(&report.settings_snapshot);
    out.push_str("backtrace:\n");
    out.push_str(&report.backtrace);
    out.push('\n');
    out
}

/// Prune `dir` to the newest `max_retained` crash logs, removing the
/// oldest files first. Files that are not crash logs are never
/// touched. A missing directory is not an error.
pub fn prune_crash_logs(dir: &Path, max_retained: usize) -> io::Result<()> {
    let mut logs = list_crash_logs(dir);
    if logs.len() <= max_retained {
        return Ok(());
    }
    // `list_crash_logs` returns newest-first; everything beyond the
    // retained count is the oldest tail.
    for file_name in logs.split_off(max_retained) {
        fs::remove_file(dir.join(file_name))?;
    }
    Ok(())
}

/// List the crash-log file names in `dir`, newest first. A missing
/// directory (or unreadable entry metadata) yields an empty list —
/// listing is a read-only convenience, never an error source.
pub fn list_crash_logs(dir: &Path) -> Vec<String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            is_crash_log_name(&name).then_some(name)
        })
        .collect();
    // Zero-padded timestamps: descending lexicographic == newest first.
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
}

/// Strict crash-log file-name check: plain file name, no separators,
/// no `..`, matching the writer's `crash-*.log` shape. Guards the
/// read/export path against path traversal.
fn is_crash_log_name(name: &str) -> bool {
    name.len() > CRASH_LOG_PREFIX.len() + CRASH_LOG_SUFFIX.len()
        && name.starts_with(CRASH_LOG_PREFIX)
        && name.ends_with(CRASH_LOG_SUFFIX)
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
}

/// Read one crash log from `dir` by file name. Rejects any name that
/// is not a strict crash-log file name (path-traversal guard).
pub fn read_crash_log(dir: &Path, file_name: &str) -> io::Result<String> {
    if !is_crash_log_name(file_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a crash-log file name: {file_name}"),
        ));
    }
    fs::read_to_string(dir.join(file_name))
}

/// Install the process panic hook.
///
/// Every panic writes one bounded crash log into `crash_dir` (when a
/// directory could be resolved) and then delegates to the previously
/// installed hook so the default stderr report is preserved. The hook
/// body is panic-free: every fallible step degrades to a skipped
/// write or a placeholder string.
pub fn install_panic_hook(app_version: &'static str, settings_snapshot: String) {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = report_from_panic(info, app_version, settings_snapshot.clone());
        if let Some(dir) = resolve_crash_dir() {
            let _ = write_crash_log(&dir, &report);
        }
        previous_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    /// Test crash directories live under the workspace target
    /// directory only — never the real user home. The pid suffix
    /// keeps parallel test runs isolated; each test cleans up.
    fn test_crash_dir(label: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-crash-dirs")
            .join(format!("{label}-{}", std::process::id()))
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn sample_report(secs: u64, nanos: u32) -> CrashReport {
        CrashReport {
            app_version: "1.0.0".to_string(),
            timestamp_unix_secs: secs,
            timestamp_subsec_nanos: nanos,
            panic_message: "explicit panic in test".to_string(),
            panic_location: Some("src/crash_report.rs:1:1".to_string()),
            backtrace: "stack capture placeholder".to_string(),
            settings_snapshot: "debug_mode=false\n".to_string(),
        }
    }

    /// Named decision point: crash logs must survive restarts and be
    /// bounded — the retention contract (oldest pruned, count fixed
    /// in code) is a data-integrity invariant of the writer.
    /// Oracle: independent readback of the crash directory contents.
    #[test]
    fn write_crash_log_writes_file_and_prunes_to_retention_bound() {
        let dir = test_crash_dir("write-prunes");
        cleanup(&dir);

        let newest = write_crash_log(&dir, &sample_report(1_700_000_010, 5)).expect("write 11");
        for secs in 1_700_000_000..1_700_000_010 {
            write_crash_log(&dir, &sample_report(secs, 0)).expect("write");
        }

        let listed = list_crash_logs(&dir);
        assert_eq!(listed.len(), MAX_RETAINED_CRASH_LOGS);
        // The log that just landed always survives the prune.
        assert_eq!(
            listed.first().map(String::as_str),
            Some(
                newest
                    .file_name()
                    .expect("file name")
                    .to_str()
                    .expect("utf8")
            )
        );
        // The oldest pre-bound log is gone.
        assert!(!dir.join("crash-1700000000-000000000.log").is_file());
        // Written content is self-describing and non-empty.
        let content = fs::read_to_string(&newest).expect("read back");
        assert!(content.contains("app_version=1.0.0"));
        assert!(content.contains("panic_message=explicit panic in test"));
        assert!(content.contains("panic_location=src/crash_report.rs:1:1"));
        assert!(content.contains("backtrace:"));
        assert!(content.contains("debug_mode=false"));

        cleanup(&dir);
    }

    /// Named decision point: `D4LC_CRASH_DIR` must override the
    /// default (tests never write the real home). Oracle: readback of
    /// the resolved path itself.
    #[test]
    fn resolve_crash_dir_respects_env_override() {
        let override_dir = test_crash_dir("env-override");
        let previous = std::env::var(CRASH_DIR_ENV_VAR).ok();
        std::env::set_var(CRASH_DIR_ENV_VAR, &override_dir);
        let resolved = resolve_crash_dir();
        match previous {
            Some(value) => std::env::set_var(CRASH_DIR_ENV_VAR, value),
            None => std::env::remove_var(CRASH_DIR_ENV_VAR),
        }
        assert_eq!(resolved, Some(override_dir));
    }

    #[test]
    fn list_crash_logs_returns_newest_first_and_ignores_foreign_files() {
        let dir = test_crash_dir("list-order");
        cleanup(&dir);
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(dir.join("crash-1700000002-000000000.log"), "b").unwrap();
        fs::write(dir.join("crash-1700000000-000000000.log"), "a").unwrap();
        fs::write(dir.join("crash-1700000001-000000000.log"), "mid").unwrap();
        fs::write(dir.join("unrelated.txt"), "keep out").unwrap();
        fs::create_dir_all(dir.join("crash-1700000003-000000000.log")).unwrap();

        let listed = list_crash_logs(&dir);

        assert_eq!(
            listed,
            vec![
                "crash-1700000002-000000000.log".to_string(),
                "crash-1700000001-000000000.log".to_string(),
                "crash-1700000000-000000000.log".to_string(),
            ]
        );
        cleanup(&dir);
    }

    /// Named decision point: the export path reads user-influenced
    /// file names — the traversal guard is a safety invariant
    /// (fail-closed). Oracle: the readback itself.
    #[test]
    fn read_crash_log_rejects_non_crash_log_names() {
        let dir = test_crash_dir("read-guard");
        cleanup(&dir);
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(dir.join("crash-1700000000-000000000.log"), "payload").unwrap();

        assert_eq!(
            read_crash_log(&dir, "crash-1700000000-000000000.log").expect("valid read"),
            "payload"
        );
        for hostile in [
            "../secrets.txt",
            "sub/crash-1700000000-000000000.log",
            "crash-..log",
            "notes.log",
        ] {
            let result = read_crash_log(&dir, hostile);
            assert!(result.is_err(), "must reject {hostile}");
        }
        cleanup(&dir);
    }

    /// Named decision point: the settings snapshot is a privacy
    /// boundary — only non-personal diagnostic fields may be rendered.
    /// Oracle: the rendered snapshot itself.
    #[test]
    fn settings_snapshot_contains_only_non_personal_diagnostic_fields() {
        let settings = AppSettings {
            selected_system_preset: "1440p_SMF".to_string(),
            controller_mode: true,
            window_geometry: Some(d4lc_core::Rect {
                x: 12,
                y: 34,
                width: 100,
                height: 200,
            }),
            ..AppSettings::default()
        };

        let snapshot = settings_snapshot(&settings);

        assert!(snapshot.contains("selected_system_preset=1440p_SMF"));
        assert!(snapshot.contains("controller_mode=true"));
        assert!(snapshot.contains("debug_mode=false"));
        // Personal / machine-specific state never appears in the
        // snapshot: geometry, capture regions, hotkey bindings.
        assert!(!snapshot.contains("window"));
        assert!(!snapshot.contains("hotkey"));
        assert!(!snapshot.contains("manual_region"));
        // The snapshot is a strict key=value list over the allowed
        // diagnostic keys only — the preset value appears exactly
        // once, as its own key's value.
        assert_eq!(snapshot.matches("1440p_SMF").count(), 1);
    }
}
