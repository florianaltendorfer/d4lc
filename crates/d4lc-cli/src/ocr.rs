use crate::detect::load_capture_frame;
use crate::roots::{cli_overrides_json, Roots};
use crate::to_json_text;
use d4lc_core::OcrLine;
use d4lc_debug::write_debug_text;
use d4lc_ocr::{OcrBackendKind, OcrEngineRegistry, OcrMarker, OcrPageSegMode, OcrProfile};
#[cfg(feature = "leptess-backend")]
use d4lc_runtime::ocr_parity::{process_tooltip_ocr_parity_prepared, TooltipOcrParityResult};
use d4lc_runtime::ocr_parity::{
    DetectedMarkerPlan, PreparedTooltipOcr, SegmentResult, TooltipCropTrace,
    UPSTREAM_ASPECT_BOTTOM_RULE,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OcrArgs {
    pub input_path: Option<PathBuf>,
    pub preset: Option<String>,
    pub language: Option<String>,
    pub backend: Option<String>,
    pub psm: Option<String>,
    pub text_output_path: Option<PathBuf>,
    pub trace_output_path: Option<PathBuf>,
    pub tessdata_path: Option<PathBuf>,
}

pub fn read_tooltip(roots: &Roots, args: &OcrArgs) -> Result<String, String> {
    let input_path = args
        .input_path
        .as_ref()
        .ok_or_else(|| "read-tooltip requires --input <png>".to_string())?;
    let frame = load_capture_frame(&roots.work_path(input_path))?;
    let files = roots.config_files()?;
    let config = files.identity_json(cli_overrides_json(&[
        ("preset", json!(args.preset)),
        ("language", json!(args.language)),
        ("ocr", json!(args.backend)),
        ("tessdata", json!(args.tessdata_path)),
    ]));
    let settings = files.into_settings()?;
    let preset = args
        .preset
        .clone()
        .unwrap_or_else(|| settings.selected_system_preset.clone());
    let language = args
        .language
        .clone()
        .unwrap_or_else(|| settings.selected_affix_language.clone());
    let backend = parse_ocr_backend(args.backend.as_deref())?;
    let _requested_psm = parse_ocr_psm(args.psm.as_deref())?;
    let profile = OcrProfile {
        backend,
        language,
        tessdata_path: args
            .tessdata_path
            .as_deref()
            .map(|path| roots.work_path(path)),
    };
    let registry = create_ocr_registry(profile.resolved_backend())?;
    let execution = prepare_tooltip_ocr(roots, preset, profile)?.execute(&registry, &frame)?;

    if let Some(path) = &args.text_output_path {
        write_debug_text(
            &roots.work_path(path),
            &segments_text_output(&execution.results),
        )?;
    }
    if let Some(path) = &args.trace_output_path {
        write_debug_text(
            &roots.work_path(path),
            &ocr_trace_json(
                input_path,
                &execution.profile,
                &execution.preset,
                &execution.crop_trace,
                &execution.marker_plan,
                &execution.results,
            ),
        )?;
    }

    Ok(ocr_output_json(
        &config,
        input_path,
        &execution.profile,
        &execution.preset,
        &execution.crop_trace,
        &execution.marker_plan,
        &execution.results,
        args.text_output_path.as_deref(),
        args.trace_output_path.as_deref(),
    ))
}

/// The OCR session of the app's two roots: settings and extracted presets
/// from the config root, game data and vendored presets from the asset root.
fn prepare_tooltip_ocr(
    roots: &Roots,
    preset: String,
    profile: OcrProfile,
) -> Result<PreparedTooltipOcr, String> {
    PreparedTooltipOcr::prepare_with_game_data_root(
        roots.config_path()?,
        &roots.assets,
        preset,
        profile,
    )
}

#[cfg(feature = "leptess-backend")]
pub(crate) fn recognize_tooltip_frame_with_read_tooltip_parity(
    roots: &Roots,
    frame: d4lc_core::CaptureFrame,
    preset: String,
    profile: OcrProfile,
    registry: &OcrEngineRegistry,
    preview_chars: usize,
) -> Result<TooltipOcrParityResult, String> {
    use d4lc_core::coords::{FramePlacement, TooltipPlacement};
    process_tooltip_ocr_parity_prepared(
        &prepare_tooltip_ocr(roots, preset, profile)?,
        registry,
        &frame,
        preview_chars,
        &TooltipPlacement::whole_frame(FramePlacement::default(), frame.width, frame.height),
    )
}

pub(crate) fn parse_ocr_backend(value: Option<&str>) -> Result<OcrBackendKind, String> {
    match value {
        Some(value) => OcrBackendKind::parse(value).map_err(|error| error.to_string()),
        None => Ok(OcrBackendKind::Auto),
    }
}

pub(crate) fn parse_ocr_psm(value: Option<&str>) -> Result<OcrPageSegMode, String> {
    match value {
        Some(value) => OcrPageSegMode::parse(value).map_err(|error| error.to_string()),
        None => Ok(OcrPageSegMode::Auto),
    }
}

pub(crate) fn create_ocr_registry(backend: OcrBackendKind) -> Result<OcrEngineRegistry, String> {
    match backend {
        OcrBackendKind::Auto | OcrBackendKind::Leptess => create_leptess_registry(),
    }
}

#[cfg(feature = "leptess-backend")]
fn create_leptess_registry() -> Result<OcrEngineRegistry, String> {
    Ok(OcrEngineRegistry::new(|profile| {
        let engine = d4lc_ocr::LeptessOcrEngine::new(profile)?;
        Ok(Box::new(engine))
    }))
}

#[cfg(not(feature = "leptess-backend"))]
fn create_leptess_registry() -> Result<OcrEngineRegistry, String> {
    Err(d4lc_ocr::OcrError::BackendUnavailable(
        "leptess OCR backend is not compiled; rebuild with --features leptess-backend and install libtesseract-dev/libleptonica-dev"
            .to_string(),
    )
    .to_string())
}

fn segments_text_output(results: &[SegmentResult]) -> String {
    let mut output = String::new();
    for (index, result) in results.iter().enumerate() {
        if index > 0 {
            output.push_str("\n\n");
        }
        output.push_str(&format!(
            "[{}:{}]\n{}",
            result.plan.index, result.plan.kind, result.text.raw_text
        ));
    }
    output
}

#[allow(clippy::too_many_arguments)]
fn ocr_output_json(
    config: &Value,
    input_path: &Path,
    profile: &OcrProfile,
    preset: &str,
    crop_trace: &TooltipCropTrace,
    marker_plan: &DetectedMarkerPlan,
    results: &[SegmentResult],
    text_output_path: Option<&Path>,
    trace_output_path: Option<&Path>,
) -> String {
    let path = |path: Option<&Path>| path.map(|path| path.display().to_string());
    to_json_text(&json!({
        "schema_version": 1,
        "config": config,
        "input": input_path.display().to_string(),
        "preset": preset,
        "backend": profile.resolved_backend().to_string(),
        "language": profile.language,
        "tesseract_language": profile.tesseract_language().to_string(),
        "capture": tooltip_capture_json(crop_trace),
        "tooltip_detection": tooltip_detection_json(crop_trace),
        "markers": marker_plan_json(marker_plan),
        "splitter_detection": splitter_detection_json(marker_plan),
        "footer_filtering": footer_filtering_json(),
        "segment_count": results.len(),
        "line_count": total_line_count(results),
        "word_count": total_word_count(results),
        "segments": results.iter().map(segment_result_json).collect::<Vec<_>>(),
        "outputs": {
            "ocr_text": path(text_output_path),
            "ocr_trace": path(trace_output_path),
        },
    }))
}

fn ocr_trace_json(
    input_path: &Path,
    profile: &OcrProfile,
    preset: &str,
    crop_trace: &TooltipCropTrace,
    marker_plan: &DetectedMarkerPlan,
    results: &[SegmentResult],
) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "source": input_path.display().to_string(),
        "preset": preset,
        "backend": profile.resolved_backend().to_string(),
        "language": profile.language,
        "capture": tooltip_capture_json(crop_trace),
        "tooltip_detection": tooltip_detection_json(crop_trace),
        "markers": marker_plan_json(marker_plan),
        "splitter_detection": splitter_detection_json(marker_plan),
        "footer_filtering": footer_filtering_json(),
        "line_count": total_line_count(results),
        "word_count": total_word_count(results),
        "segments": results.iter().map(segment_result_json).collect::<Vec<_>>(),
    }))
}

fn size_json((width, height): (u32, u32)) -> Value {
    json!({ "width": width, "height": height })
}

fn tooltip_capture_json(trace: &TooltipCropTrace) -> Value {
    json!({
        "planned_tooltip_roi": trace.planned_tooltip_roi,
        "original_input_size": size_json(trace.original_input_size),
        "final_tooltip_size": size_json(trace.final_tooltip_size),
    })
}

fn tooltip_detection_json(trace: &TooltipCropTrace) -> Value {
    json!({
        "search_roi": trace.search_roi,
        "detected_tooltip_rect": trace.detected_tooltip_rect,
        "final_tooltip_crop_rect": trace.final_tooltip_crop_rect,
        "crop_source": trace.crop_source,
        "upstream_rule": trace.upstream_rule,
        "final_tooltip_size": size_json(trace.final_tooltip_size),
    })
}

fn splitter_detection_json(plan: &DetectedMarkerPlan) -> Value {
    let trace = &plan.splitter_detection;
    let aspect = plan.aspect_area_trace.as_ref();
    json!({
        "splitter_roi": trace.roi,
        "splitter_threshold": trace.threshold,
        "splitter_template_names": trace.template_names,
        "splitter_candidate_counts": trace
            .template_results
            .iter()
            .map(|result| json!({
                "name": result.name,
                "accepted_candidate_count": result.accepted_candidate_count,
            }))
            .collect::<Vec<_>>(),
        "accepted_splitter_matches": trace
            .accepted_matches
            .iter()
            .map(|matched| json!({ "name": matched.name, "rect": matched.rect, "score": matched.score }))
            .collect::<Vec<_>>(),
        "rejected_best_scores": trace
            .template_results
            .iter()
            .map(|result| json!({ "name": result.name, "score": result.rejected_best_score }))
            .collect::<Vec<_>>(),
        "splitters_before_cleanup": marker_list_json(&trace.splitters_before_cleanup),
        "splitters_after_cleanup": marker_list_json(&trace.splitters_after_cleanup),
        "top_splitter_cleanup_applied": trace.top_splitter_cleanup_applied,
        "removed_overlapping_splitters": marker_list_json(&trace.removed_overlapping_splitters),
        "aspect_area_split_points": aspect
            .map(|aspect| {
                aspect
                    .split_points
                    .iter()
                    .map(|point| json!({ "source": point.source.to_string(), "rect": point.rect }))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "aspect_selected_boundary_source": aspect.map(|aspect| aspect.selected_boundary_source.as_str()),
        "aspect_selected_boundary_rect": aspect.and_then(|aspect| aspect.selected_boundary_rect),
        "aspect_fallback_to_tooltip_bottom": aspect.is_some_and(|aspect| aspect.fallback_to_tooltip_bottom),
        "upstream_rules": {
            "splitter_detection": trace.upstream_rule,
            "aspect_bottom_selection": aspect
                .map(|aspect| aspect.upstream_rule)
                .unwrap_or(UPSTREAM_ASPECT_BOTTOM_RULE),
        },
    })
}

fn marker_plan_json(plan: &DetectedMarkerPlan) -> Value {
    let unique_aspects = plan
        .aspect_markers
        .iter()
        .filter(|marker| {
            let lower = marker.name.to_ascii_lowercase();
            lower.contains("_unique") || lower.contains("_mythic")
        })
        .cloned()
        .collect::<Vec<_>>();
    json!({
        "affix_marker_rects": marker_list_json(&plan.affix_markers),
        "aspect_marker_rects": marker_list_json(&plan.aspect_markers),
        "unique_aspect_marker_rects": marker_list_json(&unique_aspects),
        "selected_aspect_marker_rect": plan.aspect_marker.as_ref().map(|marker| marker.rect),
        "socket_marker_rects": plan.socket_markers,
        "splitter_marker_rects": marker_list_json(&plan.splitter_markers),
        "has_tooltip_top_splitter": plan.has_tooltip_top_splitter,
        "is_unique_item": plan.is_unique_item,
    })
}

fn marker_list_json(markers: &[OcrMarker]) -> Value {
    markers
        .iter()
        .map(|marker| {
            json!({ "name": marker.name, "kind": marker.kind.to_string(), "rect": marker.rect })
        })
        .collect()
}

fn footer_filtering_json() -> Value {
    json!({
        "applied": false,
        "upstream_rule": "OcrHandler.ConvertToText removes only right-aligned OCR lines by X \
             position; upstream has no footer/control hint blacklist for Equip, Hide Comparison, \
             or Scroll Down",
    })
}

fn segment_result_json(result: &SegmentResult) -> Value {
    let plan = &result.plan;
    json!({
        "index": plan.index,
        "segment_kind": plan.kind.to_string(),
        "source": plan.source,
        "rect": plan.rect,
        "final_rect_before_ocr": plan.rect,
        "source_marker_rect": plan.source_marker_rect,
        "boundary_source": plan.boundary_source,
        "boundary_rect": plan.boundary_rect,
        "upstream_rule": plan.upstream_rule,
        "marker_name": plan.marker_name,
        "marker_index": plan.marker_index,
        "psm": plan.page_seg_mode.to_string(),
        "conversion_method": result.conversion_method,
        "source_method": result.conversion_method,
        "conversion_branch": result.conversion_branch,
        "source_image_type": plan.source,
        "right_aligned_line_filtering_applied": result.right_aligned_line_filtering_applied,
        "type_area_offset_left": result.type_area_offset_left,
        "tooltip_width": plan.tooltip_width,
        "clamp_reason": plan.clamp_reason,
        "language": plan.language,
        "ocr_ms": result.ocr_ms,
        "line_count": result.text.lines.len(),
        "word_count": word_count(&result.text.lines),
        "bounding_box_source": result.text.bounding_box_source.as_str(),
        "raw_text": result.text.raw_text,
        "converted_text": result.converted_text,
        "lines": ocr_lines_json(&result.text.lines),
    })
}

fn ocr_lines_json(lines: &[OcrLine]) -> Value {
    lines
        .iter()
        .map(|line| {
            json!({
                "text": line.text,
                "rect": line.rect,
                "words": line
                    .words
                    .iter()
                    .map(|word| json!({ "text": word.text, "rect": word.rect }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn word_count(lines: &[OcrLine]) -> usize {
    lines.iter().map(|line| line.words.len()).sum()
}

fn total_line_count(results: &[SegmentResult]) -> usize {
    results.iter().map(|result| result.text.lines.len()).sum()
}

fn total_word_count(results: &[SegmentResult]) -> usize {
    results
        .iter()
        .map(|result| word_count(&result.text.lines))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use d4lc_core::{OcrText, OcrWord, Rect};
    use d4lc_ocr::OcrSegmentKind;
    use d4lc_ocr::OcrSegmentPlan;
    use d4lc_runtime::ocr_parity::OcrInputTrace;
    use d4lc_runtime::ocr_parity::SplitterDetectionTrace;
    use d4lc_runtime::ocr_parity::TooltipOcrPreprocessVariant;
    use d4lc_runtime::ocr_parity::UPSTREAM_SPLITTER_DETECTION_RULE;
    use d4lc_runtime::ocr_parity::{
        AspectAreaTrace, AspectSplitPointTrace, SplitterMatchTrace, SplitterTemplateTrace,
    };

    /// The OCR input trace of the synthetic segment images below.
    fn test_ocr_input() -> OcrInputTrace {
        OcrInputTrace {
            variant: TooltipOcrPreprocessVariant::RawBgra,
            payload: "bgra",
            width: 461,
            height: 88,
            digest: 0,
        }
    }

    fn splitter_marker(name: &str, y: i32) -> OcrMarker {
        OcrMarker {
            kind: OcrSegmentKind::Splitter,
            name: name.to_string(),
            rect: Rect {
                x: 10,
                y,
                width: 8,
                height: 3,
            },
        }
    }

    #[test]
    fn parses_default_ocr_backend_and_psm() {
        assert_eq!(parse_ocr_backend(None), Ok(OcrBackendKind::Auto));
        assert_eq!(parse_ocr_psm(None), Ok(OcrPageSegMode::Auto));
    }

    #[test]
    fn ocr_lines_json_contains_text_rect_and_words() {
        let json = ocr_lines_json(&[OcrLine {
            text: "Legendary Gloves".to_string(),
            rect: Rect {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            },
            words: vec![OcrWord {
                text: "Legendary".to_string(),
                rect: Rect {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4,
                },
            }],
        }]);

        assert_eq!(json[0]["text"], "Legendary Gloves");
        assert_eq!(json[0]["rect"]["width"], 3);
        assert_eq!(json[0]["words"][0]["text"], "Legendary");
    }

    #[test]
    fn footer_control_text_filtering_trace_stays_disabled() {
        let json = footer_filtering_json();
        assert_eq!(json["applied"], false);
        let json = json.to_string();
        assert!(json.contains("Equip"));
        assert!(json.contains("Hide Comparison"));
        assert!(json.contains("no footer/control hint blacklist"));
    }

    #[test]
    fn splitter_detection_trace_exposes_required_fields() {
        let marker_plan = DetectedMarkerPlan {
            affix_markers: Vec::new(),
            aspect_markers: Vec::new(),
            aspect_marker: Some(OcrMarker {
                kind: OcrSegmentKind::Aspect,
                name: "dot-aspects_legendary.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 240,
                    width: 12,
                    height: 12,
                },
            }),
            socket_markers: Vec::new(),
            splitter_markers: vec![splitter_marker("dot-splitter_1.png", 300)],
            splitter_detection: SplitterDetectionTrace {
                roi: Rect {
                    x: 161,
                    y: 0,
                    width: 53,
                    height: 749,
                },
                threshold: 0.05,
                template_names: vec!["dot-splitter_1.png".to_string()],
                template_results: vec![SplitterTemplateTrace {
                    name: "dot-splitter_1.png".to_string(),
                    accepted_candidate_count: 1,
                    rejected_best_score: Some(0.5),
                }],
                accepted_matches: vec![SplitterMatchTrace {
                    name: "dot-splitter_1.png".to_string(),
                    rect: Rect {
                        x: 161,
                        y: 300,
                        width: 8,
                        height: 3,
                    },
                    score: 0.0,
                }],
                splitters_before_cleanup: vec![splitter_marker("dot-splitter_1.png", 300)],
                splitters_after_cleanup: vec![splitter_marker("dot-splitter_1.png", 300)],
                top_splitter_cleanup_applied: false,
                removed_overlapping_splitters: Vec::new(),
                upstream_rule: UPSTREAM_SPLITTER_DETECTION_RULE,
            },
            splitter_detection_ms: 0,
            aspect_area_trace: Some(AspectAreaTrace {
                split_points: vec![AspectSplitPointTrace {
                    source: "splitter_marker",
                    rect: Rect {
                        x: 10,
                        y: 300,
                        width: 8,
                        height: 3,
                    },
                }],
                selected_boundary_source: "splitter_marker".to_string(),
                selected_boundary_rect: Some(Rect {
                    x: 10,
                    y: 300,
                    width: 8,
                    height: 3,
                }),
                fallback_to_tooltip_bottom: false,
                upstream_rule: UPSTREAM_ASPECT_BOTTOM_RULE,
            }),
            is_unique_item: false,
            has_tooltip_top_splitter: false,
        };

        let json = splitter_detection_json(&marker_plan).to_string();

        assert!(json.contains("splitter_roi"));
        assert!(json.contains("splitter_threshold"));
        assert!(json.contains("splitter_template_names"));
        assert!(json.contains("splitter_candidate_counts"));
        assert!(json.contains("accepted_splitter_matches"));
        assert!(json.contains("rejected_best_scores"));
        assert!(json.contains("splitters_before_cleanup"));
        assert!(json.contains("splitters_after_cleanup"));
        assert!(json.contains("top_splitter_cleanup_applied"));
        assert!(json.contains("removed_overlapping_splitters"));
        assert!(json.contains("aspect_area_split_points"));
        assert!(json.contains("aspect_selected_boundary_source"));
        assert!(json.contains("aspect_selected_boundary_rect"));
        assert!(json.contains("aspect_fallback_to_tooltip_bottom"));
        assert!(json.contains("FindItemSplitterLocations"));
        assert!(json.contains("FindItemAspectAreas"));
    }

    #[test]
    fn segment_results_json_exposes_boundary_source_and_upstream_rule() {
        let source_marker = Rect {
            x: 5,
            y: 820,
            width: 12,
            height: 12,
        };
        let result = SegmentResult {
            plan: OcrSegmentPlan {
                index: 0,
                kind: OcrSegmentKind::Aspect,
                source: "tooltip_filter".to_string(),
                rect: Rect {
                    x: 19,
                    y: 812,
                    width: 461,
                    height: 88,
                },
                source_marker_rect: Some(source_marker),
                boundary_source: "tooltip_bottom".to_string(),
                boundary_rect: None,
                upstream_rule: "ScreenProcessHandler.FindItemAspectAreas: no later split point uses tooltip image bottom".to_string(),
                marker_name: Some("dot-aspects_legendary.png".to_string()),
                marker_index: Some(0),
                page_seg_mode: OcrPageSegMode::Default,
                language: "enUS".to_string(),
                tooltip_width: 0,
                clamp_reason: None,
            },
            text: OcrText {
                raw_text: "Aspect text\n@ Scroll Down".to_string(),
                lines: Vec::new(),
                bounding_box_source: d4lc_core::OcrBoundingBoxSource::Unavailable,
            },
            tsv: None,
            ocr_input: test_ocr_input(),
            converted_text: "Aspect text @ Scroll Down".to_string(),
            conversion_method: "OcrHandler.ConvertToAspect",
            conversion_branch: "fixed_segment_route",
            right_aligned_line_filtering_applied: true,
            type_area_offset_left: 0,
            ocr_ms: 0,
            conversion_ms: 0,
        };

        let json = segment_result_json(&result);

        assert_eq!(json["final_rect_before_ocr"]["x"], 19);
        assert_eq!(json["source_marker_rect"]["x"], 5);
        assert_eq!(json["boundary_source"], "tooltip_bottom");
        assert!(json["upstream_rule"]
            .as_str()
            .is_some_and(|rule| rule.contains("FindItemAspectAreas")));
        assert_eq!(json["conversion_method"], "OcrHandler.ConvertToAspect");
        assert_eq!(json["source_method"], "OcrHandler.ConvertToAspect");
        assert_eq!(json["conversion_branch"], "fixed_segment_route");
        assert_eq!(json["word_count"], 0);
        assert_eq!(json["bounding_box_source"], "unavailable");
        assert_eq!(json["source_image_type"], "tooltip_filter");
        assert_eq!(json["converted_text"], "Aspect text @ Scroll Down");
    }

    #[test]
    fn segment_results_json_records_multiline_affix_branch() {
        let result = SegmentResult {
            plan: OcrSegmentPlan {
                index: 0,
                kind: OcrSegmentKind::Affix,
                source: "tooltip_filter".to_string(),
                rect: Rect {
                    x: 39,
                    y: 350,
                    width: 318,
                    height: 54,
                },
                source_marker_rect: None,
                boundary_source: "socket_marker".to_string(),
                boundary_rect: None,
                upstream_rule: "ScreenProcessHandler.FindItemAffixAreas".to_string(),
                marker_name: Some("dot-affixes_normal.png".to_string()),
                marker_index: Some(0),
                page_seg_mode: OcrPageSegMode::Default,
                language: "enUS".to_string(),
                tooltip_width: 0,
                clamp_reason: None,
            },
            text: OcrText {
                raw_text: "Damage\nWhile Healthy".to_string(),
                lines: Vec::new(),
                bounding_box_source: d4lc_core::OcrBoundingBoxSource::Unavailable,
            },
            tsv: None,
            ocr_input: test_ocr_input(),
            converted_text: "Damage While Healthy".to_string(),
            conversion_method: "OcrHandler.ConvertToAffixMultiline",
            conversion_branch: "socket_detection_disabled",
            right_aligned_line_filtering_applied: true,
            type_area_offset_left: 0,
            ocr_ms: 0,
            conversion_ms: 0,
        };

        let json = segment_result_json(&result);

        assert_eq!(json["source_method"], "OcrHandler.ConvertToAffixMultiline");
        assert_eq!(json["conversion_branch"], "socket_detection_disabled");
    }
}
