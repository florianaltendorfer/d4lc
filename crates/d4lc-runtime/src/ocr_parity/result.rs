//! The parity result of one execution: previews, the parsed item, its
//! window-space row geometry, and the compared read region.

use d4lc_core::coords::{
    segment_to_tooltip, SegmentRect, TooltipPlacement, TooltipRect, WindowRect,
};
use d4lc_core::Rect;
use d4lc_fuzzy::FuzzyIndexes;
use d4lc_ocr::OcrSegmentKind;
use d4lc_parse::{
    parse_item_tooltip_from_ocr_segments, parse_item_tooltip_from_ocr_segments_with_fuzzy,
    ItemTooltipAffixRowGeometry, ItemTooltipNonAffixRow, ItemTooltipParseResult,
    TooltipOcrLineInput, TooltipOcrSegmentInput, TooltipOcrSegmentKind,
};

use super::types::{
    SegmentResult, TooltipOcrExecution, TooltipOcrParityResult, TooltipOcrSegmentPreview,
};
use super::READ_TOOLTIP_PARITY_PATH;

// ---------------------------------------------------------------------------
// Parity result construction
// ---------------------------------------------------------------------------

/// Compose the parity result of one execution: segments, lines, conversions,
/// the parsed item with its row geometry, and the read top.
///
/// Public because a caller that runs a session itself (for example the
/// same-capture preprocessing comparison) has to compose the same result the
/// one-shot entry points return.
pub fn runtime_parity_result(
    execution: &TooltipOcrExecution,
    preview_chars: usize,
    fuzzy_indexes: Option<&FuzzyIndexes>,
    placement: &TooltipPlacement,
) -> TooltipOcrParityResult {
    let results = &execution.results;
    let raw_text = results
        .iter()
        .map(|result| result.text.raw_text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let normalized_lines = results
        .iter()
        .flat_map(|result| result.converted_text.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let title_preview = normalized_lines
        .first()
        .map(|line| truncate_preview(line, preview_chars));
    let text_preview = truncate_preview(&normalized_lines.join(" | "), preview_chars);
    let line_count = results
        .iter()
        .map(|result| result.text.lines.len())
        .sum::<usize>()
        .max(normalized_lines.len());
    let segments_preview = results
        .iter()
        .take(5)
        .map(|result| TooltipOcrSegmentPreview {
            index: result.plan.index,
            segment_kind: result.plan.kind.as_str().to_string(),
            line_count: result.text.lines.len(),
            raw_text_preview: truncate_preview(&result.text.raw_text, preview_chars),
            converted_text_preview: truncate_preview(&result.converted_text, preview_chars),
        })
        .collect::<Vec<_>>();
    let item_segments = runtime_item_segments(results);
    // Upstream OcrHandler.TextToRune/TextToSigil run fuzzy matching against
    // the session-loaded candidate indexes. Without indexes (failed load) the
    // parse degrades to empty rune/sigil results, matching pre-wiring behavior.
    let mut item = match fuzzy_indexes {
        Some(indexes) => parse_item_tooltip_from_ocr_segments_with_fuzzy(&item_segments, indexes),
        None => parse_item_tooltip_from_ocr_segments(&item_segments),
    };
    let parity_crop = TooltipRect(execution.crop_trace.final_tooltip_crop_rect);
    item.affix_row_geometry = compose_affix_row_geometry(results, parity_crop, placement, &item);
    compose_non_affix_row_geometry(results, parity_crop, placement, &mut item.non_affix_rows);
    let read_top = segment_read_top(execution);
    let read_extent = segment_read_extent(execution, read_top);

    TooltipOcrParityResult {
        parity_path: READ_TOOLTIP_PARITY_PATH,
        ocr_ms: saturating_u128_to_u64(results.iter().map(|r| r.ocr_ms).sum()),
        conversion_ms: saturating_u128_to_u64(results.iter().map(|r| r.conversion_ms).sum()),
        splitter_detection_ms: saturating_u128_to_u64(execution.marker_plan.splitter_detection_ms),
        segment_count: results.len(),
        line_count,
        raw_text,
        normalized_lines,
        segments_preview,
        title_preview,
        text_preview,
        item,
        read_top,
        read_extent,
    }
}

/// Rows above a recognized text line that still belong to it (antialiased
/// glyph tops the line box can clip).
const READ_TOP_TEXT_MARGIN: i32 = 2;

/// First input-crop row whose pixels decide whether a later frame still
/// shows the analyzed item (`TooltipTextSnapshot::ocr_read_top`). The segment
/// rects live in the OCR frame, which sits at `final_tooltip_crop_rect`
/// inside the input crop.
///
/// Normally this is the first splitter below the item name: legendary,
/// unique and mythic tooltips draw the name and type lines on an animated
/// header, so those rows differ on every frame while the item stays the
/// same, and comparing them rejected every result. Everything the header
/// carries is also reflected below it (affix values follow item power), so
/// a different item at the same place still differs there. Without a
/// splitter below the text, the first recognized text line is used; without
/// any recognized line, the segment tops (conservative).
fn segment_read_top(execution: &TooltipOcrExecution) -> Option<u32> {
    let offset = execution.crop_trace.final_tooltip_crop_rect.y.max(0);
    let text_top = execution
        .results
        .iter()
        .filter_map(|result| {
            let segment_top = result.plan.rect.y.max(0);
            result
                .text
                .lines
                .iter()
                .filter(|line| !line.text.trim().is_empty())
                .map(|line| line.rect.y)
                .min()
                .map(|line_top| {
                    segment_top.saturating_add(line_top.saturating_sub(READ_TOP_TEXT_MARGIN).max(0))
                })
        })
        .min();
    let header_end = text_top.and_then(|text_top| {
        execution
            .marker_plan
            .splitter_markers
            .iter()
            .map(|marker| marker.rect.y.max(0))
            .filter(|&y| y > text_top)
            .min()
    });
    header_end
        .or(text_top)
        .or_else(|| {
            execution
                .results
                .iter()
                .map(|result| result.plan.rect.y.max(0))
                .min()
        })
        .and_then(|top| u32::try_from(offset.saturating_add(top)).ok())
}

/// Columns beside a recognized text line that still belong to it
/// (antialiased glyph edges the line box can clip).
const READ_EXTENT_TEXT_MARGIN: i32 = 4;

/// Input-crop extent the recognized text lines within the compared rows
/// cover (`TooltipTextSnapshot::ocr_read_extent`): from the leftmost line
/// start to the rightmost line end and down to the lowest line bottom, widened
/// by [`READ_EXTENT_TEXT_MARGIN`]. Columns outside them are scene or tooltip
/// frame, never item text; rows below them are the rest of the tooltip footer
/// and, in controller mode, the animated key hints above the marker. `None`
/// without such a line (the whole crop below `read_top` is compared).
///
/// Lines that end above `read_top` (the header's name and type lines) do not
/// widen the columns: those rows are not compared. The name starts further
/// left than the affix text, and in the stash the left strip it reached held
/// the animated controller glyphs of the key hints below the tooltip; every
/// result was superseded there and no item reached the overlay.
fn segment_read_extent(
    execution: &TooltipOcrExecution,
    read_top: Option<u32>,
) -> Option<crate::ReadExtent> {
    let offset = execution.crop_trace.final_tooltip_crop_rect.x.max(0);
    let offset_y = execution.crop_trace.final_tooltip_crop_rect.y.max(0);
    let read_top = read_top.map_or(0, |top| i32::try_from(top).unwrap_or(i32::MAX));
    let (left, right, bottom) = execution
        .results
        .iter()
        .flat_map(|result| {
            let segment_left = offset.saturating_add(result.plan.rect.x.max(0));
            let segment_top = offset_y.saturating_add(result.plan.rect.y.max(0));
            result
                .text
                .lines
                .iter()
                .filter(|line| !line.text.trim().is_empty())
                .filter(move |line| {
                    let height = i32::try_from(line.rect.height).unwrap_or(i32::MAX);
                    segment_top
                        .saturating_add(line.rect.y.max(0))
                        .saturating_add(height)
                        > read_top
                })
                .map(move |line| {
                    let left = segment_left.saturating_add(line.rect.x.max(0));
                    let width = i32::try_from(line.rect.width).unwrap_or(i32::MAX);
                    let height = i32::try_from(line.rect.height).unwrap_or(i32::MAX);
                    let bottom = segment_top
                        .saturating_add(line.rect.y.max(0))
                        .saturating_add(height);
                    (left, left.saturating_add(width), bottom)
                })
        })
        .reduce(
            |(left, right, bottom), (line_left, line_right, line_bottom)| {
                (
                    left.min(line_left),
                    right.max(line_right),
                    bottom.max(line_bottom),
                )
            },
        )?;
    Some(crate::ReadExtent {
        left: u32::try_from(left.saturating_sub(READ_EXTENT_TEXT_MARGIN).max(0)).ok()?,
        right: u32::try_from(right.saturating_add(READ_EXTENT_TEXT_MARGIN)).ok()?,
        bottom: u32::try_from(bottom.saturating_add(READ_EXTENT_TEXT_MARGIN)).ok()?,
    })
}

pub(crate) fn runtime_item_segments(results: &[SegmentResult]) -> Vec<TooltipOcrSegmentInput> {
    results
        .iter()
        .map(|result| TooltipOcrSegmentInput {
            index: result.plan.index,
            kind: tooltip_item_segment_kind(result.plan.kind),
            raw_text: result.text.raw_text.clone(),
            converted_text: result.converted_text.clone(),
            lines: result
                .text
                .lines
                .iter()
                .map(|line| TooltipOcrLineInput {
                    text: line.text.clone(),
                    rect: Some(line.rect),
                })
                .collect(),
            marker_name: result.plan.marker_name.clone(),
        })
        .collect()
}

/// Where one OCR segment sits in the tooltip image: the parity crop the OCR
/// frame was cut from, offset by the segment's planned rect in that frame.
fn segment_in_tooltip(parity_crop: TooltipRect, plan_rect: Rect) -> TooltipRect {
    segment_to_tooltip(parity_crop, SegmentRect(plan_rect))
}

/// Compose the window-space geometry of every parsed affix row.
///
/// Each rect is first nested inside the tooltip image — the parity crop
/// (`crop_trace.final_tooltip_crop_rect`), then the segment's planned rect,
/// then the OCR line inside the segment — and then converted to window space
/// exactly once through `placement`. No caller translates the result again.
///
/// The marker anchor follows upstream `OverlayHandler.DrawGraphicsAffixes`
/// (OverlayHandler.cs:251-318): `left = tooltip.X + OffsetX` is the tooltip's
/// left edge, `top = tooltip.Y + itemAffixLocation.Y` and the marker height
/// come from the affix marker the segment was planned from. Affixes without a
/// source line rect, or whose segment plan is missing, are skipped rather
/// than given made-up geometry.
pub(crate) fn compose_affix_row_geometry(
    results: &[SegmentResult],
    parity_crop: TooltipRect,
    placement: &TooltipPlacement,
    item: &ItemTooltipParseResult,
) -> Vec<ItemTooltipAffixRowGeometry> {
    item.affixes
        .iter()
        .filter_map(|affix| {
            let line_rect = affix.source.rect?;
            let plan = results
                .iter()
                .find(|result| result.plan.index == affix.source.source_segment_index)
                .map(|result| &result.plan)?;
            let segment = segment_in_tooltip(parity_crop, plan.rect);
            let marker_anchor = marker_anchor(
                plan.source_marker_rect
                    .filter(|_| plan.kind == OcrSegmentKind::Affix),
                parity_crop,
                placement,
            );
            Some(ItemTooltipAffixRowGeometry {
                row_id: affix.row_id,
                source_segment_index: affix.source.source_segment_index,
                source_line_index: affix.source.source_line_index,
                global_line_index: affix.source.global_line_index,
                marker_ordinal: plan.marker_index,
                text_rect: placement
                    .tooltip_to_window(segment_to_tooltip(segment, SegmentRect(line_rect))),
                affix_area: placement.tooltip_to_window(segment),
                marker_anchor,
            })
        })
        .collect()
}

/// The window-space marker anchor of a segment planned from `marker`: the
/// tooltip's left edge (`tooltip.X + OffsetX`) as the column and the marker's
/// top and height as the row (OverlayHandler.cs:263-264).
fn marker_anchor(
    marker: Option<Rect>,
    parity_crop: TooltipRect,
    placement: &TooltipPlacement,
) -> Option<WindowRect> {
    marker.map(|marker| {
        let marker = segment_to_tooltip(parity_crop, SegmentRect(marker));
        placement.tooltip_to_window(TooltipRect(Rect {
            x: parity_crop.0.x,
            ..marker.0
        }))
    })
}

/// Compose the window-space rects of the classified non-affix rows.
///
/// Mirrors [`compose_affix_row_geometry`]: the parse side classifies the
/// non-affix rows (aspect, unique aspect, rune, sigil) and anchors them to
/// their first OCR line; the line rect is nested into the tooltip image
/// through the parity crop and the segment plan, then converted to window
/// space once. Rows whose anchor line has no rect, or whose segment plan is
/// missing, keep `rect: None` (absence, no fabricated geometry).
pub(crate) fn compose_non_affix_row_geometry(
    results: &[SegmentResult],
    parity_crop: TooltipRect,
    placement: &TooltipPlacement,
    rows: &mut [ItemTooltipNonAffixRow],
) {
    for row in rows.iter_mut() {
        let Some(plan) = results
            .iter()
            .find(|result| result.plan.index == row.source.source_segment_index)
            .map(|result| &result.plan)
        else {
            continue;
        };
        row.marker_anchor = marker_anchor(plan.source_marker_rect, parity_crop, placement)
            .map(|anchor| anchor.rect());
        let Some(line_rect) = row.source.rect else {
            continue;
        };
        let segment = segment_in_tooltip(parity_crop, plan.rect);
        row.rect = Some(
            placement
                .tooltip_to_window(segment_to_tooltip(segment, SegmentRect(line_rect)))
                .rect(),
        );
    }
}

pub(crate) fn tooltip_item_segment_kind(kind: OcrSegmentKind) -> TooltipOcrSegmentKind {
    match kind {
        OcrSegmentKind::Generic => TooltipOcrSegmentKind::Generic,
        OcrSegmentKind::UpperTooltip => TooltipOcrSegmentKind::UpperTooltip,
        OcrSegmentKind::Affix => TooltipOcrSegmentKind::Affix,
        OcrSegmentKind::Aspect => TooltipOcrSegmentKind::Aspect,
        OcrSegmentKind::UniqueAspect => TooltipOcrSegmentKind::UniqueAspect,
        OcrSegmentKind::Rune => TooltipOcrSegmentKind::Rune,
        OcrSegmentKind::Sigil => TooltipOcrSegmentKind::Sigil,
        OcrSegmentKind::Socket => TooltipOcrSegmentKind::Socket,
        OcrSegmentKind::Splitter => TooltipOcrSegmentKind::Splitter,
    }
}

pub(crate) fn truncate_preview(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars.max(1)).collect()
}

pub(crate) fn saturating_u128_to_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
