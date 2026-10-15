//! One-line text measurement shared by descriptor producers and overlay
//! backends.
//!
//! Upstream sizes its paragon panels from `MeasureString(font, size, text)`
//! on the overlay font (`OverlayHandler.cs:610-611`, `:631-635`), so the
//! rules crate needs a measurement it cannot compute itself. The trait lives
//! here (no I/O, no font handling); the overlay crate implements it from the
//! font face it draws with.

/// Pixel extent of one line of text, upstream's `MeasureString(...)` point.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextExtent {
    /// Advance width of the line.
    pub width: f32,
    /// Line height of the face at the measured size.
    pub height: f32,
}

/// Measures one line of overlay text.
pub trait TextMeasure: std::fmt::Debug + Send + Sync {
    /// Extent of `text` drawn on one line at `font_px`, or `None` when no
    /// font face is available. Callers must treat `None` as an unavailable
    /// measurement and never substitute an estimated width.
    fn measure(&self, text: &str, font_px: f32) -> Option<TextExtent>;
}

/// Deterministic measure for tests and tooling: every character advances
/// `advance_em` of the font size and the line height is `line_height_em`
/// of the font size. It reads no font; production code must pass the real
/// face measure instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedTextMeasure {
    pub advance_em: f32,
    pub line_height_em: f32,
}

impl FixedTextMeasure {
    /// A half-em advance and a 1.2 em line height: the proportions of a
    /// typical monospace face.
    pub const HALF_EM: Self = Self {
        advance_em: 0.5,
        line_height_em: 1.2,
    };
}

impl TextMeasure for FixedTextMeasure {
    fn measure(&self, text: &str, font_px: f32) -> Option<TextExtent> {
        Some(TextExtent {
            width: text.chars().count() as f32 * self.advance_em * font_px,
            height: self.line_height_em * font_px,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_measure_scales_with_characters_and_font_size() {
        let measure = FixedTextMeasure {
            advance_em: 0.5,
            line_height_em: 1.0,
        };
        let extent = measure.measure("abcd", 18.0);
        assert_eq!(
            extent,
            Some(TextExtent {
                width: 36.0,
                height: 18.0,
            })
        );
        assert_eq!(
            measure.measure("", 18.0).map(|extent| extent.width),
            Some(0.0)
        );
    }
}
