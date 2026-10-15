//! Shared fixtures of the debug action tests.

use crate::app_state::AppState;

/// Sample a fresh `AppState` for handler tests.
pub(super) fn fresh_state() -> AppState {
    AppState::default_startup()
}

/// Unique temp config root for one test (never a shared path).
pub(super) fn debug_test_root(label: &str) -> crate::test_root::TestRoot {
    crate::test_root::TestRoot::new(&format!("debug-{label}"))
}
