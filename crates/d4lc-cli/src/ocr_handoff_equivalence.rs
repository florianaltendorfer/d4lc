//! Proves the raw-pixel Tesseract hand-off identical to the former PNG round
//! trip: every OCR segment the fixture frames produce, in both preprocessing
//! variants, must yield the same `get_utf8_text` and the same TSV.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::detect::load_capture_frame;
use d4lc_core::OcrText;
use d4lc_ocr::{
    LeptessImageHandoff, LeptessOcrEngine, OcrEngine, OcrEngineRegistry, OcrError, OcrImage,
    OcrImagePixels, OcrOptions, OcrPageSegMode, OcrProfile, OcrSegmentKind, OcrStructuredOutput,
};
use d4lc_runtime::ocr_parity::TooltipOcrPreprocessVariant;
use d4lc_runtime::PreparedTooltipOcr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const FIXTURE_FRAMES: [&str; 3] = [
    "tests/fixtures/live-frames/d4lc-livecap.png",
    "tests/fixtures/live-frames/frame-CTRL.png",
    "tests/fixtures/live-frames/frame-KB.png",
];

#[derive(Debug, Default)]
struct Comparison {
    gray_segments: usize,
    bgra_segments: usize,
    mismatches: Vec<String>,
}

/// Reads every image through both hand-offs, records the comparison, and
/// answers the pipeline with the live (raw) result.
struct ComparingEngine {
    engine: LeptessOcrEngine,
    comparison: Arc<Mutex<Comparison>>,
}

impl OcrEngine for ComparingEngine {
    fn recognize_lines(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        Ok(self.recognize_structured(image, options)?.text)
    }

    fn recognize_structured(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrStructuredOutput, OcrError> {
        let png = self
            .engine
            .read_raw_output(image, options, LeptessImageHandoff::PngRoundTrip)?;
        let raw = self
            .engine
            .read_raw_output(image, options, LeptessImageHandoff::RawPixels)?;
        let mut comparison = self.comparison.lock().unwrap();
        let kind = match &image.pixels {
            OcrImagePixels::Gray(_) => {
                comparison.gray_segments += 1;
                "gray"
            }
            OcrImagePixels::Bgra(_) => {
                comparison.bgra_segments += 1;
                "bgra"
            }
        };
        if png != raw {
            comparison.mismatches.push(format!(
                "{kind} {:?} {}x{} ({:?}):\n  png text {:?}\n  raw text {:?}\n  tsv equal: {}",
                options.segment_kind,
                image.width(),
                image.height(),
                options.page_seg_mode,
                png.utf8_text,
                raw.utf8_text,
                png.tsv == raw.tsv
            ));
        }
        drop(comparison);
        self.engine.recognize_structured(image, options)
    }
}

fn exported_segment_inputs(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(frames) = std::fs::read_dir(dir) else {
        return paths;
    };
    for frame in frames.flatten() {
        for variant in ["upstream_gray_binary_inv", "raw_bgra"] {
            let Ok(entries) = std::fs::read_dir(frame.path().join(variant)) else {
                continue;
            };
            paths.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|extension| extension == "png")),
            );
        }
    }
    paths.sort();
    paths
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore = "runs Tesseract over every fixture OCR segment twice; run with `cargo test -p d4lc-cli --features leptess-backend raw_pixel_handoff -- --ignored --nocapture`"]
fn raw_pixel_handoff_reads_every_fixture_segment_like_the_png_round_trip() {
    let root = repo_root();
    let profile = OcrProfile::new("enUS");
    let comparison = Arc::new(Mutex::new(Comparison::default()));
    let factory_comparison = Arc::clone(&comparison);
    let registry = OcrEngineRegistry::new(move |profile| {
        Ok(Box::new(ComparingEngine {
            engine: LeptessOcrEngine::new(profile)?,
            comparison: Arc::clone(&factory_comparison),
        }))
    });
    let prepared =
        PreparedTooltipOcr::prepare(&root, "1080p_SMF".to_string(), profile.clone()).unwrap();
    let default_variant = prepared.preprocess_variant();
    assert!(matches!(
        default_variant,
        TooltipOcrPreprocessVariant::UpstreamGrayBinaryInv { .. }
    ));
    let raw_bgra = PreparedTooltipOcr::prepare(&root, "1080p_SMF".to_string(), profile)
        .unwrap()
        .with_preprocess_variant(TooltipOcrPreprocessVariant::RawBgra);

    for frame_path in FIXTURE_FRAMES {
        let frame = load_capture_frame(&root.join(frame_path)).unwrap();
        for session in [&prepared, &raw_bgra] {
            let execution = session.execute(&registry, &frame).unwrap();
            println!(
                "{frame_path} {}: {} segments",
                session.preprocess_variant().as_str(),
                execution.results.len()
            );
        }
    }

    // Segment inputs an earlier preprocessing comparison exported from the
    // same frames (with the tooltip found at the recorded cursor); they are
    // not committed, so they widen the corpus only where present.
    let exported = root.join("debug-output/ocr-preprocess");
    let mut engine = ComparingEngine {
        engine: LeptessOcrEngine::new(&OcrProfile::new("enUS")).unwrap(),
        comparison: Arc::clone(&comparison),
    };
    let mut exported_count = 0;
    for path in exported_segment_inputs(&exported) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let gray = path.parent().unwrap().ends_with("upstream_gray_binary_inv");
        let image = if gray {
            let luma = image::open(&path).unwrap().to_luma8();
            OcrImage::from_gray(d4lc_core::GrayImage {
                width: luma.width(),
                height: luma.height(),
                pixels: luma.into_raw(),
            })
        } else {
            OcrImage::new(load_capture_frame(&path).unwrap())
        };
        let (segment_kind, page_seg_mode) = if name.contains("upper_tooltip") {
            (OcrSegmentKind::UpperTooltip, OcrPageSegMode::SparseText)
        } else if name.contains("aspect") {
            (OcrSegmentKind::Aspect, OcrPageSegMode::Default)
        } else if name.contains("affix") {
            (OcrSegmentKind::Affix, OcrPageSegMode::Default)
        } else {
            (OcrSegmentKind::Generic, OcrPageSegMode::Default)
        };
        let options = OcrOptions {
            segment_kind,
            page_seg_mode,
            collect_tsv: true,
            ..OcrOptions::default()
        };
        engine.recognize_structured(&image, &options).unwrap();
        exported_count += 1;
    }
    println!(
        "{exported_count} exported segment inputs under {}",
        exported.display()
    );

    let comparison = comparison.lock().unwrap();
    println!(
        "compared {} gray and {} BGRA segments; {} differ",
        comparison.gray_segments,
        comparison.bgra_segments,
        comparison.mismatches.len()
    );
    assert!(comparison.gray_segments > 0 && comparison.bgra_segments > 0);
    assert!(
        comparison.mismatches.is_empty(),
        "raw hand-off differs from the PNG round trip:\n{}",
        comparison.mismatches.join("\n")
    );
}
