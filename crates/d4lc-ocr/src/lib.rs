//! Native OCR primitives for d4lc.
//!
//! Upstream parity references:
//! - D4Companion.Services/OcrHandler.cs (`ObjectPool<Engine>`, `ConvertToText`,
//!   `ConvertToTextUpperTooltipSection`, `SetLanguage`)
//! - D4Companion.Services/ScreenProcessHandler.cs (`FindItemTypesPower`,
//!   `FindItemAffixAreas`, `FindItemAspectAreas`)

mod engine;
#[cfg(any(feature = "leptess-backend", test))]
mod leptess;
mod segments;
#[cfg(test)]
mod test_support;
mod text;
mod types;

pub use d4lc_core::text::upstream_convert_to_affix_clean_text;
pub use engine::{
    BoxedOcrEngine, OcrEngine, OcrEnginePool, OcrEnginePoolStats, OcrEngineRegistry,
    UnavailableOcrEngine,
};
#[cfg(feature = "leptess-backend")]
pub use leptess::{LeptessImageHandoff, LeptessOcrEngine, LeptessRawOutput};
pub use segments::{
    affix_regions, affix_type_from_marker_name, aspect_region, marker_defined_regions,
    plan_ocr_segments, upper_tooltip_region, AffixType, OcrAffixRegion, OcrMarker, OcrRegion,
    OcrSegmentPlan, OcrSegmentPlanInput, TooltipOcrRegionConfig,
};
pub use text::{
    parse_tesseract_tsv_lines, parse_upper_tooltip_text, tesseract_language_code, text_from_lines,
    upstream_conversion_method, upstream_conversion_route,
    upstream_convert_aspect_unique_clean_text, upstream_convert_to_affix_multiline_text,
    upstream_convert_to_text, upstream_converted_text, upstream_converted_text_for_route,
    OcrConversionRoute,
};
pub use types::{
    structured_output_error, OcrBackendKind, OcrError, OcrImage, OcrImagePixels, OcrOptions,
    OcrPageSegMode, OcrProfile, OcrSegmentKind, OcrStructuredOutput,
};
