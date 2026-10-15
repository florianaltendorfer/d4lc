//! Upstream tooltip crop, marker, socket and splitter detection on the
//! thresholded tooltip.

use std::time::Instant;

use d4lc_assets::TemplateKind;
use d4lc_capture::crop_frame;
use d4lc_core::{CaptureFrame, GrayImage, MatchCandidate, Rect};
use d4lc_detect::prepared::{clamp_upstream_tooltip_crop_rect, raw_upstream_tooltip_crop_rect};
use d4lc_ocr::{OcrMarker, OcrSegmentKind};
use d4lc_vision::{
    match_template_sqdiff_normed, match_template_sqdiff_normed_below, threshold_binary_inv,
    to_grayscale,
};

use super::parallel::{elapsed_ms, join_scoped, parallel_map};
use super::templates::{MarkerTemplate, TooltipTemplate};
use super::types::{
    AspectAreaTrace, AspectSplitPointTrace, DetectedMarkerPlan, SplitterDetectionTrace,
    SplitterMatchTrace, SplitterTemplateTrace, TooltipCropTrace,
};
use super::{
    UPSTREAM_ASPECT_BOTTOM_RULE, UPSTREAM_SPLITTER_DETECTION_RULE, UPSTREAM_TOOLTIP_CROP_RULE,
};

// ---------------------------------------------------------------------------
// Tooltip OCR input preparation
// ---------------------------------------------------------------------------

pub(crate) fn prepare_tooltip_ocr_input(
    frame: &CaptureFrame,
    tooltip_templates: &[TooltipTemplate],
    tooltip_width: i32,
    threshold_similarity_tooltip: f64,
) -> Result<(CaptureFrame, TooltipCropTrace), String> {
    let planned_tooltip_roi = input_rect(frame);
    let search_roi = planned_tooltip_roi;
    let original_input_size = (frame.width, frame.height);

    if let Some((_candidate, detected_rect, final_rect)) = detect_final_tooltip_crop(
        frame,
        tooltip_templates,
        tooltip_width,
        threshold_similarity_tooltip,
    ) {
        let final_frame = crop_frame(frame, final_rect)?;
        return Ok((
            final_frame,
            TooltipCropTrace {
                planned_tooltip_roi,
                search_roi,
                detected_tooltip_rect: Some(detected_rect),
                final_tooltip_crop_rect: final_rect,
                crop_source: "upstream_tooltip_template_match",
                upstream_rule: UPSTREAM_TOOLTIP_CROP_RULE,
                original_input_size,
                final_tooltip_size: (final_rect.width, final_rect.height),
            },
        ));
    }

    Ok((
        frame.clone(),
        TooltipCropTrace {
            planned_tooltip_roi,
            search_roi,
            detected_tooltip_rect: None,
            final_tooltip_crop_rect: planned_tooltip_roi,
            crop_source: "input_already_final_or_no_template_match",
            upstream_rule: UPSTREAM_TOOLTIP_CROP_RULE,
            original_input_size,
            final_tooltip_size: (frame.width, frame.height),
        },
    ))
}

// ---------------------------------------------------------------------------
// Tooltip crop detection
// ---------------------------------------------------------------------------

pub(crate) fn detect_final_tooltip_crop(
    frame: &CaptureFrame,
    tooltip_templates: &[TooltipTemplate],
    tooltip_width: i32,
    threshold_similarity_tooltip: f64,
) -> Option<(MatchCandidate, Rect, Rect)> {
    let gray = to_grayscale(frame);
    let matches = parallel_map(tooltip_templates, usize::MAX, |template| {
        match_template_sqdiff_normed_below(&gray, &template.image, threshold_similarity_tooltip)
    });
    let mut best: Option<MatchCandidate> = None;
    for (template, result) in tooltip_templates.iter().zip(matches) {
        let Some(result) = result else {
            continue;
        };
        let candidate = MatchCandidate {
            template_name: template.name.clone(),
            score: result.score,
            rect: Rect {
                x: result.x,
                y: result.y,
                width: template.image.width,
                height: template.image.height,
            },
        };
        let replace = best
            .as_ref()
            .map(|current| candidate.score < current.score)
            .unwrap_or(true);
        if replace {
            best = Some(candidate);
        }
    }

    let candidate = best?;
    let detected_rect = raw_upstream_tooltip_crop_rect(candidate.rect, tooltip_width)?;
    let final_rect = clamp_upstream_tooltip_crop_rect(detected_rect, frame.width, frame.height)?;
    Some((candidate, detected_rect, final_rect))
}

pub(crate) fn input_rect(frame: &CaptureFrame) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    }
}

// ---------------------------------------------------------------------------
// Upstream marker detection
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn detect_upstream_markers(
    frame: &CaptureFrame,
    templates: &[MarkerTemplate],
    threshold_min: u8,
    threshold_max: u8,
    settings: &d4lc_config::AppSettings,
) -> Result<DetectedMarkerPlan, String> {
    detect_upstream_markers_notifying(
        frame,
        templates,
        threshold_min,
        threshold_max,
        settings,
        |_| {},
    )
}

/// `detect_upstream_markers` that hands the splitter markers to
/// `on_splitters` as soon as they are found, while the affix, aspect and
/// socket searches still run. Nothing is handed over when the splitter
/// search fails.
pub(crate) fn detect_upstream_markers_notifying(
    frame: &CaptureFrame,
    templates: &[MarkerTemplate],
    threshold_min: u8,
    threshold_max: u8,
    settings: &d4lc_config::AppSettings,
    on_splitters: impl FnOnce(&[OcrMarker]),
) -> Result<DetectedMarkerPlan, String> {
    let gray = to_grayscale(frame);
    let tooltip_filter = threshold_binary_inv(&gray, threshold_min, threshold_max);
    let affix_templates = templates_of_kind(templates, TemplateKind::AffixMarker);
    let aspect_templates = templates_of_kind(templates, TemplateKind::AspectMarker);
    let socket_templates = templates_of_kind(templates, TemplateKind::SocketMarker);
    let splitter_templates = templates_of_kind(templates, TemplateKind::SplitterMarker);

    let left_marker_width = (tooltip_filter.width / 7).max(1);
    let affix_roi = crop_gray(
        &tooltip_filter,
        Rect {
            x: 0,
            y: 0,
            width: left_marker_width,
            height: tooltip_filter.height,
        },
    )?;
    let aspect_roi = crop_gray(
        &tooltip_filter,
        Rect {
            x: 0,
            y: 0,
            width: left_marker_width,
            height: tooltip_filter.height,
        },
    )?;
    // The affix, aspect and splitter searches read the same filtered
    // tooltip and nothing of each other (upstream runs its location
    // searches in Parallel.ForEach), so they run at once.
    let (mut affix_markers, mut aspect_markers, splitter_search) = std::thread::scope(|scope| {
        let affix = scope.spawn(|| {
            find_marker_locations(
                &affix_roi,
                affix_templates,
                settings.threshold_similarity_affix_location,
                Rect::default(),
                OcrSegmentKind::Affix,
                20,
            )
        });
        let aspect = scope.spawn(|| {
            find_marker_locations(
                &aspect_roi,
                aspect_templates,
                settings.threshold_similarity_aspect_location,
                Rect::default(),
                OcrSegmentKind::Aspect,
                20,
            )
        });
        let splitter_start = Instant::now();
        let splitter = detect_splitter_markers(
            &tooltip_filter,
            splitter_templates,
            settings.threshold_similarity_splitter_location,
        )
        .map(|found| (found, elapsed_ms(splitter_start)));
        if let Ok(((markers, _), _)) = &splitter {
            on_splitters(markers);
        }
        (join_scoped(affix), join_scoped(aspect), splitter)
    });
    let ((splitter_markers, splitter_detection), splitter_detection_ms) = splitter_search?;
    affix_markers.sort_by_key(|left| left.rect.y);
    aspect_markers.sort_by_key(|left| left.rect.y);
    let aspect_marker = aspect_markers.first().cloned();
    let is_unique_item = aspect_marker
        .as_ref()
        .map(|marker| {
            let lower = marker.name.to_ascii_lowercase();
            lower.contains("_unique") || lower.contains("_mythic")
        })
        .unwrap_or(false);

    let has_tooltip_top_splitter = splitter_markers.iter().any(is_top_splitter_marker);

    let socket_offset_y = aspect_marker
        .as_ref()
        .map(|marker| marker.rect.y)
        .or_else(|| affix_markers.last().map(|marker| marker.rect.y))
        .unwrap_or(0)
        .max(0);
    let mut socket_markers = if settings.is_socket_detection_enabled
        && u32::try_from(socket_offset_y).ok().unwrap_or(u32::MAX) < tooltip_filter.height
    {
        let socket_y = u32::try_from(socket_offset_y).map_err(|error| error.to_string())?;
        let socket_roi = crop_gray(
            &tooltip_filter,
            Rect {
                x: 0,
                y: socket_offset_y,
                width: (tooltip_filter.width / 5).max(1),
                height: tooltip_filter.height.saturating_sub(socket_y),
            },
        )?;
        find_socket_locations(
            &socket_roi,
            socket_templates,
            settings.threshold_similarity_socket_location,
        )
    } else {
        Vec::new()
    };
    socket_markers.sort_by_key(|left| left.y);
    remove_invalid_socket_locations(&mut socket_markers, &affix_markers, socket_offset_y);
    remove_invalid_affix_locations(
        &mut affix_markers,
        aspect_marker.as_ref(),
        &socket_markers,
        socket_offset_y,
    );
    let aspect_area_trace = aspect_marker.as_ref().map(|marker| {
        trace_aspect_area_split_points(&affix_markers, marker, &socket_markers, &splitter_markers)
    });

    Ok(DetectedMarkerPlan {
        affix_markers,
        aspect_markers,
        aspect_marker,
        socket_markers,
        splitter_markers,
        splitter_detection,
        splitter_detection_ms,
        aspect_area_trace,
        is_unique_item,
        has_tooltip_top_splitter,
    })
}

pub(crate) fn templates_of_kind(
    templates: &[MarkerTemplate],
    kind: TemplateKind,
) -> &[MarkerTemplate] {
    let start = templates
        .iter()
        .position(|template| template.kind == kind)
        .unwrap_or(templates.len());
    let end = templates[start..]
        .iter()
        .position(|template| template.kind != kind)
        .map(|offset| start + offset)
        .unwrap_or(templates.len());
    &templates[start..end]
}

pub(crate) fn find_marker_locations(
    roi: &GrayImage,
    templates: &[MarkerTemplate],
    max_score: f64,
    origin: Rect,
    kind: OcrSegmentKind,
    max_matches_per_template: usize,
) -> Vec<OcrMarker> {
    parallel_map(templates, usize::MAX, |template| {
        let mut markers = Vec::new();
        let mut search = roi.clone();
        for _ in 0..max_matches_per_template {
            let Some(result) =
                match_template_sqdiff_normed_below(&search, &template.image, max_score)
            else {
                break;
            };
            let rect = Rect {
                x: result.x.saturating_add(origin.x),
                y: result.y.saturating_add(origin.y),
                width: template.image.width,
                height: template.image.height,
            };
            markers.push(OcrMarker {
                kind,
                name: template.name.clone(),
                rect,
            });
            // Upstream paints the found rectangle white (255) so it is only
            // detected once (SPH.cs:852, 1304).
            mask_gray_rect_white(
                &mut search,
                Rect {
                    x: result.x,
                    y: result.y,
                    width: template.image.width,
                    height: template.image.height,
                },
            );
        }
        markers
    })
    .into_iter()
    .flatten()
    .collect()
}

/// Upstream `FindItemSocketLocations`/`FindItemSocketLocation`
/// (ScreenProcessHandler.cs:1501-1615): every `dot-socket_` template whose
/// name does not contain `mask` is searched with its `<name>_mask.png`
/// partner through the masked SQDIFF_NORMED match, at most twice, painting
/// each found rectangle white before the next search. A template without its
/// mask finds nothing (upstream returns before matching). The masks are
/// binarized like every socket template (SPH.cs:264), so the masked match
/// compares only the socket ring — a socket holding a gem is found too.
/// Rectangles are in `roi` coordinates.
pub(crate) fn find_socket_locations(
    roi: &GrayImage,
    templates: &[MarkerTemplate],
    max_score: f64,
) -> Vec<Rect> {
    let searchable: Vec<&MarkerTemplate> = templates
        .iter()
        .filter(|template| !template.name.to_ascii_lowercase().contains("mask"))
        .collect();
    parallel_map(&searchable, usize::MAX, |template| {
        let name = template.name.to_ascii_lowercase();
        let stem = name.strip_suffix(".png").unwrap_or(&name);
        let mask_name = format!("{stem}_mask.png");
        let Some(mask) = templates
            .iter()
            .find(|candidate| candidate.name.eq_ignore_ascii_case(&mask_name))
        else {
            return Vec::new();
        };
        let mut search = roi.clone();
        let mut found = Vec::new();
        for _ in 0..2 {
            let Some(result) = d4lc_vision::match_template_sqdiff_normed_masked(
                &search,
                &template.image,
                &mask.image,
            ) else {
                break;
            };
            let rect = Rect {
                x: result.x,
                y: result.y,
                width: template.image.width,
                height: template.image.height,
            };
            mask_gray_rect_white(&mut search, rect);
            if result.score >= max_score {
                break;
            }
            found.push(rect);
        }
        found
    })
    .into_iter()
    .flatten()
    .collect()
}

// ---------------------------------------------------------------------------
// Splitter detection
// ---------------------------------------------------------------------------

pub(crate) fn splitter_roi_rect(tooltip_width: u32, tooltip_height: u32) -> Rect {
    let roi_width = (tooltip_width / 7).max(1);
    let roi_start_x = i32::try_from(tooltip_width / 2)
        .unwrap_or(i32::MAX)
        .saturating_sub(i32::try_from(roi_width / 2).unwrap_or(0));
    Rect {
        x: roi_start_x,
        y: 0,
        width: roi_width,
        height: tooltip_height,
    }
}

pub(crate) fn detect_splitter_markers(
    tooltip_filter: &GrayImage,
    templates: &[MarkerTemplate],
    threshold: f64,
) -> Result<(Vec<OcrMarker>, SplitterDetectionTrace), String> {
    let roi_rect = splitter_roi_rect(tooltip_filter.width, tooltip_filter.height);
    let roi = crop_gray(tooltip_filter, roi_rect)?;
    let mut markers = Vec::new();
    let mut accepted_matches = Vec::new();
    let mut template_results = Vec::new();
    let template_names = templates
        .iter()
        .map(|template| template.name.clone())
        .collect::<Vec<_>>();

    let per_template = parallel_map(templates, usize::MAX, |template| {
        let mut markers = Vec::new();
        let mut accepted_matches = Vec::new();
        let mut search = roi.clone();
        let mut accepted_candidate_count = 0usize;
        let mut rejected_best_score = None;

        for _ in 0..20 {
            let Some(result) = match_template_sqdiff_normed(&search, &template.image) else {
                break;
            };
            if result.score >= threshold {
                rejected_best_score = Some(result.score);
                break;
            }

            let rect = Rect {
                x: result.x.saturating_add(roi_rect.x),
                y: result.y.saturating_add(roi_rect.y),
                width: template.image.width,
                height: template.image.height,
            };
            let marker = OcrMarker {
                kind: OcrSegmentKind::Splitter,
                name: template.name.clone(),
                rect,
            };
            markers.push(marker);
            accepted_matches.push(SplitterMatchTrace {
                name: template.name.clone(),
                rect,
                score: result.score,
            });
            accepted_candidate_count += 1;

            mask_gray_rect_white(
                &mut search,
                Rect {
                    x: result.x,
                    y: result.y,
                    width: template.image.width,
                    height: template.image.height,
                },
            );
        }

        let template_result = SplitterTemplateTrace {
            name: template.name.clone(),
            accepted_candidate_count,
            rejected_best_score,
        };
        (markers, accepted_matches, template_result)
    });
    for (template_markers, template_matches, template_result) in per_template {
        markers.extend(template_markers);
        accepted_matches.extend(template_matches);
        template_results.push(template_result);
    }

    markers.sort_by_key(|left| left.rect.y);
    let splitters_before_cleanup = markers.clone();
    let (top_splitter_cleanup_applied, removed_overlapping_splitters) =
        remove_overlapping_non_top_splitters(&mut markers);
    let splitters_after_cleanup = markers.clone();

    Ok((
        markers,
        SplitterDetectionTrace {
            roi: roi_rect,
            threshold,
            template_names,
            template_results,
            accepted_matches,
            splitters_before_cleanup,
            splitters_after_cleanup,
            top_splitter_cleanup_applied,
            removed_overlapping_splitters,
            upstream_rule: UPSTREAM_SPLITTER_DETECTION_RULE,
        },
    ))
}

pub(crate) fn is_top_splitter_marker(marker: &OcrMarker) -> bool {
    marker
        .name
        .to_ascii_lowercase()
        .contains("dot-splitter_top")
}

pub(crate) fn remove_overlapping_non_top_splitters(
    splitters: &mut Vec<OcrMarker>,
) -> (bool, Vec<OcrMarker>) {
    let Some(top_y) = splitters
        .iter()
        .find(|marker| is_top_splitter_marker(marker))
        .map(|marker| marker.rect.y)
    else {
        return (false, Vec::new());
    };

    let mut removed = Vec::new();
    splitters.retain(|marker| {
        let remove = !is_top_splitter_marker(marker)
            && marker.rect.y.saturating_sub(2) <= top_y
            && top_y <= marker.rect.y.saturating_add(2);
        if remove {
            removed.push(marker.clone());
        }
        !remove
    });
    (true, removed)
}

// ---------------------------------------------------------------------------
// Aspect area split-point tracing
// ---------------------------------------------------------------------------

pub(crate) fn trace_aspect_area_split_points(
    affix_markers: &[OcrMarker],
    aspect_marker: &OcrMarker,
    socket_markers: &[Rect],
    splitter_markers: &[OcrMarker],
) -> AspectAreaTrace {
    let mut split_points = Vec::new();
    split_points.extend(affix_markers.iter().map(|marker| AspectSplitPointTrace {
        source: "affix_marker",
        rect: marker.rect,
    }));
    split_points.push(AspectSplitPointTrace {
        source: "aspect_marker",
        rect: aspect_marker.rect,
    });
    if let Some(socket) = socket_markers.first().copied() {
        split_points.push(AspectSplitPointTrace {
            source: "socket_marker",
            rect: Rect {
                y: socket.y.saturating_add(aspect_marker.rect.y),
                ..socket
            },
        });
    }
    split_points.extend(splitter_markers.iter().map(|marker| AspectSplitPointTrace {
        source: "splitter_marker",
        rect: marker.rect,
    }));
    split_points.sort_by_key(|left| left.rect.y);

    let selected = split_points
        .iter()
        .find(|point| point.rect.y > aspect_marker.rect.y)
        .cloned();
    let fallback_to_tooltip_bottom = selected.is_none();
    let selected_boundary_source = selected
        .as_ref()
        .map(|point| point.source.to_string())
        .unwrap_or_else(|| "tooltip_bottom".to_string());
    let selected_boundary_rect = selected.as_ref().map(|point| point.rect);
    AspectAreaTrace {
        split_points,
        selected_boundary_source,
        selected_boundary_rect,
        fallback_to_tooltip_bottom,
        upstream_rule: UPSTREAM_ASPECT_BOTTOM_RULE,
    }
}

// ---------------------------------------------------------------------------
// Validation helpers for marker locations
// ---------------------------------------------------------------------------

pub(crate) fn remove_invalid_socket_locations(
    sockets: &mut Vec<Rect>,
    affixes: &[OcrMarker],
    socket_offset_y: i32,
) {
    let Some(first_affix_y) = affixes.first().map(|marker| marker.rect.y) else {
        return;
    };
    sockets.retain(|socket| socket.y.saturating_add(socket_offset_y) > first_affix_y);
}

pub(crate) fn remove_invalid_affix_locations(
    affixes: &mut Vec<OcrMarker>,
    aspect_marker: Option<&OcrMarker>,
    sockets: &[Rect],
    socket_offset_y: i32,
) {
    if let Some(aspect_marker) = aspect_marker {
        affixes.retain(|affix| affix.rect.y < aspect_marker.rect.y);
        return;
    }
    if let Some(socket) = sockets.first() {
        let socket_y = socket.y.saturating_add(socket_offset_y);
        affixes.retain(|affix| affix.rect.y < socket_y);
    }
}

// ---------------------------------------------------------------------------
// Image helpers
// ---------------------------------------------------------------------------

pub(crate) fn crop_gray(image: &GrayImage, rect: Rect) -> Result<GrayImage, String> {
    if rect.x < 0 || rect.y < 0 {
        return Err("gray crop cannot start outside the image".to_string());
    }
    let start_x = u32::try_from(rect.x).map_err(|error| error.to_string())?;
    let start_y = u32::try_from(rect.y).map_err(|error| error.to_string())?;
    if start_x.saturating_add(rect.width) > image.width
        || start_y.saturating_add(rect.height) > image.height
    {
        return Err(format!(
            "gray crop {:?} exceeds image {}x{}",
            rect, image.width, image.height
        ));
    }
    let width = usize::try_from(rect.width).map_err(|error| error.to_string())?;
    let height = usize::try_from(rect.height).map_err(|error| error.to_string())?;
    let image_width = usize::try_from(image.width).map_err(|error| error.to_string())?;
    let mut pixels = Vec::with_capacity(
        width
            .checked_mul(height)
            .ok_or_else(|| "gray crop capacity overflow".to_string())?,
    );
    for y in start_y..start_y + rect.height {
        let row = usize::try_from(y)
            .map_err(|error| error.to_string())?
            .checked_mul(image_width)
            .ok_or_else(|| "gray crop row overflow".to_string())?;
        for x in start_x..start_x + rect.width {
            pixels.push(image.pixels[row + usize::try_from(x).map_err(|error| error.to_string())?]);
        }
    }
    Ok(GrayImage {
        width: rect.width,
        height: rect.height,
        pixels,
    })
}

pub(crate) fn mask_gray_rect_white(image: &mut GrayImage, rect: Rect) {
    mask_gray_rect_with_value(image, rect, 255);
}

pub(crate) fn mask_gray_rect_with_value(image: &mut GrayImage, rect: Rect, value: u8) {
    if rect.x < 0 || rect.y < 0 {
        return;
    }
    let Ok(start_x) = u32::try_from(rect.x) else {
        return;
    };
    let Ok(start_y) = u32::try_from(rect.y) else {
        return;
    };
    let end_x = start_x.saturating_add(rect.width).min(image.width);
    let end_y = start_y.saturating_add(rect.height).min(image.height);
    let Ok(image_width) = usize::try_from(image.width) else {
        return;
    };
    for y in start_y..end_y {
        let Ok(row_y) = usize::try_from(y) else {
            return;
        };
        let row = row_y.saturating_mul(image_width);
        for x in start_x..end_x {
            let Ok(x_index) = usize::try_from(x) else {
                return;
            };
            image.pixels[row + x_index] = value;
        }
    }
}

#[cfg(test)]
mod tests;
