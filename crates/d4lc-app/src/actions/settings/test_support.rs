//! Shared fixtures for the settings action tests.

use std::path::Path;
use std::sync::Mutex;

use super::resolve_config_root;

/// Serializes env var modifications across tests to prevent races
/// between `D4LC_CONFIG_ROOT` set/remove in parallel test executions,
/// and the process-wide hotkey-runtime restarts a test can drive.
pub(super) static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Run `f` with `D4LC_CONFIG_ROOT` pointing at a fresh temp directory.
///
/// The override stays set, and [`ENV_MUTEX`] held, for the whole body, so
/// handlers that resolve the config root themselves see the same directory
/// and no other test of this module changes it underneath. The override is
/// removed even when `f` panics.
pub(super) fn with_temp_config_root<F>(f: F)
where
    F: FnOnce(&Path),
{
    struct RemoveOverride;
    impl Drop for RemoveOverride {
        fn drop(&mut self) {
            std::env::remove_var("D4LC_CONFIG_ROOT");
        }
    }

    let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::Builder::new()
        .prefix("d4lc-test-")
        .tempdir()
        .expect("temp config root must be creatable");
    std::env::set_var("D4LC_CONFIG_ROOT", tmp.path());
    let _remove_override = RemoveOverride;
    let config_root = resolve_config_root();
    f(&config_root);
}

/// Helper: load a config from disk for round-trip assertions.
pub(super) fn load_persisted(config_root: &Path) -> d4lc_config::AppSettings {
    d4lc_config::load_app_settings(config_root).expect("config must load after confirm")
}
