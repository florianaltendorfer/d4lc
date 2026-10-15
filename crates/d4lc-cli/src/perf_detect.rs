use crate::live::{benchmark_live_detection, LiveArgs, PerfMetricPercentiles, PerfScenarioSummary};
use crate::roots::Roots;
use crate::to_json_text;
use d4lc_capture::CaptureScope;
use d4lc_debug::write_debug_text;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const DEFAULT_REPORT_PATH: &str = "debug-output/perf/perf-detect.json";
const DEFAULT_DEBUG_DIR: &str = "debug-output/perf-detect";

pub fn perf_detect(
    roots: &Roots,
    args: &LiveArgs,
    write_path: Option<&Path>,
) -> Result<String, String> {
    args.sleep_before_capture();

    let report_path = write_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REPORT_PATH));
    let debug_report_path = args
        .output_dir
        .clone()
        .map(|path| path.join("report.json"))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DEBUG_DIR).join("report.json"));

    let mut scenarios = Vec::new();
    for capture_scope in requested_scopes(args.capture_scope.as_deref()) {
        scenarios.push(benchmark_live_detection(roots, args, capture_scope, None)?);
    }

    let report = render_report(&report_path, args, &scenarios);
    write_debug_text(&roots.work_path(&report_path), &report)?;
    write_debug_text(&roots.work_path(&debug_report_path), &report)?;
    Ok(report)
}

fn requested_scopes(capture_scope: Option<&str>) -> Vec<CaptureScope> {
    match capture_scope {
        Some("roi") => vec![CaptureScope::Roi],
        Some("full") => vec![CaptureScope::Full],
        Some("auto") | None => vec![CaptureScope::Roi, CaptureScope::Full],
        Some(_) => vec![CaptureScope::Roi, CaptureScope::Full],
    }
}

fn scope_label(capture_scope: CaptureScope) -> &'static str {
    match capture_scope {
        CaptureScope::Auto => "auto",
        CaptureScope::Full => "full",
        CaptureScope::Roi => "roi",
    }
}

fn render_report(report_path: &Path, args: &LiveArgs, scenarios: &[PerfScenarioSummary]) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "command": "perf-detect",
        "report_path": report_path.display().to_string(),
        "frames_per_scenario": args.frames.unwrap_or(120),
        "interval_ms": args.interval_millis.unwrap_or(0),
        "percentile_method": "nearest-rank",
        "scenarios": scenarios.iter().map(scenario_json).collect::<Vec<_>>(),
    }))
}

fn scenario_json(scenario: &PerfScenarioSummary) -> Value {
    json!({
        "capture_scope": scope_label(scenario.capture_scope),
        "frames_requested": scenario.frames_requested,
        "frames_processed": scenario.frames_processed,
        "dropped_frames": scenario.dropped_frames,
        "capture_method": scenario.capture_method,
        "capture_methods_observed": scenario.capture_methods_observed,
        "timings_ms": {
            "capture": metric_percentiles_json(&scenario.capture_ms),
            "match": metric_percentiles_json(&scenario.match_ms),
            "total_hotpath": metric_percentiles_json(&scenario.hotpath_total_ms),
        },
    })
}

fn metric_percentiles_json(metric: &PerfMetricPercentiles) -> Value {
    json!({ "p50": metric.p50_ms, "p95": metric.p95_ms })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scope_runs_roi_and_full_scenarios() {
        assert_eq!(
            requested_scopes(None),
            vec![CaptureScope::Roi, CaptureScope::Full]
        );
        assert_eq!(
            requested_scopes(Some("auto")),
            vec![CaptureScope::Roi, CaptureScope::Full]
        );
    }

    #[test]
    fn report_contains_required_detection_metrics() {
        let report = render_report(
            Path::new("debug-output/perf/perf-detect.json"),
            &LiveArgs::default(),
            &[
                PerfScenarioSummary {
                    capture_scope: CaptureScope::Roi,
                    frames_requested: 120,
                    frames_processed: 120,
                    dropped_frames: 0,
                    capture_method: "x11-native-xshm".to_string(),
                    capture_methods_observed: vec!["x11-native-xshm".to_string()],
                    capture_ms: PerfMetricPercentiles {
                        p50_ms: 2,
                        p95_ms: 4,
                    },
                    match_ms: PerfMetricPercentiles {
                        p50_ms: 1,
                        p95_ms: 3,
                    },
                    hotpath_total_ms: PerfMetricPercentiles {
                        p50_ms: 5,
                        p95_ms: 8,
                    },
                },
                PerfScenarioSummary {
                    capture_scope: CaptureScope::Full,
                    frames_requested: 120,
                    frames_processed: 120,
                    dropped_frames: 1,
                    capture_method: "x11-command".to_string(),
                    capture_methods_observed: vec!["x11-command".to_string()],
                    capture_ms: PerfMetricPercentiles {
                        p50_ms: 7,
                        p95_ms: 11,
                    },
                    match_ms: PerfMetricPercentiles {
                        p50_ms: 2,
                        p95_ms: 5,
                    },
                    hotpath_total_ms: PerfMetricPercentiles {
                        p50_ms: 10,
                        p95_ms: 16,
                    },
                },
            ],
        );

        let report: Value = serde_json::from_str(&report).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(report["scenarios"][0]["capture_scope"], "roi");
        assert_eq!(report["scenarios"][1]["capture_scope"], "full");
        assert_eq!(report["scenarios"][1]["dropped_frames"], 1);
        assert_eq!(report["scenarios"][0]["capture_method"], "x11-native-xshm");
        assert_eq!(
            report["scenarios"][1]["timings_ms"]["total_hotpath"],
            json!({ "p50": 10, "p95": 16 })
        );
    }
}
