//! Shared domain types for d4lc.

pub mod coords;
pub mod text;
pub mod text_measure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Mouse,
    Controller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Identity of one parsed affix row, unique across the whole tooltip.
///
/// Assigned by the parser in tooltip order. OCR provenance — segment index,
/// line index inside a segment, global line index, marker ordinal — travels
/// next to it but is never used in its place: segment-local line indexes
/// restart at zero in every segment, so two rows can share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize)]
pub struct AffixRowId(pub u32);

impl AffixRowId {
    /// The id of the row at `ordinal` in tooltip order.
    pub fn from_ordinal(ordinal: usize) -> Self {
        Self(u32::try_from(ordinal).unwrap_or(u32::MAX))
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for AffixRowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "row{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub class_name: Option<String>,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels_bgra: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchCandidate {
    pub template_name: String,
    pub score: f64,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TooltipDetection {
    pub mode: InputMode,
    pub location: Option<Rect>,
    pub best_match: Option<MatchCandidate>,
    pub candidates: Vec<MatchCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    /// Row-major, one byte per pixel.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrWord {
    pub text: String,
    pub rect: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrBoundingBoxSource {
    TesseractTsv,
    TesseractTsvWordsGrouped,
    /// Structural TSV assignment was verified and corrected by geometric
    /// containment check (smallest containing line selection, matching
    /// upstream ParseHOcrText behavior).
    TesseractTsvGeometric,
    SyntheticFullImage,
    Unavailable,
}

impl OcrBoundingBoxSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TesseractTsv => "tesseract_tsv",
            Self::TesseractTsvWordsGrouped => "tesseract_tsv_words_grouped",
            Self::TesseractTsvGeometric => "tesseract_tsv_geometric",
            Self::SyntheticFullImage => "synthetic_full_image",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrLine {
    pub text: String,
    pub rect: Rect,
    pub words: Vec<OcrWord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrText {
    pub raw_text: String,
    pub lines: Vec<OcrLine>,
    pub bounding_box_source: OcrBoundingBoxSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemRarity {
    Normal,
    Magic,
    Rare,
    Legendary,
    Unique,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemAffix {
    pub canonical_id: Option<String>,
    pub raw_text: String,
    pub normalized_text: String,
    pub value_text: Option<String>,
    pub similarity: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemTooltip {
    pub raw_text: String,
    pub item_name: Option<String>,
    pub item_type_id: Option<String>,
    pub item_power: Option<u32>,
    pub rarity: ItemRarity,
    pub affixes: Vec<ItemAffix>,
    pub aspect_id: Option<String>,
    pub unique_id: Option<String>,
    pub rune_ids: Vec<String>,
    pub sigil_id: Option<String>,
}
