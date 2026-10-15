//! d4lc-parse crate.

mod affix;
mod convert;
mod lines;
mod model;
#[cfg(test)]
mod test_support;
mod tooltip;

pub use convert::{
    convert_to_affix, convert_to_affix_multiline, convert_to_aspect, convert_to_item_power,
    convert_to_item_type, convert_to_rune, convert_to_sigil, convert_to_unique,
    text_to_affix_value, AffixConversion, ItemPowerConversion, ItemTypeConversion, PowerConversion,
    RuneConversion, SigilConversion,
};
pub use d4lc_core::AffixRowId;
pub use model::{
    ItemTooltipAffix, ItemTooltipAffixRowGeometry, ItemTooltipLegendaryPower, ItemTooltipMetadata,
    ItemTooltipMetadataValue, ItemTooltipNonAffixRow, ItemTooltipParseResult,
    ItemTooltipParseStatus, ItemTooltipRarity, ItemTooltipRune, ItemTooltipSigil,
    ItemTooltipSocket, ItemTooltipSourceLine, ItemTooltipSourceSummary, ItemTooltipStat,
    TooltipOcrLineInput, TooltipOcrSegmentInput, TooltipOcrSegmentKind,
};
pub use tooltip::{
    parse_item_tooltip_from_ocr_segments, parse_item_tooltip_from_ocr_segments_with_fuzzy,
};
