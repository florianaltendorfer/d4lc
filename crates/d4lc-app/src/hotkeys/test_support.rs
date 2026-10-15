//! Shared fixtures for the hotkey module tests.

use std::sync::Mutex;

/// Process-wide lock that serialises every test which
/// mutates `DISPLAY` / `XDG_SESSION_TYPE`. Without this
/// lock, the test runner's parallel threads race on
/// `std::env::set_var` and one test can see another test's
/// (un)set value, causing intermittent `X11Available` /
/// `X11Unavailable` mismatches.
///
/// `cargo test` defaults to one thread per logical CPU,
/// so the race is real and observed.
pub(super) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Helper: preserve / restore env vars across the test.
/// Capturing also clears `WAYLAND_DISPLAY`, so no test reaches the
/// desktop's portal unless it opts in by setting it.
pub(super) struct EnvGuard {
    prev_display: Option<String>,
    prev_session: Option<String>,
    prev_wayland_display: Option<String>,
}
impl EnvGuard {
    pub(super) fn capture() -> Self {
        let guard = Self {
            prev_display: std::env::var("DISPLAY").ok(),
            prev_session: std::env::var("XDG_SESSION_TYPE").ok(),
            prev_wayland_display: std::env::var("WAYLAND_DISPLAY").ok(),
        };
        std::env::remove_var("WAYLAND_DISPLAY");
        guard
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Best-effort restore. The test runner will not have
        // changed anything user-facing by the time we return
        // because cargo test does not set these.
        if let Some(ref v) = self.prev_display {
            std::env::set_var("DISPLAY", v);
        } else {
            std::env::remove_var("DISPLAY");
        }
        if let Some(ref v) = self.prev_session {
            std::env::set_var("XDG_SESSION_TYPE", v);
        } else {
            std::env::remove_var("XDG_SESSION_TYPE");
        }
        if let Some(ref v) = self.prev_wayland_display {
            std::env::set_var("WAYLAND_DISPLAY", v);
        } else {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
    }
}

/// Helper: write a `D4LC_CONFIG_ROOT` env var pointing
/// at a per-test tempdir so the dispatcher's
/// `confirm_controller_config` / `confirm_overlay_config`
/// can persist without clobbering the real settings.
pub(super) fn with_temp_config_root<F, R>(f: F) -> R
where
    F: FnOnce(&std::path::Path) -> R,
{
    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::Builder::new()
        .prefix("d4lc-hotkey-test-")
        .tempdir()
        .expect("temp config root must be creatable");
    std::env::set_var("D4LC_CONFIG_ROOT", tmp.path());
    let result = f(tmp.path());
    std::env::remove_var("D4LC_CONFIG_ROOT");
    result
}
