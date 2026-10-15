//! Stat, weapon detail and affix row parsing of tooltip lines.

use crate::convert::convert_to_affix;
use crate::lines::{normalized_key, FlattenedLine};
use crate::model::{
    ItemTooltipAffix, ItemTooltipSourceLine, ItemTooltipStat, TooltipOcrSegmentKind,
};
use d4lc_core::{AffixRowId, Rect};
use d4lc_fuzzy::FuzzyIndexes;

pub(crate) fn parse_primary_stat(line: &ItemTooltipSourceLine) -> Option<ItemTooltipStat> {
    let key = normalized_key(&line.normalized_text);
    if key.contains("damage per second") {
        return Some(ItemTooltipStat {
            name: "Damage Per Second".to_string(),
            raw_text: line.raw_text.clone(),
            value_text: first_numeric_text(&line.normalized_text),
            source: line.clone(),
        });
    }
    if key.ends_with("armor") || key.contains(" armor") {
        return Some(ItemTooltipStat {
            name: "Armor".to_string(),
            raw_text: line.raw_text.clone(),
            value_text: first_numeric_text(&line.normalized_text),
            source: line.clone(),
        });
    }
    None
}

pub(crate) fn parse_weapon_detail(line: &ItemTooltipSourceLine) -> Option<ItemTooltipStat> {
    let key = normalized_key(&line.normalized_text);
    if key.contains("damage per hit") {
        return Some(ItemTooltipStat {
            name: "Damage Per Hit".to_string(),
            raw_text: line.raw_text.clone(),
            value_text: value_before_text(&line.normalized_text, "Damage Per Hit"),
            source: line.clone(),
        });
    }
    if key.contains("attacks per second") {
        return Some(ItemTooltipStat {
            name: "Attacks per Second".to_string(),
            raw_text: line.raw_text.clone(),
            value_text: first_numeric_text(&line.normalized_text),
            source: line.clone(),
        });
    }
    None
}

pub(crate) fn parse_affix_line(
    line: &ItemTooltipSourceLine,
    indexes: Option<&FuzzyIndexes>,
) -> Option<ItemTooltipAffix> {
    if !is_affix_candidate(line) {
        return None;
    }
    let is_greater_affix = has_greater_affix_marker(line);
    let is_implicit = has_affix_marker_kind(line, "implicit");
    let is_tempered = has_affix_marker_kind(line, "temper");
    let without_marker = strip_greater_affix_marker(&line.normalized_text);
    let value_text = first_affix_value_text(&without_marker);
    let range_text = parse_range_text(&without_marker);
    let mut name = affix_name(line, &without_marker);
    // A marker-planned area is always one affix upstream (SPH.cs:1162-1186):
    // a text without a readable name (`+1,667`) keeps its row and marker
    // under its own text instead of vanishing.
    if name.is_empty() && line.segment_kind == TooltipOcrSegmentKind::Affix {
        name = clean_affix_name(&without_marker);
    }
    if name.is_empty() {
        return None;
    }
    let conversion = indexes.map(|indexes| convert_to_affix(&line.raw_text, indexes));
    Some(ItemTooltipAffix {
        name,
        raw_text: line.raw_text.clone(),
        value_text,
        range_text,
        text_clean: conversion
            .as_ref()
            .map(|conversion| conversion.text_clean.clone()),
        text_value: conversion.as_ref().map(|conversion| conversion.text_value),
        affix_id: conversion
            .as_ref()
            .and_then(|conversion| conversion.affix_id.clone()),
        affix_label: conversion
            .as_ref()
            .and_then(|conversion| conversion.affix_label.clone()),
        fuzzy: conversion
            .as_ref()
            .map(|conversion| conversion.fuzzy.clone()),
        conversion_method: conversion
            .as_ref()
            .map(|conversion| conversion.upstream_method),
        is_greater_affix,
        is_implicit,
        is_tempered,
        tuning_prisms: conversion
            .as_ref()
            .and_then(|conversion| conversion.affix_id.clone())
            .map(|affix_id| {
                indexes
                    .map(|indexes| indexes.affix_tuning_prisms(&affix_id))
                    .unwrap_or_default()
            })
            .unwrap_or_default(),
        // Assigned when the row joins the parse result.
        row_id: AffixRowId::default(),
        source: line.clone(),
    })
}

/// The source line of a wrapped affix after its next OCR line: the texts are
/// joined (raw text keeps the line break upstream's area text has), the rect
/// covers both lines, and the provenance stays that of the first line.
pub(crate) fn merge_wrapped_affix_line(
    first: &ItemTooltipSourceLine,
    next: &ItemTooltipSourceLine,
) -> ItemTooltipSourceLine {
    let converted_text = match (
        first.converted_text.as_deref(),
        next.converted_text.as_deref(),
    ) {
        (Some(first), Some(next)) => Some(format!("{first} {next}")),
        (first, next) => first.or(next).map(str::to_string),
    };
    let rect = match (first.rect, next.rect) {
        (Some(first), Some(next)) => {
            let left = first.x.min(next.x);
            let top = first.y.min(next.y);
            let right = (first.x + first.width as i32).max(next.x + next.width as i32);
            let bottom = (first.y + first.height as i32).max(next.y + next.height as i32);
            Some(Rect {
                x: left,
                y: top,
                width: u32::try_from(right - left).unwrap_or(0),
                height: u32::try_from(bottom - top).unwrap_or(0),
            })
        }
        (first, next) => first.or(next),
    };
    ItemTooltipSourceLine {
        raw_text: format!("{}\n{}", first.raw_text, next.raw_text),
        converted_text,
        normalized_text: format!("{} {}", first.normalized_text, next.normalized_text),
        rect,
        ..first.clone()
    }
}

pub(crate) fn is_affix_candidate(line: &ItemTooltipSourceLine) -> bool {
    if line.segment_kind == TooltipOcrSegmentKind::Affix {
        return true;
    }
    let text = strip_greater_affix_marker(&line.normalized_text);
    text.starts_with('+') || text.starts_with('x') || text.starts_with('X')
}

pub(crate) fn is_right_aligned_incidental_line(
    line: &ItemTooltipSourceLine,
    lines: &[FlattenedLine],
) -> bool {
    if !matches!(
        line.segment_kind,
        TooltipOcrSegmentKind::Generic | TooltipOcrSegmentKind::UpperTooltip
    ) {
        return false;
    }
    let Some(rect) = line.rect else {
        return false;
    };
    let Some(first_x) = lines
        .iter()
        .find(|candidate| {
            candidate.source.source_segment_index == line.source_segment_index
                && candidate.source.rect.is_some()
        })
        .and_then(|candidate| candidate.source.rect.map(|rect| rect.x))
    else {
        return false;
    };
    rect.x > first_x.saturating_add(20)
}

fn has_affix_marker_kind(line: &ItemTooltipSourceLine, marker_kind: &str) -> bool {
    let marker_name = line
        .marker_name
        .as_deref()
        .map(normalized_key)
        .unwrap_or_default();
    marker_name.contains(marker_kind)
}

fn has_greater_affix_marker(line: &ItemTooltipSourceLine) -> bool {
    let raw_trimmed = line.raw_text.trim_start();
    let normalized_trimmed = line.normalized_text.trim_start();
    let marker_name = line
        .marker_name
        .as_deref()
        .map(normalized_key)
        .unwrap_or_default();

    raw_trimmed.starts_with('★')
        || raw_trimmed.starts_with('☆')
        || raw_trimmed.starts_with('✦')
        || raw_trimmed.starts_with('✧')
        || raw_trimmed.starts_with('✪')
        || raw_trimmed.starts_with('*')
        || normalized_trimmed.starts_with('★')
        || normalized_trimmed.starts_with('☆')
        || normalized_trimmed.starts_with('✦')
        || normalized_trimmed.starts_with('✧')
        || normalized_trimmed.starts_with('✪')
        || normalized_trimmed.starts_with('*')
        || normalized_key(&line.raw_text).contains("greater affix")
        || normalized_key(&line.normalized_text).contains("greater affix")
        || marker_name.starts_with("dot affixes greater")
        || marker_name.contains("greater")
}

fn strip_greater_affix_marker(value: &str) -> String {
    value
        .trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, '★' | '☆' | '✦' | '✧' | '✪' | '*')
        })
        .trim()
        .to_string()
}

fn first_affix_value_text(value: &str) -> Option<String> {
    let text = value.trim_start();
    let mut chars = text.chars().peekable();
    if matches!(chars.peek(), Some('+') | Some('-') | Some('x') | Some('X')) {
        chars.next();
    }

    let mut token = String::new();
    let mut seen_digit = false;
    while let Some(character) = chars.peek().copied() {
        if character.is_ascii_digit() || matches!(character, '.' | ',') {
            token.push(character);
            seen_digit = true;
            chars.next();
        } else if character == '%' && seen_digit {
            token.push(character);
            chars.next();
            break;
        } else {
            break;
        }
    }

    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn first_numeric_text(value: &str) -> Option<String> {
    let mut output = String::new();
    let mut seen_digit = false;
    for character in value.chars() {
        if character.is_ascii_digit() || (seen_digit && matches!(character, ',' | '.' | '%' | '/'))
        {
            output.push(character);
            seen_digit = true;
        } else if seen_digit {
            break;
        }
    }
    if output.is_empty() {
        None
    } else {
        Some(output)
    }
}

fn value_before_text(value: &str, marker: &str) -> Option<String> {
    let index = value
        .to_ascii_lowercase()
        .find(&marker.to_ascii_lowercase())?;
    let left = value[..index].trim();
    if left.is_empty() {
        None
    } else {
        Some(left.to_string())
    }
}

fn parse_range_text(value: &str) -> Option<String> {
    let start = value.find('[')?;
    let end = value[start + 1..]
        .find(']')
        .map(|relative| start + 1 + relative)
        .unwrap_or(value.len());
    let range = value[start + 1..end].trim();
    if range.is_empty() {
        None
    } else {
        Some(range.to_string())
    }
}

fn affix_name(line: &ItemTooltipSourceLine, marker_stripped_raw: &str) -> String {
    if let Some(converted) = line.converted_text.as_deref() {
        let cleaned = clean_affix_name(converted);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    let mut text = marker_stripped_raw.trim_start().to_string();
    if text.starts_with('+')
        || text.starts_with('-')
        || text.starts_with('x')
        || text.starts_with('X')
    {
        text = text.chars().skip(1).collect();
    }
    text = text
        .trim_start_matches(|character: char| {
            character.is_ascii_digit() || matches!(character, '.' | ',' | '%' | ' ')
        })
        .trim()
        .to_string();
    if let Some(start) = text.find('[') {
        text.truncate(start);
    }
    clean_affix_name(&strip_trailing_numeric_range(&text))
}

fn clean_affix_name(value: &str) -> String {
    value
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '★' | '☆' | '✦' | '✧' | '✪' | '*' | '+' | '[' | ']'
                )
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_trailing_numeric_range(value: &str) -> String {
    let mut words = value.split_whitespace().collect::<Vec<_>>();
    while let Some(last) = words.last().copied() {
        if last.chars().any(|character| character.is_ascii_digit())
            || matches!(last, "-" | "]" | "[")
        {
            words.pop();
        } else {
            break;
        }
    }
    words.join(" ")
}
