//! Debug action handlers.
//!
//! ## Upstream evidence
//!
//! - `DebugViewModel.cs:101-104` — 4 command definitions
//! - `DebugViewModel.cs:330-380` — DebugInfo toggle, TopMost toggle
//!
//! ## Actions
//!
//! - `ResetPerformanceResults` clears the debug panel's `timing_metrics`
//!   and `debug_artifact_count`.
//! - `TakeScreenshot` captures the game window
//!   (`d4lc_capture::capture_selected_window_native`), encodes the BGRA
//!   frame to PNG and writes it under `debug-output/`. Without
//!   `real-runtime` the action is `Blocked`.
//! - `ExportDebugImages` dispatches through the app-owned
//!   `DebugCommandController`, which runs one `process_one_frame` pass plus a
//!   `detect_roi_frame_prepared` pass and writes a bounded bundle under
//!   `debug-output/export-<unix>/`.
//! - `ReloadPresetImages` makes the controller stop the live loop, rebuild
//!   the runtime from the same `config_root` and start a new loop.
//!   `Applied` only on a confirmed rebuild and restart, `Failed` on a
//!   preparation failure, `Blocked` when no live loop runs.
//! - `DebugInfo` / `TopMost` route through `DebugViewModel`; their
//!   `PersistConfig` effect is logged because `DebugPanelState` carries no
//!   persistence fields for them.
//!
//! Every action stamps `last_debug_action` with the confirmed
//! `Applied | Failed | Blocked` outcome.

mod commands;
pub mod export;
mod live;
mod output;
pub mod screenshot;
mod steppers;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod test_support;

pub use commands::{
    handle_debug_info_toggled, handle_export_crash_log, handle_export_debug_images,
    handle_open_artifacts_folder, handle_reload_preset_images, handle_reset_perf,
    handle_take_screenshot, handle_topmost_toggled, seed_debug_info_from_settings,
};
pub use live::{project_live_debug_stages, project_live_diagnostics};
pub use output::{project_debug_panels, refresh_stored_run_note, DebugOutputWatcher};
pub use steppers::{
    handle_debug_stepper_changed, project_debug_stepper_values, DebugSettingsProjection,
};
