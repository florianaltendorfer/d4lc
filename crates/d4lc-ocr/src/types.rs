use crate::tesseract_language_code;
use d4lc_core::{CaptureFrame, GrayImage, OcrText, Rect};
use std::fmt;
use std::path::PathBuf;

/// Pixels handed to an OCR backend.
///
/// Upstream feeds Tesseract the 8-bit grayscale, binary-inverted tooltip
/// (`_currentScreenTooltipFilter`, ScreenProcessHandler.cs:635-636) and keeps
/// the original BGRA tooltip (`_currentScreenTooltip`) for the template
/// matching. Both payloads are carried explicitly so a job can record which
/// pixels the engine actually saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrImagePixels {
    /// Raw BGRA capture crop.
    Bgra(CaptureFrame),
    /// 8-bit grayscale crop. Upstream hands Tesseract a `Image<Gray, byte>`
    /// bitmap (`OcrHandler.ConvertToText` saves it as BMP), never a color
    /// bitmap.
    Gray(GrayImage),
}

/// Image payload passed into an OCR backend.
///
/// Runtime code should pass already cropped regions, not full screenshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrImage {
    pub pixels: OcrImagePixels,
}

impl OcrImage {
    pub const fn new(frame: CaptureFrame) -> Self {
        Self {
            pixels: OcrImagePixels::Bgra(frame),
        }
    }

    pub const fn from_gray(gray: GrayImage) -> Self {
        Self {
            pixels: OcrImagePixels::Gray(gray),
        }
    }

    pub const fn width(&self) -> u32 {
        match &self.pixels {
            OcrImagePixels::Bgra(frame) => frame.width,
            OcrImagePixels::Gray(gray) => gray.width,
        }
    }

    pub const fn height(&self) -> u32 {
        match &self.pixels {
            OcrImagePixels::Bgra(frame) => frame.height,
            OcrImagePixels::Gray(gray) => gray.height,
        }
    }

    /// Stable name of the payload kind for traces and diagnostics.
    pub const fn variant_name(&self) -> &'static str {
        match &self.pixels {
            OcrImagePixels::Bgra(_) => "bgra",
            OcrImagePixels::Gray(_) => "gray",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OcrBackendKind {
    #[default]
    Auto,
    Leptess,
}

impl OcrBackendKind {
    pub fn parse(value: &str) -> Result<Self, OcrError> {
        match value {
            "auto" => Ok(Self::Auto),
            "leptess" | "tesseract" => Ok(Self::Leptess),
            other => Err(OcrError::InvalidOption(format!(
                "unsupported OCR backend '{other}', expected auto or leptess"
            ))),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Leptess => "leptess",
        }
    }
}

impl fmt::Display for OcrBackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// OCR segment kind: where a text came from. Semantic parsing happens in `d4lc-parse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OcrSegmentKind {
    #[default]
    Generic,
    UpperTooltip,
    Affix,
    Aspect,
    UniqueAspect,
    Rune,
    Sigil,
    Socket,
    Splitter,
}

impl OcrSegmentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::UpperTooltip => "upper_tooltip",
            Self::Affix => "affix",
            Self::Aspect => "aspect",
            Self::UniqueAspect => "unique_aspect",
            Self::Rune => "rune",
            Self::Sigil => "sigil",
            Self::Socket => "socket",
            Self::Splitter => "splitter",
        }
    }
}

impl fmt::Display for OcrSegmentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OcrPageSegMode {
    /// The mode of marker-defined regions (upstream `engine.Process(img)`).
    ///
    /// Known divergence: upstream's TesseractOCR engine sets its
    /// `DefaultPageSegMode`, `PageSegMode.Auto` (PSM 3), on every call
    /// without an explicit mode. Without a mode here the pooled engine would
    /// keep PSM 11 from the upper tooltip section it recognized first;
    /// pinning PSM 11 makes the result independent of which engine ran
    /// which segment before. With the system tessdata, PSM 3 drops affix
    /// lines that PSM 11 reads.
    #[default]
    Default,
    /// Tesseract PSM 3.
    Auto,
    /// Tesseract PSM 6.
    SingleBlock,
    /// Tesseract PSM 11.
    SparseText,
    /// Tesseract PSM 12.
    SparseTextOsd,
}

impl OcrPageSegMode {
    pub fn parse(value: &str) -> Result<Self, OcrError> {
        match value {
            "default" => Ok(Self::Default),
            "auto" => Ok(Self::Auto),
            "single-block" | "single_block" | "6" => Ok(Self::SingleBlock),
            "sparse-text" | "sparse_text" | "11" => Ok(Self::SparseText),
            "sparse-text-osd" | "sparse_text_osd" | "12" => Ok(Self::SparseTextOsd),
            other => Err(OcrError::InvalidOption(format!(
                "unsupported OCR PSM '{other}', expected default, auto, single-block, sparse-text or sparse-text-osd"
            ))),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
            Self::SingleBlock => "single-block",
            Self::SparseText => "sparse-text",
            Self::SparseTextOsd => "sparse-text-osd",
        }
    }

    /// The PSM set before every recognition. Every mode sets one: a pooled
    /// engine must never recognize with the mode of its previous segment.
    pub const fn tesseract_value(self) -> &'static str {
        match self {
            Self::Auto => "3",
            Self::SingleBlock => "6",
            Self::Default | Self::SparseText => "11",
            Self::SparseTextOsd => "12",
        }
    }
}

impl fmt::Display for OcrPageSegMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OcrProfile {
    pub backend: OcrBackendKind,
    pub language: String,
    pub tessdata_path: Option<PathBuf>,
}

impl OcrProfile {
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            backend: OcrBackendKind::Auto,
            language: language.into(),
            tessdata_path: None,
        }
    }

    pub fn resolved_backend(&self) -> OcrBackendKind {
        match self.backend {
            OcrBackendKind::Auto => OcrBackendKind::Leptess,
            other => other,
        }
    }

    pub fn tesseract_language(&self) -> &'static str {
        tesseract_language_code(&self.language)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrOptions {
    pub segment_kind: OcrSegmentKind,
    pub page_seg_mode: OcrPageSegMode,
    pub preserve_raw_text: bool,
    pub source_region: Option<Rect>,
    /// When true, Tesseract TSV word/line geometry is requested and parsed.
    ///
    /// Keep this enabled for read-tooltip parity and for live worker OCR.
    /// Disabling it is reserved for diagnostics and tests.
    pub collect_tsv: bool,
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            segment_kind: OcrSegmentKind::Generic,
            page_seg_mode: OcrPageSegMode::SparseText,
            preserve_raw_text: true,
            source_region: None,
            collect_tsv: true,
        }
    }
}

/// Result of one recognition call: the parsed page text plus the structured
/// output it was derived from.
///
/// The structured output is Tesseract's TSV page text, the exact source the
/// line and word geometry in [`OcrText::lines`] comes from. Carrying it next
/// to the parsed text lets a caller prove what the backend reported instead of
/// re-deriving it from the parsed result: the item-name chain of a capture
/// stays observable end to end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrStructuredOutput {
    pub text: OcrText,
    /// Raw structured output; `None` when the caller did not request it
    /// ([`OcrOptions::collect_tsv`] false) or the backend produces none.
    /// A backend that requested structured output and cannot read it returns
    /// an error instead of a text-only result.
    pub tsv: Option<String>,
}

/// Maps the structured-output read of a backend into an error that surfaces.
///
/// A structured-output read failure is a failed recognition: the caller would
/// otherwise consume geometry-less fallback lines as a normal success
/// Kept as a named helper so the mapping is unit-testable without a
/// Tesseract failure.
pub fn structured_output_error(error: impl fmt::Display) -> OcrError {
    OcrError::Backend(format!(
        "failed to read OCR structured output (TSV): {error}"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrError {
    InvalidOption(String),
    InvalidImage(String),
    BackendUnavailable(String),
    Backend(String),
    PoolPoisoned,
}

impl fmt::Display for OcrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOption(message)
            | Self::InvalidImage(message)
            | Self::BackendUnavailable(message)
            | Self::Backend(message) => formatter.write_str(message),
            Self::PoolPoisoned => formatter.write_str("OCR engine pool mutex is poisoned"),
        }
    }
}

impl std::error::Error for OcrError {}

#[cfg(test)]
mod tests;
