use super::frame::{process_latest_frame, ProcessedFrame};
use super::ocr_worker::create_ocr_worker;
use super::overlay::{
    build_live_overlay_frame, build_live_overlay_frame_from_result, create_live_overlay,
    overlay_window_state_for_capture,
};
use super::prepare::{
    capture_live_frame, prepare_live_runtime, LiveSetupTimings, PreparedLiveRuntime,
};
use super::report::{
    live_loop_session_start_json, live_loop_summary_json, live_session_start_json,
    manual_validation_session_summary_json, pipeline_frame_json, processed_frame_event_json,
    processed_frame_json,
};
use super::LiveArgs;
use crate::roots::Roots;
use d4lc_capture::{CaptureMethod, CaptureResult, CaptureScope, CursorState};
use d4lc_debug::write_debug_text;
use d4lc_overlay_x11::{OverlayFrame, OverlayHandle, OverlayStatus};
use d4lc_runtime::{
    process_one_frame, CropStabilityGate, LatestFrameSlot, PreparedRuntime, RuntimeSession,
};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerfMetricPercentiles {
    pub p50_ms: u128,
    pub p95_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerfScenarioSummary {
    pub capture_scope: CaptureScope,
    pub frames_requested: u64,
    pub frames_processed: u64,
    pub dropped_frames: u64,
    pub capture_method: String,
    pub capture_methods_observed: Vec<String>,
    pub capture_ms: PerfMetricPercentiles,
    pub match_ms: PerfMetricPercentiles,
    pub hotpath_total_ms: PerfMetricPercentiles,
}

pub fn live_once(roots: &Roots, args: &LiveArgs) -> Result<String, String> {
    args.sleep_before_capture();
    let prepared = prepare_live_runtime(roots, args)?;
    let processed = LiveFrameSource::new().next_frame(&roots.work, args, &prepared, None, 0)?;
    let metadata = processed_frame_json(&processed, None, &prepared.config);
    if let Some(output_dir) = args.output_dir() {
        write_debug_text(
            &roots.work_path(&output_dir.join("live-once.json")),
            &metadata,
        )?;
    }

    Ok(metadata)
}

pub fn live_loop(roots: &Roots, args: &LiveArgs) -> Result<String, String> {
    args.sleep_before_capture();
    run_live_loop_session(roots, args)
}

pub(crate) struct ManualValidationCapture {
    pub session_jsonl: String,
    pub session_summary_json: String,
    pub final_frame_json: Option<String>,
    pub final_capture: Option<CaptureResult>,
    pub final_cursor: Option<CursorState>,
    pub final_overlay_frame: Option<OverlayFrame>,
    pub final_overlay_status: Option<OverlayStatus>,
    pub final_tooltip_crop_path: Option<PathBuf>,
    pub final_detection_found: bool,
}

fn run_live_loop_session(roots: &Roots, args: &LiveArgs) -> Result<String, String> {
    let prepared = prepare_live_runtime(roots, args)?;
    let frames = args.frames.unwrap_or(100);
    let interval = Duration::from_millis(args.interval_millis.unwrap_or(0));
    let ocr_interval = Duration::from_millis(args.ocr_interval_ms.unwrap_or(250));
    let mut overlay = create_live_overlay(roots, args, &prepared)?;
    let ocr_worker = create_ocr_worker(roots, args, &prepared)?;

    let mut output = String::new();
    let mut previous_found = false;
    let mut stats = LiveLoopStats::default();

    let mut runtime_session = RuntimeSession::new(ocr_worker, ocr_interval);
    let runtime = to_prepared_runtime(&prepared);

    if !args.summary_only {
        output.push_str(&live_loop_session_start_json(
            frames,
            &prepared,
            overlay.as_ref(),
            runtime_session.ocr_worker_ref(),
        ));
        output.push('\n');
    }

    for frame_index in 0..frames {
        let pipeline_result = process_one_frame(&mut runtime_session, &runtime)
            .map_err(|e| format!("pipeline error: {e}"))?;

        let tooltip_found = pipeline_result.overlay_frame.tooltip_found;
        let hotpath_ms = pipeline_result.overlay_frame.hotpath_ms;
        let frame_event = live_event(previous_found, tooltip_found);
        previous_found = tooltip_found;

        // --- Overlay update (errors must not abort the loop) ---
        if let Some(overlay) = overlay.as_mut() {
            let _ = overlay.update(build_live_overlay_frame_from_result(
                &pipeline_result,
                prepared.overlay_descriptor_state.as_ref(),
                prepared.overlay_font_px,
            ));
        }

        // --- Stats (simplified — only hotpath_ms is available) ---
        stats.observe_hotpath(hotpath_ms);

        // --- CLI-specific: emit JSON output ---
        if !args.summary_only {
            output.push_str(&pipeline_frame_json(
                &pipeline_result,
                frame_index,
                frame_event,
            ));
            output.push('\n');
        }

        if frame_index + 1 < frames {
            thread::sleep(interval);
        }
    }

    // --- Final OCR poll result ---
    let latest_ocr_result = runtime_session.latest_ocr_result().cloned();
    let final_ocr_stats = runtime_session.ocr_stats();

    output.push_str(&live_loop_summary_json(
        frames,
        &prepared,
        &stats,
        overlay.as_ref(),
        args.ocr_worker,
        final_ocr_stats.as_ref(),
        latest_ocr_result.as_ref(),
    ));
    output.push('\n');

    Ok(output)
}

/// Convert CLI's PreparedLiveRuntime into d4lc-runtime's PreparedRuntime.
fn to_prepared_runtime(cli_prepared: &PreparedLiveRuntime) -> PreparedRuntime {
    PreparedRuntime::new(
        cli_prepared.capture_backend,
        cli_prepared.capture_target.reference.clone(),
        cli_prepared.detector.clone(),
    )
}

pub(crate) fn benchmark_live_detection(
    roots: &Roots,
    args: &LiveArgs,
    capture_scope: CaptureScope,
    output_dir: Option<PathBuf>,
) -> Result<PerfScenarioSummary, String> {
    let mut scenario_args = args.clone();
    scenario_args.summary_only = true;
    scenario_args.overlay = false;
    scenario_args.ocr_worker = false;
    scenario_args.capture_output_path = None;
    scenario_args.output_dir = output_dir;
    scenario_args.frames = Some(args.frames.unwrap_or(120));
    scenario_args.interval_millis = Some(args.interval_millis.unwrap_or(0));
    scenario_args.capture_scope = Some(
        match capture_scope {
            CaptureScope::Auto => "auto",
            CaptureScope::Full => "full",
            CaptureScope::Roi => "roi",
        }
        .to_string(),
    );

    // Benchmark the real live-loop hot path (capture plan -> ROI capture ->
    // detect), not `run_live_loop_session`: its `process_one_frame` always
    // captures the full frame and reports no per-stage timings.
    let prepared = prepare_live_runtime(roots, &scenario_args)?;
    let frames = scenario_args.frames.unwrap_or(120);
    let interval = Duration::from_millis(scenario_args.interval_millis.unwrap_or(0));
    let mut frame_source = LiveFrameSource::new();
    let mut stats = LiveLoopStats::default();

    for frame_index in 0..frames {
        let processed =
            frame_source.next_frame(&roots.work, &scenario_args, &prepared, None, frame_index)?;
        stats.observe(&processed);

        if frame_index + 1 < frames {
            thread::sleep(interval);
        }
    }

    Ok(PerfScenarioSummary {
        capture_scope,
        frames_requested: scenario_args.frames.unwrap_or(120),
        frames_processed: stats.frames_processed,
        dropped_frames: stats.dropped_frames,
        capture_method: stats.capture_method_label(),
        capture_methods_observed: stats.capture_method_labels(),
        capture_ms: stats.capture_percentiles(),
        match_ms: stats.match_percentiles(),
        hotpath_total_ms: stats.hotpath_percentiles(),
    })
}

pub fn live(roots: &Roots, args: &LiveArgs) -> Result<String, String> {
    args.sleep_before_capture();
    let prepared = prepare_live_runtime(roots, args)?;
    let frames = args.frames.unwrap_or(u64::MAX);
    let interval = Duration::from_millis(args.interval_millis.unwrap_or(250));
    let mut output = live_session_start_json(&prepared);
    output.push('\n');
    let mut previous_found = false;
    let mut frame_source = LiveFrameSource::new();

    for frame_index in 0..frames {
        let processed = frame_source.next_frame(&roots.work, args, &prepared, None, frame_index)?;
        let found = processed.detection.selected.is_some();
        let event = live_event(previous_found, found);
        previous_found = found;
        output.push_str(&processed_frame_event_json(&processed, event));
        output.push('\n');

        if frame_index + 1 < frames {
            thread::sleep(interval);
        }
    }

    Ok(output)
}

pub(crate) fn capture_manual_validation_run(
    roots: &Roots,
    args: &LiveArgs,
) -> Result<ManualValidationCapture, String> {
    args.sleep_before_capture();
    let prepared = prepare_live_runtime(roots, args)?;
    let frames_requested = args.frames.unwrap_or(30);
    let interval = Duration::from_millis(args.interval_millis.unwrap_or(250));
    let mut output = String::new();
    let mut previous_found = false;
    let mut frame_source = LiveFrameSource::new();
    let mut stats = LiveLoopStats::default();
    let mut overlay = create_live_overlay(roots, args, &prepared)?;
    let mut tooltip_frames = 0_u64;
    let mut last_event = None;
    let mut last_processed = None;

    for frame_index in 0..frames_requested {
        let processed = frame_source.next_frame(
            &roots.work,
            args,
            &prepared,
            overlay.as_ref().map(OverlayHandle::status),
            frame_index,
        )?;
        let found = processed.detection.selected.is_some();
        if found {
            tooltip_frames = tooltip_frames.saturating_add(1);
        }
        let event = live_event(previous_found, found).to_string();
        previous_found = found;

        if let Some(overlay_handle) = overlay.as_mut() {
            let _ = overlay_handle.update(build_live_overlay_frame(
                &processed,
                found,
                prepared.overlay_descriptor_state.as_ref(),
                prepared.overlay_font_px,
            ));
        }

        output.push_str(&processed_frame_event_json(&processed, &event));
        output.push('\n');
        stats.observe(&processed);
        last_event = Some(event);
        last_processed = Some(processed);

        if frame_index + 1 < frames_requested {
            thread::sleep(interval);
        }
    }

    let final_frame_json = last_processed
        .as_ref()
        .zip(last_event.as_deref())
        .map(|(processed, event)| processed_frame_json(processed, Some(event), &prepared.config));
    let final_capture = last_processed
        .as_ref()
        .map(|processed| processed.capture.clone());
    let final_cursor = last_processed
        .as_ref()
        .and_then(|processed| processed.cursor.clone());
    let final_overlay_frame = last_processed.as_ref().map(|processed| {
        build_live_overlay_frame(
            processed,
            processed.detection.selected.is_some(),
            prepared.overlay_descriptor_state.as_ref(),
            prepared.overlay_font_px,
        )
    });
    let final_tooltip_crop_path = last_processed
        .as_ref()
        .and_then(|processed| processed.detection.crop_output_path.clone());
    let final_detection_found = last_processed
        .as_ref()
        .map(|processed| processed.detection.selected.is_some())
        .unwrap_or(false);
    let final_overlay_status = overlay.as_ref().map(|handle| handle.status().clone());
    let session_summary_json = manual_validation_session_summary_json(
        frames_requested,
        tooltip_frames,
        &stats,
        prepared.setup_timings,
        overlay.as_ref(),
        last_processed.as_ref(),
        final_tooltip_crop_path.as_ref(),
    );

    Ok(ManualValidationCapture {
        session_jsonl: output,
        session_summary_json,
        final_frame_json,
        final_capture,
        final_cursor,
        final_overlay_frame,
        final_overlay_status,
        final_tooltip_crop_path,
        final_detection_found,
    })
}

/// Cross-frame capture state for the CLI live paths: every frame goes
/// capture -> latest-frame slot -> `process_latest_frame`.
struct LiveFrameSource {
    slot: LatestFrameSlot<CaptureResult>,
    stability_gate: CropStabilityGate,
}

impl LiveFrameSource {
    fn new() -> Self {
        Self {
            slot: LatestFrameSlot::new(),
            stability_gate: CropStabilityGate::new(),
        }
    }

    /// Setup timings are attributed to frame 0 only; debug files resolve
    /// against `output_base`.
    fn next_frame(
        &mut self,
        output_base: &Path,
        args: &LiveArgs,
        prepared: &PreparedLiveRuntime,
        overlay_status: Option<&OverlayStatus>,
        frame_index: u64,
    ) -> Result<ProcessedFrame, String> {
        let mut captured = capture_live_frame(prepared)?;
        captured.capture.overlay_window =
            overlay_window_state_for_capture(overlay_status, &captured.capture.source);
        self.slot.push(captured.capture);
        let latest = self
            .slot
            .take_latest()
            .ok_or_else(|| "latest-frame slot did not return captured frame".to_string())?;
        let setup_timings = if frame_index == 0 {
            prepared.setup_timings
        } else {
            LiveSetupTimings::default()
        };
        process_latest_frame(
            output_base,
            args,
            latest.sequence,
            latest.value,
            self.slot.dropped_frames(),
            captured.capture_ms,
            captured.roi_plan_ms,
            captured.window_lookup_ms,
            captured.hotpath_start,
            captured.roi_plan,
            captured.cursor,
            &mut self.stability_gate,
            prepared,
            setup_timings,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct LiveLoopStats {
    pub(super) frames_processed: u64,
    pub(super) dropped_frames: u64,
    pub(super) hotpath_min_ms: Option<u128>,
    pub(super) hotpath_max_ms: u128,
    pub(super) hotpath_sum_ms: u128,
    pub(super) capture_sum_ms: u128,
    pub(super) detection_sum_ms: u128,
    pub(super) match_sum_ms: u128,
    pub(super) output_write_sum_ms: u128,
    pub(super) capture_samples_ms: Vec<u128>,
    pub(super) match_samples_ms: Vec<u128>,
    pub(super) hotpath_samples_ms: Vec<u128>,
    pub(super) capture_methods_observed: Vec<CaptureMethod>,
}

impl LiveLoopStats {
    fn observe(&mut self, processed: &ProcessedFrame) {
        self.observe_hotpath(processed.timings.hotpath_total_ms);
        self.dropped_frames = processed.dropped_frames;
        self.capture_sum_ms = self
            .capture_sum_ms
            .saturating_add(processed.timings.capture_ms);
        self.detection_sum_ms = self
            .detection_sum_ms
            .saturating_add(processed.timings.detection_ms);
        self.match_sum_ms = self.match_sum_ms.saturating_add(processed.timings.match_ms);
        self.output_write_sum_ms = self
            .output_write_sum_ms
            .saturating_add(processed.timings.output_write_ms);
        self.capture_samples_ms.push(processed.timings.capture_ms);
        self.match_samples_ms.push(processed.timings.match_ms);
        if !self
            .capture_methods_observed
            .contains(&processed.timings.capture_method)
        {
            self.capture_methods_observed
                .push(processed.timings.capture_method);
        }
    }

    /// Counts one processed frame and records its hot-path time.
    fn observe_hotpath(&mut self, hotpath_ms: u128) {
        self.frames_processed = self.frames_processed.saturating_add(1);
        self.hotpath_min_ms = Some(match self.hotpath_min_ms {
            Some(current) => current.min(hotpath_ms),
            None => hotpath_ms,
        });
        self.hotpath_max_ms = self.hotpath_max_ms.max(hotpath_ms);
        self.hotpath_sum_ms = self.hotpath_sum_ms.saturating_add(hotpath_ms);
        self.hotpath_samples_ms.push(hotpath_ms);
    }

    pub(super) fn capture_percentiles(&self) -> PerfMetricPercentiles {
        PerfMetricPercentiles {
            p50_ms: percentile_ms(&self.capture_samples_ms, 50),
            p95_ms: percentile_ms(&self.capture_samples_ms, 95),
        }
    }

    pub(super) fn match_percentiles(&self) -> PerfMetricPercentiles {
        PerfMetricPercentiles {
            p50_ms: percentile_ms(&self.match_samples_ms, 50),
            p95_ms: percentile_ms(&self.match_samples_ms, 95),
        }
    }

    pub(super) fn hotpath_percentiles(&self) -> PerfMetricPercentiles {
        PerfMetricPercentiles {
            p50_ms: percentile_ms(&self.hotpath_samples_ms, 50),
            p95_ms: percentile_ms(&self.hotpath_samples_ms, 95),
        }
    }

    fn capture_method_label(&self) -> String {
        match self.capture_methods_observed.as_slice() {
            [] => "unknown".to_string(),
            [method] => method.to_string(),
            _ => "mixed".to_string(),
        }
    }

    pub(super) fn capture_method_labels(&self) -> Vec<String> {
        self.capture_methods_observed
            .iter()
            .map(ToString::to_string)
            .collect()
    }
}

fn percentile_ms(samples: &[u128], percentile: usize) -> u128 {
    if samples.is_empty() {
        return 0;
    }

    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = ((percentile * sorted.len()).saturating_add(99) / 100).saturating_sub(1);
    sorted[rank.min(sorted.len().saturating_sub(1))]
}

pub(super) fn live_event(previous_found: bool, current_found: bool) -> &'static str {
    match (previous_found, current_found) {
        (false, true) => "tooltip_found",
        (true, false) => "tooltip_cleared",
        (true, true) => "tooltip_present",
        (false, false) => "tooltip_absent",
    }
}

#[cfg(test)]
mod tests;
