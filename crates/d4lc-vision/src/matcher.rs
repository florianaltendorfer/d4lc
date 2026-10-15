//! Template matching via normalized squared difference (SQDIFF_NORMED).
//!
//! Upstream: D4Companion.Services/ScreenProcessHandler.cs FindMatchTemplate
//!   CvInvoke.MatchTemplate(image, template, result, TemplateMatchingType.SqdiffNormed)
//!   CvInvoke.MinMaxLoc(result, ref minVal, ref maxVal, ref minLoc, ref maxLoc)
//!   → returns (minVal, minLoc)  [lower-is-better, 0.0 = perfect match]

use d4lc_core::{GrayImage, MatchCandidate, Rect};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// Milliseconds of a measured duration, rounded up.
fn elapsed_ms(start: Instant) -> u128 {
    start.elapsed().as_micros().div_ceil(1000)
}

/// Best position found when sliding one template over a haystack.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateMatchResult {
    /// SQDIFF_NORMED score — 0.0 is a perfect match, higher values are worse.
    pub score: f64,
    /// Top-left column of the best position in haystack coordinates.
    pub x: i32,
    /// Top-left row of the best position in haystack coordinates.
    pub y: i32,
}

/// Runtime counters for one template-matching pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TemplateMatchStats {
    /// Number of valid output locations in the OpenCV result matrix.
    pub positions: u64,
    /// Number of positions fully scored.
    pub fully_scored: u64,
    /// Number of positions rejected by exact threshold/best-score pruning.
    pub pruned: u64,
}

/// Best match plus runtime counters.
#[derive(Debug, Clone, PartialEq)]
struct TimedTemplateMatchResult {
    result: TemplateMatchResult,
    stats: TemplateMatchStats,
}

/// Match one grayscale template against a grayscale haystack using SQDIFF_NORMED.
///
/// This mirrors OpenCV `TM_SQDIFF_NORMED` semantics for the image types used by
/// d4lc. It evaluates every valid result-matrix location and returns the minimum
/// score plus its location.
///
/// Implementation note: the unbounded path uses the OpenCV-equivalent expanded
/// form `sum(T²) - 2*sum(T*I) + sum(I²)` and an integral image for `sum(I²)`.
pub fn match_template_sqdiff_normed(
    haystack: &GrayImage,
    template: &GrayImage,
) -> Option<TemplateMatchResult> {
    match_template_sqdiff_normed_timed(haystack, template).map(|timed| timed.result)
}

/// Same as [`match_template_sqdiff_normed`] but also returns runtime counters.
fn match_template_sqdiff_normed_timed(
    haystack: &GrayImage,
    template: &GrayImage,
) -> Option<TimedTemplateMatchResult> {
    let mut scratch = MatchScratch::default();
    let context = HaystackMatchContext::new(haystack, &mut scratch);
    let scan = TemplateScan::new(&context, template)?;
    Some(scan.match_unbounded())
}

/// Masked SQDIFF_NORMED, OpenCV `matchTemplate(image, templ, result,
/// TM_SQDIFF_NORMED, mask)` as upstream's socket search calls it
/// (`ScreenProcessHandler.FindMatchTemplateMasked`, SPH.cs:537-548).
///
/// OpenCV turns an 8-bit mask into a binary 0/1 mask (`mask > 0`), so with
/// `M` that mask: `R = Σ M·(T − I)² / sqrt(Σ M·T² · Σ M·I²)`. Only the masked
/// pixels take part — a filled socket matches the empty-socket template by
/// its ring. A position whose masked window is all zero has a zero
/// denominator (OpenCV yields +inf there) and is never the minimum. Ties keep
/// the first position in row-major order, like `minMaxLoc`. Returns `None`
/// when the template or mask does not fit the haystack or their sizes differ.
pub fn match_template_sqdiff_normed_masked(
    haystack: &GrayImage,
    template: &GrayImage,
    mask: &GrayImage,
) -> Option<TemplateMatchResult> {
    if template.width == 0
        || template.height == 0
        || template.width > haystack.width
        || template.height > haystack.height
        || mask.width != template.width
        || mask.height != template.height
    {
        return None;
    }
    let (hw, tw, th) = (
        haystack.width as usize,
        template.width as usize,
        template.height as usize,
    );
    let masked: Vec<(usize, f64)> = (0..th)
        .flat_map(|y| (0..tw).map(move |x| (y, x)))
        .filter(|&(y, x)| mask.pixels[y * tw + x] > 0)
        .map(|(y, x)| (y * hw + x, f64::from(template.pixels[y * tw + x])))
        .collect();
    let template_energy: f64 = masked.iter().map(|(_, t)| t * t).sum();
    let mut best: Option<TemplateMatchResult> = None;
    for y in 0..=(haystack.height - template.height) as usize {
        for x in 0..=(haystack.width - template.width) as usize {
            let origin = y * hw + x;
            let (mut diff, mut energy) = (0.0_f64, 0.0_f64);
            for &(offset, t) in &masked {
                let i = f64::from(haystack.pixels[origin + offset]);
                diff += (t - i) * (t - i);
                energy += i * i;
            }
            let denominator = (template_energy * energy).sqrt();
            if denominator <= 0.0 {
                continue;
            }
            let score = diff / denominator;
            if best.as_ref().is_none_or(|current| score < current.score) {
                best = Some(TemplateMatchResult {
                    score,
                    x: x as i32,
                    y: y as i32,
                });
            }
        }
    }
    best
}

/// Match one grayscale template and return a result only when the best upstream
/// SQDIFF_NORMED score is below `max_score`.
///
/// This keeps upstream threshold semantics (`score < ThresholdSimilarityTooltip`)
/// while avoiding useless work: a candidate whose partial squared difference is
/// already above the exact threshold/best-score bound cannot later become valid.
/// The pruning is mathematically exact; it does not change the selected result.
pub fn match_template_sqdiff_normed_below(
    haystack: &GrayImage,
    template: &GrayImage,
    max_score: f64,
) -> Option<TemplateMatchResult> {
    match_template_sqdiff_normed_below_timed(haystack, template, max_score)
        .map(|timed| timed.result)
}

/// Same as [`match_template_sqdiff_normed_below`] but also returns runtime counters.
fn match_template_sqdiff_normed_below_timed(
    haystack: &GrayImage,
    template: &GrayImage,
    max_score: f64,
) -> Option<TimedTemplateMatchResult> {
    let mut scratch = MatchScratch::default();
    let context = HaystackMatchContext::new(haystack, &mut scratch);
    let scan = TemplateScan::new(&context, template)?;
    scan.match_below(max_score)
}

/// Match multiple named grayscale templates against a haystack.
pub fn match_templates(
    haystack: &GrayImage,
    templates: &[(&str, &GrayImage)],
) -> Vec<MatchCandidate> {
    let mut scratch = MatchScratch::default();
    let context = HaystackMatchContext::new(haystack, &mut scratch);
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

/// Match multiple templates and keep only candidates that pass the upstream
/// tooltip threshold.
pub fn match_templates_below(
    haystack: &GrayImage,
    templates: &[(&str, &GrayImage)],
    max_score: f64,
) -> Vec<MatchCandidate> {
    match_templates_below_parallel_timed(
        haystack,
        templates,
        max_score,
        1,
        &mut MatchScratch::default(),
    )
    .candidates
}

/// Candidates of a multi-template scan plus how it ran.
#[derive(Debug, Clone, PartialEq)]
pub struct ParallelMatchScan {
    pub candidates: Vec<MatchCandidate>,
    /// The squared integral image build, in milliseconds.
    pub integral_build_ms: u128,
    /// Threads that scanned, including the caller's.
    pub threads_used: usize,
}

/// [`match_templates_below`] with the templates and horizontal bands of
/// result rows spread over up to `threads` threads, building the integral
/// image in `scratch` so repeated calls reuse its allocation.
///
/// Candidates are bit-identical for every thread count: each band keeps its
/// own raster-order best and the bands merge by (score, y, x) (see
/// `merge_bands`). `threads <= 1` is the sequential scan on the caller's
/// thread; no thread pool outlives the call.
pub fn match_templates_below_parallel_timed(
    haystack: &GrayImage,
    templates: &[(&str, &GrayImage)],
    max_score: f64,
    threads: usize,
    scratch: &mut MatchScratch,
) -> ParallelMatchScan {
    // One haystack preparation (the squared integral image) is shared by every
    // template scan; per-template results are identical to single-template calls.
    let integral_start = Instant::now();
    let context = HaystackMatchContext::new(haystack, scratch);
    let integral_build_ms = elapsed_ms(integral_start);

    let scans: Vec<(&str, &GrayImage, TemplateScan<'_>)> = templates
        .iter()
        .filter_map(|&(name, template)| {
            TemplateScan::new(&context, template).map(|scan| (name, template, scan))
        })
        .collect();
    let bounded = bounded_threshold(max_score);
    // Unbounded scans are not live; they spread over templates only.
    let bands_per_scan = if bounded { threads.max(1) } else { 1 };
    let tasks: Vec<(usize, Range<u32>)> = scans
        .iter()
        .enumerate()
        .flat_map(|(index, (_, _, scan))| {
            row_bands(scan.result_rows, bands_per_scan)
                .into_iter()
                .map(move |rows| (index, rows))
        })
        .collect();
    let (bands, threads_used) = run_indexed(threads, tasks.len(), |task| {
        let (index, rows) = &tasks[task];
        let scan = &scans[*index].2;
        if bounded {
            scan.scan_band_below(rows.clone(), max_score)
        } else {
            let unbounded = scan.match_unbounded();
            BandScan {
                best: Some(unbounded.result),
                stats: unbounded.stats,
            }
        }
    });

    let mut bands = bands.into_iter().zip(&tasks).peekable();
    let mut candidates = Vec::with_capacity(scans.len());
    for (index, (name, template, _)) in scans.iter().enumerate() {
        let mut scan_bands = Vec::new();
        while let Some((band, _)) = bands.next_if(|(_, (task_index, _))| *task_index == index) {
            scan_bands.push(band);
        }
        if let Some(result) = merge_bands(scan_bands) {
            candidates.push(template_match_candidate(name, template, &result));
        }
    }

    ParallelMatchScan {
        candidates,
        integral_build_ms,
        threads_used,
    }
}

/// Buffers a matcher reuses across calls. It is only a capacity cache: every
/// value in it is rewritten before it is read, so results never depend on
/// what an earlier call left behind.
#[derive(Debug, Default)]
pub struct MatchScratch {
    integral: Vec<u64>,
}

#[cfg(test)]
mod equivalence_tests;
#[cfg(test)]
mod reference_matcher;

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
    haystack_sq_integral: IntegralImage<'a>,
}

impl<'a> HaystackMatchContext<'a> {
    fn new(haystack: &'a GrayImage, scratch: &'a mut MatchScratch) -> Self {
        Self {
            haystack,
            haystack_sq_integral: IntegralImage::from_squared(haystack, &mut scratch.integral),
        }
    }
}

/// Longest row slice whose per-pixel terms still sum exactly in an `i32`:
/// every squared difference or product of two `u8` values is at most 255².
const TERM_SUM_LEN: usize = (i32::MAX / (255 * 255)) as usize;

/// Sum of squared differences of two equally long rows.
///
/// Branch-free `i32` lanes (at most [`TERM_SUM_LEN`] terms per lane sum),
/// which LLVM vectorizes; the total is the exact integer sum.
#[inline]
fn row_sqdiff(haystack: &[u8], template: &[u8]) -> u64 {
    haystack
        .chunks(TERM_SUM_LEN)
        .zip(template.chunks(TERM_SUM_LEN))
        .map(|(h_chunk, t_chunk)| {
            let sum: i32 = h_chunk
                .iter()
                .zip(t_chunk)
                .map(|(&h, &t)| {
                    let diff = i32::from(i16::from(h) - i16::from(t));
                    diff * diff
                })
                .sum();
            u64::from(sum.unsigned_abs())
        })
        .sum()
}

/// Dot product of two equally long rows, exact, in vectorizable `i32` lanes.
#[inline]
fn row_dot(haystack: &[u8], template: &[u8]) -> u64 {
    haystack
        .chunks(TERM_SUM_LEN)
        .zip(template.chunks(TERM_SUM_LEN))
        .map(|(h_chunk, t_chunk)| {
            let sum: i32 = h_chunk
                .iter()
                .zip(t_chunk)
                .map(|(&h, &t)| i32::from(h) * i32::from(t))
                .sum();
            u64::from(sum.unsigned_abs())
        })
        .sum()
}

/// Whether `max_score` selects the bounded scan; anything else (negative,
/// infinite, NaN) runs the unbounded scan, as upstream has no threshold then.
fn bounded_threshold(max_score: f64) -> bool {
    max_score.is_finite() && max_score >= 0.0
}

/// Worker threads for one detection scan: half the
/// available cores, at least 1 and at most 4, so the matcher never takes
/// every core from the game. When the core count is unknown this is 1, the
/// sequential scan.
pub fn scan_threads_for_host() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |cores| cores.get() / 2)
        .clamp(1, 4)
}

/// Headroom of the energy-window bound over the exact pruning limit: covers
/// the limit's +1 slack and its ceiling, plus f64 rounding of the bound.
const ENERGY_WINDOW_SLACK: f64 = 8.0;

/// Result of scanning one horizontal band of result rows.
struct BandScan {
    best: Option<TemplateMatchResult>,
    stats: TemplateMatchStats,
}

/// Merge band results given top to bottom into the whole-image result.
///
/// The sequential scan keeps the first strictly lowest score in raster
/// order. Every band reports its own first strict minimum, and bands are
/// visited in raster order with a strict compare, so ties resolve to the
/// earliest band: the lexicographic minimum of (score, y, x), which is
/// exactly the sequential winner.
fn merge_bands(bands: impl IntoIterator<Item = BandScan>) -> Option<TemplateMatchResult> {
    let mut best: Option<TemplateMatchResult> = None;
    for band in bands {
        if let Some(candidate) = band.best {
            if best
                .as_ref()
                .is_none_or(|current| candidate.score < current.score)
            {
                best = Some(candidate);
            }
        }
    }
    best
}

/// `rows` split into at most `bands` contiguous, near-equal ranges, top to
/// bottom.
fn row_bands(rows: u32, bands: usize) -> Vec<Range<u32>> {
    let bands = u32::try_from(bands)
        .unwrap_or(u32::MAX)
        .clamp(1, rows.max(1));
    let base = rows / bands;
    let extra = rows % bands;
    let mut start = 0;
    (0..bands)
        .map(|band| {
            let end = start + base + u32::from(band < extra);
            let range = start..end;
            start = end;
            range
        })
        .collect()
}

/// Run `task(0..count)` on up to `threads` threads (the caller's plus
/// scoped helpers) and return the results in index order, plus the number of
/// threads that actually ran. A helper the OS refuses to start only means
/// fewer threads share the same tasks; the results are unchanged.
fn run_indexed<T, F>(threads: usize, count: usize, task: F) -> (Vec<T>, usize)
where
    T: Send,
    F: Fn(usize) -> T + Sync,
{
    if threads <= 1 || count <= 1 {
        return ((0..count).map(&task).collect(), 1);
    }
    let next = AtomicUsize::new(0);
    let work = || {
        let mut done = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            if index >= count {
                return done;
            }
            done.push((index, task(index)));
        }
    };
    std::thread::scope(|scope| {
        let helpers: Vec<_> = (1..threads.min(count))
            .filter_map(|_| {
                std::thread::Builder::new()
                    .name("d4lc-scan".to_string())
                    .spawn_scoped(scope, work)
                    .ok()
            })
            .collect();
        let used = 1 + helpers.len();
        let mut all = work();
        for helper in helpers {
            match helper.join() {
                Ok(done) => all.extend(done),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        all.sort_unstable_by_key(|(index, _)| *index);
        (all.into_iter().map(|(_, result)| result).collect(), used)
    })
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
        if !bounded_threshold(max_score) {
            return Some(self.match_unbounded());
        }
        let band = self.scan_band_below(0..self.result_rows, max_score);
        band.best.map(|result| TimedTemplateMatchResult {
            result,
            stats: band.stats,
        })
    }

    /// The bounded scan over result rows `rows`, in raster order: the first
    /// position with the strictly lowest score below `max_score` wins.
    ///
    /// A band starts from `max_score` rather than from the best score of the
    /// rows above it, so its pruning bound is never below the whole-image
    /// scan's bound at the same position: it only prunes positions that
    /// cannot win overall, and the band holding the overall winner selects
    /// it with the identical score (see [`merge_bands`]).
    fn scan_band_below(&self, rows: Range<u32>, max_score: f64) -> BandScan {
        let mut best_score = max_score;
        let mut best = None;
        let mut stats = TemplateMatchStats::default();
        let window = self.energy_window(max_score);

        for hy in rows {
            for hx in 0..self.result_cols {
                stats.positions += 1;
                let patch_sum_sq = self.patch_sum_sq(hx, hy);
                if window
                    .as_ref()
                    .is_some_and(|window| !window.contains(&patch_sum_sq))
                {
                    stats.pruned += 1;
                    continue;
                }
                let denom = ((patch_sum_sq as f64) * (self.template_sum_sq as f64)).sqrt();
                let limit = self.numerator_limit(best_score, denom);
                if limit.is_some_and(|limit| self.energy_gap_reaches(patch_sum_sq, limit)) {
                    stats.pruned += 1;
                    continue;
                }
                let Some(numerator) = self.numerator_below(hx, hy, limit) else {
                    stats.pruned += 1;
                    continue;
                };
                let score = self.exact_score_at(numerator, denom);
                stats.fully_scored += 1;
                if score < best_score {
                    best_score = score;
                    best = Some(TemplateMatchResult {
                        score,
                        x: hx as i32,
                        y: hy as i32,
                    });
                }
            }
        }

        BandScan { best, stats }
    }

    /// Patch energies `P = sum(I²)` outside which no position can score
    /// below `max_score`, as an inclusive integer range; `None` disables the
    /// check.
    ///
    /// The Cauchy-Schwarz bound behind [`Self::energy_gap_reaches`] depends
    /// on `P` alone for a fixed score bound: the numerator is at least
    /// `(sqrt(P) - sqrt(T))²`, which reaches `max_score * sqrt(P*T)` exactly
    /// when `P/T` lies outside the roots of `x - (2 + b)·sqrt(x) + 1 = 0`. The
    /// per-position check computed a square root and two 128-bit products for
    /// every one of the two million positions of a 1080p frame, although
    /// almost all of them fail on energy alone. The window is widened until
    /// the bound clears the pruning limit by [`ENERGY_WINDOW_SLACK`], so a
    /// position outside it is one the exact check prunes too, and the scan
    /// bound only falls below `max_score` as the scan proceeds: the selected
    /// result is unchanged.
    fn energy_window(&self, max_score: f64) -> Option<std::ops::RangeInclusive<u64>> {
        if !bounded_threshold(max_score) || max_score <= 0.0 || self.template_sum_sq == 0 {
            return None;
        }
        let template = self.template_sum_sq as f64;
        // Lower numerator bound minus the pruning limit at patch energy `p`.
        let clearance = |p: f64| {
            let root = (p * template).sqrt();
            p + template - 2.0 * root - max_score * root - ENERGY_WINDOW_SLACK
        };
        let b = 2.0 + max_score;
        let spread = (b * b - 4.0).sqrt();
        let low_root = (b - spread) / 2.0;
        let high_root = (b + spread) / 2.0;

        // Largest energy below the window with full clearance: bisect down
        // from the exact root (clearance grows as `p` falls towards zero).
        let mut high = (low_root * low_root * template).floor();
        let mut low = 0.0_f64;
        let lower_edge = if clearance(0.0) < 0.0 {
            None
        } else {
            while high - low > 1.0 {
                let mid = ((low + high) / 2.0).floor();
                if clearance(mid) >= 0.0 {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            Some(low)
        };

        // Smallest energy above the window with full clearance.
        let max_energy = u64::MAX as f64;
        let mut low = (high_root * high_root * template).ceil();
        let mut step = template.max(1.0);
        let mut high = low + step;
        while clearance(high) < 0.0 {
            step *= 2.0;
            high = low + step;
            if high >= max_energy {
                return None;
            }
        }
        while high - low > 1.0 {
            let mid = ((low + high) / 2.0).ceil();
            if clearance(mid) >= 0.0 {
                high = mid;
            } else {
                low = mid;
            }
        }
        let start = lower_edge.map_or(0, |edge| edge as u64 + 1);
        Some(start..=(high as u64).saturating_sub(1))
    }

    /// Integer pruning limit for the squared-difference numerator.
    ///
    /// `max_numerator = best_score * denom` is the exact per-position bound the
    /// bounded scan has always used; this wraps it as an integer so the
    /// pruning check is a plain compare instead of an f64 conversion. The
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

    /// Whether the patch and template energies alone put the numerator at
    /// or above `limit`, without reading a pixel.
    ///
    /// With `P = sum(I²)` and `T = sum(T²)`, Cauchy-Schwarz bounds
    /// `sum(I*T) <= sqrt(P*T)`, so the numerator `P + T - 2*sum(I*T)` is at
    /// least `P + T - 2*sqrt(P*T)`. That reaches `limit` exactly when
    /// `d = P + T - limit >= 0` and `d² >= 4*P*T`, checked in integers: a
    /// position pruned here is one [`Self::numerator_below`] prunes too.
    fn energy_gap_reaches(&self, patch_sum_sq: u64, limit: u64) -> bool {
        let energy = u128::from(patch_sum_sq) + u128::from(self.template_sum_sq);
        let Some(gap) = energy.checked_sub(u128::from(limit)) else {
            return false;
        };
        gap.checked_mul(gap).is_some_and(|gap_sq| {
            gap_sq >= 4 * u128::from(patch_sum_sq) * u128::from(self.template_sum_sq)
        })
    }

    /// Exact squared-difference numerator at one position, pruned against
    /// `bound` once per template row.
    ///
    /// Partial sums are monotone (every term is non-negative), so a position
    /// reaches `numerator >= bound` at some prefix if and only if its total
    /// numerator does. Checking at row ends instead of after every pixel
    /// therefore prunes exactly the same set, `{total >= bound}`, and
    /// survivors return the full exact numerator.
    fn numerator_below(&self, hx: u32, hy: u32, bound: Option<u64>) -> Option<u64> {
        let Some(limit) = bound else {
            return Some(self.numerator_at(hx, hy));
        };
        let tw = self.template.width as usize;
        let hw = self.context.haystack.width as usize;
        let haystack = &self.context.haystack.pixels[hy as usize * hw + hx as usize..];
        let mut numerator = 0u64;

        for (row, t_row) in self
            .template
            .pixels
            .chunks_exact(tw)
            .take(self.template.height as usize)
            .enumerate()
        {
            numerator += row_sqdiff(&haystack[row * hw..row * hw + tw], t_row);
            if numerator >= limit {
                return None;
            }
        }

        Some(numerator)
    }

    /// Full squared-difference numerator at one position without pruning.
    fn numerator_at(&self, hx: u32, hy: u32) -> u64 {
        let tw = self.template.width as usize;
        let hw = self.context.haystack.width as usize;
        let haystack = &self.context.haystack.pixels[hy as usize * hw + hx as usize..];
        self.template
            .pixels
            .chunks_exact(tw)
            .take(self.template.height as usize)
            .enumerate()
            .map(|(row, t_row)| row_sqdiff(&haystack[row * hw..row * hw + tw], t_row))
            .sum()
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
        let hw = self.context.haystack.width as usize;
        let haystack = &self.context.haystack.pixels[hy as usize * hw + hx as usize..];
        self.template
            .pixels
            .chunks_exact(tw)
            .take(self.template.height as usize)
            .enumerate()
            .map(|(row, t_row)| row_dot(&haystack[row * hw..row * hw + tw], t_row))
            .sum()
    }

    fn zero_denominator_score(&self, hx: u32, hy: u32) -> f64 {
        if self.numerator_at(hx, hy) == 0 {
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

/// Squared integral image of a haystack, borrowed from a [`MatchScratch`].
struct IntegralImage<'a> {
    width: usize,
    values: &'a [u64],
}

impl<'a> IntegralImage<'a> {
    /// Build the squared integral image into `values`, reusing its
    /// allocation. The zero top row and left column are rewritten and every
    /// other entry is overwritten, so earlier contents never leak through.
    fn from_squared(image: &GrayImage, values: &'a mut Vec<u64>) -> Self {
        let source_width = image.width as usize;
        let source_height = image.height as usize;
        let width = source_width + 1;
        let len = width * (source_height + 1);
        if values.len() == len {
            values[..width].fill(0);
            for row in values.chunks_exact_mut(width) {
                row[0] = 0;
            }
        } else {
            values.clear();
            values.resize(len, 0);
        }

        if source_width > 0 {
            let pixels = &image.pixels[..source_width * source_height];
            for (y, source_row) in pixels.chunks_exact(source_width).enumerate() {
                let (above, below) = values.split_at_mut((y + 1) * width);
                let above = &above[y * width + 1..];
                let current = &mut below[1..width];
                let mut row_sum = 0u64;
                for ((value, &above), &pixel) in current.iter_mut().zip(above).zip(source_row) {
                    let pixel = u64::from(pixel);
                    row_sum += pixel * pixel;
                    *value = above + row_sum;
                }
            }
        }

        Self {
            width,
            values: values.as_slice(),
        }
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

#[cfg(test)]
mod tests {
    /// The masked score ignores every pixel outside the mask: a filled
    /// socket (different interior) still matches the ring-only mask, while
    /// the unmasked score rejects it.
    #[test]
    fn masked_sqdiff_normed_compares_only_the_masked_pixels() {
        let ring = |interior: u8| GrayImage {
            width: 3,
            height: 3,
            pixels: vec![200, 200, 200, 200, interior, 200, 200, 200, 200],
        };
        let mask = GrayImage {
            width: 3,
            height: 3,
            pixels: vec![255, 255, 255, 255, 0, 255, 255, 255, 255],
        };
        let mut haystack = GrayImage {
            width: 5,
            height: 4,
            pixels: vec![10; 20],
        };
        let filled = ring(0);
        for y in 0..3 {
            for x in 0..3 {
                haystack.pixels[(y + 1) * 5 + x + 2] = filled.pixels[y * 3 + x];
            }
        }
        let template = ring(255);
        let masked = super::match_template_sqdiff_normed_masked(&haystack, &template, &mask)
            .unwrap_or_else(|| panic!("the template fits"));
        assert_eq!((masked.x, masked.y), (2, 1));
        assert!(masked.score.abs() < 1e-12, "masked score {}", masked.score);
        let unmasked = super::match_template_sqdiff_normed(&haystack, &template)
            .unwrap_or_else(|| panic!("the template fits"));
        assert!(unmasked.score > 0.1, "unmasked score {}", unmasked.score);
        // A mask of another size is no match at all.
        assert!(
            super::match_template_sqdiff_normed_masked(&haystack, &template, &haystack).is_none()
        );
    }

    use super::*;

    fn make_haystack() -> GrayImage {
        let mut pixels = vec![100u8; 10 * 10];
        for row in 2usize..6 {
            for col in 3usize..7 {
                pixels[row * 10 + col] = 200;
            }
        }
        GrayImage {
            width: 10,
            height: 10,
            pixels,
        }
    }

    fn template_exact() -> GrayImage {
        GrayImage {
            width: 4,
            height: 4,
            pixels: vec![200u8; 16],
        }
    }

    fn template_near() -> GrayImage {
        let mut pixels = vec![200u8; 16];
        pixels[0] = 190;
        GrayImage {
            width: 4,
            height: 4,
            pixels,
        }
    }

    fn template_far() -> GrayImage {
        GrayImage {
            width: 4,
            height: 4,
            pixels: vec![50u8; 16],
        }
    }

    fn naive_sqdiff_normed_at(haystack: &GrayImage, template: &GrayImage, hx: u32, hy: u32) -> f64 {
        let tw = template.width as usize;
        let th = template.height as usize;
        let hw = haystack.width as usize;
        let mut numerator = 0.0_f64;
        let mut patch_sum_sq = 0.0_f64;
        let mut template_sum_sq = 0.0_f64;

        for row in 0..th {
            let h_row_base = (hy as usize + row) * hw + hx as usize;
            let t_row_base = row * tw;
            for col in 0..tw {
                let h = f64::from(haystack.pixels[h_row_base + col]);
                let t = f64::from(template.pixels[t_row_base + col]);
                let diff = h - t;
                numerator += diff * diff;
                patch_sum_sq += h * h;
                template_sum_sq += t * t;
            }
        }

        let denom = (patch_sum_sq * template_sum_sq).sqrt();
        if denom < f64::EPSILON {
            if numerator < f64::EPSILON {
                0.0
            } else {
                1.0
            }
        } else {
            (numerator / denom).clamp(0.0, 1.0)
        }
    }

    fn assert_matches_naive(template: &GrayImage) {
        let haystack = make_haystack();
        let result = match_template_sqdiff_normed(&haystack, template)
            .unwrap_or_else(|| panic!("expected optimized match"));
        let naive_score =
            naive_sqdiff_normed_at(&haystack, template, result.x as u32, result.y as u32);
        assert!((result.score - naive_score).abs() < 1e-10);
    }

    /// Row-major brute-force reference: the first minimal naive score wins, so
    /// the selected location must equal the optimized scan's location.
    fn naive_best_match(haystack: &GrayImage, template: &GrayImage) -> Option<(f64, i32, i32)> {
        let result_cols = haystack.width - template.width + 1;
        let result_rows = haystack.height - template.height + 1;
        let mut best: Option<(f64, i32, i32)> = None;
        for hy in 0..result_rows {
            for hx in 0..result_cols {
                let score = naive_sqdiff_normed_at(haystack, template, hx, hy);
                match best {
                    Some((best_score, _, _)) if score >= best_score => {}
                    _ => best = Some((score, hx as i32, hy as i32)),
                }
            }
        }
        best
    }

    fn noisy_haystack(seed: u8) -> GrayImage {
        let width = 37u32;
        let height = 23u32;
        let mut pixels = Vec::with_capacity((width * height) as usize);
        let mut state = u32::from(seed) | 1;
        for _ in 0..width * height {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            pixels.push((state >> 16) as u8);
        }
        GrayImage {
            width,
            height,
            pixels,
        }
    }

    fn template_from(haystack: &GrayImage, x: u32, y: u32, width: u32, height: u32) -> GrayImage {
        let hw = haystack.width as usize;
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for row in 0..height {
            let base = ((y + row) as usize) * hw + x as usize;
            pixels.extend_from_slice(&haystack.pixels[base..base + width as usize]);
        }
        GrayImage {
            width,
            height,
            pixels,
        }
    }

    fn assert_bounded_matches_naive(template: &GrayImage, max_score: f64) {
        for seed in [1u8, 7, 42] {
            let haystack = noisy_haystack(seed);
            let expected = naive_best_match(&haystack, template);
            let actual = match_template_sqdiff_normed_below(&haystack, template, max_score);
            if let Some((score, x, y)) = expected {
                let score_ok = score < max_score;
                match actual {
                    Some(result) => {
                        assert!(
                            score_ok,
                            "expected None at seed {seed} when naive best {score} >= {max_score}"
                        );
                        assert_eq!(
                            (result.x, result.y),
                            (x, y),
                            "location mismatch at seed {seed}"
                        );
                        assert!(
                            (result.score - score).abs() < 1e-10,
                            "score mismatch at seed {seed}: {} vs {score}",
                            result.score
                        );
                    }
                    None => assert!(
                        !score_ok,
                        "expected Some at seed {seed}; naive best {score} < {max_score}"
                    ),
                }
            } else {
                assert!(actual.is_none());
            }
        }
    }

    #[test]
    fn exact_match_scores_zero() {
        let haystack = make_haystack();
        let template = template_exact();
        let result = match_template_sqdiff_normed(&haystack, &template)
            .unwrap_or_else(|| panic!("expected Some"));
        assert!(result.score < 1e-10);
    }

    #[test]
    fn exact_match_location_is_correct() {
        let haystack = make_haystack();
        let template = template_exact();
        let result = match_template_sqdiff_normed(&haystack, &template)
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(result.x, 3);
        assert_eq!(result.y, 2);
    }

    #[test]
    fn optimized_score_matches_naive_reference() {
        assert_matches_naive(&template_exact());
        assert_matches_naive(&template_near());
        assert_matches_naive(&template_far());
    }

    /// Every patch energy outside the energy window is one the exact
    /// per-position check prunes, for dim and bright templates and for tight
    /// and loose thresholds; the template's own energy is inside.
    #[test]
    fn energy_window_excludes_only_energies_the_exact_check_prunes() {
        let haystack = make_haystack();
        let mut scratch = MatchScratch::default();
        let context = HaystackMatchContext::new(&haystack, &mut scratch);
        for fill in [1u8, 7, 60, 200, 255] {
            let template = GrayImage {
                width: 3,
                height: 3,
                pixels: vec![fill; 9],
            };
            let scan =
                TemplateScan::new(&context, &template).unwrap_or_else(|| panic!("template fits"));
            for max_score in [0.0005, 0.05, 0.2, 0.9] {
                let Some(window) = scan.energy_window(max_score) else {
                    continue;
                };
                assert!(window.contains(&scan.template_sum_sq));
                let (start, end) = (*window.start(), *window.end());
                let outside = [
                    Some(0),
                    start.checked_sub(1),
                    start.checked_sub(2),
                    Some(start / 2),
                    end.checked_add(1),
                    end.checked_add(2),
                    end.checked_mul(3),
                ];
                for patch in outside.into_iter().flatten() {
                    if window.contains(&patch) {
                        continue;
                    }
                    let denom = ((patch as f64) * (scan.template_sum_sq as f64)).sqrt();
                    let pruned = scan
                        .numerator_limit(max_score, denom)
                        .map_or(denom < f64::EPSILON, |limit| {
                            scan.energy_gap_reaches(patch, limit)
                        });
                    assert!(
                        pruned,
                        "fill {fill} score {max_score}: energy {patch} outside {window:?} must be pruned"
                    );
                }
            }
        }
    }

    /// The bounded scan selects exactly the unbounded minimum when that is
    /// below the threshold, on noisy haystacks with a planted template.
    #[test]
    fn bounded_scan_matches_the_unbounded_minimum_on_noisy_frames() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 24) as u8
        };
        for round in 0..24 {
            let (width, height) = (48u32, 40u32);
            let mut pixels: Vec<u8> = (0..width * height).map(|_| next() / 3).collect();
            let template = GrayImage {
                width: 7,
                height: 6,
                pixels: (0..42).map(|_| 40 + next() / 2).collect(),
            };
            let (px, py) = (round % 30 + 3, round % 20 + 5);
            for ty in 0..6 {
                for tx in 0..7 {
                    let noise = next() % (1 + round as u8 % 12);
                    let value = template.pixels[(ty * 7 + tx) as usize].saturating_add(noise);
                    pixels[((py + ty) * width + px + tx) as usize] = value;
                }
            }
            let haystack = GrayImage {
                width,
                height,
                pixels,
            };
            let unbounded = match_template_sqdiff_normed(&haystack, &template)
                .unwrap_or_else(|| panic!("template fits"));
            for max_score in [0.001, 0.01, 0.05, 0.3] {
                let bounded = match_template_sqdiff_normed_below(&haystack, &template, max_score);
                let expected = (unbounded.score < max_score).then_some(unbounded.clone());
                assert_eq!(
                    bounded.map(|r| (r.x, r.y)),
                    expected.as_ref().map(|r| (r.x, r.y)),
                    "round {round} score {max_score}"
                );
            }
        }
    }

    #[test]
    fn bounded_exact_match_scores_zero() {
        let haystack = make_haystack();
        let template = template_exact();
        let result = match_template_sqdiff_normed_below(&haystack, &template, 0.05)
            .unwrap_or_else(|| panic!("expected bounded match"));
        assert!(result.score < 1e-10);
        assert_eq!(result.x, 3);
        assert_eq!(result.y, 2);
    }

    #[test]
    fn bounded_match_filters_scores_above_threshold() {
        let haystack = make_haystack();
        let far = template_far();
        assert!(match_template_sqdiff_normed_below(&haystack, &far, 0.05).is_none());
    }

    #[test]
    fn timed_bounded_match_reports_positions_and_pruning() {
        let haystack = make_haystack();
        let exact = template_exact();
        let result = match_template_sqdiff_normed_below_timed(&haystack, &exact, 0.05)
            .unwrap_or_else(|| panic!("expected timed match"));
        assert_eq!(result.stats.positions, 49);
        assert!(result.stats.fully_scored >= 1);
    }

    #[test]
    fn near_match_scores_higher_than_exact() {
        let haystack = make_haystack();
        let exact = match_template_sqdiff_normed(&haystack, &template_exact())
            .unwrap_or_else(|| panic!("expected Some"));
        let near = match_template_sqdiff_normed(&haystack, &template_near())
            .unwrap_or_else(|| panic!("expected Some"));
        assert!(near.score > exact.score);
    }

    #[test]
    fn far_match_scores_higher_than_near() {
        let haystack = make_haystack();
        let near = match_template_sqdiff_normed(&haystack, &template_near())
            .unwrap_or_else(|| panic!("expected Some"));
        let far = match_template_sqdiff_normed(&haystack, &template_far())
            .unwrap_or_else(|| panic!("expected Some"));
        assert!(far.score > near.score);
    }

    #[test]
    fn lower_is_better_ordering_exact_near_far() {
        let haystack = make_haystack();
        let exact = match_template_sqdiff_normed(&haystack, &template_exact())
            .unwrap_or_else(|| panic!("expected Some"));
        let near = match_template_sqdiff_normed(&haystack, &template_near())
            .unwrap_or_else(|| panic!("expected Some"));
        let far = match_template_sqdiff_normed(&haystack, &template_far())
            .unwrap_or_else(|| panic!("expected Some"));
        assert!(exact.score < near.score && near.score < far.score);
    }

    #[test]
    fn match_templates_returns_one_candidate_per_template() {
        let haystack = make_haystack();
        let exact = template_exact();
        let near = template_near();
        let far = template_far();
        let templates: &[(&str, &GrayImage)] = &[("exact", &exact), ("near", &near), ("far", &far)];
        let candidates = match_templates(&haystack, templates);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].template_name, "exact");
        assert_eq!(candidates[0].rect.x, 3);
        assert_eq!(candidates[0].rect.y, 2);
    }

    #[test]
    fn match_templates_below_keeps_only_valid_candidates() {
        let haystack = make_haystack();
        let exact = template_exact();
        let far = template_far();
        let templates: &[(&str, &GrayImage)] = &[("exact", &exact), ("far", &far)];
        let candidates = match_templates_below(&haystack, templates, 0.05);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].template_name, "exact");
    }

    #[test]
    fn template_larger_than_haystack_returns_none() {
        let haystack = GrayImage {
            width: 2,
            height: 2,
            pixels: vec![0; 4],
        };
        let template = GrayImage {
            width: 3,
            height: 3,
            pixels: vec![0; 9],
        };
        assert!(match_template_sqdiff_normed(&haystack, &template).is_none());
        assert!(match_template_sqdiff_normed_below(&haystack, &template, 0.05).is_none());
    }

    #[test]
    fn multi_template_match_finds_synthetic_fixture() {
        let haystack = make_haystack();
        let exact = template_exact();
        let templates: &[(&str, &GrayImage)] = &[("exact", &exact)];
        let candidates = match_templates_below(&haystack, templates, 0.05);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].template_name, "exact");
        assert_eq!(candidates[0].rect.x, 3);
        assert_eq!(candidates[0].rect.y, 2);
        assert!(candidates[0].score < 1e-10);
    }

    #[test]
    fn bounded_scan_matches_naive_bruteforce_at_several_thresholds() {
        let haystack = noisy_haystack(7);
        let embedded = template_from(&haystack, 11, 5, 9, 6);
        let mut shifted = embedded.clone();
        shifted.pixels[0] = shifted.pixels[0].wrapping_add(7);
        let black = GrayImage {
            width: 5,
            height: 4,
            pixels: vec![0u8; 20],
        };
        let clamping = GrayImage {
            width: 2,
            height: 2,
            pixels: vec![255, 0, 0, 255],
        };

        for template in [&embedded, &shifted, &black, &clamping] {
            // Thresholds up to 1.0: above 1.0 the bounded prune compares the
            // unclamped ratio against best*denom, so a naive-clamped 1.0 score
            // does not imply survival -- the unclamped reference cannot decide
            // those cases.
            for max_score in [0.0, 0.05, 0.5, 0.99, 1.0] {
                assert_bounded_matches_naive(template, max_score);
            }
        }
    }

    #[test]
    fn multi_template_scan_matches_single_template_calls_exactly() {
        let haystack = noisy_haystack(42);
        let a = template_from(&haystack, 3, 4, 8, 5);
        let mut b = a.clone();
        b.pixels[10] = b.pixels[10].wrapping_sub(23);
        let c = GrayImage {
            width: 4,
            height: 4,
            pixels: vec![200u8; 16],
        };

        let templates: Vec<(String, GrayImage)> = vec![
            ("a".to_string(), a.clone()),
            ("b".to_string(), b.clone()),
            ("c".to_string(), c.clone()),
        ];
        let refs: Vec<(&str, &GrayImage)> = templates
            .iter()
            .map(|(name, image)| (name.as_str(), image))
            .collect();

        for max_score in [0.05f64, 0.5, f64::INFINITY] {
            let multi = if max_score.is_finite() {
                match_templates_below(&haystack, &refs, max_score)
            } else {
                match_templates(&haystack, &refs)
            };
            let single: Vec<MatchCandidate> = refs
                .iter()
                .filter_map(|&(name, template)| {
                    let result = if max_score.is_finite() {
                        match_template_sqdiff_normed_below(&haystack, template, max_score)
                    } else {
                        match_template_sqdiff_normed(&haystack, template)
                    }?;
                    Some(MatchCandidate {
                        template_name: name.to_string(),
                        score: result.score,
                        rect: Rect {
                            x: result.x,
                            y: result.y,
                            width: template.width,
                            height: template.height,
                        },
                    })
                })
                .collect();
            assert_eq!(
                multi, single,
                "shared-haystack scan diverged at {max_score}"
            );
        }
    }

    #[test]
    fn zero_energy_template_still_selects_all_black_position() {
        let haystack = GrayImage {
            width: 6,
            height: 6,
            pixels: vec![0u8; 36],
        };
        let template = GrayImage {
            width: 3,
            height: 3,
            pixels: vec![0u8; 9],
        };
        let result = match_template_sqdiff_normed_below(&haystack, &template, 0.05)
            .unwrap_or_else(|| panic!("expected all-black match"));
        assert_eq!((result.x, result.y), (0, 0));
        assert!(result.score < 1e-12);
    }

    #[test]
    fn row_bands_cover_every_row_once_top_to_bottom() {
        for rows in 1..40u32 {
            for bands in [1usize, 2, 3, 8, 64] {
                let ranges = row_bands(rows, bands);
                assert_eq!(ranges.len(), bands.min(rows as usize));
                let mut next = 0;
                for range in &ranges {
                    assert_eq!(range.start, next);
                    assert!(range.end > range.start);
                    next = range.end;
                }
                assert_eq!(next, rows);
            }
        }
    }

    #[test]
    fn host_scan_threads_stay_within_the_owner_budget() {
        let threads = scan_threads_for_host();
        assert!((1..=4).contains(&threads), "{threads}");
    }

    #[test]
    fn zero_threshold_scan_returns_none() {
        let haystack = noisy_haystack(3);
        let template = template_from(&haystack, 2, 2, 5, 5);
        assert!(match_template_sqdiff_normed_below(&haystack, &template, 0.0).is_none());
    }
}
