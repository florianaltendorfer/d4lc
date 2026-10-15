//! One OpenMP thread per Tesseract call.
//!
//! The OCR worker recognizes the segments of a tooltip on several pooled
//! Tesseract engines at once. Tesseract's LSTM additionally forks OpenMP
//! threads inside every call; next to the per-segment threads that
//! oversubscribes the cores the game runs on, and the spinning OpenMP
//! workers make the parallel read slower than a sequential one.
//!
//! libgomp reads `OMP_THREAD_LIMIT` once, when it is loaded with the
//! process, so the limit cannot be applied later: a process started without
//! it replaces itself once, before anything else runs, with the same
//! arguments and `OMP_THREAD_LIMIT=1`. An explicit value in the environment
//! is always honored, and it also marks the relaunched process, so the
//! relaunch happens at most once.

use std::ffi::OsStr;

/// The libgomp environment variable that caps the OpenMP thread count.
pub const OMP_THREAD_LIMIT_ENV: &str = "OMP_THREAD_LIMIT";

/// Whether a process with `current` as its `OMP_THREAD_LIMIT` has to
/// relaunch: only when the variable is absent.
pub fn relaunch_needed(current: Option<&OsStr>) -> bool {
    current.is_none()
}

/// Relaunch the process with `OMP_THREAD_LIMIT=1` unless the variable is
/// set. Returns only when no relaunch was needed or the relaunch failed;
/// a failure is logged and the process keeps Tesseract's own OpenMP
/// threads.
pub fn ensure_single_thread_per_call() {
    if !relaunch_needed(std::env::var_os(OMP_THREAD_LIMIT_ENV).as_deref()) {
        return;
    }
    use std::os::unix::process::CommandExt;
    let error = match std::env::current_exe() {
        Ok(exe) => std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(OMP_THREAD_LIMIT_ENV, "1")
            .exec(),
        Err(error) => error,
    };
    eprintln!(
        "d4lc-app: relaunch with {OMP_THREAD_LIMIT_ENV}=1 failed ({error}); \
         OCR runs with Tesseract's OpenMP threads and is slower"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunches_only_without_an_explicit_limit() {
        assert!(relaunch_needed(None));
        assert!(!relaunch_needed(Some(OsStr::new("1"))));
        assert!(!relaunch_needed(Some(OsStr::new("4"))));
        assert!(!relaunch_needed(Some(OsStr::new(""))));
    }
}
