//! Bounded overlay text: one explicit font assignment, one layout contract and
//! one rasterization path, shared by the X11 and Wayland overlay backends.
//!
//! Both backends draw all overlay text through this module, so it is
//! Unicode text (not an 8-bit server-side text path or an ASCII bitmap font)
//! under one contract:
//!
//! * **Explicit font selection rule** — upstream draws all overlay text with
//!   `Consolas` bold (`OverlayHandler.cs:910`), a Windows face. The port
//!   applies one fixed rule, not a silent fallback: [`OVERLAY_FONT_FAMILY`] in
//!   [`OVERLAY_FONT_WEIGHT`] when the system font database (`fontdb`) has it,
//!   otherwise the bundled [`BUNDLED_FONT_FAMILY`] Bold
//!   (`assets/fonts/Inconsolata-Bold.ttf`, SIL OFL 1.1, see
//!   `licenses/README.md`). Nothing else is ever tried, and the face in use
//!   is always named: [`TextRasterizer::assignment`] says which of the two
//!   was selected and the overlay status limitation repeats it. A face that
//!   cannot be loaded or parsed is reported ([`TextError`]) and the backend
//!   draws no text rather than substituting another face.
//! * **Explicit font size and line height** — the size is a parameter of every
//!   layout (the overlay's `OverlayFontSize`, upstream creates its one font at
//!   that size, `OverlayHandler.cs:884`, `:910`); the baseline distance scales
//!   from [`OVERLAY_TEXT_LINE_HEIGHT_PX`] at [`OVERLAY_TEXT_SIZE_PX`] by
//!   [`line_height_px`].
//! * **Explicit text area** — every layout is bounded by the render op's
//!   geometry rect, inset by [`OVERLAY_TEXT_PAD_X`] / [`OVERLAY_TEXT_PAD_Y`].
//! * **Explicit wrap contract** — [`TextWrapPolicy::WrapWords`] breaks at
//!   whitespace inside the area and hard-breaks a word that is wider than the
//!   line; the number of lines is bounded by the area's height.
//!   [`TextWrapPolicy::SingleLine`] never wraps (the upstream marker caption
//!   is a single line; the overlay surface clip bounds it).
//!   [`TextWrapPolicy::ExactLine`] additionally drops the padding and keeps
//!   leading whitespace: the line's pen and top are the area's left and top
//!   (paragon text).
//! * **No silent character replacement** — characters that the assignment has
//!   no glyph for are reported in [`TextLayout::unsupported`] and are *not*
//!   drawn; newline/tab are mapped (line break / space) because the overlay
//!   text areas define them. Nothing is replaced by a look-alike glyph.
//!
//! Glyph rasterization uses `swash`: one reusable [`ScaleContext`] and one
//! reused glyph [`Image`](swash::scale::image::Image) buffer per backend, so a draw allocates only the
//! per-line masks. Both backends composite the same coverage OVER what is
//! already drawn, with the alpha of [`text_pixel_argb`]: X11 through a RENDER
//! A8 mask, Wayland in software ([`blend_over`]). X11 and Wayland therefore
//! produce the same text pixels for identical render ops, and a line box never
//! replaces the panel beneath it.

use d4lc_core::text_measure::{TextExtent, TextMeasure};
use d4lc_core::Rect;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;
use swash::FontRef;

/// The font family the overlay prefers: upstream's `Consolas`
/// (`OverlayHandler.cs:910`, created bold), used when the system font
/// database has it in bold.
pub const OVERLAY_FONT_FAMILY: &str = "Consolas";

/// The preferred face's weight, as reported by
/// [`TextRasterizer::assignment`].
pub const OVERLAY_FONT_WEIGHT: &str = "Bold";

/// The family bundled for hosts without [`OVERLAY_FONT_FAMILY`]: Inconsolata
/// Bold, a metric-near monospace face under the SIL OFL 1.1.
pub const BUNDLED_FONT_FAMILY: &str = "Inconsolata";

/// Inconsolata Bold v3.000 from the official googlefonts/Inconsolata release
/// (`licenses/README.md` records URL and SHA-256).
static BUNDLED_FONT: &[u8] = include_bytes!("../assets/fonts/Inconsolata-Bold.ttf");

/// The size the baseline distance [`OVERLAY_TEXT_LINE_HEIGHT_PX`] is defined
/// at; [`TextRasterizer::layout`] takes the actual size as a parameter.
pub const OVERLAY_TEXT_SIZE_PX: u32 = 12;

/// Distance between two baselines of one text area at
/// [`OVERLAY_TEXT_SIZE_PX`]; other sizes scale it by the same ratio
/// ([`line_height_px`]).
pub const OVERLAY_TEXT_LINE_HEIGHT_PX: u32 = 14;

/// Baseline distance for `font_px`: [`OVERLAY_TEXT_LINE_HEIGHT_PX`] scaled by
/// `font_px / OVERLAY_TEXT_SIZE_PX`, rounded to whole pixels.
pub fn line_height_px(font_px: u32) -> u32 {
    (font_px * OVERLAY_TEXT_LINE_HEIGHT_PX + OVERLAY_TEXT_SIZE_PX / 2) / OVERLAY_TEXT_SIZE_PX
}

/// Left and right inset of the text area.
pub const OVERLAY_TEXT_PAD_X: i32 = 4;

/// Top inset of the text area.
pub const OVERLAY_TEXT_PAD_Y: i32 = 3;

/// Bound on the distinct unsupported codepoints one handle reports, so a
/// hostile/unusual string cannot grow the diagnostics without limit.
const MAX_REPORTED_UNSUPPORTED: usize = 16;

/// Why the overlay text path cannot render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    /// The system font database could not be read.
    FontDatabase(String),
    /// The selected face's data could not be loaded.
    FaceDataUnavailable { family: String },
    /// The selected face's data is not a font this build can parse.
    FontParse { family: String },
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FontDatabase(reason) => {
                write!(formatter, "system font database unavailable: {reason}")
            }
            Self::FaceDataUnavailable { family } => {
                write!(
                    formatter,
                    "overlay font \"{family}\" data could not be loaded"
                )
            }
            Self::FontParse { family } => {
                write!(
                    formatter,
                    "overlay font \"{family}\" is not a parseable font"
                )
            }
        }
    }
}

/// How a text area treats lines that are longer than the area is wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextWrapPolicy {
    /// One line, never wrapped. Used for the upstream marker caption, whose
    /// intended area is the overlay surface row (the surface clip bounds it).
    SingleLine,
    /// Break at whitespace inside the area; hard-break a word wider than the
    /// line; the area's height bounds the number of lines.
    WrapWords,
    /// One line, never wrapped, placed exactly: the pen starts at the area's
    /// left edge (no padding) and the line's top is the area's top, so the
    /// baseline sits one ascent below it. Upstream's paragon text
    /// (`DrawText(font, size, brush, left, top, text)`).
    ExactLine,
}

/// Size parameters shared by every line of one layout.
#[derive(Debug, Clone, Copy)]
struct LineStyle {
    line_height: u32,
    font_px: u32,
}

/// One laid-out line inside its text area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaidOutLine {
    /// The line's characters in source order. Characters the assignment
    /// cannot draw remain part of the line (they hold a space's advance, so
    /// neighbouring glyphs never collide), are listed in
    /// [`TextLayout::unsupported`], and rasterize to no ink at all.
    pub text: String,
    /// Measured line width in pixels.
    pub width_px: u32,
    /// Absolute x of the line's pen origin.
    pub pen_x: i32,
    /// Absolute y of the line's baseline.
    pub baseline_y: i32,
    /// Font size the line was laid out at, in whole pixels.
    pub font_px: u32,
}

/// The bounded layout of one render op's text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextLayout {
    /// Laid-out lines, top to bottom, at most the area's line budget.
    pub lines: Vec<LaidOutLine>,
    /// Codepoints the assigned font has no glyph for. They are not drawn and
    /// not replaced; a name/preset string carrying one is drawn without it.
    pub unsupported: Vec<char>,
    /// Characters the area had no room for: neither drawn nor consumed as a
    /// wrap break.
    pub clipped_chars: usize,
}

impl TextLayout {
    /// Whether every character of the source text was drawn inside the area.
    pub fn is_complete(&self) -> bool {
        self.unsupported.is_empty() && self.clipped_chars == 0
    }
}

/// A glyph coverage mask for one laid-out line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextLineMask {
    /// Absolute x of the mask's left edge.
    pub x: i32,
    /// Absolute y of the mask's top edge.
    pub y: i32,
    /// Mask width in pixels.
    pub width: u32,
    /// Mask height in pixels.
    pub height: u32,
    /// `width * height` glyph coverage bytes, row-major. Shared with the
    /// rasterizer's line cache: an unchanged line hands out the same buffer.
    pub coverage: Arc<[u8]>,
}

impl TextLineMask {
    /// The 8-bit alpha payload of this mask at `opacity`: one
    /// `coverage * opacity / 255` byte per pixel (the alpha
    /// [`text_pixel_argb`] gives the pixel), rows padded to `row_stride`
    /// bytes. X11 uploads it as the A8 mask it composites the text colour
    /// through, so the glyphs blend over the panel beneath instead of
    /// replacing the whole line box.
    pub fn to_alpha_payload(&self, opacity: u8, row_stride: usize) -> Vec<u8> {
        let width = self.width as usize;
        let row_stride = row_stride.max(width);
        let mut payload = vec![0u8; row_stride * self.height as usize];
        if width == 0 {
            return payload;
        }
        for (row, coverage_row) in self.coverage.chunks_exact(width).enumerate() {
            let target = &mut payload[row * row_stride..row * row_stride + width];
            for (alpha, coverage) in target.iter_mut().zip(coverage_row) {
                *alpha = ((u32::from(opacity) * u32::from(*coverage)) / 255) as u8;
            }
        }
        payload
    }

    /// Composites this mask over an existing premultiplied-ARGB32 surface
    /// (the Wayland SHM canvas), clipped to the surface. Pixels outside the
    /// surface are dropped, which is what bounds a single-line marker caption
    /// to the overlay's own area.
    pub fn paint_into(
        &self,
        surface: &mut [u8],
        surface_width: u32,
        surface_height: u32,
        color_argb: u32,
        opacity: u8,
    ) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        for row in 0..self.height {
            let y = self.y + row as i32;
            if y < 0 || y >= surface_height as i32 {
                continue;
            }
            for column in 0..self.width {
                let x = self.x + column as i32;
                if x < 0 || x >= surface_width as i32 {
                    continue;
                }
                let coverage =
                    self.coverage[(row as usize) * (self.width as usize) + column as usize];
                if coverage == 0 {
                    continue;
                }
                let index = ((y as usize) * (surface_width as usize) + x as usize) * 4;
                let Some(target) = surface.get_mut(index..index + 4) else {
                    continue;
                };
                let beneath = u32::from_le_bytes([target[0], target[1], target[2], target[3]]);
                let pixel = blend_over(text_pixel_argb(color_argb, opacity, coverage), beneath);
                target.copy_from_slice(&pixel.to_le_bytes());
            }
        }
    }
}

/// Porter-Duff OVER of two premultiplied ARGB32 pixels: what the X11 RENDER
/// `Over` composite does with the same alpha mask, so both backends blend
/// text onto a panel identically.
pub fn blend_over(source: u32, beneath: u32) -> u32 {
    let inverse = 255 - (source >> 24);
    let channel = |shift: u32| {
        let top = (source >> shift) & 0xFF;
        let bottom = (beneath >> shift) & 0xFF;
        (top + (bottom * inverse + 127) / 255).min(255) << shift
    };
    channel(24) | channel(16) | channel(8) | channel(0)
}

/// The one coverage-to-pixel rule both backends use: `color_argb` carries the
/// render op's straight-alpha color, `opacity` the op's alpha and `coverage`
/// one glyph mask byte; the result is premultiplied ARGB32.
pub fn text_pixel_argb(color_argb: u32, opacity: u8, coverage: u8) -> u32 {
    let alpha = (u32::from(opacity) * u32::from(coverage)) / 255;
    let channel = |shift: u32| ((color_argb >> shift) & 0xFF) * alpha / 255;
    (alpha << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// Everything one overlay handle reports about its text path. Built up from
/// the layouts it produced; never swallows a problem silently.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextDiagnostics {
    /// Set when the assigned font could not be resolved: the exact reason,
    /// with the family name. Text is then not drawn at all.
    pub font_unavailable: Option<String>,
    /// The distinct codepoints the assigned font has no glyph for.
    pub unsupported_codepoints: Vec<char>,
    /// Source characters that did not fit their text area, summed over all
    /// layouts.
    pub clipped_chars: u64,
}

impl TextDiagnostics {
    /// Records one layout's unsupported codepoints and clipped characters.
    pub fn record(&mut self, layout: &TextLayout) {
        self.clipped_chars = self
            .clipped_chars
            .saturating_add(layout.clipped_chars as u64);
        for codepoint in &layout.unsupported {
            if self.unsupported_codepoints.len() >= MAX_REPORTED_UNSUPPORTED {
                break;
            }
            if !self.unsupported_codepoints.contains(codepoint) {
                self.unsupported_codepoints.push(*codepoint);
            }
        }
    }

    /// Records that the assigned font is unavailable (and clears the text
    /// path; nothing is drawn from a substituted face).
    pub fn record_font_unavailable(&mut self, error: &TextError) {
        self.font_unavailable = Some(error.to_string());
    }

    /// Whether every character of every laid-out text was drawn.
    pub fn is_clean(&self) -> bool {
        self.font_unavailable.is_none()
            && self.unsupported_codepoints.is_empty()
            && self.clipped_chars == 0
    }
}

/// The resolved font face: its data and the assignment it was resolved
/// from. Cheap to clone (the data is shared) and free of any scaling state,
/// so it can measure text from any thread.
#[derive(Clone)]
pub struct FaceMetrics {
    data: Arc<Vec<u8>>,
    face_index: usize,
    assignment: Arc<str>,
}

impl fmt::Debug for FaceMetrics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FaceMetrics")
            .field("assignment", &self.assignment)
            .finish_non_exhaustive()
    }
}

impl FaceMetrics {
    /// The selected face, named (see [`TextRasterizer::assignment`]).
    pub fn assignment(&self) -> &str {
        &self.assignment
    }

    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.data, self.face_index)
    }

    /// Distance from a line's baseline to its top at `font_px`.
    fn ascent_px(&self, font_px: u32) -> f32 {
        self.font()
            .map_or(0.0, |font| font.metrics(&[]).scale(font_px as f32).ascent)
    }

    /// Distance from a line's baseline to its bottom at `font_px`.
    fn descent_px(&self, font_px: u32) -> f32 {
        self.font()
            .map_or(0.0, |font| font.metrics(&[]).scale(font_px as f32).descent)
    }
}

impl TextMeasure for FaceMetrics {
    /// Advance width of `text` on one line, with the same character mapping
    /// [`TextRasterizer::layout`] draws: `\r` and control characters take no
    /// room, a tab is a space, a character the face has no glyph for holds a
    /// space's advance, and a newline ends the line (the rest is not
    /// drawn on a single line). The height is the face's ascent plus descent.
    fn measure(&self, text: &str, font_px: f32) -> Option<TextExtent> {
        let font = self.font()?;
        let charmap = font.charmap();
        let glyph_metrics = font.glyph_metrics(&[]).scale(font_px);
        let space_advance = glyph_metrics.advance_width(charmap.map(' '));
        let mut width = 0.0f32;
        for character in text.chars() {
            match character {
                '\n' => break,
                '\r' => {}
                '\t' => width += space_advance,
                character if character.is_control() => {}
                character => {
                    let glyph_id = charmap.map(character);
                    width += if glyph_id == 0 {
                        space_advance
                    } else {
                        glyph_metrics.advance_width(glyph_id)
                    };
                }
            }
        }
        let metrics = font.metrics(&[]).scale(font_px);
        Some(TextExtent {
            width,
            height: metrics.ascent + metrics.descent,
        })
    }
}

/// One reusable glyph rasterizer: the resolved face, one `swash` scaling
/// context and one reused glyph image buffer.
pub struct TextRasterizer {
    face: FaceMetrics,
    context: ScaleContext,
    glyph_image: swash::scale::image::Image,
    glyphs: HashMap<(u16, u32), Option<Arc<GlyphBitmap>>>,
    /// Finished line coverage by font size, then text.
    lines: HashMap<u32, HashMap<String, CachedLine>>,
    line_count: usize,
    stats: TextCacheStats,
}

/// Most distinct glyph bitmaps / line masks kept before a cache is dropped
/// and refilled: far above what the paragon panels and markers need, so the
/// bound only protects against unbounded growth.
const MAX_CACHED_GLYPHS: usize = 4096;
const MAX_CACHED_LINES: usize = 512;

/// One rasterized glyph at one size, positioned from the pen origin and
/// baseline.
#[derive(Debug)]
struct GlyphBitmap {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[derive(Debug)]
struct CachedLine {
    width: u32,
    height: u32,
    coverage: Arc<[u8]>,
}

/// Hits and misses of the rasterizer's caches since it was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextCacheStats {
    pub glyph_hits: u64,
    pub glyph_misses: u64,
    pub line_hits: u64,
    pub line_misses: u64,
}

impl fmt::Debug for TextRasterizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextRasterizer")
            .field("assignment", &self.face.assignment)
            .finish_non_exhaustive()
    }
}

impl TextMeasure for TextRasterizer {
    fn measure(&self, text: &str, font_px: f32) -> Option<TextExtent> {
        self.face.measure(text, font_px)
    }
}

impl TextRasterizer {
    /// Resolves the assigned family against the system font database.
    ///
    /// # Errors
    ///
    /// Returns [`TextError`] when the selected face (the system
    /// [`OVERLAY_FONT_FAMILY`], else the bundled [`BUNDLED_FONT_FAMILY`])
    /// cannot be loaded or parsed. The caller reports the reason through
    /// [`TextDiagnostics`]; no other font is substituted.
    pub fn load_system() -> Result<Self, TextError> {
        Ok(Self::with_face(system_face()?))
    }

    fn with_face(face: FaceMetrics) -> Self {
        Self {
            face,
            context: ScaleContext::new(),
            glyph_image: swash::scale::image::Image::new(),
            glyphs: HashMap::new(),
            lines: HashMap::new(),
            line_count: 0,
            stats: TextCacheStats::default(),
        }
    }

    /// Cache hits and misses so far.
    pub fn cache_stats(&self) -> TextCacheStats {
        self.stats
    }
}

/// How long a failed font resolution is remembered before the font database
/// is scanned again. A missing family is reported, not retried on every
/// measurement.
const FACE_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

enum SystemFace {
    Unresolved,
    Loaded(FaceMetrics),
    Failed {
        error: TextError,
        at: std::time::Instant,
    },
}

static SYSTEM_FACE: std::sync::Mutex<SystemFace> = std::sync::Mutex::new(SystemFace::Unresolved);

/// The assigned face, resolved once per process and shared by the overlay
/// backends and the descriptor producers, so what is measured is what is
/// drawn. A failure is remembered for `FACE_RETRY_AFTER` before a retry.
///
/// # Errors
///
/// The same errors as [`TextRasterizer::load_system`]; no fallback face.
pub fn system_face() -> Result<FaceMetrics, TextError> {
    let mut cached = SYSTEM_FACE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match &*cached {
        SystemFace::Loaded(face) => return Ok(face.clone()),
        SystemFace::Failed { error, at } if at.elapsed() < FACE_RETRY_AFTER => {
            return Err(error.clone());
        }
        _ => {}
    }
    match resolve_system_face() {
        Ok(face) => {
            *cached = SystemFace::Loaded(face.clone());
            Ok(face)
        }
        Err(error) => {
            *cached = SystemFace::Failed {
                error: error.clone(),
                at: std::time::Instant::now(),
            };
            Err(error)
        }
    }
}

/// Resolves the system face on a background thread, so the first measurement
/// or overlay start does not wait for the font database scan. A failure is
/// reported by the first consumer, as without the preload.
pub fn preload_system_face() {
    let _ = std::thread::Builder::new()
        .name("d4lc-font-preload".to_string())
        .spawn(|| {
            let _ = system_face();
        });
}

/// Measures text on the system face; `None` while it cannot be resolved.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTextMeasure;

impl TextMeasure for SystemTextMeasure {
    fn measure(&self, text: &str, font_px: f32) -> Option<TextExtent> {
        system_face().ok()?.measure(text, font_px)
    }
}

/// Applies the font selection rule: the system's [`OVERLAY_FONT_FAMILY`] in
/// bold, otherwise the bundled face.
fn resolve_system_face() -> Result<FaceMetrics, TextError> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(OVERLAY_FONT_FAMILY)],
        weight: fontdb::Weight::BOLD,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    };
    // `fontdb` answers with the nearest weight of the family; only a bold
    // face satisfies the rule.
    let system = database.query(&query).filter(|id| {
        database
            .face(*id)
            .is_some_and(|face| face.weight.0 >= fontdb::Weight::SEMIBOLD.0)
    });
    if let Some(id) = system {
        let (data, face_index) = database
            .with_face_data(id, |data, index| (data.to_vec(), index as usize))
            .ok_or_else(|| TextError::FaceDataUnavailable {
                family: OVERLAY_FONT_FAMILY.to_string(),
            })?;
        return face_from_data(
            Arc::new(data),
            face_index,
            OVERLAY_FONT_FAMILY,
            format!("{OVERLAY_FONT_FAMILY} {OVERLAY_FONT_WEIGHT} (system)"),
        );
    }
    face_from_data(
        Arc::new(BUNDLED_FONT.to_vec()),
        0,
        BUNDLED_FONT_FAMILY,
        format!(
            "{BUNDLED_FONT_FAMILY} {OVERLAY_FONT_WEIGHT} (bundled; {OVERLAY_FONT_FAMILY} \
             {OVERLAY_FONT_WEIGHT} is not installed)"
        ),
    )
}

fn face_from_data(
    data: Arc<Vec<u8>>,
    face_index: usize,
    family: &str,
    assignment: String,
) -> Result<FaceMetrics, TextError> {
    FontRef::from_index(&data, face_index).ok_or_else(|| TextError::FontParse {
        family: family.to_string(),
    })?;
    Ok(FaceMetrics {
        data,
        face_index,
        assignment: assignment.into(),
    })
}

impl TextRasterizer {
    /// The selected face, named: `"Consolas Bold (system)"` or
    /// `"Inconsolata Bold (bundled; ...)"`.
    pub fn assignment(&self) -> &str {
        self.face.assignment()
    }

    /// A shareable measurer for the resolved face.
    pub fn metrics(&self) -> FaceMetrics {
        self.face.clone()
    }

    /// The distance from a line's baseline to its top at `font_px`, in
    /// pixels.
    pub fn ascent_px(&self, font_px: u32) -> f32 {
        self.face.ascent_px(font_px)
    }

    /// Lays `text` out inside `area` under `policy`, bounded by the area.
    ///
    /// Contract (see the module docs): the pen starts at `area.x +
    /// OVERLAY_TEXT_PAD_X`, line `n`'s baseline sits at `area.y +
    /// OVERLAY_TEXT_PAD_Y + ascent + n * OVERLAY_TEXT_LINE_HEIGHT_PX`, the
    /// usable width is the area minus the horizontal padding, and the line
    /// budget follows from the area's height. Whitespace is a break
    /// opportunity and is never drawn at a line start; characters with no
    /// glyph in the assignment are reported in `unsupported` and skipped;
    /// characters the area has no room for are counted in `clipped_chars`.
    pub fn layout(
        &self,
        area: Rect,
        policy: TextWrapPolicy,
        text: &str,
        font_px: u32,
    ) -> TextLayout {
        let font = match self.face.font() {
            Some(font) => font,
            None => return TextLayout::default(),
        };
        let charmap = font.charmap();
        let glyph_metrics = font.glyph_metrics(&[]).scale(font_px as f32);
        let space_advance = glyph_metrics.advance_width(charmap.map(' '));
        let line_height = line_height_px(font_px).max(1);
        let (pad_x, pad_y) = match policy {
            TextWrapPolicy::ExactLine => (0, 0),
            TextWrapPolicy::SingleLine | TextWrapPolicy::WrapWords => {
                (OVERLAY_TEXT_PAD_X, OVERLAY_TEXT_PAD_Y)
            }
        };
        let pen_x = area.x.saturating_add(pad_x);
        let available_width = match policy {
            // A single line is never wrapped: its intended area is the
            // overlay surface row, and the surface clip bounds it.
            TextWrapPolicy::SingleLine | TextWrapPolicy::ExactLine => f32::INFINITY,
            TextWrapPolicy::WrapWords => area
                .width
                .saturating_sub((OVERLAY_TEXT_PAD_X * 2) as u32)
                .max(1) as f32,
        };
        let max_lines = match policy {
            TextWrapPolicy::SingleLine | TextWrapPolicy::ExactLine => 1,
            TextWrapPolicy::WrapWords => {
                let usable_height = area.height.saturating_sub((OVERLAY_TEXT_PAD_Y * 2) as u32);
                ((usable_height / line_height) as usize).max(1)
            }
        };
        // The first baseline sits one rounded ascent below the (padded) top.
        let first_baseline = area.y + pad_y + self.face.ascent_px(font_px).round() as i32;

        // Map the source characters once: newline/tab are mapped (the text
        // areas define them), every other control character and every
        // character without a glyph is reported and skipped — never replaced.
        let mut mapped: Vec<(char, bool)> = Vec::with_capacity(text.chars().count());
        let mut unsupported: Vec<char> = Vec::new();
        for character in text.chars() {
            match character {
                '\r' => {}
                '\n' => mapped.push(('\n', false)),
                '\t' => mapped.push((' ', false)),
                character if character.is_control() => {
                    if !unsupported.contains(&character) {
                        unsupported.push(character);
                    }
                }
                character => {
                    let missing = charmap.map(character) == 0;
                    if missing && !unsupported.contains(&character) {
                        unsupported.push(character);
                    }
                    mapped.push((character, missing));
                }
            }
        }

        let advance = |character: char, missing: bool| -> f32 {
            if missing {
                space_advance
            } else {
                glyph_metrics.advance_width(charmap.map(character))
            }
        };

        let line_style = LineStyle {
            line_height,
            font_px,
        };
        let mut lines: Vec<LaidOutLine> = Vec::new();
        let mut line_chars: Vec<char> = Vec::new();
        let mut line_width = 0.0f32;
        // Mapped characters that are not drawn because they are consumed as a
        // break or as line-start whitespace. Kept apart from `clipped_chars`
        // so "the area is too small" is never confused with "the text had
        // whitespace".
        let mut consumed_mapped = 0usize;
        let mut stopped_early = false;
        let mut pending_spaces = 0usize;

        let mut index = 0usize;
        while index < mapped.len() {
            let (character, _missing) = mapped[index];
            if character == '\n' {
                lines.push(Self::finish_line(
                    &line_chars,
                    line_width,
                    pen_x,
                    first_baseline,
                    lines.len(),
                    line_style,
                ));
                line_chars.clear();
                line_width = 0.0;
                pending_spaces = 0;
                consumed_mapped += 1;
                index += 1;
                if lines.len() >= max_lines && index < mapped.len() {
                    stopped_early = true;
                    break;
                }
                continue;
            }
            if character == ' ' {
                pending_spaces += 1;
                index += 1;
                continue;
            }

            // Measure the whole word, so the wrap decision is per word.
            let word_start = index;
            let mut word_width = 0.0f32;
            while index < mapped.len() && mapped[index].0 != ' ' && mapped[index].0 != '\n' {
                word_width += advance(mapped[index].0, mapped[index].1);
                index += 1;
            }
            let line_has_content = !line_chars.is_empty();
            let space_width = if line_has_content {
                pending_spaces as f32 * space_advance
            } else {
                0.0
            };

            if line_has_content && line_width + space_width + word_width > available_width {
                // Wrap before the word: the pending whitespace is the break.
                lines.push(Self::finish_line(
                    &line_chars,
                    line_width,
                    pen_x,
                    first_baseline,
                    lines.len(),
                    line_style,
                ));
                line_chars.clear();
                line_width = 0.0;
                consumed_mapped += pending_spaces;
                pending_spaces = 0;
                if lines.len() >= max_lines {
                    // The word and everything after it have no room.
                    stopped_early = true;
                    break;
                }
            }
            // An exact line draws its leading whitespace, as DirectWrite does
            // and the measurement counts; wrapped and single-line text
            // consume it at a line start.
            if !line_chars.is_empty() || policy == TextWrapPolicy::ExactLine {
                line_chars.resize(line_chars.len() + pending_spaces, ' ');
                line_width += pending_spaces as f32 * space_advance;
            } else {
                // Whitespace at a line start is consumed, not drawn.
                consumed_mapped += pending_spaces;
            }
            pending_spaces = 0;

            // Place the word, hard-breaking it when it is wider than a line.
            let word_end = index;
            for (offset, (character, missing)) in
                mapped[word_start..word_end].iter().copied().enumerate()
            {
                let character_width = advance(character, missing);
                if !line_chars.is_empty() && line_width + character_width > available_width {
                    lines.push(Self::finish_line(
                        &line_chars,
                        line_width,
                        pen_x,
                        first_baseline,
                        lines.len(),
                        line_style,
                    ));
                    line_chars.clear();
                    line_width = 0.0;
                    if lines.len() >= max_lines {
                        index = word_start + offset;
                        stopped_early = true;
                        break;
                    }
                }
                line_chars.push(character);
                line_width += character_width;
            }
            if stopped_early {
                break;
            }
        }

        if !stopped_early && !line_chars.is_empty() && lines.len() < max_lines {
            lines.push(Self::finish_line(
                &line_chars,
                line_width,
                pen_x,
                first_baseline,
                lines.len(),
                line_style,
            ));
        }

        let drawn_characters: usize = lines.iter().map(|line| line.text.chars().count()).sum();
        let clipped_chars = mapped
            .len()
            .saturating_sub(drawn_characters)
            .saturating_sub(consumed_mapped);

        TextLayout {
            lines,
            unsupported,
            clipped_chars,
        }
    }

    fn finish_line(
        characters: &[char],
        width: f32,
        pen_x: i32,
        first_baseline: i32,
        line_index: usize,
        style: LineStyle,
    ) -> LaidOutLine {
        LaidOutLine {
            text: characters.iter().collect(),
            width_px: width.ceil().max(0.0) as u32,
            pen_x,
            baseline_y: first_baseline + style.line_height as i32 * line_index as i32,
            font_px: style.font_px,
        }
    }

    /// Rasterizes one laid-out line into `mask` (reused across calls). The
    /// mask carries the absolute position of its top-left corner, so a
    /// backend only composites it: X11 uploads [`TextLineMask::to_alpha_payload`]
    /// as the mask at `(mask.x, mask.y)`, Wayland calls
    /// [`TextLineMask::paint_into`].
    ///
    /// The coverage is position independent, so a line (text and size) is
    /// rasterized once and handed out again from the cache; glyph bitmaps are
    /// cached per glyph and size, so a new line of known glyphs only
    /// composes them.
    ///
    /// Glyph ink outside the line box (a face whose ascender exceeds the line
    /// height, for example) is clipped by the mask bounds.
    pub fn rasterize_line_into(&mut self, line: &LaidOutLine, mask: &mut TextLineMask) {
        let width = line.width_px.max(1);
        // The line box spans the face ascent above and descent below the
        // baseline, with one spare row on each side so a hinted outline is
        // never clipped by rounding.
        let ascent_px = self.face.ascent_px(line.font_px);
        let height = ascent_px.ceil() as u32 + self.face.descent_px(line.font_px).ceil() as u32 + 2;
        let baseline_in_mask = ascent_px.ceil() as i32 + 1;
        mask.x = line.pen_x;
        mask.y = line.baseline_y - baseline_in_mask;
        mask.width = width;
        mask.height = height;

        if let Some(cached) = self
            .lines
            .get(&line.font_px)
            .and_then(|lines| lines.get(line.text.as_str()))
            .filter(|cached| cached.width == width && cached.height == height)
        {
            self.stats.line_hits += 1;
            mask.coverage = Arc::clone(&cached.coverage);
            return;
        }
        self.stats.line_misses += 1;

        let mut coverage = vec![0u8; (width as usize) * (height as usize)];
        let face = self.face.clone();
        if let Some(font) = face.font() {
            let charmap = font.charmap();
            let glyph_metrics = font.glyph_metrics(&[]).scale(line.font_px as f32);
            let space_advance = glyph_metrics.advance_width(charmap.map(' '));
            let mut pen_x = 0.0f32;
            for character in line.text.chars() {
                let glyph_id = charmap.map(character);
                let advance = if glyph_id == 0 {
                    space_advance
                } else {
                    glyph_metrics.advance_width(glyph_id)
                };
                if glyph_id != 0 {
                    if let Some(bitmap) = self.glyph(line.font_px, glyph_id) {
                        compose_glyph(
                            &mut coverage,
                            width,
                            height,
                            &bitmap,
                            pen_x.round() as i32,
                            baseline_in_mask,
                        );
                    }
                }
                pen_x += advance;
            }
        }

        let coverage: Arc<[u8]> = coverage.into();
        if self.line_count >= MAX_CACHED_LINES {
            self.lines.clear();
            self.line_count = 0;
        }
        self.lines.entry(line.font_px).or_default().insert(
            line.text.clone(),
            CachedLine {
                width,
                height,
                coverage: Arc::clone(&coverage),
            },
        );
        self.line_count += 1;
        mask.coverage = coverage;
    }

    /// The glyph's bitmap at `font_px`, rendered on first use. `None` when
    /// the glyph has no outline.
    fn glyph(&mut self, font_px: u32, glyph_id: u16) -> Option<Arc<GlyphBitmap>> {
        if let Some(cached) = self.glyphs.get(&(glyph_id, font_px)) {
            self.stats.glyph_hits += 1;
            return cached.clone();
        }
        self.stats.glyph_misses += 1;
        let face = self.face.clone();
        let font = face.font()?;
        let mut scaler = self
            .context
            .builder(font)
            .size(font_px as f32)
            .hint(true)
            .build();
        let mut render = Render::new(&[Source::Outline]);
        render.format(Format::Alpha);
        // `render_into` only resizes the reused buffer and accumulates the
        // new coverage into it, so the previous glyph's pixels must go
        // first or they bleed into this one.
        self.glyph_image.data.clear();
        let bitmap = render
            .render_into(&mut scaler, glyph_id, &mut self.glyph_image)
            .then(|| {
                let placement = self.glyph_image.placement;
                Arc::new(GlyphBitmap {
                    left: placement.left,
                    top: placement.top,
                    width: placement.width,
                    height: placement.height,
                    data: self.glyph_image.data.clone(),
                })
            });
        if self.glyphs.len() >= MAX_CACHED_GLYPHS {
            self.glyphs.clear();
        }
        self.glyphs.insert((glyph_id, font_px), bitmap.clone());
        bitmap
    }
}

/// Adds `bitmap` to a line's coverage with its pen at column `pen_column` and
/// the baseline `baseline_row` rows down, clipped to the line box. `swash`
/// reports the image offset from the baseline in the font's y-up space: a
/// capital's image sits `top` pixels *above* the baseline.
fn compose_glyph(
    coverage: &mut [u8],
    width: u32,
    height: u32,
    bitmap: &GlyphBitmap,
    pen_column: i32,
    baseline_row: i32,
) {
    let glyph_column = pen_column + bitmap.left;
    let glyph_row = baseline_row - bitmap.top;
    let source_stride = bitmap.width as usize;
    for row in 0..bitmap.height as i32 {
        let mask_row = glyph_row + row;
        if mask_row < 0 || mask_row >= height as i32 {
            continue;
        }
        for column in 0..source_stride {
            let value = bitmap.data[row as usize * source_stride + column];
            if value == 0 {
                continue;
            }
            let mask_column = glyph_column + column as i32;
            if mask_column < 0 || mask_column >= width as i32 {
                continue;
            }
            let slot = mask_row as usize * width as usize + mask_column as usize;
            coverage[slot] = coverage[slot].saturating_add(value);
        }
    }
}

/// Uploadable alpha payloads of finished lines, per (line coverage, opacity,
/// row stride): X11's A8 mask upload is built once per distinct line instead
/// of on every draw. Keyed by the line cache's shared coverage buffer, which
/// the entry keeps alive so its address is never reused for different
/// content.
#[derive(Debug, Default)]
pub struct PayloadCache {
    entries: HashMap<(usize, u8, usize), PayloadEntry>,
}

#[derive(Debug)]
struct PayloadEntry {
    _coverage: Arc<[u8]>,
    payload: Arc<[u8]>,
}

impl PayloadCache {
    /// The alpha payload of `mask` at `opacity` with `row_stride`-byte rows.
    pub fn payload(&mut self, mask: &TextLineMask, opacity: u8, row_stride: usize) -> Arc<[u8]> {
        let key = (
            Arc::as_ptr(&mask.coverage).cast::<u8>() as usize,
            opacity,
            row_stride,
        );
        if let Some(entry) = self.entries.get(&key) {
            return Arc::clone(&entry.payload);
        }
        if self.entries.len() >= MAX_CACHED_LINES {
            self.entries.clear();
        }
        let payload: Arc<[u8]> = mask.to_alpha_payload(opacity, row_stride).into();
        self.entries.insert(
            key,
            PayloadEntry {
                _coverage: Arc::clone(&mask.coverage),
                payload: Arc::clone(&payload),
            },
        );
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The assigned font must be installed for the overlay to draw text at
    /// all, so these tests resolve it exactly like the backend does and fail
    /// with the precise reason instead of skipping.
    fn overlay_rasterizer() -> TextRasterizer {
        match TextRasterizer::load_system() {
            Ok(rasterizer) => rasterizer,
            Err(error) => panic!("overlay text contract requires the assigned font: {error}"),
        }
    }

    fn area(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn ink_columns(mask: &TextLineMask) -> Vec<(u32, u32)> {
        let mut columns = Vec::new();
        for column in 0..mask.width {
            let mut ink = 0u32;
            for row in 0..mask.height {
                ink += u32::from(mask.coverage[(row * mask.width + column) as usize]);
            }
            if ink > 0 {
                columns.push((column, ink));
            }
        }
        columns
    }

    fn ink_rows(mask: &TextLineMask) -> Vec<u32> {
        (0..mask.height)
            .filter(|row| {
                (0..mask.width)
                    .any(|column| mask.coverage[(row * mask.width + column) as usize] > 0)
            })
            .collect()
    }

    #[test]
    fn text_pixel_premultiplies_coverage_and_opacity() {
        // Half opacity over half coverage: alpha is 128 * 128 / 255 = 64 and
        // each colour channel is that much of the straight-alpha colour.
        let pixel = text_pixel_argb(0xFF204080, 128, 128);
        assert_eq!(pixel >> 24, 64);
        assert_eq!((pixel >> 16) & 0xFF, 0x08);
        assert_eq!((pixel >> 8) & 0xFF, 0x10);
        assert_eq!(pixel & 0xFF, 0x20);
        // Full opacity and coverage is the straight colour, premultiplied.
        assert_eq!(text_pixel_argb(0xFF204080, 255, 255), 0xFF204080);
        // Zero coverage paints nothing at all.
        assert_eq!(text_pixel_argb(0xFF204080, 255, 0), 0);
    }

    #[test]
    fn the_alpha_payload_is_coverage_times_opacity_with_padded_rows() {
        let mask = TextLineMask {
            x: 0,
            y: 0,
            width: 3,
            height: 2,
            coverage: vec![0, 128, 255, 255, 0, 64].into(),
        };
        let payload = mask.to_alpha_payload(255, 4);
        assert_eq!(payload, vec![0, 128, 255, 0, 255, 0, 64, 0]);
        // Half opacity halves every alpha byte, exactly like the alpha of
        // `text_pixel_argb` for the same coverage.
        let half = mask.to_alpha_payload(128, 3);
        assert_eq!(half[2], (text_pixel_argb(0xFFFFFFFF, 128, 255) >> 24) as u8);
        assert_eq!(half.len(), 6);
    }

    #[test]
    fn canvas_text_blends_over_the_panel_beneath() {
        let mask = TextLineMask {
            x: 1,
            y: 0,
            width: 3,
            height: 1,
            coverage: vec![0, 128, 255].into(),
        };
        // A semi-transparent dark panel, premultiplied.
        let panel = 0xC8131313u32;
        let mut surface: Vec<u8> = std::iter::repeat_n(panel.to_le_bytes(), 4)
            .flatten()
            .collect();
        mask.paint_into(&mut surface, 4, 1, 0xFFFFFFFF, 255);
        let pixel = |x: usize| {
            u32::from_le_bytes([
                surface[x * 4],
                surface[x * 4 + 1],
                surface[x * 4 + 2],
                surface[x * 4 + 3],
            ])
        };
        // Outside the mask and under zero coverage the panel is untouched:
        // the line box never punches a hole into what lies beneath.
        assert_eq!(pixel(0), panel);
        assert_eq!(pixel(1), panel);
        // Partial coverage mixes text and panel; full coverage is the text.
        assert_eq!(
            pixel(2),
            blend_over(text_pixel_argb(0xFFFFFFFF, 255, 128), panel)
        );
        assert!(pixel(2) >> 24 > panel >> 24);
        assert_eq!(pixel(3), 0xFFFFFFFF);
    }

    #[test]
    fn a_glyph_rasterizes_the_same_after_any_other_glyph() {
        // The glyph image buffer is reused across glyphs; a narrow glyph
        // rendered after a wide one must not carry the wide one's pixels.
        let line_mask = |rasterizer: &mut TextRasterizer, text: &str| {
            let layout =
                rasterizer.layout(area(0, 0, 400, 40), TextWrapPolicy::SingleLine, text, 18);
            let line = match layout.lines.first() {
                Some(line) => line.clone(),
                None => panic!("{text} must lay out"),
            };
            let mut mask = TextLineMask::default();
            rasterizer.rasterize_line_into(&line, &mut mask);
            mask
        };
        let mut fresh = overlay_rasterizer();
        let alone = line_mask(&mut fresh, "i");
        let mut used = overlay_rasterizer();
        line_mask(&mut used, "W@M");
        let after_wide = line_mask(&mut used, "i");
        assert_eq!(alone.coverage, after_wide.coverage);
    }

    #[test]
    fn blending_over_nothing_keeps_the_source_pixel() {
        assert_eq!(blend_over(0x80402010, 0), 0x80402010);
        assert_eq!(blend_over(0, 0xC8131313), 0xC8131313);
        assert_eq!(blend_over(0xFF204080, 0xC8131313), 0xFF204080);
    }

    #[test]
    fn long_panel_text_wraps_inside_its_area() {
        let rasterizer = overlay_rasterizer();
        let panel = area(40, 100, 180, 50);
        let text = "Greater affix of the ancestral fury with a very long name \
                   that keeps going past the panel";
        let layout =
            rasterizer.layout(panel, TextWrapPolicy::WrapWords, text, OVERLAY_TEXT_SIZE_PX);
        assert!(!layout.lines.is_empty(), "the text must lay out at all");
        let usable = (panel.width - (OVERLAY_TEXT_PAD_X * 2) as u32) as i32;
        for line in &layout.lines {
            assert!(
                line.width_px as i32 <= usable,
                "line {:?} is {}px wide, the area allows {usable}px",
                line.text,
                line.width_px
            );
        }
        // The baseline budget comes from the area's height, and every
        // baseline plus its descent stays inside the area.
        let budget = ((panel.height - (OVERLAY_TEXT_PAD_Y * 2) as u32)
            / line_height_px(OVERLAY_TEXT_SIZE_PX)) as usize;
        assert!(!layout.lines.is_empty() && layout.lines.len() <= budget.max(1));
        for line in &layout.lines {
            assert!(
                line.baseline_y <= panel.y + panel.height as i32,
                "baseline {} escapes the area",
                line.baseline_y
            );
        }
        assert!(layout.unsupported.is_empty(), "ASCII text has no gaps");
        // The area is far too small for this text, so the tail is reported —
        // not silently dropped and not overflowed over the panel edge.
        assert!(
            layout.clipped_chars > 0,
            "the area must report the characters it could not draw"
        );
    }

    #[test]
    fn measure_matches_the_laid_out_line_width() {
        let rasterizer = overlay_rasterizer();
        let metrics = rasterizer.metrics();
        for font_px in [12_u32, 18, 30] {
            let text = "Board 1 Decimator (90°) (Marshal)";
            let layout = rasterizer.layout(
                area(0, 0, 4000, 50),
                TextWrapPolicy::SingleLine,
                text,
                font_px,
            );
            let extent = metrics
                .measure(text, font_px as f32)
                .unwrap_or_else(|| panic!("the resolved face measures"));
            assert_eq!(layout.lines.len(), 1);
            assert_eq!(layout.lines[0].width_px, extent.width.ceil() as u32);
            assert!(extent.height >= font_px as f32 * 0.9);
        }
    }

    #[test]
    fn measure_gives_a_missing_glyph_the_space_advance() {
        let metrics = overlay_rasterizer().metrics();
        let space = metrics.measure(" ", 18.0).map(|extent| extent.width);
        let missing = metrics.measure("漢", 18.0).map(|extent| extent.width);
        assert_eq!(space, missing);
        assert_eq!(
            metrics.measure("a\nb", 18.0),
            metrics.measure("a", 18.0),
            "a newline ends the single line"
        );
    }

    #[test]
    fn line_height_scales_from_the_default_size() {
        assert_eq!(
            line_height_px(OVERLAY_TEXT_SIZE_PX),
            OVERLAY_TEXT_LINE_HEIGHT_PX
        );
        assert_eq!(line_height_px(18), 21);
    }

    #[test]
    fn a_repeated_line_is_served_from_the_cache_without_rasterizing() {
        let mut rasterizer = overlay_rasterizer();
        let layout = rasterizer.layout(
            area(10, 20, 600, 40),
            TextWrapPolicy::ExactLine,
            "Decimator (0°) (Marshal)",
            18,
        );
        let line = &layout.lines[0];
        let mut first = TextLineMask::default();
        rasterizer.rasterize_line_into(line, &mut first);
        let after_first = rasterizer.cache_stats();
        assert_eq!(after_first.line_misses, 1);
        assert!(after_first.glyph_misses > 0);

        let mut second = TextLineMask::default();
        rasterizer.rasterize_line_into(line, &mut second);
        let after_second = rasterizer.cache_stats();
        assert_eq!(after_second.line_hits, 1);
        assert_eq!(after_second.glyph_misses, after_first.glyph_misses);
        assert!(
            Arc::ptr_eq(&first.coverage, &second.coverage),
            "an unchanged line shares its coverage buffer"
        );
        assert_eq!(first, second);

        // The same text elsewhere is the same coverage at another position.
        let moved = rasterizer.layout(
            area(300, 90, 600, 40),
            TextWrapPolicy::ExactLine,
            "Decimator (0°) (Marshal)",
            18,
        );
        let mut third = TextLineMask::default();
        rasterizer.rasterize_line_into(&moved.lines[0], &mut third);
        assert_eq!(third.coverage, first.coverage);
        assert_eq!((third.x - first.x, third.y - first.y), (290, 70));
    }

    #[test]
    fn a_new_line_of_known_glyphs_only_composes_cached_bitmaps() {
        let mut rasterizer = overlay_rasterizer();
        let mut mask = TextLineMask::default();
        for text in ["abc", "cab"] {
            let layout =
                rasterizer.layout(area(0, 0, 200, 30), TextWrapPolicy::ExactLine, text, 18);
            rasterizer.rasterize_line_into(&layout.lines[0], &mut mask);
        }
        let stats = rasterizer.cache_stats();
        assert_eq!(stats.line_misses, 2);
        assert_eq!(stats.glyph_misses, 3, "a, b and c are rendered once");
        assert_eq!(stats.glyph_hits, 3, "the second line reuses all three");
    }

    #[test]
    fn the_payload_cache_builds_each_line_opacity_once() {
        let mask = TextLineMask {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            coverage: vec![0, 255].into(),
        };
        let mut cache = PayloadCache::default();
        let first = cache.payload(&mask, 255, 4);
        let second = cache.payload(&mask, 255, 4);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(&*first, mask.to_alpha_payload(255, 4).as_slice());
        let other_opacity = cache.payload(&mask, 128, 4);
        assert!(!Arc::ptr_eq(&first, &other_opacity));
    }

    #[test]
    fn an_exact_line_keeps_leading_whitespace_like_the_measurement_counts_it() {
        let rasterizer = overlay_rasterizer();
        let text = "  ab";
        let exact = rasterizer.layout(area(0, 0, 300, 30), TextWrapPolicy::ExactLine, text, 18);
        assert_eq!(exact.lines[0].text, "  ab");
        let width = rasterizer
            .metrics()
            .measure(text, 18.0)
            .unwrap_or_else(|| panic!("measures"))
            .width;
        assert_eq!(exact.lines[0].width_px, width.ceil() as u32);
        assert_eq!(exact.clipped_chars, 0);

        // Other policies still consume it at the line start.
        let single = rasterizer.layout(area(0, 0, 300, 30), TextWrapPolicy::SingleLine, text, 18);
        assert_eq!(single.lines[0].text, "ab");
    }

    #[test]
    fn the_selection_rule_names_the_face_in_use() {
        let name = overlay_rasterizer().assignment().to_string();
        assert!(
            name.starts_with("Consolas Bold (system)")
                || name.starts_with("Inconsolata Bold (bundled; Consolas Bold is not installed)"),
            "unexpected face name {name:?}"
        );
    }

    #[test]
    fn the_bundled_face_is_a_bold_monospace_inconsolata() {
        let data = Arc::new(BUNDLED_FONT.to_vec());
        let face = face_from_data(data, 0, BUNDLED_FONT_FAMILY, "bundled".to_string())
            .unwrap_or_else(|error| panic!("bundled face must parse: {error}"));
        let font = face
            .font()
            .unwrap_or_else(|| panic!("bundled face must open"));
        assert_eq!(font.attributes().weight().0, 700);
        // Monospace: half an em per glyph at any size.
        let narrow = face.measure("iiii", 20.0).map(|extent| extent.width);
        let wide = face.measure("MMMM", 20.0).map(|extent| extent.width);
        assert_eq!(narrow, wide);
        assert_eq!(narrow, Some(40.0));
    }

    #[test]
    fn exact_line_starts_at_the_origin_without_padding_or_wrapping() {
        let rasterizer = overlay_rasterizer();
        let origin = area(100, 200, 10, 22);
        let text = "Decimator (0°) (Marshal) with a long tail past the area";
        let layout = rasterizer.layout(origin, TextWrapPolicy::ExactLine, text, 18);
        assert_eq!(layout.lines.len(), 1, "an exact line never wraps");
        assert_eq!(layout.clipped_chars, 0);
        let line = &layout.lines[0];
        assert_eq!(line.pen_x, 100, "no horizontal padding");
        assert_eq!(
            line.baseline_y,
            200 + rasterizer.ascent_px(18).round() as i32,
            "the line's top is the origin's top"
        );
        assert_eq!(line.font_px, 18);
    }

    #[test]
    fn text_that_fits_reports_no_clipping() {
        let rasterizer = overlay_rasterizer();
        let panel = area(0, 0, 200, 50);
        let layout = rasterizer.layout(
            panel,
            TextWrapPolicy::WrapWords,
            "Tier 15",
            OVERLAY_TEXT_SIZE_PX,
        );
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].text, "Tier 15");
        assert_eq!(layout.clipped_chars, 0);
        assert!(layout.unsupported.is_empty());
        assert!(layout.is_complete());
        // The first baseline is the area top, the top padding and the
        // rounded ascent of the selected face.
        assert_eq!(
            layout.lines[0].baseline_y,
            panel.y
                + OVERLAY_TEXT_PAD_Y
                + rasterizer.ascent_px(OVERLAY_TEXT_SIZE_PX).round() as i32
        );
    }

    #[test]
    fn single_line_policy_never_wraps_marker_text() {
        let rasterizer = overlay_rasterizer();
        // The upstream sigil marker caption is one line: it may be wider than
        // the marker box, and it is bounded by the overlay surface clip.
        let marker = area(200, 300, 20, 20);
        let layout = rasterizer.layout(
            marker,
            TextWrapPolicy::SingleLine,
            "Tier 15",
            OVERLAY_TEXT_SIZE_PX,
        );
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].text, "Tier 15");
        assert_eq!(layout.clipped_chars, 0);
        assert!(layout.lines[0].width_px > marker.width);
    }

    #[test]
    fn newlines_break_lines_and_tabs_map_to_a_space() {
        let rasterizer = overlay_rasterizer();
        let panel = area(0, 0, 200, 50);
        let layout = rasterizer.layout(
            panel,
            TextWrapPolicy::WrapWords,
            "one\ttwo\nthree",
            OVERLAY_TEXT_SIZE_PX,
        );
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["one two", "three"]
        );
        assert!(layout.is_complete());
    }

    #[test]
    fn undisplayable_characters_are_reported_not_replaced() {
        let rasterizer = overlay_rasterizer();
        let panel = area(0, 0, 200, 50);
        // U+6F22 (CJK) and U+1F600 (emoji) are outside the assigned
        // monospace face's repertoire.
        let layout = rasterizer.layout(
            panel,
            TextWrapPolicy::WrapWords,
            "ab漢c\u{1F600}d",
            OVERLAY_TEXT_SIZE_PX,
        );
        assert_eq!(layout.unsupported, vec!['漢', '\u{1F600}']);
        // The laid-out line stays faithful to the source text (a flagged
        // character holds a space's advance so glyphs never collide) and is
        // reported as undrawable.
        let drawn: String = layout.lines.iter().map(|line| line.text.clone()).collect();
        assert_eq!(drawn, "ab漢c\u{1F600}d");
        assert!(!layout.is_complete());

        // No replacement glyph: a character the assignment cannot draw
        // rasterizes to no ink at all (no tofu box, no fallback face).
        let mut rasterizer = overlay_rasterizer();
        let only_undrawable = rasterizer.layout(
            area(0, 0, 200, 20),
            TextWrapPolicy::SingleLine,
            "漢",
            OVERLAY_TEXT_SIZE_PX,
        );
        assert_eq!(only_undrawable.unsupported, vec!['漢']);
        let line = match only_undrawable.lines.first() {
            Some(line) => line,
            None => panic!("the flagged character still lays out"),
        };
        let mut mask = TextLineMask::default();
        rasterizer.rasterize_line_into(line, &mut mask);
        assert!(
            ink_columns(&mask).is_empty(),
            "an undrawable character must not be replaced by any glyph"
        );
    }

    #[test]
    fn umlauts_and_other_non_ascii_characters_render_to_ink() {
        // A byte-oriented X11 text path cannot render these at all; each one
        // must resolve to a real glyph of the assignment and rasterize ink.
        let area = area(0, 0, 200, 20);
        for character in ['ä', 'ö', 'ü', 'ß', 'Ä', 'Ö', 'Ü', '€'] {
            let mut rasterizer = overlay_rasterizer();
            let text = character.to_string();
            let layout = rasterizer.layout(
                area,
                TextWrapPolicy::SingleLine,
                &text,
                OVERLAY_TEXT_SIZE_PX,
            );
            assert!(
                layout.unsupported.is_empty(),
                "{character} is missing from the assignment: {:?}",
                layout.unsupported
            );
            let line = match layout.lines.first() {
                Some(line) => line,
                None => panic!("{character} must lay out"),
            };
            assert_eq!(line.text, text);
            let mut mask = TextLineMask::default();
            rasterizer.rasterize_line_into(line, &mut mask);
            assert!(
                !ink_columns(&mask).is_empty(),
                "{character} must rasterize real glyph ink"
            );
        }

        let mut rasterizer = overlay_rasterizer();
        let layout = rasterizer.layout(
            area,
            TextWrapPolicy::SingleLine,
            "Größe äöüß",
            OVERLAY_TEXT_SIZE_PX,
        );
        assert!(layout.unsupported.is_empty());
        let line = match layout.lines.first() {
            Some(line) => line,
            None => panic!("the umlaut word must lay out"),
        };
        assert_eq!(line.text, "Größe äöüß");
        let mut mask = TextLineMask::default();
        rasterizer.rasterize_line_into(line, &mut mask);
        assert!(!ink_columns(&mask).is_empty());
    }

    #[test]
    fn rasterized_glyph_ink_sits_above_the_baseline_inside_the_line_box() {
        let mut rasterizer = overlay_rasterizer();
        let area = area(0, 0, 200, 20);
        let layout = rasterizer.layout(area, TextWrapPolicy::SingleLine, "H", OVERLAY_TEXT_SIZE_PX);
        let line = match layout.lines.first() {
            Some(line) => line,
            None => panic!("H must lay out"),
        };
        let mut mask = TextLineMask::default();
        rasterizer.rasterize_line_into(line, &mut mask);
        let rows = ink_rows(&mask);
        assert!(!rows.is_empty(), "H must produce ink");
        // The mask's top edge is `baseline - ascent`, so the capital's ink
        // must sit strictly above the baseline row inside the mask.
        let baseline_row = (line.baseline_y - mask.y) as u32;
        assert!(
            rows.iter().all(|row| *row < baseline_row),
            "capital ink rows {rows:?} must stay above the baseline row {baseline_row}"
        );
        assert!(
            rows.iter().all(|row| *row < mask.height),
            "ink must stay inside the line box"
        );
        // A descender proves the other direction: its ink reaches below the
        // baseline.
        let layout = rasterizer.layout(area, TextWrapPolicy::SingleLine, "g", OVERLAY_TEXT_SIZE_PX);
        let line = match layout.lines.first() {
            Some(line) => line,
            None => panic!("g must lay out"),
        };
        let mut mask = TextLineMask::default();
        rasterizer.rasterize_line_into(line, &mut mask);
        let rows = ink_rows(&mask);
        let baseline_row = (line.baseline_y - mask.y) as u32;
        assert!(
            rows.iter().any(|row| *row >= baseline_row),
            "the descender must reach below the baseline (rows {rows:?} vs {baseline_row})"
        );
    }

    #[test]
    fn mask_blit_clips_text_to_the_surface() {
        let mut rasterizer = overlay_rasterizer();
        let area = area(60, 0, 100, 20);
        let layout = rasterizer.layout(
            area,
            TextWrapPolicy::SingleLine,
            "Tier 15",
            OVERLAY_TEXT_SIZE_PX,
        );
        let line = match layout.lines.first() {
            Some(line) => line,
            None => panic!("tier text must lay out"),
        };
        let mut mask = TextLineMask::default();
        rasterizer.rasterize_line_into(line, &mut mask);
        // A surface that ends inside the text: the blit must drop the ink
        // past the surface edge instead of writing outside it.
        let mut surface = vec![0u8; (40 * 20 * 4) as usize];
        mask.paint_into(&mut surface, 40, 20, 0xFFFFFFFF, 255);
        for row in 0..20usize {
            for column in 0..40usize {
                let index = (row * 40 + column) * 4;
                let pixel = u32::from_le_bytes([
                    surface[index],
                    surface[index + 1],
                    surface[index + 2],
                    surface[index + 3],
                ]);
                if pixel != 0 {
                    assert!(
                        (column as i32) < 40 && column >= area.x as usize,
                        "only the surface columns inside the text start may be painted"
                    );
                }
            }
        }
    }

    #[test]
    fn diagnostics_report_unsupported_and_clipped_characters() {
        let rasterizer = overlay_rasterizer();
        let panel = area(0, 0, 80, 20);
        let layout = rasterizer.layout(
            panel,
            TextWrapPolicy::WrapWords,
            "aa漢bb cc dd ee ff",
            OVERLAY_TEXT_SIZE_PX,
        );
        let mut diagnostics = TextDiagnostics::default();
        diagnostics.record(&layout);
        assert_eq!(diagnostics.unsupported_codepoints, vec!['漢']);
        assert!(diagnostics.clipped_chars > 0);
        assert!(!diagnostics.is_clean());
        diagnostics.record_font_unavailable(&TextError::FontParse {
            family: "Missing Face".to_string(),
        });
        assert!(diagnostics
            .font_unavailable
            .as_deref()
            .unwrap_or("")
            .contains("Missing Face"));
    }
}
