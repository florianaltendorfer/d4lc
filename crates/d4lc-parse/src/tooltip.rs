//! Segment-oriented item tooltip parse: walks the flattened OCR lines and
//! assembles the [`ItemTooltipParseResult`].

use crate::affix::{
    is_affix_candidate, is_right_aligned_incidental_line, merge_wrapped_affix_line,
    parse_affix_line, parse_primary_stat, parse_weapon_detail,
};
use crate::convert::{
    convert_to_aspect, convert_to_item_type, convert_to_rune, convert_to_sigil, convert_to_unique,
};
use crate::lines::{
    apply_metadata_line, clean_display_line, combined_item_type_text, contains_case_insensitive,
    first_u32_in_text, is_comparison_header, is_legendary_power_line, is_rarity_or_base_type_line,
    is_socket_line_at, is_ui_chrome_line, looks_like_item_section_line, normalize_line,
    normalized_key, parse_item_type_line, parse_number_before_phrase, FlattenedLine,
};
use crate::model::{
    ItemTooltipLegendaryPower, ItemTooltipMetadata, ItemTooltipNonAffixRow, ItemTooltipParseResult,
    ItemTooltipParseStatus, ItemTooltipRune, ItemTooltipSigil, ItemTooltipSocket,
    ItemTooltipSourceLine, ItemTooltipSourceSummary, TooltipOcrSegmentInput, TooltipOcrSegmentKind,
};
use d4lc_core::AffixRowId;
use d4lc_fuzzy::FuzzyIndexes;

/// Parse a structured Diablo IV item tooltip from full upstream-parity OCR segments.
///
/// The input is intentionally segment-oriented: callers must pass the full OCR execution
/// result before any preview truncation. Preview strings are not accepted by this boundary.
pub fn parse_item_tooltip_from_ocr_segments(
    segments: &[TooltipOcrSegmentInput],
) -> ItemTooltipParseResult {
    parse_item_tooltip_from_ocr_segments_internal(segments, None)
}

pub fn parse_item_tooltip_from_ocr_segments_with_fuzzy(
    segments: &[TooltipOcrSegmentInput],
    indexes: &FuzzyIndexes,
) -> ItemTooltipParseResult {
    parse_item_tooltip_from_ocr_segments_internal(segments, Some(indexes))
}

fn parse_item_tooltip_from_ocr_segments_internal(
    segments: &[TooltipOcrSegmentInput],
    indexes: Option<&FuzzyIndexes>,
) -> ItemTooltipParseResult {
    let lines = flatten_ocr_lines(segments);
    let mut result = ItemTooltipParseResult {
        source: ItemTooltipSourceSummary {
            segment_count: segments.len(),
            line_count: lines.len(),
        },
        ..ItemTooltipParseResult::default()
    };

    resolve_item_type_id(segments, indexes, &mut result);
    convert_rune_and_sigil_segments(segments, &lines, indexes, &mut result);
    let LineWalk {
        legendary_sources,
        consumed_line_indexes,
    } = walk_lines(&lines, indexes, &mut result);
    attach_legendary_power(&mut result, legendary_sources, indexes);

    if !result.name_lines.is_empty() {
        result.display_name = Some(result.name_lines.join(" "));
    }

    // Rune/sigil segments classify in the segment loop, before the line
    // loop collects the aspect row; restore tooltip source order.
    result
        .non_affix_rows
        .sort_by_key(|row| row.source.global_line_index);
    result.status = classify_status(&result, consumed_line_indexes.len());
    result
}

/// Resolve the canonical item type id of the upper tooltip section.
fn resolve_item_type_id(
    segments: &[TooltipOcrSegmentInput],
    indexes: Option<&FuzzyIndexes>,
    result: &mut ItemTooltipParseResult,
) {
    // Canonical item type id of the upper tooltip section, resolved exactly
    // like the runtime's segment planning does
    // (`ScreenProcessHandler.FindItemTypePower`/`FindItemAffixes` ->
    // `OcrHandler.ConvertToItemType`): the fuzzy winner id of the item type
    // index is the catalog `Type` value. The display derivation of the
    // rarity/base-type line stays its own field, and a section without a
    // resolved winner keeps the id unknown instead of falling back to it.
    if let Some(indexes) = indexes {
        if let Some(upper) = segments
            .iter()
            .find(|segment| segment.kind == TooltipOcrSegmentKind::UpperTooltip)
        {
            result.item_type_id = convert_to_item_type(&upper.raw_text, indexes)
                .type_id
                .filter(|type_id| !type_id.trim().is_empty());
        }
    }
}

/// Convert the rune and sigil segments into their typed entries and rows.
fn convert_rune_and_sigil_segments(
    segments: &[TooltipOcrSegmentInput],
    lines: &[FlattenedLine],
    indexes: Option<&FuzzyIndexes>,
    result: &mut ItemTooltipParseResult,
) {
    if let Some(indexes) = indexes {
        for segment in segments {
            match segment.kind {
                TooltipOcrSegmentKind::Rune => {
                    let conversion = convert_to_rune(&segment.raw_text, indexes);
                    result.runes.push(ItemTooltipRune {
                        raw_text: conversion.raw_text,
                        text_clean: conversion.text_clean,
                        candidate_texts: conversion.candidate_texts,
                        rune_id: conversion.rune_id,
                        rune_label: conversion.rune_label,
                        rune_type: conversion.rune_type,
                        fuzzy: conversion.fuzzy,
                        conversion_method: conversion.upstream_method,
                    });
                    push_segment_non_affix_row(
                        &mut result.non_affix_rows,
                        lines,
                        segment,
                        result.runes.last().and_then(|rune| rune.rune_id.clone()),
                        result.runes.last().and_then(|rune| rune.rune_label.clone()),
                    );
                }
                TooltipOcrSegmentKind::Sigil => {
                    let conversion = convert_to_sigil(&segment.raw_text, indexes);
                    result.sigils.push(ItemTooltipSigil {
                        raw_text: conversion.raw_text,
                        text_clean: conversion.text_clean,
                        sigil_id: conversion.sigil_id,
                        sigil_label: conversion.sigil_label,
                        sigil_type: conversion.sigil_type,
                        fuzzy: conversion.fuzzy,
                        conversion_method: conversion.upstream_method,
                    });
                    push_segment_non_affix_row(
                        &mut result.non_affix_rows,
                        lines,
                        segment,
                        result
                            .sigils
                            .last()
                            .and_then(|sigil| sigil.sigil_id.clone()),
                        result
                            .sigils
                            .last()
                            .and_then(|sigil| sigil.sigil_label.clone()),
                    );
                }
                _ => {}
            }
        }
    }
}

/// What the line walk collected besides the parse result.
struct LineWalk {
    /// The lines of the legendary power, in tooltip order.
    legendary_sources: Vec<ItemTooltipSourceLine>,
    /// Global indexes of every line some classifier consumed.
    consumed_line_indexes: Vec<usize>,
}

/// Classify the flattened lines one by one into the parse result.
fn walk_lines(
    lines: &[FlattenedLine],
    indexes: Option<&FuzzyIndexes>,
    result: &mut ItemTooltipParseResult,
) -> LineWalk {
    let mut comparison_mode = false;
    let mut legendary_sources = Vec::new();
    let mut saw_legendary = false;
    let mut consumed_line_indexes = Vec::new();

    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index].source;
        let normalized = line.normalized_text.as_str();
        if normalized.is_empty() {
            index += 1;
            continue;
        }

        if is_right_aligned_incidental_line(line, lines) {
            result.right_aligned_filtered.push(line.clone());
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if matches!(
            line.segment_kind,
            TooltipOcrSegmentKind::Rune | TooltipOcrSegmentKind::Sigil
        ) {
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if is_ui_chrome_line(normalized) {
            result.ui_chrome_ignored.push(line.clone());
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if is_comparison_header(normalized) {
            comparison_mode = true;
            result.comparison_delta.push(line.clone());
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }
        if comparison_mode {
            result.comparison_delta.push(line.clone());
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if apply_metadata_line(&mut result.metadata, line) {
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        // The content-guessing classifiers below read upper-tooltip fields
        // (rarity, item power, base armor/DPS, sockets, legendary power) from
        // line text alone. Upstream never does that for an affix: every area
        // below an affix marker is converted as one affix
        // (`ScreenProcessHandler.FindItemAffix`, SPH.cs:1162-1186), so a
        // marker-planned line or an affix-shaped line (`+…`, `x…`) never feeds
        // them — otherwise an affix such as `+1,667 Armor` would be taken for
        // the base armor stat and lose its row and marker.
        let affix_shaped = is_affix_candidate(line);
        let marker_planned_affix = line.segment_kind == TooltipOcrSegmentKind::Affix;

        if !marker_planned_affix && is_legendary_power_line(line) {
            saw_legendary = true;
            legendary_sources.push(line.clone());
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if !marker_planned_affix && is_socket_line_at(lines, index, saw_legendary) {
            result.sockets.push(ItemTooltipSocket {
                raw_text: line.raw_text.clone(),
                source: line.clone(),
            });
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if !affix_shaped && result.rarity.is_none() && is_rarity_or_base_type_line(normalized) {
            let (combined, consumed_next) = combined_item_type_text(lines, index);
            let parsed = parse_item_type_line(&combined);
            result.rarity = parsed.rarity;
            result.is_ancestral = parsed.is_ancestral;
            result.is_sacred = parsed.is_sacred;
            result.base_type = parsed.base_type;
            consumed_line_indexes.push(line.global_line_index);
            if consumed_next {
                consumed_line_indexes.push(lines[index + 1].source.global_line_index);
            }
            index += if consumed_next { 2 } else { 1 };
            continue;
        }

        if !affix_shaped && result.item_power.is_none() {
            if let Some(item_power) = parse_number_before_phrase(normalized, "item power") {
                result.item_power = Some(item_power);
                consumed_line_indexes.push(line.global_line_index);
                index += 1;
                continue;
            }
        }

        if !affix_shaped
            && result.quality.is_none()
            && contains_case_insensitive(normalized, "quality")
        {
            if let Some(quality) = first_u32_in_text(normalized) {
                result.quality = Some(quality);
                consumed_line_indexes.push(line.global_line_index);
                index += 1;
                continue;
            }
        }

        if let Some(stat) = parse_primary_stat(line).filter(|_| !affix_shaped) {
            result.primary_stats.push(stat);
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if let Some(stat) = parse_weapon_detail(line).filter(|_| !affix_shaped) {
            result.weapon_details.push(stat);
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        // One affix marker plans one affix segment, and upstream reads the
        // whole area below that marker as one affix
        // (`ScreenProcessHandler.FindItemAffix`, SPH.cs:1165-1186, converts
        // the complete area text). A wrapped affix therefore stays one row:
        // the next line of the same affix segment extends it instead of
        // becoming a second row with its own marker.
        if let Some(previous) = result.affixes.last_mut().filter(|previous| {
            line.segment_kind == TooltipOcrSegmentKind::Affix
                && previous.source.segment_kind == TooltipOcrSegmentKind::Affix
                && previous.source.source_segment_index == line.source_segment_index
        }) {
            let merged = merge_wrapped_affix_line(&previous.source, line);
            if let Some(mut affix) = parse_affix_line(&merged, indexes) {
                affix.row_id = previous.row_id;
                *previous = affix;
            }
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if let Some(mut affix) = parse_affix_line(line, indexes) {
            affix.row_id = AffixRowId::from_ordinal(result.affixes.len());
            result.affixes.push(affix);
            consumed_line_indexes.push(line.global_line_index);
            index += 1;
            continue;
        }

        if should_collect_name_line(result, line) {
            let cleaned = clean_display_line(normalized);
            if !cleaned.is_empty() {
                result.name_lines.push(cleaned);
                result.name_line_sources.push(line.clone());
                consumed_line_indexes.push(line.global_line_index);
            }
            index += 1;
            continue;
        }

        result.unclassified_item_lines.push(line.clone());
        consumed_line_indexes.push(line.global_line_index);
        index += 1;
    }

    LineWalk {
        legendary_sources,
        consumed_line_indexes,
    }
}

/// Build the legendary power and its non-affix row from the collected lines.
fn attach_legendary_power(
    result: &mut ItemTooltipParseResult,
    legendary_sources: Vec<ItemTooltipSourceLine>,
    indexes: Option<&FuzzyIndexes>,
) {
    if !legendary_sources.is_empty() {
        let joined_text = legendary_sources
            .iter()
            .map(|line| line.normalized_text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let raw_text = legendary_sources
            .iter()
            .map(|line| line.raw_text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let is_imprinted = contains_case_insensitive(&joined_text, "imprinted");
        let is_unique = legendary_sources
            .iter()
            .any(|line| line.segment_kind == TooltipOcrSegmentKind::UniqueAspect);
        let conversion = indexes.map(|indexes| {
            if is_unique {
                convert_to_unique(&raw_text, indexes)
            } else {
                convert_to_aspect(&raw_text, indexes)
            }
        });
        let power_anchor = legendary_sources.first().cloned();
        result.legendary_power = Some(ItemTooltipLegendaryPower {
            raw_text,
            joined_text,
            is_imprinted,
            text_clean: conversion
                .as_ref()
                .map(|conversion| conversion.text_clean.clone()),
            power_id: conversion
                .as_ref()
                .and_then(|conversion| conversion.power_id.clone()),
            power_label: conversion
                .as_ref()
                .and_then(|conversion| conversion.power_label.clone()),
            fuzzy: conversion
                .as_ref()
                .map(|conversion| conversion.fuzzy.clone()),
            conversion_method: conversion
                .as_ref()
                .map(|conversion| conversion.upstream_method),
            is_unique,
            sources: legendary_sources,
        });
        if let Some(source) = power_anchor {
            result.non_affix_rows.push(ItemTooltipNonAffixRow {
                kind: if is_unique {
                    TooltipOcrSegmentKind::UniqueAspect
                } else {
                    TooltipOcrSegmentKind::Aspect
                },
                id: conversion
                    .as_ref()
                    .and_then(|conversion| conversion.power_id.clone()),
                label: conversion
                    .as_ref()
                    .and_then(|conversion| conversion.power_label.clone()),
                source,
                rect: None,
                marker_anchor: None,
            });
        }
    }
}

fn flatten_ocr_lines(segments: &[TooltipOcrSegmentInput]) -> Vec<FlattenedLine> {
    let mut output = Vec::new();
    for segment in segments {
        let raw_lines = segment_raw_lines(segment);
        let converted_lines = segment
            .converted_text
            .lines()
            .map(normalize_line)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();

        for (line_index, raw_line) in raw_lines.into_iter().enumerate() {
            let converted_text =
                converted_text_for_line(&converted_lines, line_index, raw_line.len());
            let normalized_text = normalize_line(&raw_line);
            if normalized_text.is_empty() {
                continue;
            }
            let global_line_index = output.len();
            output.push(FlattenedLine {
                source: ItemTooltipSourceLine {
                    source_segment_index: segment.index,
                    source_line_index: line_index,
                    global_line_index,
                    segment_kind: segment.kind,
                    raw_text: raw_line.clone(),
                    converted_text,
                    normalized_text,
                    rect: segment.lines.get(line_index).and_then(|line| line.rect),
                    marker_name: segment.marker_name.clone(),
                },
            });
        }
    }
    output
}

/// Anchor a classified rune/sigil segment to its first flattened line and
/// record the non-affix row with the classification the conversion resolved.
/// Segments without a flattened line carry no anchor and are skipped (no
/// fabricated line index, mirroring the affix row geometry absence rule).
fn push_segment_non_affix_row(
    rows: &mut Vec<ItemTooltipNonAffixRow>,
    lines: &[FlattenedLine],
    segment: &TooltipOcrSegmentInput,
    id: Option<String>,
    label: Option<String>,
) {
    let Some(anchor) = lines
        .iter()
        .find(|line| line.source.source_segment_index == segment.index)
        .map(|line| line.source.clone())
    else {
        return;
    };
    rows.push(ItemTooltipNonAffixRow {
        kind: segment.kind,
        id,
        label,
        source: anchor,
        rect: None,
        marker_anchor: None,
    });
}

fn segment_raw_lines(segment: &TooltipOcrSegmentInput) -> Vec<String> {
    if !segment.lines.is_empty() {
        return segment
            .lines
            .iter()
            .map(|line| normalize_line(&line.text))
            .filter(|line| !line.is_empty())
            .collect();
    }

    segment
        .raw_text
        .lines()
        .map(normalize_line)
        .filter(|line| !line.is_empty())
        .collect()
}

fn converted_text_for_line(
    converted_lines: &[String],
    line_index: usize,
    raw_line_len: usize,
) -> Option<String> {
    let converted = if converted_lines.len() == 1 && raw_line_len > 0 {
        converted_lines.first()
    } else {
        converted_lines.get(line_index)
    }?;
    if converted.is_empty() {
        None
    } else {
        Some(converted.clone())
    }
}

fn should_collect_name_line(result: &ItemTooltipParseResult, line: &ItemTooltipSourceLine) -> bool {
    if line.segment_kind != TooltipOcrSegmentKind::UpperTooltip {
        return false;
    }
    if !result.affixes.is_empty()
        || !result.primary_stats.is_empty()
        || !result.weapon_details.is_empty()
        || result.item_power.is_some()
        || result.quality.is_some()
        || result.rarity.is_some()
    {
        return false;
    }
    let key = normalized_key(&line.normalized_text);
    !key.is_empty() && !looks_like_item_section_line(&line.normalized_text)
}

fn classify_status(
    result: &ItemTooltipParseResult,
    consumed_line_count: usize,
) -> ItemTooltipParseStatus {
    let has_identity = result.display_name.is_some()
        || result.rarity.is_some()
        || result.item_power.is_some()
        || result.base_type.is_some();
    let has_item_sections = !result.affixes.is_empty()
        || !result.primary_stats.is_empty()
        || !result.weapon_details.is_empty()
        || result.legendary_power.is_some()
        || !result.runes.is_empty()
        || !result.sigils.is_empty()
        || !result.sockets.is_empty()
        || has_metadata(&result.metadata);

    if has_identity && has_item_sections && result.unclassified_item_lines.is_empty() {
        ItemTooltipParseStatus::Parsed
    } else if has_identity || has_item_sections || consumed_line_count > 0 {
        ItemTooltipParseStatus::Partial
    } else {
        ItemTooltipParseStatus::NotItem
    }
}

fn has_metadata(metadata: &ItemTooltipMetadata) -> bool {
    metadata.requires_level.is_some()
        || metadata.account_bound.is_some()
        || metadata.sell_value.is_some()
        || metadata.durability.is_some()
        || metadata.tempers.is_some()
}

#[cfg(test)]
mod tests;
