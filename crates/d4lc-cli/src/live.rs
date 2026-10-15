mod frame;
mod ocr_worker;
mod overlay;
mod prepare;
mod report;
mod session;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub(crate) use session::{
    benchmark_live_detection, capture_manual_validation_run, ManualValidationCapture,
    PerfMetricPercentiles, PerfScenarioSummary,
};
pub use session::{live, live_loop, live_once};

use d4lc_capture::WindowQuery;
use d4lc_core::Rect;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LiveArgs {
    pub match_title: Option<String>,
    pub class_name: Option<String>,
    pub pid: Option<u32>,
    pub exact_title: bool,
    pub manual_region: Option<Rect>,
    pub preset: Option<String>,
    pub language: Option<String>,
    pub controller_config: Vec<String>,
    pub mode: Option<String>,
    pub threshold_min: Option<u8>,
    pub threshold_max: Option<u8>,
    pub threshold_similarity_tooltip: Option<f64>,
    pub output_dir: Option<PathBuf>,
    pub capture_output_path: Option<PathBuf>,
    pub delay_millis: Option<u64>,
    pub frames: Option<u64>,
    pub interval_millis: Option<u64>,
    pub backend: Option<String>,
    pub capture_scope: Option<String>,
    pub overlay: bool,
    pub summary_only: bool,
    pub ocr_worker: bool,
    pub ocr_interval_ms: Option<u64>,
    pub ocr_text_preview_chars: Option<u64>,
}

impl LiveArgs {
    fn window_query(&self) -> WindowQuery {
        WindowQuery {
            title: self.match_title.clone(),
            class_name: self.class_name.clone(),
            pid: self.pid,
            exact_title: self.exact_title,
        }
    }

    fn debug_enabled(&self) -> bool {
        self.output_dir().is_some() || self.capture_output_path.is_some()
    }

    fn output_dir(&self) -> Option<PathBuf> {
        self.output_dir.clone().or_else(|| {
            if self.overlay {
                Some(PathBuf::from("debug-output/capture-overlay"))
            } else {
                None
            }
        })
    }

    pub(crate) fn sleep_before_capture(&self) {
        if let Some(delay_millis) = self.delay_millis {
            thread::sleep(Duration::from_millis(delay_millis));
        }
    }
}

fn elapsed_ms(start: Instant) -> u128 {
    start.elapsed().as_micros().div_ceil(1000)
}
