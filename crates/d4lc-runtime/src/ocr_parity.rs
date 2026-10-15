//! Upstream-parity OCR pipeline extracted from `d4lc-cli/src/ocr.rs`.
//!
//! Both the CLI and the app live-loop use the same [`process_tooltip_ocr_parity`]
//! entry point to run the complete read-tooltip pipeline: tooltip template
//! matching, marker detection, splitter detection, OCR per segment, and parsing.

mod chain_export;
mod detection;
mod parallel;
mod prepared;
mod result;
mod templates;
#[cfg(test)]
mod test_support;
mod types;

pub use chain_export::{export_tooltip_ocr_chain, TooltipOcrChainArtifacts};
pub use prepared::{
    execute_tooltip_ocr_with_registry, process_tooltip_ocr_parity,
    process_tooltip_ocr_parity_prepared, PreparedTooltipOcr, OCR_SEGMENT_THREADS,
};
pub use result::runtime_parity_result;
pub use templates::{load_template_gray_image, MarkerTemplate, TEMPLATE_IMAGE_MAX_SIDE};
#[cfg(test)]
pub(crate) use types::ocr_input_trace;
pub(crate) use types::segment_ocr_image;
pub use types::{
    AspectAreaTrace, AspectSplitPointTrace, DetectedMarkerPlan, OcrInputTrace, SegmentResult,
    SplitterDetectionTrace, SplitterMatchTrace, SplitterTemplateTrace, TooltipCropTrace,
    TooltipOcrExecution, TooltipOcrParityResult, TooltipOcrPreprocessVariant,
    TooltipOcrSegmentPreview,
};

// ---------------------------------------------------------------------------
// Upstream rule references (kept as &'static str so tooltip-crop trace JSON is
// not unnecessarily allocated).
// ---------------------------------------------------------------------------

pub const UPSTREAM_TOOLTIP_CROP_RULE: &str = "ScreenProcessHandler.FindTooltip/FindTooltips: tooltip template match returns Rectangle(location.X, 0, Settings.TooltipWidth, location.Y); FindTooltips bounds width to currentScreenSource then copies currentScreenSource.Copy(location) into _currentScreenTooltip and thresholds it as _currentScreenTooltipFilter";
pub const UPSTREAM_SPLITTER_DETECTION_RULE: &str = "ScreenProcessHandler.FindItemSplitterLocations/FindItemSplitterLocation: roiWidth=_currentScreenTooltip.Width/7; roiStartX=(_currentScreenTooltip.Width/2)-(roiWidth/2); source=_currentScreenTooltipFilter.Copy(center ROI); iterate every dot-splitter_ template; repeated SqdiffNormed FindMatchTemplate while similarity < ThresholdSimilaritySplitterLocation and counter < 20; after each match mask the found template rectangle white (255); sort by Y; remove non-top splitters within +/-2 Y of the first dot-splitter_top; HasTooltipTopSplitter is set from remaining splitters";
pub const UPSTREAM_ASPECT_BOTTOM_RULE: &str = "ScreenProcessHandler.FindItemAspectAreas: areaSplitPoints = affix locations + aspect location + first socket location with offset + splitter locations, sorted by Y; the first split point with Y > ItemAspectLocation.Y becomes the bottom boundary minus AffixAreaHeightOffsetBottom; if none exists, bottom = _currentTooltip.Location.Height; no footer/control-text filtering is applied";
pub const UPSTREAM_OCR_PREPROCESS_RULE: &str = "ScreenProcessHandler: every OCR area comes from _currentScreenTooltipFilter = _currentScreenTooltip.Convert<Gray, byte>().ThresholdBinaryInv(ThresholdMin, ThresholdMax) (SPH.cs:635-636), used for the item-type/power area (SPH.cs:706-710), the affix areas (SPH.cs:1118) and the aspect areas (SPH.cs:1412); OcrHandler saves that gray area as an 8-bit bitmap and lets Tesseract read it";

/// Marker the live loop records so runtime OCR proves it ran the
/// read-tooltip parity implementation.
pub const READ_TOOLTIP_PARITY_PATH: &str = "read-tooltip";
