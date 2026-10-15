//! Offline performance baseline benchmarks.
//!
//! `perf-detect-offline` replays the detection stage over real fixture
//! frames (headless: no capture backend, no display). `perf-fuzzy` replays
//! the catalog labels plus near-miss variants through the fuzzy matcher.
//! Both report nearest-rank p50/p95 and write JSON into
//! `debug-output/perf/baseline/` by default; `--write` overrides.
//!
//! Corpus note: there is no OCR ground-truth corpus and no 2560×1440
//! capture frame in the repository, so the fuzzy corpus is the catalog
//! labels plus edited variants, and the detect benchmark covers the
//! 1920×1080 fixture frames only. Both gaps are stated in the reports.

use crate::detect::{load_capture_frame, prepare_detector, resolve_detection_config};
use crate::roots::Roots;
use crate::to_json_text;
use d4lc_assets::AssetRepository;
use d4lc_core::CaptureFrame;
use d4lc_debug::write_debug_text;
use d4lc_detect::prepared::{detect_roi_frame_prepared, RoiFrame};
use d4lc_fuzzy::{FuzzyCategory, FuzzyIndexes};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

const DEFAULT_BASELINE_DIR: &str = "debug-output/perf/baseline";
const DEFAULT_DETECT_RUNS: u64 = 50;
const WARMUP_RUNS: u64 = 3;
const DEFAULT_DETECT_FRAMES: &[&str] = &[
    "tests/fixtures/live-frames/d4lc-livecap.png",
    "tests/fixtures/live-frames/frame-CTRL.png",
    "tests/fixtures/live-frames/frame-KB.png",
];
const FUZZY_EDIT_VARIANTS_PER_LABEL: usize = 3;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PerfDetectOfflineArgs {
    pub input_path: Option<PathBuf>,
    pub runs: Option<u64>,
    pub write_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PerfFuzzyArgs {
    pub language: Option<String>,
    pub write_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// perf-detect-offline
// ---------------------------------------------------------------------------

/// Benchmark the detection stage over real fixture frames, headless.
///
/// Every frame is prepared once (thresholds, templates, matcher backend) and
/// then scanned `runs` times after `WARMUP_RUNS` warm-up scans. Each run
/// reports the detection stage's own timings (crop, grayscale, integral
/// build, template scan) plus the wall time of the whole call.
pub fn perf_detect_offline(roots: &Roots, args: &PerfDetectOfflineArgs) -> Result<String, String> {
    let runs = args.runs.unwrap_or(DEFAULT_DETECT_RUNS);
    if runs == 0 {
        return Err("runs must be greater than zero".to_string());
    }
    // `--input` is the user's file; the default frames are the recorded
    // fixtures of a developer checkout, which the packages do not ship.
    let frames = match &args.input_path {
        Some(path) => vec![(path.clone(), roots.work_path(path))],
        None => DEFAULT_DETECT_FRAMES
            .iter()
            .map(|path| (PathBuf::from(path), roots.assets.join(path)))
            .collect(),
    };
    if args.input_path.is_none() {
        if let Some((_, missing)) = frames.iter().find(|(_, full_path)| !full_path.is_file()) {
            let shown = std::path::absolute(missing).unwrap_or_else(|_| missing.clone());
            return Err(format!(
                "perf-detect-offline requires a developer checkout: {} is missing; pass --input <png> to benchmark your own frame",
                shown.display()
            ));
        }
    }

    let (resolved, _) = resolve_detection_config(roots, None, None, &[], None, None, None, None)?;
    let detector = prepare_detector(roots, resolved)?;

    let mut frame_reports = Vec::new();
    for (frame_path, full_path) in &frames {
        let frame = load_capture_frame(full_path)?;
        frame_reports.push(benchmark_frame(&detector, frame_path, &frame, runs)?);
    }

    let write_path = args
        .write_path
        .clone()
        .unwrap_or_else(|| default_baseline_path("detect-offline.json"));
    let scan_budget = detector.scan.threads();
    let json = render_detect_json(runs, scan_budget, &frame_reports);
    write_report(roots, &write_path, &json)?;
    let markdown_path = with_extension(&write_path, "md");
    let markdown = render_detect_markdown(runs, scan_budget, &frame_reports);
    write_report(roots, &markdown_path, &markdown)?;
    Ok(format!("{markdown}\n"))
}

struct FrameDetectReport {
    path: String,
    width: u32,
    height: u32,
    crop_ms: Vec<u64>,
    gray_ms: Vec<u64>,
    integral_ms: Vec<u64>,
    scan_ms: Vec<u64>,
    total_us: Vec<u64>,
    /// Fewest and most threads that ran the template scan.
    scan_threads: (usize, usize),
}

fn benchmark_frame(
    detector: &d4lc_detect::prepared::PreparedDetector,
    frame_path: &Path,
    frame: &CaptureFrame,
    runs: u64,
) -> Result<FrameDetectReport, String> {
    let mut report = FrameDetectReport {
        path: frame_path.display().to_string(),
        width: frame.width,
        height: frame.height,
        crop_ms: Vec::new(),
        gray_ms: Vec::new(),
        integral_ms: Vec::new(),
        scan_ms: Vec::new(),
        total_us: Vec::new(),
        scan_threads: (usize::MAX, 0),
    };
    let runs_usize = usize::try_from(runs + WARMUP_RUNS).unwrap_or(usize::MAX);
    report.crop_ms.reserve(runs_usize);
    for _ in 0..(runs + WARMUP_RUNS) {
        let started = Instant::now();
        let result = detect_roi_frame_prepared(
            RoiFrame::full_window(frame),
            detector,
            0,    // mouse_x: the cursor does not narrow the search
            None, // no trace output
            None, // no crop output
        )?;
        let total = elapsed_micros(started);
        let timings = result.timings;
        if timings.integral_ms > timings.match_ms {
            return Err(format!(
                "integral build ({}) cannot exceed the whole match stage ({})",
                timings.integral_ms, timings.match_ms
            ));
        }
        // The warm-up runs are not reported: they pay cold caches and
        // allocator growth, so they would overstate the steady state.
        if report.crop_ms.len() < usize::try_from(runs).unwrap_or(usize::MAX) {
            report.crop_ms.push(u64_ms(timings.roi_crop_ms));
            report.gray_ms.push(u64_ms(timings.grayscale_ms));
            report.integral_ms.push(u64_ms(timings.integral_ms));
            report
                .scan_ms
                .push(u64_ms(timings.match_ms).saturating_sub(u64_ms(timings.integral_ms)));
            report.total_us.push(total);
            report.scan_threads = (
                report.scan_threads.0.min(timings.scan_threads),
                report.scan_threads.1.max(timings.scan_threads),
            );
        }
    }
    Ok(report)
}

fn render_detect_json(runs: u64, scan_budget: usize, frames: &[FrameDetectReport]) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "command": "perf-detect-offline",
        "warmup_runs": WARMUP_RUNS,
        "runs_per_frame": runs,
        "scan_thread_budget": scan_budget,
        "percentile_method": "nearest-rank",
        "corpus_gaps": ["no 2560x1440 capture frame", "no OCR ground-truth corpus"],
        "frames": frames.iter().map(frame_json).collect::<Vec<_>>(),
    }))
}

fn frame_json(frame: &FrameDetectReport) -> Value {
    json!({
        "path": frame.path,
        "width": frame.width,
        "height": frame.height,
        "scan_threads": { "min": frame.scan_threads.0, "max": frame.scan_threads.1 },
        "timings_ms": {
            "crop": percentiles_json(&frame.crop_ms),
            "gray": percentiles_json(&frame.gray_ms),
            "integral": percentiles_json(&frame.integral_ms),
            "scan": percentiles_json(&frame.scan_ms),
            "total_us": percentiles_json(&frame.total_us),
        },
    })
}

fn render_detect_markdown(runs: u64, scan_budget: usize, frames: &[FrameDetectReport]) -> String {
    let mut output = String::new();
    output.push_str("# Offline detection baseline (perf-detect-offline)\n\n");
    output.push_str(&format!(
        "{runs} measured runs per frame after {WARMUP_RUNS} warm-up runs; \
         nearest-rank percentiles, milliseconds.\n\n"
    ));
    output.push_str(&format!(
        "Template scan thread budget on this host: {scan_budget} \
         (half the available cores, 1–4).\n\n"
    ));
    output.push_str("Corpus gaps: no 2560×1440 capture frame, no OCR ground-truth corpus.\n\n");
    for frame in frames {
        output.push_str(&format!(
            "## {} ({}×{})\n\n",
            frame.path, frame.width, frame.height
        ));
        let (fewest, most) = frame.scan_threads;
        if fewest == most {
            output.push_str(&format!("Scan threads: {most}\n\n"));
        } else {
            output.push_str(&format!("Scan threads: {fewest}–{most}\n\n"));
        }
        output.push_str("| stage | p50 | p95 |\n|---|---|---|\n");
        output.push_str(&format!(
            "| crop | {} | {} |\n",
            percentile(&frame.crop_ms, 50),
            percentile(&frame.crop_ms, 95)
        ));
        output.push_str(&format!(
            "| gray | {} | {} |\n",
            percentile(&frame.gray_ms, 50),
            percentile(&frame.gray_ms, 95)
        ));
        output.push_str(&format!(
            "| integral | {} | {} |\n",
            percentile(&frame.integral_ms, 50),
            percentile(&frame.integral_ms, 95)
        ));
        output.push_str(&format!(
            "| scan | {} | {} |\n",
            percentile(&frame.scan_ms, 50),
            percentile(&frame.scan_ms, 95)
        ));
        output.push_str(&format!(
            "| total (µs) | {} | {} |\n\n",
            percentile(&frame.total_us, 50),
            percentile(&frame.total_us, 95)
        ));
    }
    output
}

// ---------------------------------------------------------------------------
// perf-fuzzy
// ---------------------------------------------------------------------------

/// Benchmark the fuzzy matcher over the catalog corpus.
///
/// For every category, every catalog label and three seeded 1–3-edit
/// variants of it are matched through `FuzzyIndexes::match_text` (the
/// uncached path the live parse uses for new text). Reports p50/p95 per
/// call per category, and per-item percentiles where one item is one text
/// matched against all six categories.
pub fn perf_fuzzy(roots: &Roots, args: &PerfFuzzyArgs) -> Result<String, String> {
    let language = args.language.as_deref().unwrap_or("enUS");
    let repo = AssetRepository::new(&roots.assets);
    let indexes = FuzzyIndexes::load(&repo, language)?;
    let mut rng = XorShift64::new(0x9E37_79B9_7F4A_7C15);

    let categories = [
        FuzzyCategory::ItemType,
        FuzzyCategory::Affix,
        FuzzyCategory::Aspect,
        FuzzyCategory::Unique,
        FuzzyCategory::Rune,
        FuzzyCategory::Sigil,
    ];

    // The union of every category's labels is the corpus of "OCR lines" the
    // tool replays: each category is matched against each text with 1–3-edit
    // variants, mirroring the live parse's unknown-category input.
    let mut corpus: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for category in categories {
        for label in indexes.category_index(category).labels() {
            if seen.insert(label.to_string()) {
                corpus.push(label.to_string());
            }
        }
    }
    let mut full_corpus: Vec<String> = Vec::new();
    for label in &corpus {
        full_corpus.push(label.clone());
        for edits in 1..=FUZZY_EDIT_VARIANTS_PER_LABEL {
            full_corpus.push(apply_edits(label, edits, &mut rng));
        }
    }

    let mut category_reports = Vec::new();
    for category in categories {
        let index = indexes.category_index(category);
        let label_count = index.candidate_count();
        let mut corpus: Vec<String> = Vec::new();
        for label in index.labels() {
            corpus.push(label.to_string());
            for edits in 1..=FUZZY_EDIT_VARIANTS_PER_LABEL {
                corpus.push(apply_edits(label, edits, &mut rng));
            }
        }

        let mut call_us = Vec::with_capacity(corpus.len());
        for text in &corpus {
            let started = Instant::now();
            let result = indexes.match_text(category, text);
            let elapsed = elapsed_micros(started);
            std::hint::black_box(&result);
            call_us.push(elapsed);
        }
        category_reports.push(CategoryFuzzyReport {
            category: category.as_str().to_string(),
            label_count,
            corpus_size: corpus.len(),
            call_us,
        });
    }

    // One per-item pass: the full corpus matched against all six
    // categories, the cost of resolving one OCR line without knowing its
    // category.
    let mut item_pass_us = Vec::with_capacity(full_corpus.len());
    for text in &full_corpus {
        let started = Instant::now();
        for category in categories {
            let result = indexes.match_text(category, text);
            std::hint::black_box(&result);
        }
        item_pass_us.push(elapsed_micros(started));
    }

    let write_path = args
        .write_path
        .clone()
        .unwrap_or_else(|| default_baseline_path("fuzzy.json"));
    let json = render_fuzzy_json(language, &category_reports, &item_pass_us);
    write_report(roots, &write_path, &json)?;
    let markdown_path = with_extension(&write_path, "md");
    let markdown = render_fuzzy_markdown(language, &category_reports, &item_pass_us);
    write_report(roots, &markdown_path, &markdown)?;
    Ok(format!("{markdown}\n"))
}

struct CategoryFuzzyReport {
    category: String,
    label_count: usize,
    corpus_size: usize,
    call_us: Vec<u64>,
}

fn render_fuzzy_json(
    language: &str,
    categories: &[CategoryFuzzyReport],
    item_passes: &[u64],
) -> String {
    let category_json = |category: &CategoryFuzzyReport| {
        json!({
            "category": category.category,
            "label_count": category.label_count,
            "corpus_size": category.corpus_size,
            "call_us": percentiles_json(&category.call_us),
        })
    };
    to_json_text(&json!({
        "schema_version": 1,
        "command": "perf-fuzzy",
        "language": language,
        "match_path": "uncached match_text",
        "edit_variants_per_label": FUZZY_EDIT_VARIANTS_PER_LABEL,
        "corpus_gaps": [
            "no OCR ground-truth corpus; the catalog labels plus 1-3-edit variants stand in"
        ],
        "categories": categories.iter().map(category_json).collect::<Vec<_>>(),
        "item_pass_us": percentiles_json(item_passes),
    }))
}

fn render_fuzzy_markdown(
    language: &str,
    categories: &[CategoryFuzzyReport],
    item_passes: &[u64],
) -> String {
    let mut output = String::new();
    output.push_str("# Fuzzy matcher baseline (perf-fuzzy)\n\n");
    output.push_str(&format!(
        "Language `{language}`; uncached `match_text` over every catalog label \
         plus three seeded 1–3-edit variants; nearest-rank percentiles, \
         microseconds.\n\n"
    ));
    output.push_str(
        "Corpus gap: there is no OCR ground-truth corpus; catalog labels plus \
         edited variants stand in.\n\n",
    );
    output.push_str("| category | labels | corpus | call p50 | call p95 |\n");
    output.push_str("|---|---|---|---|---|\n");
    for category in categories {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            category.category,
            category.label_count,
            category.corpus_size,
            percentile(&category.call_us, 50),
            percentile(&category.call_us, 95)
        ));
    }
    output.push_str(&format!(
        "\nOne item (all six categories for one text): p50 {} µs, p95 {} µs.\n",
        percentile(item_passes, 50),
        percentile(item_passes, 95)
    ));
    output
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn write_report(roots: &Roots, report_path: &Path, report: &str) -> Result<(), String> {
    write_debug_text(&roots.work_path(report_path), report)
}

fn default_baseline_path(file: &str) -> PathBuf {
    PathBuf::from(DEFAULT_BASELINE_DIR).join(file)
}

fn with_extension(path: &Path, extension: &str) -> PathBuf {
    let mut path = path.to_path_buf();
    path.set_extension(extension);
    path
}

fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn u64_ms(ms: u128) -> u64 {
    u64::try_from(ms).unwrap_or(u64::MAX)
}

/// Nearest-rank percentile of the observed values (`p` in 1..=99).
fn percentile(values: &[u64], p: usize) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    nearest_rank(&sorted, p)
}

fn nearest_rank(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied().unwrap_or(0)
}

fn percentiles_json(values: &[u64]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    json!({ "p50": nearest_rank(&sorted, 50), "p95": nearest_rank(&sorted, 95) })
}

/// Deterministic xorshift64* generator, so the edited corpus is stable
/// across runs and machines.
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % bound as u64).unwrap_or(0)
    }
}

fn random_letter(rng: &mut XorShift64) -> char {
    (b'a' + u8::try_from(rng.next_usize(26)).unwrap_or(0)) as char
}

/// Apply `edits` random delete/insert/replace operations to `text`.
fn apply_edits(text: &str, edits: usize, rng: &mut XorShift64) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for _ in 0..edits {
        if chars.is_empty() {
            break;
        }
        match rng.next_usize(3) {
            0 => {
                let position = rng.next_usize(chars.len());
                chars.remove(position);
            }
            1 => {
                let position = rng.next_usize(chars.len() + 1);
                chars.insert(position, random_letter(rng));
            }
            _ => {
                let position = rng.next_usize(chars.len());
                chars[position] = random_letter(rng);
            }
        }
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_uses_the_rank_ceiling() {
        let mut values: Vec<u64> = (1..=100).collect();
        values.sort_unstable();
        assert_eq!(nearest_rank(&values, 50), 50);
        assert_eq!(nearest_rank(&values, 95), 95);
        assert_eq!(nearest_rank(&values, 99), 99);
        assert_eq!(nearest_rank(&values, 1), 1);
        assert_eq!(nearest_rank(&[7], 50), 7);
        assert_eq!(nearest_rank(&[], 50), 0);
    }

    #[test]
    fn edited_variants_are_deterministic_and_differ_from_the_label() {
        let label = "+740 Maximum Life";
        let mut rng = XorShift64::new(0x9E37_79B9_7F4A_7C15);
        let first = apply_edits(label, 2, &mut rng);
        let mut rng = XorShift64::new(0x9E37_79B9_7F4A_7C15);
        let second = apply_edits(label, 2, &mut rng);
        assert_eq!(first, second, "the seeded generator must be stable");
        assert_ne!(first, label, "two edits must change the label");

        let unchanged = apply_edits("", 2, &mut rng);
        assert_eq!(unchanged, "", "editing an empty label stays empty");
    }

    #[test]
    fn percentiles_render_as_json() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(percentiles_json(&values), json!({ "p50": 50, "p95": 95 }));
    }

    #[test]
    fn default_write_paths_live_in_the_baseline_dir() {
        assert_eq!(
            default_baseline_path("fuzzy.json"),
            PathBuf::from("debug-output/perf/baseline/fuzzy.json")
        );
        assert_eq!(
            with_extension(&default_baseline_path("fuzzy.json"), "md"),
            PathBuf::from("debug-output/perf/baseline/fuzzy.md")
        );
    }
}
