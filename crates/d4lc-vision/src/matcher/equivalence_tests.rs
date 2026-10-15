//! Equivalence of the live matcher with the verbatim former matcher
//! (`reference_matcher`): identical candidates (name, location, score bits)
//! on the real fixture frames and on random haystacks and templates.

use super::reference_matcher as reference;
use super::*;
use crate::{to_grayscale, to_grayscale_into};
use d4lc_core::CaptureFrame;
use std::path::{Path, PathBuf};

/// Thread budgets the banded scan must be identical under.
const THREAD_COUNTS: [usize; 4] = [1, 2, 3, 8];

const FIXTURE_FRAMES: &[&str] = &["d4lc-livecap.png", "frame-CTRL.png", "frame-KB.png"];

/// Known marker positions on the fixture frames (see the CLI live-frame
/// tests); windows around them keep the unbounded comparison affordable.
const FIXTURE_MARKERS: &[(&str, u32, u32)] = &[
    ("d4lc-livecap.png", 1094, 843),
    ("frame-KB.png", 1038, 882),
    ("frame-CTRL.png", 825, 869),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_rgba(path: &Path) -> image::RgbaImage {
    image::ImageReader::open(path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", path.display()))
        .decode()
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
        .to_rgba8()
}

fn load_frame(file: &str) -> CaptureFrame {
    let rgba = load_rgba(
        &workspace_root()
            .join("tests/fixtures/live-frames")
            .join(file),
    );
    let (width, height) = rgba.dimensions();
    CaptureFrame {
        width,
        height,
        stride: width as usize * 4,
        pixels_bgra: rgba
            .pixels()
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
            .collect(),
    }
}

/// Every tooltip template of the 1080p presets, gray-converted the way the
/// entry points load them.
fn fixture_templates() -> Vec<(String, GrayImage)> {
    let presets = workspace_root().join("assets/original/systempresets");
    let mut paths = Vec::new();
    for preset in ["1080p_SMF", "1080p_SSF"] {
        for dir in ["Tooltips", "Tooltips/Optional"] {
            let dir = presets.join(preset).join(dir);
            let entries = std::fs::read_dir(&dir)
                .unwrap_or_else(|error| panic!("failed to list {}: {error}", dir.display()));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "png") {
                    paths.push(path);
                }
            }
        }
    }
    paths.sort();
    assert!(paths.len() >= 11, "expected the 1080p tooltip templates");
    paths
        .iter()
        .map(|path| {
            let rgba = load_rgba(path);
            let (width, height) = rgba.dimensions();
            let pixels = rgba
                .pixels()
                .map(|pixel| {
                    let r = u32::from(pixel[0]);
                    let g = u32::from(pixel[1]);
                    let b = u32::from(pixel[2]);
                    ((299 * r + 587 * g + 114 * b) / 1000) as u8
                })
                .collect();
            let name = path
                .strip_prefix(&presets)
                .unwrap_or(path)
                .display()
                .to_string();
            (
                name,
                GrayImage {
                    width,
                    height,
                    pixels,
                },
            )
        })
        .collect()
}

fn template_refs(templates: &[(String, GrayImage)]) -> Vec<(&str, &GrayImage)> {
    templates
        .iter()
        .map(|(name, image)| (name.as_str(), image))
        .collect()
}

fn crop_gray(image: &GrayImage, x: u32, y: u32, width: u32, height: u32) -> GrayImage {
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for row in y..y + height {
        let start = (row * image.width + x) as usize;
        pixels.extend_from_slice(&image.pixels[start..start + width as usize]);
    }
    GrayImage {
        width,
        height,
        pixels,
    }
}

fn assert_identical(actual: &[MatchCandidate], expected: &[MatchCandidate], context: &str) {
    assert_eq!(actual.len(), expected.len(), "candidate count: {context}");
    for (a, e) in actual.iter().zip(expected) {
        assert_eq!(a.template_name, e.template_name, "name: {context}");
        assert_eq!(a.rect, e.rect, "rect of {}: {context}", e.template_name);
        assert_eq!(
            a.score.to_bits(),
            e.score.to_bits(),
            "score bits of {} ({} vs {}): {context}",
            e.template_name,
            a.score,
            e.score
        );
    }
}

fn assert_identical_timed(
    actual: Option<TimedTemplateMatchResult>,
    expected: Option<TimedTemplateMatchResult>,
    context: &str,
) {
    match (actual, expected) {
        (None, None) => {}
        (Some(a), Some(e)) => {
            assert_eq!(
                (a.result.x, a.result.y),
                (e.result.x, e.result.y),
                "{context}"
            );
            assert_eq!(
                a.result.score.to_bits(),
                e.result.score.to_bits(),
                "{context}"
            );
            // The energy window prunes by `max_score` even after an exact
            // (score 0) match switched the per-position bound off, so it
            // may score fewer positions; it never scores more.
            assert_eq!(a.stats.positions, e.stats.positions, "{context}");
            assert!(
                a.stats.fully_scored <= e.stats.fully_scored,
                "{context}: {:?} vs {:?}",
                a.stats,
                e.stats
            );
        }
        (a, e) => panic!("presence differs ({a:?} vs {e:?}): {context}"),
    }
}

#[test]
fn fixture_frames_grayscale_like_the_reference() {
    // One reused buffer across frames of different content, starting from a
    // larger stale image, must still reproduce the fresh conversion.
    let mut reused = GrayImage {
        width: 7,
        height: 3000,
        pixels: vec![0xAB; 7 * 3000],
    };
    for file in FIXTURE_FRAMES {
        let frame = load_frame(file);
        let expected = reference::to_grayscale(&frame);
        assert_eq!(to_grayscale(&frame), expected, "{file}");
        to_grayscale_into(&frame, &mut reused);
        assert_eq!(reused, expected, "{file} (reused buffer)");
    }
}

#[test]
fn grayscale_honours_stride_padding() {
    let mut frame = CaptureFrame {
        width: 3,
        height: 2,
        stride: 3 * 4 + 5,
        pixels_bgra: Vec::new(),
    };
    let mut state = 17u32;
    frame.pixels_bgra = (0..frame.stride * 2)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16) as u8
        })
        .collect();
    assert_eq!(to_grayscale(&frame), reference::to_grayscale(&frame));
}

#[test]
fn fixture_frames_match_the_reference_bounded_scan() {
    let templates = fixture_templates();
    let refs = template_refs(&templates);
    let mut scratch = MatchScratch::default();
    for file in FIXTURE_FRAMES {
        let gray = to_grayscale(&load_frame(file));
        // 0.025 is the upstream default ThresholdSimilarityTooltip; looser
        // thresholds run on the marker windows below.
        for max_score in [0.025, 0.05] {
            let expected = reference::match_templates_below(&gray, &refs, max_score);
            for threads in THREAD_COUNTS {
                let actual = match_templates_below_parallel_timed(
                    &gray,
                    &refs,
                    max_score,
                    threads,
                    &mut scratch,
                );
                assert_identical(
                    &actual.candidates,
                    &expected,
                    &format!("{file} at {max_score}, {threads} threads"),
                );
            }
        }
    }
}

#[test]
fn fixture_marker_windows_match_the_reference_unbounded_and_loose_scans() {
    let templates = fixture_templates();
    let refs = template_refs(&templates);
    let mut scratch = MatchScratch::default();
    for &(file, marker_x, marker_y) in FIXTURE_MARKERS {
        let gray = to_grayscale(&load_frame(file));
        let window = crop_gray(&gray, marker_x - 60, marker_y - 40, 180, 100);
        assert_identical(
            &match_templates(&window, &refs),
            &reference::match_templates(&window, &refs),
            &format!("{file} unbounded"),
        );
        for max_score in [0.5, 1.0] {
            let expected = reference::match_templates_below(&window, &refs, max_score);
            for threads in THREAD_COUNTS {
                let actual = match_templates_below_parallel_timed(
                    &window,
                    &refs,
                    max_score,
                    threads,
                    &mut scratch,
                );
                assert_identical(
                    &actual.candidates,
                    &expected,
                    &format!("{file} window at {max_score}, {threads} threads"),
                );
            }
        }
    }
}

/// Deterministic generator for the property test (no extra dependency).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, bound: u32) -> u32 {
        self.next() % bound.max(1)
    }

    fn range(&mut self, low: u32, high: u32) -> u32 {
        low + self.below(high - low + 1)
    }
}

fn random_haystack(rng: &mut Lcg) -> GrayImage {
    let width = rng.range(1, 48);
    let height = rng.range(1, 32);
    let palette: Vec<u8> = match rng.below(4) {
        // Few distinct values: many exact ties between positions.
        0 => vec![0, 7, 200],
        1 => vec![0, 255],
        _ => Vec::new(),
    };
    let mut pixels: Vec<u8> = (0..width * height)
        .map(|_| {
            if palette.is_empty() {
                rng.next() as u8
            } else {
                palette[rng.below(palette.len() as u32) as usize]
            }
        })
        .collect();
    if rng.below(3) == 0 {
        // A zero-energy block: degenerate denominators.
        let bw = rng.range(1, width);
        let bh = rng.range(1, height);
        let bx = rng.below(width - bw + 1);
        let by = rng.below(height - bh + 1);
        for y in by..by + bh {
            for x in bx..bx + bw {
                pixels[(y * width + x) as usize] = 0;
            }
        }
    }
    if rng.below(4) == 0 {
        // A repeated tile: identical patches at several positions.
        let tile = [rng.next() as u8, rng.next() as u8, rng.next() as u8];
        for (index, pixel) in pixels.iter_mut().enumerate() {
            if index % 5 < 3 {
                *pixel = tile[index % 3];
            }
        }
    }
    GrayImage {
        width,
        height,
        pixels,
    }
}

fn random_template(rng: &mut Lcg, haystack: &GrayImage) -> GrayImage {
    let width = rng.range(1, haystack.width.min(20));
    let height = rng.range(1, haystack.height.min(12));
    match rng.below(5) {
        0 | 1 => {
            let x = rng.below(haystack.width - width + 1);
            let y = rng.below(haystack.height - height + 1);
            let mut template = crop_gray(haystack, x, y, width, height);
            if rng.below(2) == 0 {
                let index = rng.below(width * height) as usize;
                template.pixels[index] =
                    template.pixels[index].wrapping_add(rng.range(1, 40) as u8);
            }
            template
        }
        2 => GrayImage {
            width,
            height,
            pixels: vec![0; (width * height) as usize],
        },
        3 => GrayImage {
            width,
            height,
            pixels: vec![rng.next() as u8; (width * height) as usize],
        },
        _ => GrayImage {
            width,
            height,
            pixels: (0..width * height).map(|_| rng.next() as u8).collect(),
        },
    }
}

#[test]
fn random_haystacks_and_templates_match_the_reference() {
    let thresholds = [
        0.0,
        1e-9,
        0.025,
        0.1,
        0.5,
        0.99,
        1.0,
        2.0,
        f64::INFINITY,
        -1.0,
        f64::NAN,
    ];
    let mut rng = Lcg(0x5EED_D41C);
    // One scratch across every case: stale buffers of other sizes (and of
    // the same length with other dimensions) must never leak into a result.
    let mut scratch = MatchScratch::default();
    for case in 0..400 {
        let haystack = random_haystack(&mut rng);
        let count = rng.range(1, 3);
        let templates: Vec<(String, GrayImage)> = (0..count)
            .map(|index| (format!("t{index}"), random_template(&mut rng, &haystack)))
            .collect();
        let refs = template_refs(&templates);
        for max_score in thresholds {
            let context = format!(
                "case {case} ({}x{}) at {max_score}",
                haystack.width, haystack.height
            );
            let expected = reference::match_templates_below(&haystack, &refs, max_score);
            for threads in THREAD_COUNTS {
                let actual = match_templates_below_parallel_timed(
                    &haystack,
                    &refs,
                    max_score,
                    threads,
                    &mut scratch,
                );
                assert_identical(
                    &actual.candidates,
                    &expected,
                    &format!("{context}, {threads} threads"),
                );
            }
            for &(name, template) in &refs {
                assert_identical_timed(
                    match_template_sqdiff_normed_below_timed(&haystack, template, max_score),
                    reference::match_template_sqdiff_normed_below_timed(
                        &haystack, template, max_score,
                    ),
                    &format!("{context} single {name}"),
                );
            }
        }
        assert_identical(
            &match_templates(&haystack, &refs),
            &reference::match_templates(&haystack, &refs),
            &format!("case {case} unbounded"),
        );
    }
}

#[test]
fn integral_scratch_reused_across_equal_length_shapes_stays_exact() {
    // 5x3 and 3x5 (and 6x4 and 4x6) haystacks give integral buffers of the
    // same length with a different row width, so a reused buffer keeps stale
    // values where the new zero border goes: it must be rewritten.
    let mut rng = Lcg(99);
    let mut scratch = MatchScratch::default();
    for (width, height) in [(5u32, 3u32), (3, 5), (5, 3), (6, 4), (4, 6), (3, 5)] {
        let haystack = GrayImage {
            width,
            height,
            pixels: (0..width * height).map(|_| rng.next() as u8).collect(),
        };
        let template = crop_gray(&haystack, 1, 1, 2, 2);
        let refs = [("t", &template)];
        for max_score in [0.05, 0.5, f64::INFINITY] {
            let actual =
                match_templates_below_parallel_timed(&haystack, &refs, max_score, 1, &mut scratch)
                    .candidates;
            assert_identical(
                &actual,
                &reference::match_templates_below(&haystack, &refs, max_score),
                &format!("{width}x{height} at {max_score}"),
            );
        }
    }
}

#[test]
fn equal_scores_in_several_bands_keep_the_first_in_raster_order() {
    // A haystack of identical rows: every row holds the same best patch, so
    // each band finds an equal score and the merge must keep the topmost.
    let row: Vec<u8> = (0..24u8).map(|x| x.wrapping_mul(37)).collect();
    let haystack = GrayImage {
        width: 24,
        height: 16,
        pixels: row.repeat(16),
    };
    let template = crop_gray(&haystack, 5, 3, 4, 2);
    let refs = [("t", &template)];
    let expected = reference::match_templates_below(&haystack, &refs, 0.5);
    assert_eq!(expected.len(), 1);
    assert_eq!((expected[0].rect.x, expected[0].rect.y), (5, 0));
    let mut scratch = MatchScratch::default();
    for threads in THREAD_COUNTS {
        let actual =
            match_templates_below_parallel_timed(&haystack, &refs, 0.5, threads, &mut scratch);
        assert_identical(&actual.candidates, &expected, &format!("{threads} threads"));
    }
}

#[test]
fn a_thread_budget_above_one_really_scans_in_parallel() {
    let haystack = GrayImage {
        width: 32,
        height: 32,
        pixels: (0..32 * 32u32).map(|index| (index * 7) as u8).collect(),
    };
    let template = crop_gray(&haystack, 3, 4, 5, 5);
    let refs = [("t", &template)];
    let mut scratch = MatchScratch::default();
    let sequential = match_templates_below_parallel_timed(&haystack, &refs, 0.5, 1, &mut scratch);
    assert_eq!(sequential.threads_used, 1);
    let parallel = match_templates_below_parallel_timed(&haystack, &refs, 0.5, 3, &mut scratch);
    assert_eq!(parallel.threads_used, 3);
    assert_identical(&parallel.candidates, &sequential.candidates, "3 threads");
}
