//! Shared fixtures for the OCR parity module tests.

use std::path::{Path, PathBuf};

use d4lc_assets::AssetRepository;
use d4lc_core::{CaptureFrame, OcrText, Rect};
use d4lc_fuzzy::FuzzyIndexes;
use d4lc_ocr::{
    BoxedOcrEngine, OcrBackendKind, OcrEngine, OcrEngineRegistry, OcrError, OcrImage, OcrOptions,
    OcrPageSegMode, OcrProfile, OcrSegmentKind, OcrSegmentPlan, OcrStructuredOutput,
};

use super::types::{
    ocr_input_trace, segment_ocr_image, DetectedMarkerPlan, OcrInputTrace, SegmentResult,
    SplitterDetectionTrace, TooltipCropTrace, TooltipOcrExecution, TooltipOcrPreprocessVariant,
};
use super::{UPSTREAM_SPLITTER_DETECTION_RULE, UPSTREAM_TOOLTIP_CROP_RULE};

pub(super) fn white_frame(width: u32, height: u32) -> CaptureFrame {
    CaptureFrame {
        width,
        height,
        stride: usize::try_from(width).unwrap_or(0) * 4,
        pixels_bgra: vec![255; usize::try_from(width * height * 4).unwrap_or(0)],
    }
}

pub(super) fn put_black_pixel(frame: &mut CaptureFrame, x: usize, y: usize) {
    let width = usize::try_from(frame.width).unwrap_or(0);
    let index = (y * width + x) * 4;
    frame.pixels_bgra[index..index + 4].copy_from_slice(&[0, 0, 0, 255]);
}

pub(super) fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates| crates.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| panic!("could not resolve workspace root"))
}

/// One recorded reference frame (`tests/fixtures/runtime/manifest.json`).
pub(super) fn reference_capture(file: &str) -> CaptureFrame {
    let path = workspace_root()
        .join("tests/fixtures/live-frames")
        .join(file);
    let rgba = image::ImageReader::open(&path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", path.display()))
        .decode()
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    CaptureFrame {
        width,
        height,
        stride: usize::try_from(width).unwrap_or(0) * 4,
        pixels_bgra: rgba
            .pixels()
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
            .collect(),
    }
}

pub(super) fn enus_profile() -> OcrProfile {
    OcrProfile {
        backend: OcrBackendKind::Auto,
        language: "enUS".to_string(),
        tessdata_path: None,
    }
}

/// Recognizes canned text per segment kind: the pixels still travel
/// through the real crop, preprocessing and conversion path, only the
/// engine is a stub. Keeps the chain tests independent of Tesseract.
struct CannedOcrEngine;

impl OcrEngine for CannedOcrEngine {
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
        let raw_text = match options.segment_kind {
            OcrSegmentKind::UpperTooltip => "Storm Amulet\n900 Item Power",
            OcrSegmentKind::Affix => "+32.0% Attack Speed [15 - 32]",
            _ => "unclassified",
        }
        .to_string();
        let lines = raw_text
            .lines()
            .enumerate()
            .map(|(index, text)| d4lc_core::OcrLine {
                text: text.to_string(),
                rect: Rect {
                    x: 4,
                    y: i32::try_from(index).unwrap_or(0) * 12,
                    width: image.width().saturating_sub(8),
                    height: 12,
                },
                words: Vec::new(),
            })
            .collect::<Vec<_>>();
        Ok(OcrStructuredOutput {
            text: OcrText {
                raw_text,
                lines,
                bounding_box_source: d4lc_core::OcrBoundingBoxSource::TesseractTsv,
            },
            tsv: Some("level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n".to_string()),
        })
    }
}

pub(super) fn canned_registry() -> OcrEngineRegistry {
    OcrEngineRegistry::new(|_profile| Ok(Box::new(CannedOcrEngine) as BoxedOcrEngine))
}

pub(super) fn load_enus_fuzzy_indexes() -> FuzzyIndexes {
    let repo = AssetRepository::new(workspace_root());
    FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"))
}

/// The OCR frame of the synthetic executions below.
pub(super) fn test_ocr_frame() -> CaptureFrame {
    white_frame(375, 720)
}

/// A coherent OCR input trace for a segment of [`test_ocr_frame`],
/// derived through the same helpers the pipeline uses.
pub(super) fn test_ocr_input(rect: Rect, variant: TooltipOcrPreprocessVariant) -> OcrInputTrace {
    let image = segment_ocr_image(&test_ocr_frame(), rect, variant)
        .unwrap_or_else(|error| panic!("synthetic segment crop failed: {error}"));
    ocr_input_trace(&image, variant)
}

pub(super) fn segment_result(index: usize, kind: OcrSegmentKind, raw_text: &str) -> SegmentResult {
    let rect = Rect {
        x: 0,
        y: i32::try_from(index * 40).unwrap_or(0),
        width: 320,
        height: 32,
    };
    SegmentResult {
        plan: OcrSegmentPlan {
            index,
            kind,
            source: "tooltip_filter".to_string(),
            rect,
            source_marker_rect: None,
            boundary_source: "tooltip_bottom".to_string(),
            boundary_rect: None,
            upstream_rule: "test".to_string(),
            marker_name: None,
            marker_index: None,
            page_seg_mode: OcrPageSegMode::Default,
            language: "enUS".to_string(),
            tooltip_width: 0,
            clamp_reason: None,
        },
        text: OcrText {
            raw_text: raw_text.to_string(),
            lines: Vec::new(),
            bounding_box_source: d4lc_core::OcrBoundingBoxSource::Unavailable,
        },
        tsv: None,
        ocr_input: test_ocr_input(rect, TooltipOcrPreprocessVariant::RawBgra),
        converted_text: raw_text.to_string(),
        conversion_method: "",
        conversion_branch: "",
        right_aligned_line_filtering_applied: true,
        type_area_offset_left: 0,
        ocr_ms: 0,
        conversion_ms: 0,
    }
}

pub(super) fn rune_sigil_execution(results: Vec<SegmentResult>) -> TooltipOcrExecution {
    TooltipOcrExecution {
        profile: OcrProfile {
            backend: OcrBackendKind::Auto,
            language: "enUS".to_string(),
            tessdata_path: None,
        },
        preset: "1080p_SMF".to_string(),
        ocr_frame: white_frame(375, 720),
        preprocess_variant: TooltipOcrPreprocessVariant::RawBgra,
        crop_trace: TooltipCropTrace {
            planned_tooltip_roi: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 720,
            },
            search_roi: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 720,
            },
            detected_tooltip_rect: None,
            final_tooltip_crop_rect: Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 720,
            },
            crop_source: "input_already_final_or_no_template_match",
            upstream_rule: UPSTREAM_TOOLTIP_CROP_RULE,
            original_input_size: (375, 720),
            final_tooltip_size: (375, 720),
        },
        marker_plan: DetectedMarkerPlan {
            affix_markers: Vec::new(),
            aspect_markers: Vec::new(),
            aspect_marker: None,
            socket_markers: Vec::new(),
            splitter_markers: Vec::new(),
            splitter_detection: SplitterDetectionTrace {
                roi: Rect {
                    x: 161,
                    y: 0,
                    width: 53,
                    height: 720,
                },
                threshold: 0.05,
                template_names: Vec::new(),
                template_results: Vec::new(),
                accepted_matches: Vec::new(),
                splitters_before_cleanup: Vec::new(),
                splitters_after_cleanup: Vec::new(),
                top_splitter_cleanup_applied: false,
                removed_overlapping_splitters: Vec::new(),
                upstream_rule: UPSTREAM_SPLITTER_DETECTION_RULE,
            },
            splitter_detection_ms: 0,
            aspect_area_trace: None,
            is_unique_item: false,
            has_tooltip_top_splitter: false,
        },
        results,
    }
}
