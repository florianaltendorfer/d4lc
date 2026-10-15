//! Debug ViewModel — mirrors upstream DebugViewModel.cs.
//!
//! ## Upstream evidence
//!
//! - `DebugViewModel.cs:1-739` — 4 command definitions, 10+ properties
//! - `DebugViewModel.cs:101-104` — TakeScreenshotExecute, ExportDebugImagesExecute,
//!   ReloadPresetImagesExecute, ResetPerformanceTimersExecute

use crate::DebugPanelState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugEffect {
    RequestScreenshot,
    ExportDebugImages,
    ReloadPresetImages,
    ResetPerformanceResults,
    PersistConfig,
    SetWindowTopmost(bool),
    /// Export one local crash log by file name. The app layer gates
    /// this on the opt-in diagnostics setting and performs a local
    /// copy only — there is no upload path.
    ExportCrashLog(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DebugViewModel {
    pub state: DebugPanelState,
}

impl DebugViewModel {
    pub fn new(state: DebugPanelState) -> Self {
        Self { state }
    }

    /// Command: take a screenshot.
    pub fn take_screenshot(&mut self) -> Vec<DebugEffect> {
        vec![DebugEffect::RequestScreenshot]
    }

    /// Command: export debug images to disk.
    pub fn export_debug_images(&mut self) -> Vec<DebugEffect> {
        vec![DebugEffect::ExportDebugImages]
    }

    /// Command: reload preset images from disk.
    pub fn reload_preset_images(&mut self) -> Vec<DebugEffect> {
        vec![DebugEffect::ReloadPresetImages]
    }

    /// Command: reset performance timing metrics and artifact count.
    pub fn reset_performance_results(&mut self) -> Vec<DebugEffect> {
        self.state.timing_metrics.clear();
        self.state.debug_artifact_count = 0;
        vec![DebugEffect::ResetPerformanceResults]
    }

    /// Toggle debug-info enabled. Returns effect(s) for dispatch.
    ///
    /// Upstream writes `Settings.IsDebugInfoEnabled` and saves on every
    /// flip (`DebugViewModel.cs:168-178`); the port keeps the bit on the
    /// panel state and returns `PersistConfig` so the app layer writes
    /// it through `d4lc-config`.
    pub fn debug_info_toggled(&mut self, enabled: bool) -> Vec<DebugEffect> {
        self.state.debug_info_enabled = enabled;
        vec![DebugEffect::PersistConfig]
    }

    /// Toggle window topmost. Returns effect(s) for dispatch.
    pub fn topmost_toggled(&mut self, topmost: bool) -> Vec<DebugEffect> {
        vec![
            DebugEffect::SetWindowTopmost(topmost),
            DebugEffect::PersistConfig,
        ]
    }

    /// Command: export one local crash log by file name. Produces the
    /// effect only; the app layer owns the opt-in gate, the local
    /// copy, and the manual-attach-to-GitHub-issue guidance.
    pub fn export_crash_log(&mut self, file_name: String) -> Vec<DebugEffect> {
        vec![DebugEffect::ExportCrashLog(file_name)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DebugArtifactKind, DebugArtifactLink, DebugPanelFixture, DebugTimingMetric};

    fn sample_debug_panel_state() -> DebugPanelState {
        DebugPanelState::from_fixture(DebugPanelFixture {
            artifact_links: vec![DebugArtifactLink::new(
                DebugArtifactKind::RawCapture,
                "debug-output/capture.png",
            )],
            active_language: "enUS".to_string(),
            overlay_status: None,
            timing_metrics: vec![DebugTimingMetric {
                name: "capture".to_string(),
                value_ms: 42.0,
            }],
            debug_info_enabled: false,
            stage_panels: Vec::new(),
            perf_rows: Vec::new(),
            ocr_panels: Vec::new(),
        })
    }

    #[test]
    fn new_wraps_panel_state() {
        let state = sample_debug_panel_state();
        let vm = DebugViewModel::new(state.clone());
        assert_eq!(vm.state, state);
    }

    #[test]
    fn take_screenshot_returns_request_effect() {
        let state = sample_debug_panel_state();
        let mut vm = DebugViewModel::new(state);
        let effects = vm.take_screenshot();
        assert_eq!(effects, vec![DebugEffect::RequestScreenshot]);
    }

    #[test]
    fn export_debug_images_returns_export_effect() {
        let state = sample_debug_panel_state();
        let mut vm = DebugViewModel::new(state);
        let effects = vm.export_debug_images();
        assert_eq!(effects, vec![DebugEffect::ExportDebugImages]);
    }

    #[test]
    fn reload_preset_images_returns_reload_effect() {
        let state = sample_debug_panel_state();
        let mut vm = DebugViewModel::new(state);
        let effects = vm.reload_preset_images();
        assert_eq!(effects, vec![DebugEffect::ReloadPresetImages]);
    }

    #[test]
    fn reset_performance_results_clears_state() {
        let state = sample_debug_panel_state();
        let mut vm = DebugViewModel::new(state);
        vm.state.debug_artifact_count = 5;
        let effects = vm.reset_performance_results();
        assert_eq!(vm.state.debug_artifact_count, 0);
        assert!(vm.state.timing_metrics.is_empty());
        assert_eq!(effects, vec![DebugEffect::ResetPerformanceResults]);
    }

    #[test]
    fn debug_info_toggled_returns_persist_and_sets_the_bit() {
        let state = sample_debug_panel_state();
        let mut vm = DebugViewModel::new(state);
        let effects = vm.debug_info_toggled(true);
        assert_eq!(effects, vec![DebugEffect::PersistConfig]);
        assert!(
            vm.state.debug_info_enabled,
            "the toggle must carry the DebugInfo bit the app layer persists"
        );
    }

    #[test]
    fn topmost_toggled_returns_two_effects() {
        let state = sample_debug_panel_state();
        let mut vm = DebugViewModel::new(state);
        let effects = vm.topmost_toggled(true);
        assert_eq!(
            effects,
            vec![
                DebugEffect::SetWindowTopmost(true),
                DebugEffect::PersistConfig
            ]
        );
    }

    #[test]
    fn export_crash_log_returns_named_export_effect() {
        let state = sample_debug_panel_state();
        let mut vm = DebugViewModel::new(state);
        let effects = vm.export_crash_log("crash-0000000000-000000000.log".to_string());
        assert_eq!(
            effects,
            vec![DebugEffect::ExportCrashLog(
                "crash-0000000000-000000000.log".to_string()
            )]
        );
    }
}
