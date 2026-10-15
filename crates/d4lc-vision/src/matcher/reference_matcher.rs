//! The template matcher and grayscale conversion as they were before the
//! allocation-free, vectorized rewrite, kept verbatim as the equivalence
//! oracle: every optimized path must return bit-identical results.

use d4lc_core::{GrayImage, MatchCandidate, Rect};

use super::{TemplateMatchResult, TemplateMatchStats, TimedTemplateMatchResult};

/// Verbatim former `match_template_sqdiff_normed_below_timed`.
pub(super) fn match_template_sqdiff_normed_below_timed(
    haystack: &GrayImage,
    template: &GrayImage,
    max_score: f64,
) -> Option<TimedTemplateMatchResult> {
    let context = HaystackMatchContext::new(haystack);
    let scan = TemplateScan::new(&context, template)?;
    scan.match_below(max_score)
}

/// Straightforward per-pixel form of `to_grayscale`.
pub(super) fn to_grayscale(frame: &d4lc_core::CaptureFrame) -> GrayImage {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        let row_start = y * frame.stride;
        for x in 0..width {
            let i = row_start + x * 4;
            let b = u32::from(frame.pixels_bgra[i]);
            let g = u32::from(frame.pixels_bgra[i + 1]);
            let r = u32::from(frame.pixels_bgra[i + 2]);
            // OpenCV's 8-bit RGB→GRAY (BT.601, 14-bit fixed point, rounded).
            pixels.push(((4899 * r + 9617 * g + 1868 * b + (1 << 13)) >> 14) as u8);
        }
    }
    GrayImage {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

/// Verbatim former `match_templates`.
pub(super) fn match_templates(
    haystack: &GrayImage,
    templates: &[(&str, &GrayImage)],
) -> Vec<MatchCandidate> {
    let context = HaystackMatchContext::new(haystack);
    let mut candidates = Vec::with_capacity(templates.len());
    for &(name, template) in templates {
        let Some(scan) = TemplateScan::new(&context, template) else {
            continue;
        };
        candidates.push(template_match_candidate(
            name,
            template,
            &scan.match_unbounded().result,
        ));
    }
    candidates
}

/// Verbatim former `match_templates_below` (the live path).
pub(super) fn match_templates_below(
    haystack: &GrayImage,
    templates: &[(&str, &GrayImage)],
    max_score: f64,
) -> Vec<MatchCandidate> {
    let context = HaystackMatchContext::new(haystack);
    let mut candidates = Vec::with_capacity(templates.len());
    for &(name, template) in templates {
        let Some(scan) = TemplateScan::new(&context, template) else {
            continue;
        };
        if let Some(result) = scan.match_below(max_score) {
            candidates.push(template_match_candidate(name, template, &result.result));
        }
    }
    candidates
}

fn template_match_candidate(
    name: &str,
    template: &GrayImage,
    result: &TemplateMatchResult,
) -> MatchCandidate {
    MatchCandidate {
        template_name: name.to_owned(),
        score: result.score,
        rect: Rect {
            x: result.x,
            y: result.y,
            width: template.width,
            height: template.height,
        },
    }
}

/// Haystack-level preparation shared by every template scan against one image.
///
/// The squared integral image is the only per-haystack state; sharing it across
/// templates keeps multi-template live detection at one integral build per frame
/// instead of one per template. Scores and locations are unchanged: the integral
/// values are exactly the ones a per-template build would produce.
struct HaystackMatchContext<'a> {
    haystack: &'a GrayImage,
    haystack_sq_integral: IntegralImage,
}

impl<'a> HaystackMatchContext<'a> {
    fn new(haystack: &'a GrayImage) -> Self {
        Self {
            haystack,
            haystack_sq_integral: IntegralImage::from_squared(haystack),
        }
    }
}

/// One template scanned against a prepared haystack.
struct TemplateScan<'a> {
    context: &'a HaystackMatchContext<'a>,
    template: &'a GrayImage,
    template_sum_sq: u64,
    result_cols: u32,
    result_rows: u32,
}

impl<'a> TemplateScan<'a> {
    fn new(context: &'a HaystackMatchContext<'a>, template: &'a GrayImage) -> Option<Self> {
        let haystack = context.haystack;
        if template.width == 0
            || template.height == 0
            || template.width > haystack.width
            || template.height > haystack.height
        {
            return None;
        }

        Some(Self {
            context,
            template,
            template_sum_sq: sum_sq(&template.pixels),
            result_cols: haystack.width - template.width + 1,
            result_rows: haystack.height - template.height + 1,
        })
    }

    fn match_unbounded(&self) -> TimedTemplateMatchResult {
        let mut best_score = f64::INFINITY;
        let mut best_x = 0i32;
        let mut best_y = 0i32;
        let mut stats = TemplateMatchStats::default();

        for hy in 0..self.result_rows {
            for hx in 0..self.result_cols {
                stats.positions += 1;
                let patch_sum_sq = self.patch_sum_sq(hx, hy);
                let score = self.sqdiff_normed_expanded_at(hx, hy, patch_sum_sq);
                stats.fully_scored += 1;
                if score < best_score {
                    best_score = score;
                    best_x = hx as i32;
                    best_y = hy as i32;
                }
            }
        }

        TimedTemplateMatchResult {
            result: TemplateMatchResult {
                score: best_score,
                x: best_x,
                y: best_y,
            },
            stats,
        }
    }

    fn match_below(&self, max_score: f64) -> Option<TimedTemplateMatchResult> {
        if !max_score.is_finite() || max_score < 0.0 {
            return Some(self.match_unbounded());
        }

        let mut best_score = max_score;
        let mut best_x = 0i32;
        let mut best_y = 0i32;
        let mut found = false;
        let mut stats = TemplateMatchStats::default();

        for hy in 0..self.result_rows {
            for hx in 0..self.result_cols {
                stats.positions += 1;
                let patch_sum_sq = self.patch_sum_sq(hx, hy);
                let denom = ((patch_sum_sq as f64) * (self.template_sum_sq as f64)).sqrt();
                let limit = self.numerator_limit(best_score, denom);
                let Some(numerator) = self.numerator_below(hx, hy, limit) else {
                    stats.pruned += 1;
                    continue;
                };
                let score = self.exact_score_at(numerator, denom);
                stats.fully_scored += 1;
                if score < best_score {
                    best_score = score;
                    best_x = hx as i32;
                    best_y = hy as i32;
                    found = true;
                }
            }
        }

        if found {
            Some(TimedTemplateMatchResult {
                result: TemplateMatchResult {
                    score: best_score,
                    x: best_x,
                    y: best_y,
                },
                stats,
            })
        } else {
            None
        }
    }

    /// Integer pruning limit for the squared-difference numerator.
    ///
    /// `max_numerator = best_score * denom` is the exact per-position bound the
    /// bounded scan has always used; this wraps it as an integer so the
    /// per-pixel check is a plain compare instead of an f64 conversion. The
    /// +1 slack and the exact-range guard keep the limit conservative against
    /// f64 product rounding: pruning with it can only ever skip positions the
    /// exact check would also skip. `None` disables pruning (degenerate
    /// zero-energy patches/templates, non-finite bounds).
    fn numerator_limit(&self, best_score: f64, denom: f64) -> Option<u64> {
        if !best_score.is_finite() || best_score <= 0.0 {
            return None;
        }
        let max_numerator = best_score * denom;
        if !max_numerator.is_finite() || max_numerator >= 9_007_199_254_740_992.0 {
            return None;
        }
        let bound = (max_numerator.ceil() as u64).saturating_add(1);
        (bound != 0).then_some(bound)
    }

    /// Exact squared-difference numerator at one position, with per-pixel
    /// pruning against `bound`.
    ///
    /// Row sums are monotone, so a position reaches `numerator >= bound` at
    /// some pixel prefix if and only if its total numerator satisfies the same
    /// condition: the pruned set is exactly the set an exact-bound check
    /// prunes, and survivors return the full exact numerator. The check is a
    /// plain integer compare — no per-pixel float conversion.
    fn numerator_below(&self, hx: u32, hy: u32, bound: Option<u64>) -> Option<u64> {
        let Some(limit) = bound else {
            return Some(self.numerator_at(hx, hy));
        };
        let tw = self.template.width as usize;
        let th = self.template.height as usize;
        let hw = self.context.haystack.width as usize;
        let mut numerator = 0u64;

        for row in 0..th {
            let h_row_base = (hy as usize + row) * hw + hx as usize;
            let t_row_base = row * tw;
            let h_row = &self.context.haystack.pixels[h_row_base..h_row_base + tw];
            let t_row = &self.template.pixels[t_row_base..t_row_base + tw];
            for (&h, &t) in h_row.iter().zip(t_row.iter()) {
                let diff = i32::from(h) - i32::from(t);
                numerator += (diff * diff) as u64;
                if numerator >= limit {
                    return None;
                }
            }
        }

        Some(numerator)
    }

    /// Full squared-difference numerator at one position without pruning.
    fn numerator_at(&self, hx: u32, hy: u32) -> u64 {
        let tw = self.template.width as usize;
        let th = self.template.height as usize;
        let hw = self.context.haystack.width as usize;
        let mut numerator = 0u64;

        for row in 0..th {
            let h_row_base = (hy as usize + row) * hw + hx as usize;
            let t_row_base = row * tw;
            let h_row = &self.context.haystack.pixels[h_row_base..h_row_base + tw];
            let t_row = &self.template.pixels[t_row_base..t_row_base + tw];
            for (&h, &t) in h_row.iter().zip(t_row.iter()) {
                let diff = i32::from(h) - i32::from(t);
                numerator += (diff * diff) as u64;
            }
        }

        numerator
    }

    /// Exact upstream score for a position with a known full numerator and
    /// denominator.
    ///
    /// For non-degenerate denominators this is `numerator / denom`, the same
    /// integer division the bounded path has always used. Degenerate
    /// (`denom < EPSILON`) positions keep the upstream contract: an all-equal
    /// patch scores 0.0, anything else scores 1.0.
    fn exact_score_at(&self, numerator: u64, denom: f64) -> f64 {
        if denom < f64::EPSILON {
            return if numerator == 0 { 0.0 } else { 1.0 };
        }
        ((numerator as f64) / denom).clamp(0.0, 1.0)
    }

    fn patch_sum_sq(&self, hx: u32, hy: u32) -> u64 {
        self.context.haystack_sq_integral.rect_sum(
            hx,
            hy,
            self.template.width,
            self.template.height,
        )
    }

    fn sqdiff_normed_expanded_at(&self, hx: u32, hy: u32, patch_sum_sq: u64) -> f64 {
        let denom = ((patch_sum_sq as f64) * (self.template_sum_sq as f64)).sqrt();
        if denom < f64::EPSILON {
            return self.zero_denominator_score(hx, hy);
        }

        let dot = self.dot_at(hx, hy);
        let numerator = self
            .template_sum_sq
            .saturating_add(patch_sum_sq)
            .saturating_sub(dot.saturating_mul(2));
        ((numerator as f64) / denom).clamp(0.0, 1.0)
    }

    fn dot_at(&self, hx: u32, hy: u32) -> u64 {
        let tw = self.template.width as usize;
        let th = self.template.height as usize;
        let hw = self.context.haystack.width as usize;
        let mut dot = 0u64;

        for row in 0..th {
            let h_row_base = (hy as usize + row) * hw + hx as usize;
            let t_row_base = row * tw;
            let h_row = &self.context.haystack.pixels[h_row_base..h_row_base + tw];
            let t_row = &self.template.pixels[t_row_base..t_row_base + tw];
            dot += h_row
                .iter()
                .zip(t_row.iter())
                .map(|(&h, &t)| u64::from(h) * u64::from(t))
                .sum::<u64>();
        }

        dot
    }

    fn zero_denominator_score(&self, hx: u32, hy: u32) -> f64 {
        let tw = self.template.width as usize;
        let th = self.template.height as usize;
        let hw = self.context.haystack.width as usize;
        let mut numerator = 0u64;

        for row in 0..th {
            let h_row_base = (hy as usize + row) * hw + hx as usize;
            let t_row_base = row * tw;
            let h_row = &self.context.haystack.pixels[h_row_base..h_row_base + tw];
            let t_row = &self.template.pixels[t_row_base..t_row_base + tw];
            for (&h, &t) in h_row.iter().zip(t_row.iter()) {
                let diff = i32::from(h) - i32::from(t);
                numerator += (diff * diff) as u64;
            }
        }

        if numerator == 0 {
            0.0
        } else {
            1.0
        }
    }
}

fn sum_sq(values: &[u8]) -> u64 {
    values
        .iter()
        .map(|value| {
            let value = u64::from(*value);
            value * value
        })
        .sum()
}

struct IntegralImage {
    width: usize,
    values: Vec<u64>,
}

impl IntegralImage {
    fn from_squared(image: &GrayImage) -> Self {
        let source_width = image.width as usize;
        let source_height = image.height as usize;
        let width = source_width + 1;
        let mut values = vec![0u64; width * (source_height + 1)];

        for y in 0..source_height {
            let mut row_sum = 0u64;
            for x in 0..source_width {
                let pixel = u64::from(image.pixels[y * source_width + x]);
                row_sum += pixel * pixel;
                let above = values[y * width + x + 1];
                values[(y + 1) * width + x + 1] = above + row_sum;
            }
        }

        Self { width, values }
    }

    fn rect_sum(&self, x: u32, y: u32, width: u32, height: u32) -> u64 {
        let x1 = x as usize;
        let y1 = y as usize;
        let x2 = x1 + width as usize;
        let y2 = y1 + height as usize;
        self.values[y2 * self.width + x2] + self.values[y1 * self.width + x1]
            - self.values[y1 * self.width + x2]
            - self.values[y2 * self.width + x1]
    }
}
