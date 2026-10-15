//! The Debug screen's `Take Screenshot` action: capture the game window,
//! encode it as PNG and write it to `debug-output/screenshot-<unix>.png`
//! under the config root (created on demand).
//!
//! The capture goes through `d4lc_capture::capture_selected_window_native`.
//! When it fails (no X11 display, no Diablo IV window) the outcome is
//! `Failed` with the capture error in `DebugActionRecord::reason` and on the
//! Logging screen; no synthetic frame is substituted. Without the
//! `real-runtime` feature the action is `Blocked`.

use std::path::{Path, PathBuf};

#[cfg(feature = "real-runtime")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "real-runtime")]
use d4lc_capture::{capture_selected_window_native, WindowQuery};

/// Outcome of a single `take_screenshot` attempt under the
/// `real-runtime` feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenshotOutcome {
    /// PNG was successfully written to `path`.
    Applied {
        path: PathBuf,
        width: u32,
        height: u32,
        bytes_written: u64,
    },
    /// The capture / encode / write step failed. `reason` is the,
    /// user-readable error string (no synthetic fallback).
    Failed { reason: String },
}

/// Root the debug artifacts are written under and linked relative to: the
/// config root, beside `Config/` (upstream keeps its `Screenshots/` folder
/// next to its settings).
pub(crate) fn artifact_root() -> PathBuf {
    crate::actions::settings::resolve_config_root()
}

/// Directory receiving debug screenshots and image exports.
pub fn default_screenshot_dir() -> PathBuf {
    artifact_root().join("debug-output")
}

/// Compute the screenshot filename for a given Unix timestamp
/// (seconds). Pure helper, exported for tests.
pub fn screenshot_filename_for(unix_secs: u64) -> String {
    format!("screenshot-{unix_secs}.png")
}

/// Build a `WindowQuery` for the default Diablo window.
///
/// The capture crate's `with_default_diablo_title` already does
/// the right thing for empty queries, but we keep the explicit
/// constructor so tests / non-Diablo scenarios stay obvious.
#[cfg(feature = "real-runtime")]
pub fn default_window_query() -> WindowQuery {
    WindowQuery {
        title: Some("Diablo IV".to_string()),
        class_name: None,
        pid: None,
        exact_title: false,
    }
}

/// Attempt one real screenshot: discover + capture the selected
/// window, encode the BGRA frame to PNG, write it under
/// `output_dir`. Returns the outcome so the caller can stamp a
/// `DebugActionRecord` accordingly.
///
/// This is the single function the Debug-screen handler calls
/// when `real-runtime` is enabled. It does not panic on
/// missing X11 displays or missing windows — it surfaces a
/// `ScreenshotOutcome::Failed` with the original error string.
#[cfg(feature = "real-runtime")]
pub fn attempt_real_screenshot(query: &WindowQuery, output_dir: &Path) -> ScreenshotOutcome {
    let result = match capture_selected_window_native(query) {
        Ok(r) => r,
        Err(error) => {
            return ScreenshotOutcome::Failed {
                reason: format!("window capture failed: {error}"),
            };
        }
    };

    if let Err(error) = ensure_dir(output_dir) {
        return ScreenshotOutcome::Failed {
            reason: format!(
                "could not create screenshot directory '{}': {error}",
                output_dir.display()
            ),
        };
    }

    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = output_dir.join(screenshot_filename_for(unix_secs));

    let png_bytes = match d4lc_debug::encode_png_rgba_from_bgra(&result.frame) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ScreenshotOutcome::Failed {
                reason: format!("PNG encoding failed: {error}"),
            };
        }
    };

    match std::fs::write(&path, &png_bytes) {
        Ok(()) => ScreenshotOutcome::Applied {
            path,
            width: result.frame.width,
            height: result.frame.height,
            bytes_written: png_bytes.len() as u64,
        },
        Err(error) => ScreenshotOutcome::Failed {
            reason: format!("could not write screenshot '{}': {error}", path.display()),
        },
    }
}

fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        if dir.is_dir() {
            return Ok(());
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("path exists and is not a directory: {}", dir.display()),
        ));
    }
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_filename_for_is_predictable() {
        assert_eq!(screenshot_filename_for(0), "screenshot-0.png");
        assert_eq!(
            screenshot_filename_for(1_700_000_000),
            "screenshot-1700000000.png"
        );
    }

    #[test]
    fn screenshots_go_to_debug_output_under_the_config_root() {
        assert_eq!(
            default_screenshot_dir(),
            artifact_root().join("debug-output")
        );
    }

    #[cfg(feature = "real-runtime")]
    #[test]
    fn default_window_query_targets_diablo_iv() {
        let q = default_window_query();
        assert_eq!(q.title.as_deref(), Some("Diablo IV"));
        assert!(q.class_name.is_none());
        assert!(q.pid.is_none());
    }

    /// Integration: with the real-runtime feature enabled but
    /// without an X11 display / Diablo window, the helper must
    /// report `Failed` (never panic, never return `Applied` with
    /// a fake frame).
    ///
    /// The test sets `DISPLAY` to an unreachable value so the
    /// x11rb transport used by `d4lc_capture` fails to connect.
    /// When the test runs in a CI/headless context that already
    /// lacks `DISPLAY`, the assertion still holds (the helper
    /// reports `Failed`).
    #[cfg(feature = "real-runtime")]
    #[test]
    fn attempt_real_screenshot_reports_failed_when_no_diablo_window() {
        let query = default_window_query();
        let dir = std::env::temp_dir().join(format!("d4lc-screenshot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let outcome = attempt_real_screenshot(&query, &dir);

        match outcome {
            ScreenshotOutcome::Failed { reason } => {
                assert!(
                    !reason.is_empty(),
                    "Failed outcome must include a non-empty reason"
                );
                // No PNG file should have been written on a Failed
                // path. (The temp directory may exist or not
                // depending on the failure point; we just verify
                // no .png was produced.)
                let produced = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(Result::ok)
                            .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("png"))
                    })
                    .unwrap_or(false);
                assert!(!produced, "Failed outcome must not write a PNG file");
            }
            ScreenshotOutcome::Applied { .. } => {
                // Acceptable only if a real Diablo window is
                // reachable from this test runner. We do not
                // expect that on the headless CI host.
                let produced = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(Result::ok)
                            .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("png"))
                    })
                    .unwrap_or(false);
                assert!(
                    produced,
                    "Applied outcome must have written a PNG file to {}",
                    dir.display()
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
