//! The data the tooltip OCR pipeline produces and traces, and the OCR input
//! preprocessing every segment goes through.

use d4lc_capture::crop_frame;
use d4lc_core::{CaptureFrame, OcrText, Rect};
use d4lc_ocr::{OcrImage, OcrImagePixels, OcrMarker, OcrProfile, OcrSegmentPlan};
use d4lc_parse::ItemTooltipParseResult;
use d4lc_vision::{threshold_binary_inv, to_grayscale};

use super::UPSTREAM_OCR_PREPROCESS_RULE;

/// Which pixels the runtime hands to the OCR backend for one tooltip segment.
///
/// Upstream runs every OCR step on `_currentScreenTooltipFilter`, the tooltip
/// crop converted to grayscale with `ThresholdBinaryInv(ThresholdMin,
/// ThresholdMax)` applied (ScreenProcessHandler.cs:635-636), and feeds that
/// image to the OCR areas (706-710, 1118, 1412). The port recognized the raw
/// BGRA crop instead.
///
/// Both variants stay implemented: the default is a measured decision on the
/// repository's reference captures with one language model, not a preference,
/// and the comparison that decided it stays reproducible
/// (`reference_captures_compare_ocr_preprocess_variants`, evidence under
/// `debug-output/ocr-preprocess/`).
///
/// # Decision
///
/// The upstream variant is the live default. Measured on the reference
/// captures of `tests/fixtures/runtime/manifest.json`, one engine registry and
/// one language model (settings language enUS -> `eng`, thresholds from
/// `Config/Settings.json`):
///
/// | capture | variant | segments | lines | affix rows | item name |
/// | --- | --- | --- | --- | --- | --- |
/// | frame-KB | upstream | 9 | 11 | 3 | `M Drunven Lot VICI®US FOCUS` |
/// | frame-KB | raw BGRA | 9 | 14 | 5 | `VICI®US FOCUS` |
/// | d4lc-livecap | upstream | 10 | 14 | 5 | `©BSCURE STRANGLERS` |
/// | d4lc-livecap | raw BGRA | 10 | 17 | 7 | `©BSCURE STRANGLERS` |
/// | frame-CTRL | upstream | 10 | 14 | 5 | `©BSCURE STRANGLERS` |
/// | frame-CTRL | raw BGRA | 10 | 18 | 8 | `©BSCURE STRANGLERS` |
///
/// Every difference is attributable to one concrete step:
///
/// - The affix rows the upstream variant loses carry text whose luminance is
///   at or below `ThresholdMin` (70): the same capture with `ThresholdMin`
///   lowered to 40 reads them again (frame-KB: 17 lines, 7 affix rows). That
///   is upstream's own settings knob, not a port behavior.
/// - The extra upper-tooltip line on frame-KB is low-confidence game-world
///   text above the tooltip (TSV confidence 32/0/14 in segment row 0) that the
///   thresholded image exposes to SparseText.
/// - The mangled name characters (`©`/`®` for `O`) are identical in both
///   variants: they come from the recognition step, not from the crop, the
///   plan or the preprocessing.
///
/// Parity decides over the higher row count of the pre-fix variant: the port's
/// OCR input has to be the input upstream recognizes, otherwise no result of
/// this port can be compared with the reference implementation, and the
/// settings knob above stays the only lever on the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipOcrPreprocessVariant {
    /// Upstream: grayscale, then
    /// [`d4lc_vision::threshold_binary_inv`] with these thresholds.
    UpstreamGrayBinaryInv {
        threshold_min: u8,
        threshold_max: u8,
    },
    /// The port's pre-fix behavior: the raw BGRA tooltip crop.
    RawBgra,
}

impl TooltipOcrPreprocessVariant {
    /// The variant the live path uses: upstream's preprocessing with the
    /// thresholds the settings carry (`ThresholdMin`/`ThresholdMax`).
    pub const fn for_settings(settings: &d4lc_config::AppSettings) -> Self {
        Self::UpstreamGrayBinaryInv {
            threshold_min: settings.threshold_min,
            threshold_max: settings.threshold_max,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamGrayBinaryInv { .. } => "upstream_gray_binary_inv",
            Self::RawBgra => "raw_bgra",
        }
    }

    /// The rule this variant follows, for traces.
    pub const fn upstream_rule(self) -> &'static str {
        match self {
            Self::UpstreamGrayBinaryInv { .. } => UPSTREAM_OCR_PREPROCESS_RULE,
            Self::RawBgra => "earlier port behavior: OCR recognized the original BGRA tooltip crop",
        }
    }
}

/// FNV-1a over the pixel bytes of one OCR input image.
///
/// Recorded per segment so an exported chain artifact can be proven to be the
/// same image the job handed to Tesseract instead of an assumption.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The exact image one segment handed to the OCR backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrInputTrace {
    /// Preprocessing variant that produced the payload, thresholds included.
    pub variant: TooltipOcrPreprocessVariant,
    /// Payload kind of the image ([`OcrImage::variant_name`]): `bgra` or `gray`.
    pub payload: &'static str,
    pub width: u32,
    pub height: u32,
    /// FNV-1a 64 of the payload pixel bytes.
    pub digest: u64,
}

/// [`OcrInputTrace`] of one prepared OCR image.
pub(crate) fn ocr_input_trace(
    image: &OcrImage,
    variant: TooltipOcrPreprocessVariant,
) -> OcrInputTrace {
    let digest = match &image.pixels {
        OcrImagePixels::Bgra(frame) => fnv1a64(&frame.pixels_bgra),
        OcrImagePixels::Gray(gray) => fnv1a64(&gray.pixels),
    };
    OcrInputTrace {
        variant,
        payload: image.variant_name(),
        width: image.width(),
        height: image.height(),
        digest,
    }
}

/// Crops one planned segment and applies the preprocessing variant: upstream
/// crops the thresholded grayscale tooltip for OCR, the pre-fix port cropped
/// the BGRA tooltip.
pub(crate) fn segment_ocr_image(
    ocr_frame: &CaptureFrame,
    rect: Rect,
    variant: TooltipOcrPreprocessVariant,
) -> Result<OcrImage, String> {
    let cropped = crop_frame(ocr_frame, rect)?;
    Ok(match variant {
        TooltipOcrPreprocessVariant::RawBgra => OcrImage::new(cropped),
        TooltipOcrPreprocessVariant::UpstreamGrayBinaryInv {
            threshold_min,
            threshold_max,
        } => OcrImage::from_gray(threshold_binary_inv(
            &to_grayscale(&cropped),
            threshold_min,
            threshold_max,
        )),
    })
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipCropTrace {
    pub planned_tooltip_roi: Rect,
    pub search_roi: Rect,
    pub detected_tooltip_rect: Option<Rect>,
    pub final_tooltip_crop_rect: Rect,
    pub crop_source: &'static str,
    pub upstream_rule: &'static str,
    pub original_input_size: (u32, u32),
    pub final_tooltip_size: (u32, u32),
}

#[derive(Debug, Clone)]
pub struct SegmentResult {
    pub plan: OcrSegmentPlan,
    pub text: OcrText,
    /// Tesseract TSV of this segment, when the backend reported it. The
    /// structured output the line geometry was derived from stays attached to
    /// the segment so the chain of a capture is observable.
    pub tsv: Option<String>,
    /// The exact pixels this segment handed to the OCR backend.
    pub ocr_input: OcrInputTrace,
    pub converted_text: String,
    pub conversion_method: &'static str,
    pub conversion_branch: &'static str,
    pub right_aligned_line_filtering_applied: bool,
    pub type_area_offset_left: u32,
    pub ocr_ms: u128,
    pub conversion_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipOcrSegmentPreview {
    pub index: usize,
    pub segment_kind: String,
    pub line_count: usize,
    pub raw_text_preview: String,
    pub converted_text_preview: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TooltipOcrParityResult {
    pub parity_path: &'static str,
    pub ocr_ms: u64,
    pub conversion_ms: u64,
    pub splitter_detection_ms: u64,
    pub segment_count: usize,
    pub line_count: usize,
    pub raw_text: String,
    pub normalized_lines: Vec<String>,
    pub segments_preview: Vec<TooltipOcrSegmentPreview>,
    pub title_preview: Option<String>,
    pub text_preview: String,
    pub item: ItemTooltipParseResult,
    /// Topmost row any recognized segment covered, in the coordinates of the
    /// input crop; `None` when no segment was recognized. The runtime
    /// coordinator compares only the rows from here down when it decides
    /// whether a later crop is already processed content.
    pub read_top: Option<u32>,
    /// Crop columns the recognized text lines cover, in the coordinates of
    /// the input crop; `None` when no line was recognized. The coordinator
    /// compares only these columns (see `read_rows_unchanged`).
    pub read_extent: Option<crate::ReadExtent>,
}

pub struct TooltipOcrExecution {
    pub profile: OcrProfile,
    pub preset: String,
    /// The final tooltip crop the segments of this execution were cut from
    /// (`prepare_tooltip_ocr_input`), kept so the recognized images stay
    /// re-derivable for the observable chain.
    pub ocr_frame: CaptureFrame,
    /// The OCR input preprocessing every segment of this execution used
    /// ([`TooltipOcrPreprocessVariant`]).
    pub preprocess_variant: TooltipOcrPreprocessVariant,
    pub crop_trace: TooltipCropTrace,
    pub marker_plan: DetectedMarkerPlan,
    pub results: Vec<SegmentResult>,
}

#[derive(Debug, Clone)]
pub struct DetectedMarkerPlan {
    pub affix_markers: Vec<OcrMarker>,
    pub aspect_markers: Vec<OcrMarker>,
    pub aspect_marker: Option<OcrMarker>,
    pub socket_markers: Vec<Rect>,
    pub splitter_markers: Vec<OcrMarker>,
    pub splitter_detection: SplitterDetectionTrace,
    pub splitter_detection_ms: u128,
    pub aspect_area_trace: Option<AspectAreaTrace>,
    pub is_unique_item: bool,
    pub has_tooltip_top_splitter: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitterDetectionTrace {
    pub roi: Rect,
    pub threshold: f64,
    pub template_names: Vec<String>,
    pub template_results: Vec<SplitterTemplateTrace>,
    pub accepted_matches: Vec<SplitterMatchTrace>,
    pub splitters_before_cleanup: Vec<OcrMarker>,
    pub splitters_after_cleanup: Vec<OcrMarker>,
    pub top_splitter_cleanup_applied: bool,
    pub removed_overlapping_splitters: Vec<OcrMarker>,
    pub upstream_rule: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitterTemplateTrace {
    pub name: String,
    pub accepted_candidate_count: usize,
    pub rejected_best_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitterMatchTrace {
    pub name: String,
    pub rect: Rect,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AspectAreaTrace {
    pub split_points: Vec<AspectSplitPointTrace>,
    pub selected_boundary_source: String,
    pub selected_boundary_rect: Option<Rect>,
    pub fallback_to_tooltip_bottom: bool,
    pub upstream_rule: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AspectSplitPointTrace {
    pub source: &'static str,
    pub rect: Rect,
}

#[cfg(test)]
mod tests;
