use d4lc_assets::{AssetRepository, TemplateKind};
use d4lc_core::{CaptureFrame, GrayImage, InputMode, MatchCandidate, Rect};
use d4lc_debug::{write_capture_png, write_gray_png, write_match_trace_json};
use d4lc_detect::prepared::{
    derive_thresholded_crop_path, DetectFrameResult, DetectionTimings, PreparedDetector, RoiFrame,
    ThresholdSettings, TooltipDetectionTrace,
};
use d4lc_vision::{threshold_binary_inv, to_grayscale};
use serde_json::{json, Value};

use crate::roots::{cli_overrides_json, Roots};
use crate::to_json_text;

// Imports needed only by tests.
#[cfg(test)]
use d4lc_capture::crop_frame;
#[cfg(test)]
use d4lc_core::Point;
#[cfg(test)]
use d4lc_detect::prepared::{
    clamp_upstream_tooltip_crop_rect, raw_upstream_tooltip_crop_rect, translate_candidate,
    UPSTREAM_TOOLTIP_CROP_RULE,
};
use image::{ImageReader, RgbaImage};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DetectArgs {
    pub input_path: Option<PathBuf>,
    pub preset: Option<String>,
    pub language: Option<String>,
    pub controller_config: Vec<String>,
    pub mode: Option<String>,
    pub mouse_x: Option<i32>,
    pub mouse_y: Option<i32>,
    pub threshold_min: Option<u8>,
    pub threshold_max: Option<u8>,
    pub threshold_similarity_tooltip: Option<f64>,
    pub trace_output_path: Option<PathBuf>,
    pub crop_output_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DetectFrameArgs {
    pub preset: String,
    pub language: String,
    pub controller_config: Vec<String>,
    pub mode: InputMode,
    pub mouse_x: i32,
    pub scan_height_percent: f64,
    pub thresholds: ThresholdSettings,
    pub trace_output_path: Option<PathBuf>,
    pub crop_output_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedDetectionConfig {
    pub preset: String,
    pub language: String,
    pub controller_config: Vec<String>,
    pub mode: InputMode,
    pub scan_height_percent: f64,
    pub thresholds: ThresholdSettings,
}

pub fn detect_tooltip(roots: &Roots, args: &DetectArgs) -> Result<String, String> {
    let input_path = args
        .input_path
        .as_ref()
        .ok_or_else(|| "detect-tooltip requires --input <png>".to_string())?;
    let (resolved, config) = resolve_detection_config(
        roots,
        args.preset.clone(),
        args.language.clone(),
        &args.controller_config,
        args.mode.as_deref(),
        args.threshold_min,
        args.threshold_max,
        args.threshold_similarity_tooltip,
    )?;
    if matches!(resolved.mode, InputMode::Mouse) && args.mouse_x.is_none() {
        return Err("detect-tooltip mouse mode requires --mouse-x <int>".to_string());
    }

    let frame = load_capture_frame(&roots.work_path(input_path))?;
    let result = detect_frame(
        roots,
        &frame,
        &DetectFrameArgs {
            preset: resolved.preset,
            language: resolved.language,
            controller_config: resolved.controller_config,
            mode: resolved.mode,
            mouse_x: args.mouse_x.unwrap_or_default(),
            scan_height_percent: resolved.scan_height_percent,
            thresholds: resolved.thresholds,
            trace_output_path: args
                .trace_output_path
                .clone()
                .or_else(|| Some(PathBuf::from("debug-output/detect/detect-trace.json"))),
            crop_output_path: args
                .crop_output_path
                .clone()
                .or_else(|| Some(PathBuf::from("debug-output/detect/detect-crop.png"))),
        },
    )?;

    Ok(detect_output_json(input_path, &result, config))
}

/// The detection settings of the config root with the CLI overrides
/// applied, and where they came from (`config` of the reports).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_detection_config(
    roots: &Roots,
    preset: Option<String>,
    language: Option<String>,
    controller_config: &[String],
    mode: Option<&str>,
    threshold_min: Option<u8>,
    threshold_max: Option<u8>,
    threshold_similarity_tooltip: Option<f64>,
) -> Result<(ResolvedDetectionConfig, Value), String> {
    let files = roots.config_files()?;
    let config = files.identity_json(cli_overrides_json(&[
        ("preset", json!(preset)),
        ("language", json!(language)),
        ("mode", json!(mode)),
        ("controller", json!(controller_config)),
        ("threshold_min", json!(threshold_min)),
        ("threshold_max", json!(threshold_max)),
        (
            "threshold_similarity_tooltip",
            json!(threshold_similarity_tooltip),
        ),
    ]));
    let settings = files.clone().into_settings()?;
    let resolved_mode = match mode {
        Some(value) => parse_mode(Some(value))?,
        None if settings.controller_mode => InputMode::Controller,
        None => InputMode::Mouse,
    };
    let resolved_controller_config = if controller_config.is_empty() {
        files.into_controllers()?
    } else {
        controller_config.to_vec()
    };

    let resolved = ResolvedDetectionConfig {
        preset: preset.unwrap_or(settings.selected_system_preset),
        language: language.unwrap_or(settings.selected_affix_language),
        controller_config: resolved_controller_config,
        mode: resolved_mode,
        scan_height_percent: settings.scan_height_percent,
        thresholds: ThresholdSettings {
            min: threshold_min.unwrap_or(settings.threshold_min),
            max: threshold_max.unwrap_or(settings.threshold_max),
            similarity_tooltip: threshold_similarity_tooltip
                .unwrap_or(settings.threshold_similarity_tooltip),
        },
    };
    Ok((resolved, config))
}

pub(crate) fn prepare_detector(
    roots: &Roots,
    resolved: ResolvedDetectionConfig,
) -> Result<PreparedDetector, String> {
    let repo = roots.preset_repo(&resolved.preset)?;
    let loaded = repo.load_preset_templates(&resolved.preset)?;
    let template_names = selected_template_names(
        &repo,
        &resolved.preset,
        resolved.mode,
        &resolved.language,
        &resolved.controller_config,
    )?;
    let templates = load_tooltip_templates(repo.root(), &loaded.templates, &template_names)?;

    Ok(PreparedDetector {
        preset: resolved.preset,
        mode: resolved.mode,
        scan_height_percent: resolved.scan_height_percent,
        thresholds: resolved.thresholds,
        tooltip_width: loaded.config.tooltip_width,
        templates,
        scan: d4lc_detect::prepared::DetectScan::default(),
    })
}

pub(crate) fn detect_frame(
    roots: &Roots,
    frame: &CaptureFrame,
    args: &DetectFrameArgs,
) -> Result<DetectFrameResult, String> {
    let prepare_start = Instant::now();
    let resolved = ResolvedDetectionConfig {
        preset: args.preset.clone(),
        language: args.language.clone(),
        controller_config: args.controller_config.clone(),
        mode: args.mode,
        scan_height_percent: args.scan_height_percent,
        thresholds: args.thresholds,
    };
    let prepared = prepare_detector(roots, resolved)?;
    let template_prepare_ms = elapsed_ms(prepare_start);
    let mut result = detect_frame_prepared(
        &roots.work,
        frame,
        &prepared,
        args.mouse_x,
        args.trace_output_path.clone(),
        args.crop_output_path.clone(),
    )?;
    result.timings.asset_load_ms = template_prepare_ms;
    result.timings.template_select_ms = 0;
    result.timings.template_load_ms = 0;
    result.timings.template_prepare_ms = template_prepare_ms;
    result.timings.total_ms = result.timings.total_ms.saturating_add(template_prepare_ms);
    Ok(result)
}

/// Detect in `frame`; the debug paths resolve against `output_base`.
pub(crate) fn detect_frame_prepared(
    output_base: &Path,
    frame: &CaptureFrame,
    prepared: &PreparedDetector,
    mouse_x: i32,
    trace_output_path: Option<PathBuf>,
    crop_output_path: Option<PathBuf>,
) -> Result<DetectFrameResult, String> {
    let mut result = d4lc_detect::prepared::detect_roi_frame_prepared(
        RoiFrame::full_window(frame),
        prepared,
        mouse_x,
        trace_output_path.clone(),
        crop_output_path.clone(),
    )?;

    // Write debug files (trace JSON, crop PNGs) — the prepared pipeline no
    // longer does this itself.
    let trace_start = Instant::now();
    if let Some(path) = &trace_output_path {
        write_match_trace_json(
            &output_base.join(path),
            result.roi.width,
            result.roi.height,
            &result.candidates,
        )?;
    }
    result.timings.trace_write_ms = elapsed_ms(trace_start);

    let crop_start = Instant::now();
    if let Some(path) = &crop_output_path {
        if let Some(crop) = &result.crop {
            write_capture_png(&output_base.join(path), crop)?;
            let crop_gray = to_grayscale(crop);
            let crop_thresholded =
                threshold_binary_inv(&crop_gray, result.thresholds.min, result.thresholds.max);
            let thresholded_path = derive_thresholded_crop_path(path);
            write_gray_png(&output_base.join(&thresholded_path), &crop_thresholded)?;
        }
    }
    result.timings.crop_write_ms = elapsed_ms(crop_start);

    Ok(result)
}

pub(crate) fn parse_mode(value: Option<&str>) -> Result<InputMode, String> {
    match value {
        Some("mouse") => Ok(InputMode::Mouse),
        Some("controller") => Ok(InputMode::Controller),
        Some(other) => Err(format!("unsupported --mode '{other}'")),
        None => Err("detect-tooltip requires --mode mouse|controller".to_string()),
    }
}

fn selected_template_names(
    repo: &AssetRepository,
    preset: &str,
    mode: InputMode,
    language: &str,
    controller_config: &[String],
) -> Result<Vec<String>, String> {
    match mode {
        InputMode::Mouse => Ok(repo
            .controller_image_names(preset, language)?
            .into_iter()
            .filter(|name| name.starts_with("tooltip_kb_"))
            .collect()),
        InputMode::Controller => {
            repo.active_controller_image_names(preset, language, controller_config)
        }
    }
}

fn load_tooltip_templates(
    root: &Path,
    templates: &[d4lc_assets::TemplateAsset],
    selected_names: &[String],
) -> Result<Vec<(String, GrayImage)>, String> {
    let mut selected = Vec::new();
    for template in templates {
        if template.kind != TemplateKind::Tooltip {
            continue;
        }
        if !selected_names
            .iter()
            .any(|selected_name| selected_name.eq_ignore_ascii_case(&template.file_name))
        {
            continue;
        }
        let image = load_gray_png(&root.join(&template.relative_path))?;
        selected.push((template.file_name.clone(), image));
    }
    Ok(selected)
}

pub(crate) fn load_capture_frame(path: &Path) -> Result<CaptureFrame, String> {
    let rgba = load_rgba_image(path)?;
    let (width, height) = rgba.dimensions();
    let stride = usize::try_from(width)
        .map_err(|error| error.to_string())?
        .checked_mul(4)
        .ok_or_else(|| "frame stride overflow".to_string())?;
    let pixels_bgra = rgba
        .pixels()
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
        .collect::<Vec<_>>();
    Ok(CaptureFrame {
        width,
        height,
        stride,
        pixels_bgra,
    })
}

fn load_gray_png(path: &Path) -> Result<GrayImage, String> {
    let rgba = load_rgba_image(path)?;
    let (width, height) = rgba.dimensions();
    Ok(d4lc_vision::gray_from_rgba8(width, height, rgba.as_raw()))
}

fn load_rgba_image(path: &Path) -> Result<RgbaImage, String> {
    ImageReader::open(path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?
        .decode()
        .map_err(|error| format!("failed to decode {}: {error}", path.display()))
        .map(|image| image.to_rgba8())
}

pub(crate) fn planned_tooltip_roi_source(rect: Option<Rect>) -> &'static str {
    if rect.is_some() {
        "marker_anchored_estimate"
    } else {
        "none"
    }
}

pub(crate) fn planned_tooltip_roi_confidence(rect: Option<Rect>) -> &'static str {
    if rect.is_some() {
        "estimated"
    } else {
        "none"
    }
}

fn detect_output_json(input_path: &Path, result: &DetectFrameResult, config: Value) -> String {
    let path = |path: &Option<PathBuf>| path.as_ref().map(|path| path.display().to_string());
    to_json_text(&json!({
        "schema_version": 1,
        "config": config,
        "input": input_path.display().to_string(),
        "preset": result.preset,
        "mode": input_mode_label(result.mode),
        "roi": result.roi,
        "roi_role": result.roi_role.to_string(),
        "detection_roi": result.roi,
        "planned_tooltip_roi": result.planned_tooltip_roi,
        "planned_tooltip_roi_source": planned_tooltip_roi_source(result.planned_tooltip_roi),
        "planned_tooltip_roi_confidence": planned_tooltip_roi_confidence(result.planned_tooltip_roi),
        "tooltip_detection": tooltip_detection_json(&result.tooltip_detection),
        "content_roi_capture_requested": result.crop_output_path.is_some(),
        "thresholds": thresholds_json(result.thresholds),
        "timings": detection_timings_json(result.timings),
        "candidate_count": result.candidates.len(),
        "selected_match": result.selected.as_ref().map(match_candidate_json),
        "trace_output": path(&result.trace_output_path),
        "crop_output": path(&result.crop_output_path),
        "thresholded_crop_output": path(&result.thresholded_crop_output_path),
    }))
}

pub(crate) fn input_mode_label(mode: InputMode) -> &'static str {
    match mode {
        InputMode::Mouse => "mouse",
        InputMode::Controller => "controller",
    }
}

pub(crate) fn tooltip_detection_json(trace: &TooltipDetectionTrace) -> Value {
    json!({
        "search_roi": trace.search_roi,
        "detected_tooltip_rect": trace.detected_tooltip_rect,
        "final_tooltip_crop_rect": trace.final_tooltip_crop_rect,
        "crop_source": trace.crop_source,
        "upstream_rule": trace.upstream_rule,
        "final_tooltip_size": trace
            .final_tooltip_size
            .map(|(width, height)| json!({ "width": width, "height": height })),
    })
}

pub(crate) fn thresholds_json(thresholds: ThresholdSettings) -> Value {
    json!({
        "threshold_min": thresholds.min,
        "threshold_max": thresholds.max,
        "threshold_similarity_tooltip": thresholds.similarity_tooltip,
    })
}

pub(crate) fn match_candidate_json(candidate: &MatchCandidate) -> Value {
    json!({
        "template_name": candidate.template_name,
        "score": candidate.score,
        "rect": candidate.rect,
    })
}

pub(crate) fn detection_timings_json(timings: DetectionTimings) -> Value {
    json!({
        "asset_load_ms": timings.asset_load_ms,
        "roi_crop_ms": timings.roi_crop_ms,
        "grayscale_ms": timings.grayscale_ms,
        "template_select_ms": timings.template_select_ms,
        "template_load_ms": timings.template_load_ms,
        "template_prepare_ms": timings.template_prepare_ms,
        "integral_ms": timings.integral_ms,
        "match_ms": timings.match_ms,
        "scan_threads": timings.scan_threads,
        "coordinate_translate_ms": timings.coordinate_translate_ms,
        "trace_write_ms": timings.trace_write_ms,
        "crop_write_ms": timings.crop_write_ms,
        "total_ms": timings.total_ms,
    })
}

fn elapsed_ms(start: Instant) -> u128 {
    duration_ms(start.elapsed())
}

fn duration_ms(duration: Duration) -> u128 {
    duration.as_micros().div_ceil(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_pixel_template() -> GrayImage {
        GrayImage {
            width: 1,
            height: 1,
            pixels: vec![0],
        }
    }

    fn white_frame(width: u32, height: u32) -> CaptureFrame {
        let pixels = (0..width * height)
            .flat_map(|_| [255, 255, 255, 255])
            .collect::<Vec<_>>();
        CaptureFrame {
            width,
            height,
            stride: usize::try_from(width).unwrap_or(0) * 4,
            pixels_bgra: pixels,
        }
    }

    fn put_black_pixel(frame: &mut CaptureFrame, x: usize, y: usize) {
        let width = usize::try_from(frame.width).unwrap_or(0);
        let index = (y * width + x) * 4;
        frame.pixels_bgra[index..index + 4].copy_from_slice(&[0, 0, 0, 255]);
    }

    fn prepared_detector() -> PreparedDetector {
        PreparedDetector {
            preset: "synthetic".to_string(),
            mode: InputMode::Controller,
            scan_height_percent: 100.0,
            thresholds: ThresholdSettings {
                min: 0,
                max: 255,
                similarity_tooltip: 0.01,
            },
            tooltip_width: 2,
            templates: vec![("synthetic-template.png".to_string(), one_pixel_template())],
            scan: d4lc_detect::prepared::DetectScan::default(),
        }
    }

    #[test]
    fn prepared_detector_reuses_template_buffers() {
        let detector = prepared_detector();
        assert_eq!(detector.template_count(), 1);
        let first = detector.template_refs();
        let second = detector.template_refs();
        assert_eq!(first[0].0, "synthetic-template.png");
        assert!(std::ptr::eq(first[0].1, second[0].1));
    }

    #[test]
    fn detect_frame_prepared_without_debug_paths_returns_null_outputs() {
        let root = Path::new(".");
        let mut frame = white_frame(4, 4);
        put_black_pixel(&mut frame, 1, 3);
        let detector = prepared_detector();
        let result = detect_frame_prepared(root, &frame, &detector, 0, None, None)
            .unwrap_or_else(|error| panic!("detect failed: {error}"));

        assert!(result.selected.is_some());
        assert!(result.crop.is_some());
        assert_eq!(result.trace_output_path, None);
        assert_eq!(result.crop_output_path, None);
        assert_eq!(result.thresholded_crop_output_path, None);
    }

    #[test]
    fn translates_roi_local_candidate_to_full_window_rect() {
        let candidate = MatchCandidate {
            template_name: "marker".to_string(),
            score: 0.0,
            rect: Rect {
                x: 3,
                y: 4,
                width: 5,
                height: 6,
            },
        };

        let translated = translate_candidate(candidate, 100, 200);

        assert_eq!(
            translated.rect,
            Rect {
                x: 103,
                y: 204,
                width: 5,
                height: 6,
            }
        );
    }

    #[test]
    fn synthetic_full_frame_and_roi_frame_return_same_logical_match() {
        let root = Path::new(".");
        let mut frame = white_frame(8, 8);
        let marker_x = 5usize;
        let marker_y = 6usize;
        let idx = (marker_y * 8 + marker_x) * 4;
        frame.pixels_bgra[idx..idx + 4].copy_from_slice(&[0, 0, 0, 255]);
        let detector = PreparedDetector {
            mode: InputMode::Mouse,
            scan_height_percent: 50.0,
            templates: vec![(
                "black-marker.png".to_string(),
                GrayImage {
                    width: 1,
                    height: 1,
                    pixels: vec![0],
                },
            )],
            ..prepared_detector()
        };

        let full = detect_frame_prepared(root, &frame, &detector, 5, None, None)
            .unwrap_or_else(|error| panic!("full detect failed: {error}"));
        let roi_rect = Rect {
            x: 2,
            y: 4,
            width: 6,
            height: 4,
        };
        let roi_capture =
            crop_frame(&frame, roi_rect).unwrap_or_else(|error| panic!("ROI crop failed: {error}"));
        let roi = d4lc_detect::prepared::detect_roi_frame_prepared(
            d4lc_detect::prepared::RoiFrame::window_relative_roi(
                &roi_capture,
                Point {
                    x: roi_rect.x,
                    y: roi_rect.y,
                },
                frame.width,
                frame.height,
                roi_rect,
                d4lc_detect::RoiRole::TooltipContent,
            ),
            &detector,
            5,
            None,
            None,
        )
        .unwrap_or_else(|error| panic!("roi detect failed: {error}"));

        let full_match = full
            .selected
            .unwrap_or_else(|| panic!("full-frame synthetic detection selected no match"));
        let roi_match = roi
            .selected
            .unwrap_or_else(|| panic!("ROI synthetic detection selected no match"));
        assert_eq!(full_match.template_name, roi_match.template_name);
        assert_eq!(full_match.rect, roi_match.rect);
    }
    #[test]
    fn upstream_tooltip_crop_uses_template_match_x_top_zero_tooltip_width_and_match_y() {
        let raw = raw_upstream_tooltip_crop_rect(
            Rect {
                x: 84,
                y: 960,
                width: 16,
                height: 12,
            },
            375,
        )
        .unwrap_or_else(|| panic!("missing raw crop rect"));
        let final_rect = clamp_upstream_tooltip_crop_rect(raw, 520, 998)
            .unwrap_or_else(|| panic!("missing final crop rect"));

        assert_eq!(raw.x, 84);
        assert_eq!(raw.y, 0);
        assert_eq!(raw.width, 375);
        assert_eq!(raw.height, 960);
        assert_eq!(final_rect, raw);
    }

    #[test]
    fn upstream_tooltip_crop_clamps_width_but_does_not_expand_marker_roi() {
        let raw = raw_upstream_tooltip_crop_rect(
            Rect {
                x: 430,
                y: 900,
                width: 20,
                height: 20,
            },
            375,
        )
        .unwrap_or_else(|| panic!("missing raw crop rect"));
        let final_rect = clamp_upstream_tooltip_crop_rect(raw, 520, 998)
            .unwrap_or_else(|| panic!("missing final crop rect"));

        assert_eq!(final_rect.width, 90);
        assert_eq!(final_rect.height, 900);
        assert_eq!(final_rect.x, 430);
    }

    #[test]
    fn trace_json_contains_broad_roi_and_final_crop_rect() {
        let trace = TooltipDetectionTrace {
            search_roi: Rect {
                x: 0,
                y: 0,
                width: 520,
                height: 998,
            },
            detected_tooltip_rect: Some(Rect {
                x: 64,
                y: 0,
                width: 375,
                height: 960,
            }),
            final_tooltip_crop_rect: Some(Rect {
                x: 64,
                y: 0,
                width: 375,
                height: 960,
            }),
            crop_source: "upstream_tooltip_template_match",
            upstream_rule: UPSTREAM_TOOLTIP_CROP_RULE,
            final_tooltip_size: Some((375, 960)),
        };

        let json = tooltip_detection_json(&trace);

        assert_eq!(json["search_roi"]["x"], 0);
        assert_eq!(json["final_tooltip_crop_rect"]["x"], 64);
        assert!(json["upstream_rule"]
            .as_str()
            .is_some_and(|rule| rule.contains("FindTooltip/FindTooltips")));
        assert_eq!(json["final_tooltip_size"]["width"], 375);
    }

    // ---- live-frame regression coverage ----
    //
    // Recorded frames under `tests/fixtures/live-frames/`. They pin the
    // upstream search contract: the marker scan covers the captured frame, so
    // the tooltip is found no matter where the cursor sits.

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn detect_synthetic_frame(
        roots: &Roots,
        frame: &CaptureFrame,
        mode: InputMode,
        mouse_x: i32,
    ) -> DetectFrameResult {
        // `Config/Controllers.json` is created at runtime, so the controller
        // tooltip template is passed explicitly instead of read from the root.
        let controller_config = ["tooltip_gc_x.png".to_string()];
        let (resolved, _) = resolve_detection_config(
            roots,
            Some("1080p_SMF".to_string()),
            Some("enUS".to_string()),
            &controller_config,
            Some(input_mode_label(mode)),
            None,
            None,
            None,
        )
        .unwrap_or_else(|error| panic!("detection config failed: {error}"));
        detect_frame(
            roots,
            frame,
            &DetectFrameArgs {
                preset: resolved.preset,
                language: resolved.language,
                controller_config: resolved.controller_config,
                mode: resolved.mode,
                mouse_x,
                scan_height_percent: resolved.scan_height_percent,
                thresholds: resolved.thresholds,
                trace_output_path: None,
                crop_output_path: None,
            },
        )
        .unwrap_or_else(|error| panic!("detection failed: {error}"))
    }

    fn detect_live_frame(file: &str, mode: InputMode, mouse_x: i32) -> DetectFrameResult {
        let root = workspace_root();
        let path = root.join("tests/fixtures/live-frames").join(file);
        let frame = load_capture_frame(&path)
            .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
        detect_synthetic_frame(&Roots::for_tests(&root), &frame, mode, mouse_x)
    }

    #[test]
    fn mouse_mode_detection_roi_spans_the_full_frame_for_every_cursor_quadrant() {
        let root = workspace_root();
        let frame = white_frame(240, 160);

        for mouse_x in [0, 60, 120, 180, 239] {
            let result =
                detect_synthetic_frame(&Roots::for_tests(&root), &frame, InputMode::Mouse, mouse_x);

            assert_eq!(result.mode, InputMode::Mouse);
            assert_eq!(result.roi_role, d4lc_detect::RoiRole::FullFrame);
            assert_eq!(
                result.roi,
                Rect {
                    x: 0,
                    y: 0,
                    width: 240,
                    height: 160,
                },
                "mouse ROI for cursor x={mouse_x} must be the captured frame"
            );
            assert_eq!(result.tooltip_detection.search_roi, result.roi);
        }
    }

    #[test]
    fn mouse_mode_detects_the_live_capture_tooltip_outside_the_cursor_neighbourhood() {
        // Recorded live frame: window-relative hover (397, 462). The
        // `tooltip_kb_all` marker sits at (1094, 843), 697 px to the right of
        // the cursor, i.e. outside a `TooltipWidth * 2.5` cursor crop.
        let result = detect_live_frame("d4lc-livecap.png", InputMode::Mouse, 397);

        assert_eq!(
            result.roi,
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }
        );
        let selected = result
            .selected
            .unwrap_or_else(|| panic!("live capture frame must detect its tooltip"));
        assert_eq!(selected.template_name, "tooltip_kb_all.png");
        assert_eq!(
            selected.rect,
            Rect {
                x: 1094,
                y: 843,
                width: 41,
                height: 22,
            }
        );
        assert_eq!(
            result.tooltip_detection.final_tooltip_crop_rect,
            Some(Rect {
                x: 1094,
                y: 0,
                width: 375,
                height: 843,
            })
        );
    }

    #[test]
    fn mouse_mode_detects_the_tooltip_of_the_keyboard_live_frame() {
        let result = detect_live_frame("frame-KB.png", InputMode::Mouse, 397);

        let selected = result
            .selected
            .unwrap_or_else(|| panic!("keyboard live frame must detect its tooltip"));
        assert_eq!(selected.template_name, "tooltip_kb_all.png");
        assert_eq!(
            selected.rect,
            Rect {
                x: 1038,
                y: 882,
                width: 41,
                height: 22,
            }
        );
        assert_eq!(
            result.tooltip_detection.final_tooltip_crop_rect,
            Some(Rect {
                x: 1038,
                y: 0,
                width: 375,
                height: 882,
            })
        );
    }

    #[test]
    fn controller_mode_detects_the_tooltip_of_the_controller_live_frame() {
        let result = detect_live_frame("frame-CTRL.png", InputMode::Controller, 0);

        assert_eq!(result.mode, InputMode::Controller);
        assert_eq!(result.roi_role, d4lc_detect::RoiRole::FullFrame);
        assert_eq!(
            result.roi,
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }
        );
        let selected = result
            .selected
            .unwrap_or_else(|| panic!("controller live frame must detect its tooltip"));
        assert_eq!(selected.template_name, "tooltip_gc_x.png");
        assert_eq!(
            selected.rect,
            Rect {
                x: 825,
                y: 869,
                width: 26,
                height: 22,
            }
        );
        assert_eq!(
            result.tooltip_detection.final_tooltip_crop_rect,
            Some(Rect {
                x: 825,
                y: 0,
                width: 375,
                height: 869,
            })
        );
    }

    /// `frame` embedded at (`left`, `top`) in a larger capture of opaque grey.
    fn padded_frame(frame: &CaptureFrame, left: u32, top: u32) -> CaptureFrame {
        let width = frame.width + left + 1;
        let height = frame.height + top + 1;
        let stride = width as usize * 4;
        let mut pixels_bgra = [90u8, 90, 90, 255].repeat((width * height) as usize);
        for row in 0..frame.height as usize {
            let src = row * frame.stride;
            let dst = (row + top as usize) * stride + left as usize * 4;
            let len = frame.width as usize * 4;
            pixels_bgra[dst..dst + len].copy_from_slice(&frame.pixels_bgra[src..src + len]);
        }
        CaptureFrame {
            width,
            height,
            stride,
            pixels_bgra,
        }
    }

    #[test]
    fn full_window_detection_without_the_roi_copy_matches_the_copied_path() {
        // A full-window frame is grayscaled in place; the same frame embedded
        // in a larger capture forces the ROI copy. Both must detect the same.
        let root = workspace_root();
        for mode in [InputMode::Mouse, InputMode::Controller] {
            let roots = Roots::for_tests(&root);
            let (resolved, _) = resolve_detection_config(
                &roots,
                Some("1080p_SMF".to_string()),
                Some("enUS".to_string()),
                &[],
                Some(input_mode_label(mode)),
                None,
                None,
                None,
            )
            .unwrap_or_else(|error| panic!("detection config failed: {error}"));
            let detector = prepare_detector(&roots, resolved)
                .unwrap_or_else(|error| panic!("detector failed: {error}"));
            for file in ["d4lc-livecap.png", "frame-CTRL.png", "frame-KB.png"] {
                let path = root.join("tests/fixtures/live-frames").join(file);
                let frame = load_capture_frame(&path)
                    .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
                let direct = d4lc_detect::prepared::detect_roi_frame_prepared(
                    RoiFrame::full_window(&frame),
                    &detector,
                    0,
                    None,
                    None,
                )
                .unwrap_or_else(|error| panic!("direct detect failed: {error}"));
                let padded = padded_frame(&frame, 2, 1);
                let copied = d4lc_detect::prepared::detect_roi_frame_prepared(
                    RoiFrame::window_relative_roi(
                        &padded,
                        Point { x: -2, y: -1 },
                        frame.width,
                        frame.height,
                        Rect {
                            x: 0,
                            y: 0,
                            width: frame.width,
                            height: frame.height,
                        },
                        d4lc_detect::RoiRole::FullFrame,
                    ),
                    &detector,
                    0,
                    None,
                    None,
                )
                .unwrap_or_else(|error| panic!("copied detect failed: {error}"));
                assert_eq!(direct.candidates, copied.candidates, "{file} {mode:?}");
                assert_eq!(direct.selected, copied.selected, "{file} {mode:?}");
                assert_eq!(direct.roi, copied.roi, "{file} {mode:?}");
                assert_eq!(
                    direct.planned_tooltip_roi, copied.planned_tooltip_roi,
                    "{file} {mode:?}"
                );
                assert_eq!(
                    direct.tooltip_detection, copied.tooltip_detection,
                    "{file} {mode:?}"
                );
                assert_eq!(direct.crop, copied.crop, "{file} {mode:?}");
            }
        }
    }
}
