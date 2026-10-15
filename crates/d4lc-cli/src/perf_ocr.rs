#[cfg(feature = "leptess-backend")]
use crate::detect::load_capture_frame;
#[cfg(feature = "leptess-backend")]
use crate::ocr::{create_ocr_registry, parse_ocr_backend};
use crate::roots::Roots;
#[cfg(feature = "leptess-backend")]
use crate::to_json_text;
#[cfg(any(test, feature = "leptess-backend"))]
use d4lc_core::coords::{FramePlacement, TooltipPlacement};
#[cfg(feature = "leptess-backend")]
use d4lc_debug::write_debug_text;
#[cfg(feature = "leptess-backend")]
use d4lc_ocr::{OcrEnginePoolStats, OcrProfile};
#[cfg(any(test, feature = "leptess-backend"))]
use d4lc_runtime::TooltipTextSnapshot;
#[cfg(feature = "leptess-backend")]
use d4lc_runtime::{process_tooltip_ocr_parity_prepared, PreparedTooltipOcr};
#[cfg(any(test, feature = "leptess-backend"))]
use d4lc_runtime::{OcrJob, OcrWorker, OcrWorkerStats, TooltipCaptureInfo, TooltipImage};
#[cfg(feature = "leptess-backend")]
use serde_json::json;
#[cfg(feature = "leptess-backend")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(test, feature = "leptess-backend"))]
use std::thread;
#[cfg(any(test, feature = "leptess-backend"))]
use std::time::{Duration, Instant};

#[cfg(feature = "leptess-backend")]
#[cfg(feature = "leptess-backend")]
const DEFAULT_PRESET: &str = "1080p_SMF";
#[cfg(feature = "leptess-backend")]
const DEFAULT_LANGUAGE: &str = "enUS";
#[cfg(feature = "leptess-backend")]
const DEFAULT_REPORT_PATH: &str = "debug-output/perf/perf-ocr.md";
#[cfg(feature = "leptess-backend")]
const DEBUG_DIR: &str = "debug-output/perf-ocr";
#[cfg(feature = "leptess-backend")]
const DEBUG_REPORT_FILE: &str = "perf-ocr.md";
#[cfg(feature = "leptess-backend")]
const DEBUG_JSON_FILE: &str = "perf-ocr.json";
#[cfg(feature = "leptess-backend")]
const STATIC_FIXTURE_ROUNDS: usize = 4;
#[cfg(feature = "leptess-backend")]
const PREVIEW_CHARS: usize = 80;
#[cfg(any(test, feature = "leptess-backend"))]
const LIVE_RETRY_INTERVAL_MS: u64 = 250;
#[cfg(any(test, feature = "leptess-backend"))]
const LIVE_FRAME_INTERVAL_MS: u64 = 16;
#[cfg(feature = "leptess-backend")]
const STATIC_FIXTURE_PATHS: &[&str] = &[
    "debug-output/perf-ocr-fixtures/live-1a/tooltip-crop-000000.png",
    "debug-output/perf-ocr-fixtures/live-1b/tooltip-crop-000000.png",
    "debug-output/perf-ocr-fixtures/live-1c/tooltip-crop-000000.png",
    "debug-output/perf-ocr-fixtures/live-1d/tooltip-crop-000000.png",
    "debug-output/perf-ocr-fixtures/real/tooltip-crop-000000.png",
];
#[cfg(feature = "leptess-backend")]
const SCHEDULED_CROP_COUNT: usize = 7;
#[cfg(feature = "leptess-backend")]
const SCHEDULED_PASSES: usize = 4;
#[cfg(feature = "leptess-backend")]
const SCHEDULED_FRAMES_PER_ENCOUNTER: usize = 18;
#[cfg(feature = "leptess-backend")]
const SCHEDULED_SIGNATURE_BASE: u64 = 1000;
#[cfg(feature = "leptess-backend")]
const SYNTHETIC_CROP_WIDTH: u32 = 497;
#[cfg(feature = "leptess-backend")]
const SYNTHETIC_CROP_HEIGHT: u32 = 740;
#[cfg(feature = "leptess-backend")]
const SYNTHETIC_CROP_BACKGROUND: u8 = 30;
#[cfg(feature = "leptess-backend")]
const STRUCTURAL_HOTPATH_EVIDENCE: &str = "d4lc-runtime e2e::ocr_worker_keeps_slow_ocr_off_the_frame_hotpath (OCR executes on the worker thread; hotpath only pays submit/poll)";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PerfOcrArgs {
    pub write_path: Option<PathBuf>,
    pub write_json_path: Option<PathBuf>,
    pub preset: Option<String>,
    pub language: Option<String>,
    pub backend: Option<String>,
    pub tessdata_path: Option<PathBuf>,
    /// Use synthetic marker-composed tooltip crops instead of the five live
    /// capture fixture PNGs. The report labels the provenance either way.
    pub synthetic_crops: bool,
}

#[cfg(feature = "leptess-backend")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureProvenance {
    /// The five `debug-output/perf-ocr-fixtures/*/tooltip-crop-000000.png` live-capture crops.
    LiveCaptureFixtures,
    /// Synthetic tooltip crops composed from preset marker templates.
    SyntheticCrops,
}

#[cfg(feature = "leptess-backend")]
impl FixtureProvenance {
    fn as_str(self) -> &'static str {
        match self {
            FixtureProvenance::LiveCaptureFixtures => "live-capture-fixtures",
            FixtureProvenance::SyntheticCrops => "synthetic-crops",
        }
    }
}

#[cfg(feature = "leptess-backend")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct StaticFixtureSample {
    label: String,
    ocr_ms: u64,
    conversion_ms: u64,
    /// Harness wall time around the whole OCR execution (setup, crop
    /// preparation, marker detection, Tesseract and conversion).
    execute_ms: u64,
    segment_count: usize,
    line_count: usize,
}

#[cfg(feature = "leptess-backend")]
#[derive(Debug)]
struct StaticFixtureBenchmark {
    /// All samples including the warmup round (round 1).
    samples: Vec<StaticFixtureSample>,
    /// Samples excluding the warmup round.
    counted: Vec<StaticFixtureSample>,
    /// Wall time of the once-per-session `PreparedTooltipOcr::prepare`.
    prepare_ms: u64,
    pool_stats: OcrEnginePoolStats,
    provenance: FixtureProvenance,
}

#[cfg(any(test, feature = "leptess-backend"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LiveWorkerFixtureFrame {
    signature: Option<u64>,
}

#[cfg(any(test, feature = "leptess-backend"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct OcrSubmissionSchedulerStats {
    pub submitted: u64,
    pub skipped_unchanged_crops: u64,
}

#[cfg(any(test, feature = "leptess-backend"))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OcrSubmissionScheduler {
    last_seen_signature: Option<u64>,
    stable_frames: u32,
    last_submitted_signature: Option<u64>,
    last_completed_signature: Option<u64>,
    queued_signature: Option<u64>,
    last_submit_at: Option<Instant>,
    submitted: u64,
    skipped_unchanged_crops: u64,
}

#[cfg(any(test, feature = "leptess-backend"))]
impl OcrSubmissionScheduler {
    const REQUIRED_STABLE_FRAMES: u32 = 2;

    pub(crate) fn observe_completed(&mut self, signature: u64) {
        self.last_completed_signature = Some(signature);
        if self.queued_signature == Some(signature) {
            self.queued_signature = None;
        }
    }

    pub(crate) fn observe_failed(&mut self, signature: u64) {
        if self.queued_signature == Some(signature) {
            self.queued_signature = None;
        }
    }

    pub(crate) fn observe_absent(&mut self) {
        self.last_seen_signature = None;
        self.stable_frames = 0;
        self.queued_signature = None;
        self.last_submitted_signature = None;
        self.last_completed_signature = None;
        self.last_submit_at = None;
    }

    pub(crate) fn should_submit(
        &mut self,
        signature: u64,
        worker_busy: bool,
        retry_interval: Duration,
        now: Instant,
    ) -> bool {
        if self.last_seen_signature == Some(signature) {
            self.stable_frames = self.stable_frames.saturating_add(1);
        } else {
            self.last_seen_signature = Some(signature);
            self.stable_frames = 1;
        }

        if self.stable_frames < Self::REQUIRED_STABLE_FRAMES {
            return false;
        }

        if self.queued_signature == Some(signature) {
            self.skipped_unchanged_crops = self.skipped_unchanged_crops.saturating_add(1);
            return false;
        }

        if self.last_completed_signature == Some(signature) {
            self.skipped_unchanged_crops = self.skipped_unchanged_crops.saturating_add(1);
            return false;
        }

        if worker_busy && self.last_submitted_signature == Some(signature) {
            self.skipped_unchanged_crops = self.skipped_unchanged_crops.saturating_add(1);
            return false;
        }

        if self.last_submitted_signature == Some(signature)
            && self
                .last_submit_at
                .map(|submitted_at| now.duration_since(submitted_at) < retry_interval)
                .unwrap_or(false)
        {
            self.skipped_unchanged_crops = self.skipped_unchanged_crops.saturating_add(1);
            return false;
        }

        self.last_submitted_signature = Some(signature);
        self.queued_signature = Some(signature);
        self.last_submit_at = Some(now);
        self.submitted = self.submitted.saturating_add(1);
        true
    }

    pub(crate) fn stats(&self) -> OcrSubmissionSchedulerStats {
        OcrSubmissionSchedulerStats {
            submitted: self.submitted,
            skipped_unchanged_crops: self.skipped_unchanged_crops,
        }
    }
}

#[cfg(feature = "leptess-backend")]
#[derive(Debug)]
struct ScheduledWorkerBenchmark {
    /// OCR wall time (ms) per completed job as reported by the parity path.
    ocr_ms_samples: Vec<u64>,
    /// Conversion wall time (ms) per completed job.
    conversion_ms_samples: Vec<u64>,
    /// Submit-to-result latency (ms), sampled at idle-worker submissions only.
    completion_ms_samples: Vec<u64>,
    worker_stats: OcrWorkerStats,
    scheduler_stats: OcrSubmissionSchedulerStats,
    crop_count: usize,
    frames: usize,
    pool_stats: OcrEnginePoolStats,
    provenance: FixtureProvenance,
}

#[cfg(any(test, feature = "leptess-backend"))]
#[derive(Debug, Clone)]
struct LiveWorkerFixtureBenchmark {
    scheduler_stats: OcrSubmissionSchedulerStats,
    worker_stats: OcrWorkerStats,
    ocr_ms_samples: Vec<u64>,
}

#[cfg(feature = "leptess-backend")]
pub fn perf_ocr_report(roots: &Roots, args: &PerfOcrArgs) -> Result<String, String> {
    let report_path = args
        .write_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REPORT_PATH));
    let json_path = args.write_json_path.clone();
    let preset = args
        .preset
        .clone()
        .unwrap_or_else(|| DEFAULT_PRESET.to_string());
    let language = args
        .language
        .clone()
        .unwrap_or_else(|| DEFAULT_LANGUAGE.to_string());
    let backend = parse_ocr_backend(args.backend.as_deref())?;
    let profile = OcrProfile {
        backend,
        language,
        tessdata_path: args.tessdata_path.clone(),
    };
    let registry = create_ocr_registry(profile.resolved_backend())?;

    let crops = load_benchmark_crops(roots, &preset, args.synthetic_crops)?;
    let provenance = if args.synthetic_crops {
        FixtureProvenance::SyntheticCrops
    } else {
        FixtureProvenance::LiveCaptureFixtures
    };

    let static_fixtures =
        run_static_fixture_benchmark(roots, &preset, &profile, &registry, &crops, provenance)?;
    let scheduled_worker =
        run_scheduled_worker_benchmark(roots, &preset, &profile, &crops, provenance)?;
    let live_worker = run_live_worker_fixture_benchmark()?;
    let environment = gather_environment(&roots.assets);

    let report = render_report(
        &report_path,
        &preset,
        &profile,
        &environment,
        &static_fixtures,
        &scheduled_worker,
        &live_worker,
    );
    let report_json = render_report_json(
        json_path.as_deref().unwrap_or(&report_path),
        &preset,
        &profile,
        &environment,
        &static_fixtures,
        &scheduled_worker,
        &live_worker,
    );

    let debug_dir = roots.work_path(Path::new(DEBUG_DIR));
    write_debug_text(&debug_dir.join(DEBUG_JSON_FILE), &report_json)?;
    write_debug_text(&debug_dir.join(DEBUG_REPORT_FILE), &report)?;
    if let Some(json_path) = &json_path {
        write_debug_text(&roots.work_path(json_path), &report_json)?;
    }
    write_debug_text(&roots.work_path(&report_path), &report)?;

    Ok(report)
}

#[cfg(not(feature = "leptess-backend"))]
pub fn perf_ocr_report(_roots: &Roots, _args: &PerfOcrArgs) -> Result<String, String> {
    Err(
        "perf-ocr-report requires the leptess-backend feature; rebuild with --features leptess-backend and install libtesseract-dev/libleptonica-dev"
            .to_string(),
    )
}

/// The OCR session of the app's two roots (config root, asset root).
#[cfg(feature = "leptess-backend")]
fn prepare_benchmark_ocr(
    roots: &Roots,
    preset: &str,
    profile: &OcrProfile,
) -> Result<PreparedTooltipOcr, String> {
    PreparedTooltipOcr::prepare_with_game_data_root(
        roots.config_path()?,
        &roots.assets,
        preset.to_string(),
        profile.clone(),
    )
}

/// Load the OCR benchmark crops. `--synthetic-crops` composes tooltip crops
/// from the preset marker templates; otherwise the five live-capture fixture
/// PNGs are required (missing files fail with an actionable error).
#[cfg(feature = "leptess-backend")]
fn load_benchmark_crops(
    roots: &Roots,
    preset: &str,
    synthetic: bool,
) -> Result<Vec<d4lc_core::CaptureFrame>, String> {
    if synthetic {
        build_synthetic_tooltip_crops(&roots.assets, preset)
    } else {
        // Earlier live captures wrote these crops below the current directory.
        let missing = STATIC_FIXTURE_PATHS
            .iter()
            .filter(|path| !roots.work_path(Path::new(path)).is_file())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let missing_list = missing
                .iter()
                .map(|path| path.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "live-capture OCR fixtures are missing ({}); they are generated by prior live captures and are not committed. Re-run with --synthetic-crops to compose synthetic tooltip crops from the preset marker templates, or restore the fixtures on a quiet machine: {}",
                missing.len(),
                missing_list
            ));
        }
        STATIC_FIXTURE_PATHS
            .iter()
            .map(|path| load_capture_frame(&roots.work_path(Path::new(path))))
            .collect()
    }
}

/// Compose `SCHEDULED_CROP_COUNT` synthetic tooltip crops: a dark tooltip-sized
/// background with real preset marker templates pasted at live-like
/// positions. Marker detection binarizes haystack and template with the same
/// thresholds, so the pasted templates match exactly and drive real segment
/// planning (upper tooltip, affix areas, aspect area) and real Tesseract work.
///
/// Synthetic runs measure the real code path, never hardware or text-dense
/// tooltip latency; reports must label this provenance.
#[cfg(feature = "leptess-backend")]
fn build_synthetic_tooltip_crops(
    root: &Path,
    preset: &str,
) -> Result<Vec<d4lc_core::CaptureFrame>, String> {
    let preset_dir = root.join("assets/original/systempresets").join(preset);
    let affix_dot = load_dot_template(&preset_dir, "dot-affixes_normal.png")?;
    let aspect_dot = load_dot_template(&preset_dir, "dot-aspects_legendary.png")?;
    let top_splitter = load_dot_template(&preset_dir, "dot-splitter_top_1.png")?;

    let mut crops = Vec::with_capacity(SCHEDULED_CROP_COUNT);
    for index in 0..SCHEDULED_CROP_COUNT {
        let index = index as u32;
        let mut frame = synthetic_background_frame();
        // Top splitter inside the splitter ROI strip (center ± width/14).
        paste_dot(&mut frame, &top_splitter, 230, 8)?;
        // affix dots in the left marker strip (width / 7).
        for affix_index in 0..=index {
            paste_dot(&mut frame, &affix_dot, 8, 80 + affix_index * 80)?;
        }
        // aspect dot below the last affix dot.
        let aspect_y = 80 + (index + 1) * 80 + 40;
        paste_dot(&mut frame, &aspect_dot, 8, aspect_y)?;
        crops.push(frame);
    }
    Ok(crops)
}

#[cfg(feature = "leptess-backend")]
fn load_dot_template(preset_dir: &Path, file_name: &str) -> Result<image::GrayImage, String> {
    let path = preset_dir.join(file_name);
    let image = image::ImageReader::open(&path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?
        .decode()
        .map_err(|error| format!("failed to decode {}: {error}", path.display()))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let gray = d4lc_vision::gray_from_rgba8(width, height, image.as_raw());
    image::GrayImage::from_raw(width, height, gray.pixels)
        .ok_or_else(|| format!("{}: gray buffer size mismatch", path.display()))
}

#[cfg(feature = "leptess-backend")]
fn synthetic_background_frame() -> d4lc_core::CaptureFrame {
    let width = SYNTHETIC_CROP_WIDTH;
    let height = SYNTHETIC_CROP_HEIGHT;
    let pixels = vec![
        SYNTHETIC_CROP_BACKGROUND,
        SYNTHETIC_CROP_BACKGROUND,
        SYNTHETIC_CROP_BACKGROUND,
        255,
    ]
    .into_iter()
    .cycle()
    .take((width as usize) * (height as usize) * 4)
    .collect::<Vec<_>>();
    d4lc_core::CaptureFrame {
        width,
        height,
        stride: (width as usize) * 4,
        pixels_bgra: pixels,
    }
}

/// Paste a dot template's raw luma values into the BGRA frame. The parity path
/// binarizes frame and templates with the same `threshold_binary_inv`, so the
/// pasted region matches its template exactly.
#[cfg(feature = "leptess-backend")]
fn paste_dot(
    frame: &mut d4lc_core::CaptureFrame,
    dot: &image::GrayImage,
    x: u32,
    y: u32,
) -> Result<(), String> {
    let width = i32::try_from(frame.width).unwrap_or(0);
    let height = i32::try_from(frame.height).unwrap_or(0);
    if x + dot.width() > frame.width || y + dot.height() > frame.height {
        return Err(format!(
            "synthetic dot placement ({x},{y})+{}x{} exceeds frame {}x{}",
            dot.width(),
            dot.height(),
            frame.width,
            frame.height
        ));
    }
    for (row, dot_row) in dot.rows().enumerate() {
        let frame_y = y as usize + row;
        for (column, pixel) in dot_row.enumerate() {
            let frame_x = x as usize + column;
            let start = frame_y * frame.stride + frame_x * 4;
            frame.pixels_bgra[start..start + 4]
                .copy_from_slice(&[pixel.0[0], pixel.0[0], pixel.0[0], 255]);
        }
    }
    debug_assert!(width > 0 && height > 0);
    Ok(())
}

/// Prepare the OCR session state once per benchmark (settings + template
/// images), mirroring the live worker's prepare-once strategy; every measured
/// job then runs `process_tooltip_ocr_parity_prepared` — exactly what the live
/// OCR worker executes per job.
#[cfg(feature = "leptess-backend")]
fn run_static_fixture_benchmark(
    roots: &Roots,
    preset: &str,
    profile: &OcrProfile,
    registry: &d4lc_ocr::OcrEngineRegistry,
    crops: &[d4lc_core::CaptureFrame],
    provenance: FixtureProvenance,
) -> Result<StaticFixtureBenchmark, String> {
    let prepare_start = Instant::now();
    let ocr_session = prepare_benchmark_ocr(roots, preset, profile)?;
    let prepare_ms = elapsed_ms(prepare_start);

    let mut samples = Vec::new();
    for _round in 0..STATIC_FIXTURE_ROUNDS {
        for (index, frame) in crops.iter().enumerate() {
            let label = fixture_label(provenance, index);
            let start = Instant::now();
            let parity = process_tooltip_ocr_parity_prepared(
                &ocr_session,
                registry,
                frame,
                PREVIEW_CHARS,
                &TooltipPlacement::whole_frame(
                    FramePlacement::default(),
                    frame.width,
                    frame.height,
                ),
            )?;
            let execute_ms = elapsed_ms(start);
            samples.push(StaticFixtureSample {
                label,
                ocr_ms: parity.ocr_ms,
                conversion_ms: parity.conversion_ms,
                execute_ms,
                segment_count: parity.segment_count,
                line_count: parity.line_count,
            });
        }
    }

    let counted = samples[crops.len()..].to_vec();
    let pool = registry
        .pool_for(profile.clone())
        .map_err(|error| error.to_string())?;
    let pool_stats = pool.stats().map_err(|error| error.to_string())?;

    Ok(StaticFixtureBenchmark {
        samples,
        counted,
        prepare_ms,
        pool_stats,
        provenance,
    })
}

#[cfg(feature = "leptess-backend")]
fn fixture_label(provenance: FixtureProvenance, index: usize) -> String {
    match provenance {
        FixtureProvenance::LiveCaptureFixtures => STATIC_FIXTURE_PATHS
            .get(index)
            .map(|path| (*path).to_string())
            .unwrap_or_else(|| format!("fixture-{index}")),
        FixtureProvenance::SyntheticCrops => format!("synthetic-tooltip-crop-{index}"),
    }
}

/// Scheduled live-worker benchmark: replays tooltip encounters through the real
/// `OcrSubmissionScheduler` and the real `OcrWorker` (capture-free: the crop
/// comes from the loaded fixture set instead of a live capture). Each encounter
/// feeds `SCHEDULED_FRAMES_PER_ENCOUNTER` stable frames (16 ms cadence), then
/// the tooltip disappears (`observe_absent`) before the next crop.
///
/// Owns its `OcrEngineRegistry` so the closure is `'static`; its engine pool
/// counters are reported separately from the static scenario.
#[cfg(feature = "leptess-backend")]
fn run_scheduled_worker_benchmark(
    roots: &Roots,
    preset: &str,
    profile: &OcrProfile,
    crops: &[d4lc_core::CaptureFrame],
    provenance: FixtureProvenance,
) -> Result<ScheduledWorkerBenchmark, String> {
    let registry = std::sync::Arc::new(create_ocr_registry(profile.resolved_backend())?);
    let post_run_profile = profile.clone();
    let worker_registry = std::sync::Arc::clone(&registry);
    let mut worker_crops: Vec<d4lc_core::CaptureFrame> = Vec::new();
    for crop in crops.iter().take(SCHEDULED_CROP_COUNT) {
        worker_crops.push(crop.clone());
    }
    let crop_count = worker_crops.len();
    let signature_base = SCHEDULED_SIGNATURE_BASE;
    // Prepare the OCR session state once per worker, exactly like the live
    // `--ocr-worker` path.
    let ocr_session = prepare_benchmark_ocr(roots, preset, profile)?;

    let worker = OcrWorker::spawn(move |job: OcrJob| {
        let parity = process_tooltip_ocr_parity_prepared(
            &ocr_session,
            worker_registry.as_ref(),
            job.image.pixels(),
            PREVIEW_CHARS,
            &job.image.capture().placement,
        )?;
        Ok(TooltipTextSnapshot {
            sequence: 0, // set by the worker
            source_frame_sequence: job.frame_id(),
            tooltip_signature: job.tooltip_signature,
            content_fingerprint: job.content_fingerprint(),
            token: job.token,
            ocr_read_top: parity.read_top,
            ocr_read_extent: parity.read_extent,
            parity_path: parity.parity_path.to_string(),
            ocr_ms: parity.ocr_ms,
            capture_ms: 0,
            conversion_ms: parity.conversion_ms,
            splitter_detection_ms: Some(parity.splitter_detection_ms),
            segment_count: parity.segment_count,
            line_count: parity.line_count,
            raw_text: parity.raw_text,
            normalized_lines: parity.normalized_lines,
            segments_preview: parity
                .segments_preview
                .into_iter()
                .map(|segment| d4lc_runtime::TooltipTextSegmentSnapshot {
                    index: segment.index,
                    segment_kind: segment.segment_kind,
                    line_count: segment.line_count,
                    raw_text_preview: segment.raw_text_preview,
                    converted_text_preview: segment.converted_text_preview,
                })
                .collect(),
            title_preview: parity.title_preview,
            text_preview: parity.text_preview,
            item: Some(parity.item),
            error: None,
            debug: None,
        })
    });

    let mut scheduler = OcrSubmissionScheduler::default();
    let mut ocr_ms_samples = Vec::new();
    let mut conversion_ms_samples = Vec::new();
    let mut completion_ms_samples = Vec::new();
    let mut last_submit_at: Option<Instant> = None;
    let mut last_submit_started_idle = false;
    let retry_interval = Duration::from_millis(LIVE_RETRY_INTERVAL_MS);
    let frame_interval = Duration::from_millis(LIVE_FRAME_INTERVAL_MS);
    let mut frames = 0usize;

    for pass in 0..SCHEDULED_PASSES {
        if pass > 0 {
            // The tooltip re-occurs later: reset per-signature memory the same
            // way the live loop does when the tooltip disappears.
            scheduler.observe_absent();
        }
        for (crop_index, crop) in worker_crops.iter().enumerate() {
            let signature = signature_base + crop_index as u64;
            for _ in 0..SCHEDULED_FRAMES_PER_ENCOUNTER {
                frames += 1;
                if let Some(result) = worker.poll_latest_result() {
                    if result.error.is_none() {
                        scheduler.observe_completed(result.tooltip_signature);
                    } else {
                        scheduler.observe_failed(result.tooltip_signature);
                    }
                    if last_submit_started_idle {
                        if let Some(submitted_at) = last_submit_at {
                            completion_ms_samples.push(elapsed_ms(submitted_at));
                        }
                    }
                    last_submit_started_idle = false;
                    ocr_ms_samples.push(result.ocr_ms);
                    conversion_ms_samples.push(result.conversion_ms);
                }
                let busy = worker.is_busy();
                if scheduler.should_submit(signature, busy, retry_interval, Instant::now()) {
                    last_submit_at = Some(Instant::now());
                    last_submit_started_idle = !busy;
                    worker.submit(fixture_job(frames as u64, signature, crop.clone()));
                }
                thread::sleep(frame_interval);
            }
        }
    }

    drain_scheduled_worker(
        &worker,
        &mut scheduler,
        &mut ocr_ms_samples,
        &mut conversion_ms_samples,
        &mut completion_ms_samples,
        &mut last_submit_started_idle,
        last_submit_at,
    )?;

    let worker_stats = worker.stats();
    let scheduler_stats = scheduler.stats();
    let pool = registry
        .pool_for(post_run_profile)
        .map_err(|error| error.to_string())?;
    let pool_stats = pool.stats().map_err(|error| error.to_string())?;

    Ok(ScheduledWorkerBenchmark {
        ocr_ms_samples,
        conversion_ms_samples,
        completion_ms_samples,
        worker_stats,
        scheduler_stats,
        crop_count,
        frames,
        pool_stats,
        provenance,
    })
}

#[cfg(feature = "leptess-backend")]
#[allow(clippy::too_many_arguments)]
fn drain_scheduled_worker(
    worker: &OcrWorker,
    scheduler: &mut OcrSubmissionScheduler,
    ocr_ms_samples: &mut Vec<u64>,
    conversion_ms_samples: &mut Vec<u64>,
    completion_ms_samples: &mut Vec<u64>,
    last_submit_started_idle: &mut bool,
    last_submit_at: Option<Instant>,
) -> Result<(), String> {
    for _ in 0..2000 {
        if let Some(result) = worker.poll_latest_result() {
            if result.error.is_none() {
                scheduler.observe_completed(result.tooltip_signature);
            } else {
                scheduler.observe_failed(result.tooltip_signature);
            }
            if *last_submit_started_idle {
                if let Some(submitted_at) = last_submit_at {
                    completion_ms_samples.push(elapsed_ms(submitted_at));
                }
            }
            *last_submit_started_idle = false;
            ocr_ms_samples.push(result.ocr_ms);
            conversion_ms_samples.push(result.conversion_ms);
        }
        let stats = worker.stats();
        if stats.submitted <= stats.completed.saturating_add(stats.failed + stats.dropped) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(2));
    }
    Err("scheduled worker benchmark did not drain within the bounded wait window".to_string())
}

#[cfg(any(test, feature = "leptess-backend"))]
fn observe_scheduler_result(scheduler: &mut OcrSubmissionScheduler, result: &TooltipTextSnapshot) {
    if result.error.is_none() {
        scheduler.observe_completed(result.tooltip_signature);
    } else {
        scheduler.observe_failed(result.tooltip_signature);
    }
}

#[cfg(any(test, feature = "leptess-backend"))]
fn run_live_worker_fixture_benchmark() -> Result<LiveWorkerFixtureBenchmark, String> {
    let frames = live_worker_fixture_frames();
    let mut scheduler = OcrSubmissionScheduler::default();
    let worker = OcrWorker::spawn(|job| Ok(fake_worker_snapshot(&job)));
    let mut results = Vec::new();
    let retry_interval = Duration::from_millis(LIVE_RETRY_INTERVAL_MS);
    let start = Instant::now();
    for (frame_index, frame) in frames.iter().enumerate() {
        if let Some(result) = worker.poll_latest_result() {
            observe_scheduler_result(&mut scheduler, &result);
            results.push(result.ocr_ms);
        }

        if let Some(signature) = frame.signature {
            let now = start
                .checked_add(Duration::from_millis(
                    (frame_index as u64).saturating_mul(LIVE_FRAME_INTERVAL_MS),
                ))
                .unwrap_or(start);
            if scheduler.should_submit(signature, worker.is_busy(), retry_interval, now) {
                worker.submit(fixture_job(
                    frame_index as u64,
                    signature,
                    d4lc_core::CaptureFrame {
                        width: 1,
                        height: 1,
                        stride: 4,
                        pixels_bgra: vec![0, 0, 0, 255],
                    },
                ));
                drain_worker_until_idle(&worker, &mut scheduler, &mut results)?;
            }
        } else {
            scheduler.observe_absent();
        }
    }

    for _ in 0..200 {
        if let Some(result) = worker.poll_latest_result() {
            observe_scheduler_result(&mut scheduler, &result);
            results.push(result.ocr_ms);
        }
        let stats = worker.stats();
        if stats
            .completed
            .saturating_add(stats.failed)
            .saturating_add(stats.dropped)
            >= stats.submitted
        {
            while let Some(result) = worker.poll_latest_result() {
                observe_scheduler_result(&mut scheduler, &result);
                results.push(result.ocr_ms);
            }
            return Ok(LiveWorkerFixtureBenchmark {
                scheduler_stats: scheduler.stats(),
                worker_stats: stats,
                ocr_ms_samples: results,
            });
        }
        thread::sleep(Duration::from_millis(1));
    }

    Err("live worker fixture did not drain within the bounded wait window".to_string())
}

#[cfg(any(test, feature = "leptess-backend"))]
fn drain_worker_until_idle(
    worker: &OcrWorker,
    scheduler: &mut OcrSubmissionScheduler,
    results: &mut Vec<u64>,
) -> Result<(), String> {
    for _ in 0..200 {
        if let Some(result) = worker.poll_latest_result() {
            observe_scheduler_result(scheduler, &result);
            results.push(result.ocr_ms);
        }
        if !worker.is_busy() {
            if let Some(result) = worker.poll_latest_result() {
                observe_scheduler_result(scheduler, &result);
                results.push(result.ocr_ms);
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(1));
    }

    Err(
        "live worker fixture did not reach an idle state within the bounded wait window"
            .to_string(),
    )
}

#[cfg(any(test, feature = "leptess-backend"))]
fn live_worker_fixture_frames() -> Vec<LiveWorkerFixtureFrame> {
    vec![
        LiveWorkerFixtureFrame {
            signature: Some(11),
        },
        LiveWorkerFixtureFrame {
            signature: Some(11),
        },
        LiveWorkerFixtureFrame {
            signature: Some(11),
        },
        LiveWorkerFixtureFrame {
            signature: Some(11),
        },
        LiveWorkerFixtureFrame {
            signature: Some(27),
        },
        LiveWorkerFixtureFrame {
            signature: Some(27),
        },
        LiveWorkerFixtureFrame { signature: None },
        LiveWorkerFixtureFrame {
            signature: Some(43),
        },
        LiveWorkerFixtureFrame {
            signature: Some(43),
        },
        LiveWorkerFixtureFrame {
            signature: Some(43),
        },
        LiveWorkerFixtureFrame {
            signature: Some(59),
        },
        LiveWorkerFixtureFrame {
            signature: Some(59),
        },
    ]
}

/// A job over a fixture crop that stands for a whole 1:1 window capture.
#[cfg(any(test, feature = "leptess-backend"))]
fn fixture_job(frame_id: u64, tooltip_signature: u64, pixels: d4lc_core::CaptureFrame) -> OcrJob {
    let (width, height) = (pixels.width, pixels.height);
    OcrJob {
        tooltip_signature,
        token: d4lc_runtime::AnalysisToken::default(),
        image: TooltipImage::new(
            pixels,
            TooltipCaptureInfo {
                frame_id,
                captured_at_micros: 0,
                placement: TooltipPlacement::whole_frame(FramePlacement::default(), width, height),
            },
        ),
    }
}

#[cfg(any(test, feature = "leptess-backend"))]
fn fake_worker_snapshot(job: &OcrJob) -> TooltipTextSnapshot {
    let ocr_ms = match job.tooltip_signature {
        11 => 8,
        27 => 12,
        43 => 9,
        59 => 15,
        _ => 6,
    };
    TooltipTextSnapshot {
        sequence: 0,
        source_frame_sequence: job.frame_id(),
        tooltip_signature: job.tooltip_signature,
        content_fingerprint: job.content_fingerprint(),
        token: job.token,
        ocr_read_top: None,
        ocr_read_extent: None,
        parity_path: "perf-ocr-fixture".to_string(),
        ocr_ms,
        capture_ms: 0,
        conversion_ms: 1,
        splitter_detection_ms: Some(0),
        segment_count: 1,
        line_count: 1,
        raw_text: format!("fixture-{}", job.tooltip_signature),
        normalized_lines: vec![format!("fixture-{}", job.tooltip_signature)],
        segments_preview: Vec::new(),
        title_preview: Some(format!("fixture-{}", job.tooltip_signature)),
        text_preview: format!("fixture-{}", job.tooltip_signature),
        item: None,
        error: None,
        debug: None,
    }
}

// ---------------------------------------------------------------------------
// Environment metadata (performance-superiority report schema)
// ---------------------------------------------------------------------------

#[cfg(feature = "leptess-backend")]
struct EnvironmentInfo {
    os: String,
    kernel: String,
    cpu: String,
    cores: usize,
    ram_gb: u64,
    session: String,
    compositor: String,
    display: String,
    build_mode: &'static str,
    binary_size_bytes: u64,
    repo_commit: String,
    background_load: String,
}

#[cfg(feature = "leptess-backend")]
fn gather_environment(root: &Path) -> EnvironmentInfo {
    let cores = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(0);
    EnvironmentInfo {
        os: read_os_name(),
        kernel: read_first_line("/proc/sys/kernel/osrelease"),
        cpu: read_cpu_model(),
        cores,
        ram_gb: read_ram_gb(),
        session: read_session_type(),
        compositor: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string()),
        display: "n/a (fixture/synthetic run, no live capture)".to_string(),
        build_mode: "release",
        binary_size_bytes: current_exe_size(),
        repo_commit: read_repo_commit(root),
        background_load: std::env::var("D4LC_PERF_BACKGROUND_LOAD")
            .unwrap_or_else(|_| "not recorded".to_string()),
    }
}

#[cfg(feature = "leptess-backend")]
fn read_os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("PRETTY_NAME="))
                .map(|line| {
                    line.trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string()
                })
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

#[cfg(feature = "leptess-backend")]
fn read_first_line(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|text| text.lines().next().unwrap_or_default().trim().to_string())
        .unwrap_or_else(|_| "unavailable".to_string())
}

#[cfg(feature = "leptess-backend")]
fn read_cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("model name"))
                .and_then(|line| line.split(':').next_back())
                .map(|value| value.trim().to_string())
        })
        .unwrap_or_else(|| "unavailable".to_string())
}

#[cfg(feature = "leptess-backend")]
fn read_ram_gb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let rest = line.strip_prefix("MemTotal:")?;
                let value = rest
                    .trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()?;
                Some(value / (1024 * 1024))
            })
        })
        .unwrap_or(0)
}

#[cfg(feature = "leptess-backend")]
fn read_session_type() -> String {
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        "Wayland".to_string()
    } else if std::env::var("DISPLAY").is_ok() {
        "X11".to_string()
    } else {
        "headless".to_string()
    }
}

#[cfg(feature = "leptess-backend")]
fn current_exe_size() -> u64 {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

#[cfg(feature = "leptess-backend")]
fn read_repo_commit(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| "unavailable".to_string())
}

// ---------------------------------------------------------------------------
// Report rendering
// ---------------------------------------------------------------------------

#[cfg(feature = "leptess-backend")]
fn render_report(
    report_path: &Path,
    preset: &str,
    profile: &OcrProfile,
    environment: &EnvironmentInfo,
    static_fixtures: &StaticFixtureBenchmark,
    scheduled_worker: &ScheduledWorkerBenchmark,
    live_worker: &LiveWorkerFixtureBenchmark,
) -> String {
    let static_ocr = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.ocr_ms
    }));
    let static_conversion = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.conversion_ms
    }));
    let static_execute = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.execute_ms
    }));
    let scheduled_ocr = metric_summary(&scheduled_worker.ocr_ms_samples);
    let scheduled_conversion = metric_summary(&scheduled_worker.conversion_ms_samples);
    let scheduled_completion = metric_summary(&scheduled_worker.completion_ms_samples);

    let mut output = String::new();
    output.push_str("# OCR scheduling benchmark\n\n");
    output.push_str(&format!("- report date (UTC): {}\n", chrono_utc_date()));
    output.push_str(
        "- command: `target/release/d4lc perf-ocr-report` (release build, `--features leptess-backend`)\n",
    );
    output.push_str(&format!("- report path: `{}`\n", report_path.display()));
    output.push_str(&format!(
        "- preset: `{preset}`\n- language: `{}`\n- backend: `{}`\n",
        profile.language,
        profile.resolved_backend().as_str(),
    ));
    output.push_str(&format!(
        "- fixture provenance: `{}`\n",
        static_fixtures.provenance.as_str()
    ));
    output.push_str("- upstream references: `upstream/Diablo4Companion/D4Companion.Services/OcrHandler.cs::ConvertToText`, `upstream/Diablo4Companion/D4Companion.Services/OcrHandler.cs::ConvertToTextUpperTooltipSection`, `upstream/Diablo4Companion/D4Companion.Services/OcrHandler.cs::SetLanguage`, `upstream/Diablo4Companion/D4Companion.Services/ScreenProcessHandler.cs::FindItemAffix`\n");
    output.push_str(&format!(
        "- debug artifacts: `{DEBUG_DIR}/{DEBUG_REPORT_FILE}`, `{DEBUG_DIR}/{DEBUG_JSON_FILE}`\n\n"
    ));

    output.push_str("## Environment\n\n");
    output.push_str(&format!(
        "- OS / kernel: {} / {}\n- CPU: {} ({} threads)\n- RAM: {} GiB\n",
        environment.os, environment.kernel, environment.cpu, environment.cores, environment.ram_gb
    ));
    output.push_str(&format!(
        "- session / compositor / display: {} / {} / {}\n",
        environment.session, environment.compositor, environment.display
    ));
    output.push_str(&format!(
        "- build mode: {} (binary {} bytes, commit `{}`)\n",
        environment.build_mode, environment.binary_size_bytes, environment.repo_commit
    ));
    output.push_str(&format!(
        "- OCR backend / language / psm: {} / {} / per-segment (SparseText upper tooltip, Default others)\n",
        profile.resolved_backend().as_str(),
        profile.language
    ));
    output.push_str(&format!(
        "- tessdata source: {}\n",
        profile
            .tessdata_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "system".to_string())
    ));
    output.push_str(&format!(
        "- background load: {}\n\n",
        environment.background_load
    ));

    output.push_str("## Methodology\n\n");
    output.push_str(&format!(
        "- Static crop benchmark: {} crops x {} rounds; round 1 is the warmup round after engine init and is excluded from percentiles ({} counted samples).\n",
        static_fixtures.samples.len() / STATIC_FIXTURE_ROUNDS,
        STATIC_FIXTURE_ROUNDS,
        static_fixtures.counted.len()
    ));
    output.push_str(&format!(
        "- Scheduled live-worker benchmark: {} tooltip encounters x {} passes replayed through `OcrSubmissionScheduler` + `OcrWorker` ({} frames at {} ms cadence); submission gating and dedupe identical to the live loop; the worker's capture step is replaced by the loaded fixture crop (no game interaction).\n",
        crops_len(scheduled_worker),
        SCHEDULED_PASSES,
        scheduled_worker.frames,
        LIVE_FRAME_INTERVAL_MS
    ));
    output.push_str("- Percentiles: nearest-rank over integer-millisecond samples (identical to `crates/d4lc-cli/src/live.rs::percentile_ms`), computed over counted samples only; completion latency sampled at idle-worker submissions (poll granularity 16 ms).\n");
    output.push_str(
        "- Warmup: static round 1 discarded in-process; scheduled encounters warm the engine pool during pass 1 (engine init counted separately).\n",
    );
    if static_fixtures.provenance == FixtureProvenance::SyntheticCrops {
        output.push_str("- Fixture provenance: synthetic tooltip crops composed from preset marker templates (`dot-affixes_normal.png`, `dot-aspects_legendary.png`, `dot-splitter_top_1.png`) on a dark tooltip-sized background; marker detection binarizes haystack and template identically, so pasted dots match exactly and drive real segment planning and real Tesseract work. Synthetic runs measure the code path, never hardware or text-dense tooltip latency.\n");
    } else {
        output.push_str("- Fixture provenance: the five `debug-output/perf-ocr-fixtures/*/tooltip-crop-000000.png` crops from prior live captures (fixtures are not committed; they must name their producing live run in the release report).\n");
    }
    output.push_str(&format!(
        "- OCR session prepare (settings + template images, once per worker): {} ms; every measured job executes the prepared parity path.\n",
        static_fixtures.prepare_ms
    ));
    output.push_str(&format!(
        "- Structural hotpath evidence: {STRUCTURAL_HOTPATH_EVIDENCE}\n\n"
    ));

    output.push_str("## Results — static crops\n\n");
    output.push_str("| Metric | p50 | p95 | p99 | samples |\n|---|---:|---:|---:|---:|\n");
    output.push_str(&format!(
        "| OCR ms | {} | {} | {} | {} |\n",
        static_ocr.p50,
        static_ocr.p95,
        static_ocr.p99,
        static_fixtures.counted.len()
    ));
    output.push_str(&format!(
        "| conversion ms | {} | {} | {} | {} |\n",
        static_conversion.p50,
        static_conversion.p95,
        static_conversion.p99,
        static_fixtures.counted.len()
    ));
    output.push_str(&format!(
        "| execute (setup+OCR+conversion) ms | {} | {} | {} | {} |\n\n",
        static_execute.p50,
        static_execute.p95,
        static_execute.p99,
        static_fixtures.counted.len()
    ));

    output.push_str("## Results — scheduled live worker\n\n");
    output.push_str("| Metric | p50 | p95 | p99 | samples |\n|---|---:|---:|---:|---:|\n");
    output.push_str(&format!(
        "| worker OCR ms | {} | {} | {} | {} |\n",
        scheduled_ocr.p50,
        scheduled_ocr.p95,
        scheduled_ocr.p99,
        scheduled_worker.ocr_ms_samples.len()
    ));
    output.push_str(&format!(
        "| worker conversion ms | {} | {} | {} | {} |\n",
        scheduled_conversion.p50,
        scheduled_conversion.p95,
        scheduled_conversion.p99,
        scheduled_worker.conversion_ms_samples.len()
    ));
    output.push_str(&format!(
        "| job completion ms (idle-worker submit to result) | {} | {} | {} | {} |\n\n",
        scheduled_completion.p50,
        scheduled_completion.p95,
        scheduled_completion.p99,
        scheduled_worker.completion_ms_samples.len()
    ));
    output.push_str(&format!(
        "- engine init count: {} (static) / {} (scheduled)\n- engine reuse count: {} (static) / {} (scheduled)\n- queue depth: max observed {} (single-slot latest-job-wins pending slot + in-progress job)\n- worker busy time: total {} ms, max job {} ms\n- scheduled submitted/completed/dropped/failed: {}/{}/{}/{}\n- skipped unchanged crops: {} (scheduled) / {} (scripted fixture)\n- scripted scheduler fixture (deterministic, fake OCR): submitted/completed/dropped/failed {}/{}/{}/{}\n\n",
        static_fixtures.pool_stats.initialized,
        scheduled_worker.pool_stats.initialized,
        static_fixtures.pool_stats.reused,
        scheduled_worker.pool_stats.reused,
        scheduled_worker.worker_stats.max_queue_depth,
        scheduled_worker.worker_stats.total_busy_ms,
        scheduled_worker.worker_stats.max_busy_ms,
        scheduled_worker.worker_stats.submitted,
        scheduled_worker.worker_stats.completed,
        scheduled_worker.worker_stats.dropped,
        scheduled_worker.worker_stats.failed,
        scheduled_worker.scheduler_stats.skipped_unchanged_crops,
        live_worker.scheduler_stats.skipped_unchanged_crops,
        live_worker.worker_stats.submitted,
        live_worker.worker_stats.completed,
        live_worker.worker_stats.dropped,
        live_worker.worker_stats.failed,
    ));

    output.push_str("## Budget check\n\n");
    output.push_str("| Budget | Measured | Pass | Provisional |\n|---|---|---|---|\n");
    output.push_str(&format!(
        "| OCR never inside `hotpath_total_ms` (structural) | worker-thread execution; hotpath pays submit/poll only ({STRUCTURAL_HOTPATH_EVIDENCE}) | PASS | no |\n"
    ));
    let engine_gate =
        engine_pool_growth_bounded(&static_fixtures.pool_stats, &scheduled_worker.pool_stats);
    output.push_str(&format!(
        "| zero engine re-initializations after warmup per profile (structural) | static init {}, scheduled init {} | {} | no |\n",
        static_fixtures.pool_stats.initialized,
        scheduled_worker.pool_stats.initialized,
        pass_or_fail(engine_gate)
    ));
    let dedupe_gate = live_worker.scheduler_stats.skipped_unchanged_crops > 0
        && scheduled_worker.scheduler_stats.skipped_unchanged_crops > 0;
    output.push_str(&format!(
        "| unchanged-crop dedupe active whenever duplicate crops occur (structural) | scheduled skips {}, scripted fixture skips {} | {} | no |\n",
        scheduled_worker.scheduler_stats.skipped_unchanged_crops,
        live_worker.scheduler_stats.skipped_unchanged_crops,
        pass_or_fail(dedupe_gate)
    ));
    if scheduled_worker.provenance == FixtureProvenance::SyntheticCrops {
        output.push_str(
            "| scheduled OCR job completion latency | (see scheduled table) | NOT LIVE-MEASURED — synthetic content cannot validate the release budget | yes |\n\n",
        );
    } else {
        output.push_str(
            "| scheduled OCR job completion latency | (see scheduled table) | first live measurement — fixes the provisional budget | yes |\n\n",
        );
    }

    output.push_str("## Bottlenecks and recommendations\n\n");
    for recommendation in recommendations(static_fixtures, scheduled_worker, live_worker) {
        output.push_str(&format!("- {recommendation}\n"));
    }
    output
}

#[cfg(feature = "leptess-backend")]
fn crops_len(benchmark: &ScheduledWorkerBenchmark) -> usize {
    benchmark.crop_count
}

#[cfg(feature = "leptess-backend")]
#[cfg(feature = "leptess-backend")]
fn pass_or_fail(pass: bool) -> &'static str {
    if pass {
        "PASS"
    } else {
        "FAIL"
    }
}

#[cfg(feature = "leptess-backend")]
#[cfg(feature = "leptess-backend")]
fn engine_pool_growth_bounded(
    static_stats: &OcrEnginePoolStats,
    scheduled_stats: &OcrEnginePoolStats,
) -> bool {
    // The live runtime prewarms one engine per parallel segment worker
    // (d4lc-runtime ocr_parity::OCR_SEGMENT_THREADS) and every further job
    // must be a reuse. Growth beyond the working-set size is a re-init leak;
    // a smaller pool is fine (fewer concurrent segments than threads).
    let working_set = d4lc_runtime::ocr_parity::OCR_SEGMENT_THREADS;
    [static_stats, scheduled_stats]
        .iter()
        .all(|stats| stats.initialized as usize <= working_set && stats.reused >= stats.initialized)
}

#[cfg(feature = "leptess-backend")]
fn chrono_utc_date() -> String {
    std::process::Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

#[cfg(feature = "leptess-backend")]
fn static_vec(
    samples: &[StaticFixtureSample],
    field: impl Fn(&StaticFixtureSample) -> u64,
) -> Vec<u64> {
    samples.iter().map(field).collect()
}

#[cfg(feature = "leptess-backend")]
fn render_report_json(
    report_path: &Path,
    preset: &str,
    profile: &OcrProfile,
    environment: &EnvironmentInfo,
    static_fixtures: &StaticFixtureBenchmark,
    scheduled_worker: &ScheduledWorkerBenchmark,
    live_worker: &LiveWorkerFixtureBenchmark,
) -> String {
    let static_ocr = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.ocr_ms
    }));
    let static_conversion = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.conversion_ms
    }));
    let static_execute = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.execute_ms
    }));
    let scheduled_ocr = metric_summary(&scheduled_worker.ocr_ms_samples);
    let scheduled_conversion = metric_summary(&scheduled_worker.conversion_ms_samples);
    let scheduled_completion = metric_summary(&scheduled_worker.completion_ms_samples);
    let engine_gate =
        engine_pool_growth_bounded(&static_fixtures.pool_stats, &scheduled_worker.pool_stats);
    let dedupe_gate = live_worker.scheduler_stats.skipped_unchanged_crops > 0
        && scheduled_worker.scheduler_stats.skipped_unchanged_crops > 0;

    let scripted_ocr = metric_summary(&live_worker.ocr_ms_samples);

    to_json_text(&json!({
        "schema_version": 1,
        "command": "perf-ocr-report",
        "report_path": report_path.display().to_string(),
        "percentile_method": "nearest-rank",
        "fixture_provenance": static_fixtures.provenance.as_str(),
        "preset": preset,
        "language": profile.language,
        "backend": profile.resolved_backend().as_str(),
        "environment": {
            "os": environment.os,
            "kernel": environment.kernel,
            "cpu": environment.cpu,
            "cores": environment.cores,
            "ram_gb": environment.ram_gb,
            "session": environment.session,
            "compositor": environment.compositor,
            "display": environment.display,
            "build_mode": environment.build_mode,
            "binary_size_bytes": environment.binary_size_bytes,
            "repo_commit": environment.repo_commit,
            "background_load": environment.background_load,
        },
        "warmup": {
            "static_warmup_rounds_discarded": 1,
            "static_counted_samples": static_fixtures.counted.len(),
            "scheduled_warmup": "pass 1 warms the engine pool; engine init counted separately",
        },
        "scenarios": {
            "static_crops": {
                "samples": static_fixtures.counted.len(),
                "ocr_p50_ms": static_ocr.p50,
                "ocr_p95_ms": static_ocr.p95,
                "ocr_p99_ms": static_ocr.p99,
                "conversion_p50_ms": static_conversion.p50,
                "conversion_p95_ms": static_conversion.p95,
                "conversion_p99_ms": static_conversion.p99,
                "execute_p50_ms": static_execute.p50,
                "execute_p95_ms": static_execute.p95,
                "execute_p99_ms": static_execute.p99,
            },
            "scheduled_live_worker": {
                "frames": scheduled_worker.frames,
                "ocr_samples": scheduled_worker.ocr_ms_samples.len(),
                "ocr_p50_ms": scheduled_ocr.p50,
                "ocr_p95_ms": scheduled_ocr.p95,
                "ocr_p99_ms": scheduled_ocr.p99,
                "conversion_p50_ms": scheduled_conversion.p50,
                "conversion_p95_ms": scheduled_conversion.p95,
                "conversion_p99_ms": scheduled_conversion.p99,
                "completion_p50_ms": scheduled_completion.p50,
                "completion_p95_ms": scheduled_completion.p95,
                "completion_p99_ms": scheduled_completion.p99,
                "completion_samples": scheduled_worker.completion_ms_samples.len(),
            },
            "scripted_scheduler_fixture": {
                "ocr_p50_ms": scripted_ocr.p50,
                "ocr_p95_ms": scripted_ocr.p95,
            },
        },
        "counters": {
            "static_engine_init_count": static_fixtures.pool_stats.initialized,
            "static_engine_reuse_count": static_fixtures.pool_stats.reused,
            "scheduled_engine_init_count": scheduled_worker.pool_stats.initialized,
            "scheduled_engine_reuse_count": scheduled_worker.pool_stats.reused,
            "scheduled_worker_submitted": scheduled_worker.worker_stats.submitted,
            "scheduled_worker_completed": scheduled_worker.worker_stats.completed,
            "scheduled_worker_dropped": scheduled_worker.worker_stats.dropped,
            "scheduled_worker_failed": scheduled_worker.worker_stats.failed,
            "max_queue_depth": scheduled_worker.worker_stats.max_queue_depth,
            "worker_total_busy_ms": scheduled_worker.worker_stats.total_busy_ms,
            "worker_max_busy_ms": scheduled_worker.worker_stats.max_busy_ms,
            "scheduled_skipped_unchanged_crops":
                scheduled_worker.scheduler_stats.skipped_unchanged_crops,
            "scripted_skipped_unchanged_crops": live_worker.scheduler_stats.skipped_unchanged_crops,
        },
        "budgets": {
            "ocr_off_hotpath": {
                "pass": true,
                "provisional": false,
                "evidence": "structural: worker-thread execution; \
                    e2e::ocr_worker_keeps_slow_ocr_off_the_frame_hotpath",
            },
            "engine_reinit_zero_after_warmup": { "pass": engine_gate, "provisional": false },
            "unchanged_crop_dedupe": { "pass": dedupe_gate, "provisional": false },
            "scheduled_ocr_completion_latency": {
                "budget_p95_ms": null,
                "pass": null,
                "provisional": true,
                "live_measured": false,
                "note": "first live measurement fixes this budget; synthetic-content runs \
                    measure the code path only",
            },
        },
    }))
}

#[cfg(feature = "leptess-backend")]
fn recommendations(
    static_fixtures: &StaticFixtureBenchmark,
    scheduled_worker: &ScheduledWorkerBenchmark,
    live_worker: &LiveWorkerFixtureBenchmark,
) -> Vec<String> {
    let static_execute = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.execute_ms
    }));
    let static_ocr = metric_summary(&static_vec(&static_fixtures.counted, |sample| {
        sample.ocr_ms
    }));
    let mut items = Vec::new();
    items.push(format!(
        "Keep one `OcrEngineRegistry` per OCR profile in the live runtime. This run initialized {} static / {} scheduled engine(s) and reused {} / {} existing engine checkout(s).",
        static_fixtures.pool_stats.initialized,
        scheduled_worker.pool_stats.initialized,
        static_fixtures.pool_stats.reused,
        scheduled_worker.pool_stats.reused,
    ));
    items.push(format!(
        "Treat tooltip-signature dedupe as a release-gate counter. The scheduled benchmark skipped {} unchanged crop submissions; the deterministic scripted fixture skipped {}.",
        scheduled_worker.scheduler_stats.skipped_unchanged_crops,
        live_worker.scheduler_stats.skipped_unchanged_crops,
    ));
    let overhead = static_execute.p95.saturating_sub(static_ocr.p95);
    items.push(format!(
        "Static crop execute p95 ({} ms) minus OCR p95 ({} ms) leaves {} ms of per-job tooltip/marker/splitter template matching — per-frame upstream-parity detection inside the worker (session preparation itself is once-per-worker, measured at {} ms). Reducing it means faster detection scans, not queue policy changes.",
        static_execute.p95,
        static_ocr.p95,
        overhead,
        static_fixtures.prepare_ms,
    ));
    if scheduled_worker.provenance == FixtureProvenance::SyntheticCrops {
        items.push(
            "Scheduled-latency numbers were measured on synthetic marker-composed crops: they validate code-path behavior (gates, dedupe, engine reuse), not text-dense tooltip latency. Validate the provisional completion-latency budget on a quiet machine with the five live-capture fixtures or a real-scene run.".to_string(),
        );
    }
    items
}

#[cfg(any(test, feature = "leptess-backend"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MetricSummary {
    p50: u64,
    p95: u64,
    p99: u64,
}

#[cfg(any(test, feature = "leptess-backend"))]
fn metric_summary(values: &[u64]) -> MetricSummary {
    MetricSummary {
        p50: percentile(values, 50),
        p95: percentile(values, 95),
        p99: percentile(values, 99),
    }
}

#[cfg(any(test, feature = "leptess-backend"))]
fn percentile(values: &[u64], percentile: u32) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (((sorted.len() as f64) * (percentile as f64 / 100.0)).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[index]
}

#[cfg(feature = "leptess-backend")]
fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank() {
        assert_eq!(percentile(&[8, 12, 9, 15], 50), 9);
        assert_eq!(percentile(&[8, 12, 9, 15], 95), 15);
    }

    #[test]
    fn metric_summary_emits_p99() {
        let summary = metric_summary(&[8, 12, 9, 15]);
        assert_eq!(summary.p50, 9);
        assert_eq!(summary.p95, 15);
        assert_eq!(summary.p99, 15);
    }

    #[cfg(feature = "leptess-backend")]
    #[test]
    fn engine_pool_growth_gate_accepts_a_reused_working_set_but_not_a_leak() {
        let stats = |initialized: u64, reused: u64| OcrEnginePoolStats {
            initialized,
            reused,
            returned: reused,
            available: 0,
        };
        // Parallel segment workers each hold one engine; later jobs reuse.
        assert!(engine_pool_growth_bounded(&stats(3, 165), &stats(3, 160)));
        // A single warm engine is still fine.
        assert!(engine_pool_growth_bounded(&stats(1, 167), &stats(1, 167)));
        // Growth past the segment-thread working set is a re-init leak.
        assert!(!engine_pool_growth_bounded(
            &stats(d4lc_runtime::ocr_parity::OCR_SEGMENT_THREADS as u64 + 1, 3),
            &stats(1, 1)
        ));
        // Engines created but never reused were wasted re-initializations.
        assert!(!engine_pool_growth_bounded(&stats(2, 1), &stats(1, 1)));
    }

    #[test]
    fn live_worker_fixture_reports_skipped_unchanged_crops() {
        let benchmark = run_live_worker_fixture_benchmark()
            .unwrap_or_else(|error| panic!("fixture benchmark failed: {error}"));

        assert_eq!(benchmark.worker_stats.submitted, 4);
        assert_eq!(benchmark.worker_stats.completed, 4);
        assert_eq!(benchmark.worker_stats.failed, 0);
        assert_eq!(benchmark.scheduler_stats.submitted, 4);
        assert_eq!(benchmark.scheduler_stats.skipped_unchanged_crops, 3);
        assert_eq!(metric_summary(&benchmark.ocr_ms_samples).p95, 15);
    }

    #[test]
    #[cfg(feature = "leptess-backend")]
    fn missing_live_fixtures_fail_with_actionable_error() {
        let Err(error) = load_benchmark_crops(
            &Roots::for_tests(Path::new("/nonexistent")),
            "1080p_SMF",
            false,
        ) else {
            panic!("missing fixtures must fail");
        };
        assert!(error.contains("--synthetic-crops"), "error: {error}");
        assert!(error.contains("tooltip-crop-000000.png"), "error: {error}");
    }

    #[test]
    #[cfg(feature = "leptess-backend")]
    fn synthetic_tooltip_crops_are_distinct_valid_frames() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .join("../..")
            .canonicalize()
            .unwrap_or(manifest_dir);
        let crops = build_synthetic_tooltip_crops(&repo_root, "1080p_SMF")
            .unwrap_or_else(|error| panic!("synthetic crop build failed: {error}"));

        assert_eq!(crops.len(), SCHEDULED_CROP_COUNT);
        let mut hashes = Vec::new();
        for crop in &crops {
            assert_eq!(crop.width, SYNTHETIC_CROP_WIDTH);
            assert_eq!(crop.height, SYNTHETIC_CROP_HEIGHT);
            assert_eq!(crop.pixels_bgra.len(), (crop.stride * crop.height as usize));
            hashes.push(d4lc_runtime::hash_capture_frame(crop));
        }
        hashes.sort_unstable();
        hashes.dedup();
        assert_eq!(
            hashes.len(),
            SCHEDULED_CROP_COUNT,
            "synthetic crops must be distinct"
        );
    }

    /// Parity guard for the prepared OCR session: it must produce identical
    /// OCR results to the one-shot path for the same inputs (timing fields
    /// differ by construction).
    #[test]
    #[cfg(feature = "leptess-backend")]
    fn prepared_ocr_parity_matches_one_shot_execution() {
        use crate::ocr::parse_ocr_backend;
        use d4lc_runtime::process_tooltip_ocr_parity;

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .join("../..")
            .canonicalize()
            .unwrap_or(manifest_dir);
        let backend = parse_ocr_backend(Some("leptess"))
            .unwrap_or_else(|error| panic!("backend parse failed: {error}"));
        let profile = OcrProfile {
            backend,
            language: DEFAULT_LANGUAGE.to_string(),
            tessdata_path: None,
        };
        let crops = build_synthetic_tooltip_crops(&repo_root, "1080p_SMF")
            .unwrap_or_else(|error| panic!("synthetic crop build failed: {error}"));
        let registry = create_ocr_registry(profile.resolved_backend())
            .unwrap_or_else(|error| panic!("registry init failed: {error}"));

        for crop in crops.iter().take(3) {
            let one_shot = process_tooltip_ocr_parity(
                &repo_root,
                crop.clone(),
                "1080p_SMF".to_string(),
                profile.clone(),
                &registry,
                PREVIEW_CHARS,
            )
            .unwrap_or_else(|error| panic!("one-shot execution failed: {error}"));

            let prepared_state =
                PreparedTooltipOcr::prepare(&repo_root, "1080p_SMF".to_string(), profile.clone())
                    .unwrap_or_else(|error| panic!("prepare failed: {error}"));
            let prepared = process_tooltip_ocr_parity_prepared(
                &prepared_state,
                &registry,
                crop,
                PREVIEW_CHARS,
                &TooltipPlacement::whole_frame(FramePlacement::default(), crop.width, crop.height),
            )
            .unwrap_or_else(|error| panic!("prepared execution failed: {error}"));

            assert_eq!(one_shot.raw_text, prepared.raw_text);
            assert_eq!(one_shot.normalized_lines, prepared.normalized_lines);
            assert_eq!(one_shot.segment_count, prepared.segment_count);
            assert_eq!(one_shot.line_count, prepared.line_count);
            assert_eq!(one_shot.title_preview, prepared.title_preview);
            assert_eq!(one_shot.text_preview, prepared.text_preview);
            assert_eq!(
                one_shot.segments_preview, prepared.segments_preview,
                "segment plans must be identical"
            );
        }
    }
}
