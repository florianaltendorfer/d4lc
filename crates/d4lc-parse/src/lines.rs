//! Tooltip line cleanup and classification: UI chrome, comparison, metadata,
//! socket, legendary power and rarity/base-type lines.

use crate::model::{
    ItemTooltipMetadata, ItemTooltipMetadataValue, ItemTooltipRarity, ItemTooltipSourceLine,
    TooltipOcrSegmentKind,
};

#[derive(Debug, Clone)]
pub(crate) struct FlattenedLine {
    pub(crate) source: ItemTooltipSourceLine,
}

pub(crate) fn normalize_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn clean_display_line(value: &str) -> String {
    value
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '|' | '[' | ']' | '{' | '}'
                )
        })
        .trim()
        .to_string()
}

pub(crate) fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

pub(crate) fn normalized_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character.is_whitespace() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn is_ui_chrome_line(line: &str) -> bool {
    let key = normalized_key(line);
    if key.is_empty() {
        return false;
    }
    matches!(
        key.as_str(),
        "scroll down"
            | "equip"
            | "link"
            | "drop"
            | "hide comparison"
            | "mark as junk"
            | "unmark item"
            | "ctrl"
            | "control"
            | "shift"
            | "mouse"
            | "left mouse button"
            | "right mouse button"
            | "middle mouse button"
            | "button a"
            | "button b"
            | "button x"
            | "button y"
    ) || key.contains("mouse button")
        || key.starts_with("ctrl ")
        || key.starts_with("shift ")
        || key.contains(" hide comparison")
        || key.contains(" mark as junk")
        || key.contains(" unmark item")
}

pub(crate) fn is_comparison_header(line: &str) -> bool {
    let key = normalized_key(line);
    key.contains("properties lost when equipped")
        || key.contains("properties gained when equipped")
        || key.contains("when equipped")
}

pub(crate) fn apply_metadata_line(
    metadata: &mut ItemTooltipMetadata,
    line: &ItemTooltipSourceLine,
) -> bool {
    let normalized = line.normalized_text.as_str();
    let key = normalized_key(normalized);
    if key.starts_with("requires level") {
        if let Some(level) = first_u32_in_text(normalized) {
            metadata.requires_level = Some(ItemTooltipMetadataValue {
                value: level,
                source: line.clone(),
            });
            return true;
        }
    }
    if key == "account bound" {
        metadata.account_bound = Some(ItemTooltipMetadataValue {
            value: true,
            source: line.clone(),
        });
        return true;
    }
    if key.starts_with("sell value") {
        metadata.sell_value = Some(ItemTooltipMetadataValue {
            value: value_after_separator(normalized).unwrap_or_else(|| normalized.to_string()),
            source: line.clone(),
        });
        return true;
    }
    if key.starts_with("durability") {
        metadata.durability = Some(ItemTooltipMetadataValue {
            value: value_after_separator(normalized).unwrap_or_else(|| normalized.to_string()),
            source: line.clone(),
        });
        return true;
    }
    if key.starts_with("tempers") {
        metadata.tempers = Some(ItemTooltipMetadataValue {
            value: value_after_separator(normalized).unwrap_or_else(|| normalized.to_string()),
            source: line.clone(),
        });
        return true;
    }
    false
}

fn value_after_separator(value: &str) -> Option<String> {
    let (_, right) = value.split_once(':')?;
    let text = right.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub(crate) fn is_legendary_power_line(line: &ItemTooltipSourceLine) -> bool {
    matches!(
        line.segment_kind,
        TooltipOcrSegmentKind::Aspect | TooltipOcrSegmentKind::UniqueAspect
    ) || normalized_key(&line.normalized_text).starts_with("imprinted")
}

pub(crate) fn is_socket_line_at(
    lines: &[FlattenedLine],
    index: usize,
    saw_legendary: bool,
) -> bool {
    let line = &lines[index].source;
    if line.segment_kind == TooltipOcrSegmentKind::Socket {
        return true;
    }
    let key = normalized_key(&line.normalized_text);
    if key.contains("empty socket") || key == "socket" {
        return true;
    }
    if !saw_legendary {
        return false;
    }
    if key.contains("physical damage multiplier") {
        return next_non_ui_line_key(lines, index)
            .map(|next| next.contains("empty socket"))
            .unwrap_or(false);
    }
    false
}

fn next_non_ui_line_key(lines: &[FlattenedLine], index: usize) -> Option<String> {
    lines
        .iter()
        .skip(index + 1)
        .map(|line| normalized_key(&line.source.normalized_text))
        .find(|key| !key.is_empty() && !is_ui_chrome_line(key))
}

pub(crate) fn is_rarity_or_base_type_line(line: &str) -> bool {
    let key = normalized_key(line);
    key.contains("ancestral")
        || key.contains("sacred")
        || key.contains("legendary")
        || key.contains("unique")
        || key.contains("rare")
        || key.contains("magic")
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ParsedItemTypeLine {
    pub(crate) rarity: Option<ItemTooltipRarity>,
    pub(crate) is_ancestral: Option<bool>,
    pub(crate) is_sacred: Option<bool>,
    pub(crate) base_type: Option<String>,
}

pub(crate) fn combined_item_type_text(lines: &[FlattenedLine], index: usize) -> (String, bool) {
    let current = clean_item_type_text(&lines[index].source.normalized_text);
    let Some(next) = lines.get(index + 1) else {
        return (current, false);
    };
    let next_text = next.source.normalized_text.as_str();
    if is_probable_base_type_continuation(&current, next_text) {
        return (
            clean_item_type_text(&format!("{current} {next_text}")),
            true,
        );
    }
    (current, false)
}

fn clean_item_type_text(value: &str) -> String {
    value
        .replace("Two- } Handed", "Two-Handed")
        .replace("Two- Handed", "Two-Handed")
        .replace("Two - Handed", "Two-Handed")
        .chars()
        .map(|character| {
            if character.is_ascii_alphabetic() || character == '-' || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_probable_base_type_continuation(current: &str, next: &str) -> bool {
    let current_key = normalized_key(current);
    let next_key = normalized_key(next);
    current.contains("Two-")
        || current_key.ends_with("two")
        || next_key.starts_with("handed")
        || (is_rarity_or_base_type_line(current) && !looks_like_item_section_line(next))
}

pub(crate) fn looks_like_item_section_line(line: &str) -> bool {
    let key = normalized_key(line);
    let trimmed = line.trim_start();
    key.contains("item power")
        || key.contains("quality")
        || key.contains("damage per second")
        || key.contains("damage per hit")
        || key.contains("attacks per second")
        || key.contains("armor")
        || key.starts_with("requires level")
        || key.starts_with("sell value")
        || key.starts_with("durability")
        || key.starts_with("tempers")
        || key.starts_with("imprinted")
        || key.starts_with("properties lost")
        || key.starts_with("properties gained")
        || trimmed.starts_with('+')
        || trimmed.starts_with('x')
        || trimmed.starts_with('X')
}

pub(crate) fn parse_item_type_line(line: &str) -> ParsedItemTypeLine {
    let mut parsed = ParsedItemTypeLine::default();
    let lower = line.to_ascii_lowercase();
    parsed.is_ancestral = Some(lower.contains("ancestral"));
    parsed.is_sacred = Some(lower.contains("sacred"));
    parsed.rarity = if lower.contains("legendary") {
        Some(ItemTooltipRarity::Legendary)
    } else if lower.contains("unique") {
        Some(ItemTooltipRarity::Unique)
    } else if lower.contains("rare") {
        Some(ItemTooltipRarity::Rare)
    } else if lower.contains("magic") {
        Some(ItemTooltipRarity::Magic)
    } else if lower.contains("normal") {
        Some(ItemTooltipRarity::Normal)
    } else {
        None
    };

    let base = line
        .split_whitespace()
        .filter(|word| {
            let key = word.to_ascii_lowercase();
            !matches!(
                key.as_str(),
                "ancestral" | "sacred" | "legendary" | "unique" | "rare" | "magic" | "normal"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    if !base.is_empty() {
        parsed.base_type = Some(base);
    }
    parsed
}

pub(crate) fn parse_number_before_phrase(line: &str, phrase: &str) -> Option<u32> {
    let lower = line.to_ascii_lowercase();
    let phrase_index = lower.find(phrase)?;
    first_u32_in_text(&line[..phrase_index])
}

pub(crate) fn first_u32_in_text(line: &str) -> Option<u32> {
    let mut digits = String::new();
    let mut seen_digit = false;
    for character in line.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
            seen_digit = true;
        } else if seen_digit && (character == ',' || character == '.') {
            continue;
        } else if seen_digit {
            break;
        }
    }
    if digits.is_empty() {
        None
    } else {
        digits.parse::<u32>().ok()
    }
}
