//! Upstream `OcrHandler.ConvertTo*` conversions: item power and type,
//! affixes, aspects and uniques, runes and sigils.

use d4lc_core::text::{upstream_affix_multiline_candidates, upstream_convert_to_affix_clean_text};
use d4lc_fuzzy::{FuzzyCategory, FuzzyIndexes, FuzzyMatchResult};
use regex::Regex;
use std::sync::LazyLock;

/// Upstream `OcrHandler.TextToAffixValue` number pattern. Compiled once
/// instead of per call: the pattern is a fixed literal, so hoisting it is
/// behavior-identical while keeping the affix hot path free of per-line
/// regex compilation cost.
static AFFIX_VALUE_NUMBER_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\d+\.\d+|\d+\,\d+|\d+").ok());

/// Upstream leading item-power digits pattern (`ConvertToPower` line scan).
static LEADING_ITEM_POWER_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\d+").ok());

const MIN_ITEM_POWER: u32 = 150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPowerConversion {
    pub upstream_method: &'static str,
    pub matched_text: Option<String>,
    pub text_clean: Option<String>,
    pub item_power: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemTypeConversion {
    pub upstream_method: &'static str,
    pub item_power: Option<u32>,
    pub candidate_texts: Vec<String>,
    pub fuzzy: Option<FuzzyMatchResult>,
    pub type_id: Option<String>,
    pub type_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AffixConversion {
    pub upstream_method: &'static str,
    pub raw_text: String,
    pub text_clean: String,
    pub text_value: f64,
    pub candidate_texts: Vec<String>,
    pub fuzzy: FuzzyMatchResult,
    pub affix_id: Option<String>,
    pub affix_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PowerConversion {
    pub upstream_method: &'static str,
    pub raw_text: String,
    pub text_clean: String,
    pub fuzzy: FuzzyMatchResult,
    pub power_id: Option<String>,
    pub power_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuneConversion {
    pub upstream_method: &'static str,
    pub raw_text: String,
    pub text_clean: String,
    pub candidate_texts: Vec<String>,
    pub fuzzy: FuzzyMatchResult,
    pub rune_id: Option<String>,
    pub rune_label: Option<String>,
    pub rune_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SigilConversion {
    pub upstream_method: &'static str,
    pub raw_text: String,
    pub text_clean: String,
    pub fuzzy: FuzzyMatchResult,
    pub sigil_id: Option<String>,
    pub sigil_label: Option<String>,
    pub sigil_type: Option<String>,
}

pub fn convert_to_item_power(text: &str) -> ItemPowerConversion {
    let lines = split_non_blank_lines(text);
    let mut matched_text = None;
    let mut text_clean = None;
    let mut item_power = None;

    for line in lines.iter().rev() {
        if let Some((digits, value)) = parse_leading_item_power(line) {
            matched_text = Some(line.clone());
            text_clean = Some(digits);
            item_power = Some(value);
            break;
        }
    }

    ItemPowerConversion {
        upstream_method: "OcrHandler.ConvertToPower",
        matched_text,
        text_clean,
        item_power,
    }
}

pub fn convert_to_item_type(text: &str, indexes: &FuzzyIndexes) -> ItemTypeConversion {
    // Upstream v5.4.0 removed the "all words < 3 chars" artifact-line
    // heuristic from ConvertToItemType; lines are trimmed + non-empty only.
    let lines = split_non_blank_lines(text);
    let power = find_power_line_in_lines(&lines);
    let item_power = power.as_ref().map(|(_, _, value)| *value);
    let candidate_texts = build_item_type_candidates(&lines, power.as_ref().map(|(i, _, _)| *i));

    let fuzzy = candidate_texts
        .iter()
        .map(|candidate| indexes.match_text_cached(FuzzyCategory::ItemType, candidate))
        .max_by(|left, right| {
            let left_score = left.score.unwrap_or(f64::NEG_INFINITY);
            let right_score = right.score.unwrap_or(f64::NEG_INFINITY);
            left_score
                .partial_cmp(&right_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let type_id = fuzzy
        .as_ref()
        .filter(|trace| trace.accepted)
        .and_then(|trace| trace.winner_id.clone());
    let type_label = fuzzy
        .as_ref()
        .filter(|trace| trace.accepted)
        .and_then(|trace| trace.winner_label.clone());

    ItemTypeConversion {
        upstream_method: "OcrHandler.ConvertToItemType",
        item_power,
        candidate_texts,
        fuzzy,
        type_id,
        type_label,
    }
}

/// Convert one OCR'd affix row to its canonical affix identity.
///
/// The winner id is the complete `IdName` of the pinned catalog row the fuzzy
/// affix index resolved (`OcrHandler.ConvertToAffix` -> `TextToAffix`), never
/// a shortened `IdNameList` name — the import paths store the same identity,
/// so the rule evaluation's exact id comparison can match.
pub fn convert_to_affix(raw_text: &str, indexes: &FuzzyIndexes) -> AffixConversion {
    let text_clean = upstream_convert_to_affix_clean_text(raw_text);
    let fuzzy = indexes.match_text_cached(FuzzyCategory::Affix, &text_clean);
    AffixConversion {
        upstream_method: "OcrHandler.ConvertToAffix",
        raw_text: raw_text.to_string(),
        text_clean,
        text_value: text_to_affix_value(raw_text),
        candidate_texts: Vec::new(),
        affix_id: fuzzy.winner_id.clone(),
        affix_label: fuzzy.winner_label.clone(),
        fuzzy,
    }
}

pub fn convert_to_affix_multiline(raw_text: &str, indexes: &FuzzyIndexes) -> AffixConversion {
    let candidate_texts = upstream_affix_multiline_candidates(raw_text);
    let text_clean = candidate_texts
        .iter()
        .map(|candidate| indexes.match_text_cached(FuzzyCategory::Affix, candidate))
        .max_by(|left, right| {
            let left_score = left.score.unwrap_or(f64::NEG_INFINITY);
            let right_score = right.score.unwrap_or(f64::NEG_INFINITY);
            left_score
                .partial_cmp(&right_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|trace| trace.raw_text)
        .unwrap_or_default();
    let fuzzy = indexes.match_text_cached(FuzzyCategory::Affix, &text_clean);
    AffixConversion {
        upstream_method: "OcrHandler.ConvertToAffixMultiline",
        raw_text: raw_text.to_string(),
        text_clean,
        text_value: text_to_affix_value(raw_text),
        candidate_texts,
        affix_id: fuzzy.winner_id.clone(),
        affix_label: fuzzy.winner_label.clone(),
        fuzzy,
    }
}

pub fn text_to_affix_value(text: &str) -> f64 {
    let text_clean = text
        .replace('\n', " ")
        .chars()
        .filter(|character| !matches!(character, '[' | ']' | '(' | ')' | '+' | '-' | '%'))
        .collect::<String>()
        .trim()
        .to_string();

    let Some(regex) = AFFIX_VALUE_NUMBER_REGEX.as_ref() else {
        return 0.0;
    };
    let Some(value_match) = regex.find(&text_clean) else {
        return 0.0;
    };
    let mut text_value = value_match.as_str().to_string();
    if text_value.find('.') != Some(text_value.len().saturating_sub(2)) {
        text_value = text_value.replace('.', "");
    }
    if text_value.find(',') != Some(text_value.len().saturating_sub(2)) {
        text_value = text_value.replace(',', "");
    }
    text_value = text_value.replace(',', ".");
    text_value.parse::<f64>().unwrap_or(0.0)
}

pub fn convert_to_aspect(raw_text: &str, indexes: &FuzzyIndexes) -> PowerConversion {
    convert_to_power(
        raw_text,
        indexes,
        FuzzyCategory::Aspect,
        "OcrHandler.ConvertToAspect",
    )
}

pub fn convert_to_unique(raw_text: &str, indexes: &FuzzyIndexes) -> PowerConversion {
    convert_to_power(
        raw_text,
        indexes,
        FuzzyCategory::Unique,
        "OcrHandler.ConvertToUnique",
    )
}

pub fn convert_to_rune(raw_text: &str, indexes: &FuzzyIndexes) -> RuneConversion {
    let candidate_texts = rune_candidates(raw_text);
    let fuzzy = candidate_texts
        .iter()
        .map(|candidate| indexes.match_text(FuzzyCategory::Rune, candidate))
        .max_by(|left, right| {
            let left_score = left.score.unwrap_or(f64::NEG_INFINITY);
            let right_score = right.score.unwrap_or(f64::NEG_INFINITY);
            left_score
                .partial_cmp(&right_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.raw_text.len().cmp(&right.raw_text.len()))
        })
        .unwrap_or_else(|| indexes.match_text(FuzzyCategory::Rune, ""));
    let rune_id = fuzzy.accepted.then(|| fuzzy.winner_id.clone()).flatten();
    RuneConversion {
        upstream_method: "OcrHandler.ConvertToRune",
        raw_text: raw_text.to_string(),
        text_clean: fuzzy.raw_text.clone(),
        candidate_texts,
        rune_type: rune_id.as_deref().and_then(rune_type_from_id),
        rune_id,
        rune_label: fuzzy.accepted.then(|| fuzzy.winner_label.clone()).flatten(),
        fuzzy,
    }
}

pub fn convert_to_sigil(raw_text: &str, indexes: &FuzzyIndexes) -> SigilConversion {
    let text_clean = raw_text.split('\n').next().unwrap_or_default().to_string();
    let fuzzy = indexes.match_text(FuzzyCategory::Sigil, &text_clean);
    let sigil_id = fuzzy.accepted.then(|| fuzzy.winner_id.clone()).flatten();
    SigilConversion {
        upstream_method: "OcrHandler.ConvertToSigil",
        raw_text: raw_text.to_string(),
        text_clean,
        sigil_type: sigil_id.as_deref().and_then(sigil_type_from_id),
        sigil_id,
        sigil_label: fuzzy.accepted.then(|| fuzzy.winner_label.clone()).flatten(),
        fuzzy,
    }
}

fn rune_candidates(raw_text: &str) -> Vec<String> {
    let first_block = raw_text.split("\n\n").next().unwrap_or_default();
    let lines = first_block
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let mut candidates = Vec::new();
    if !lines.is_empty() {
        candidates.push(lines[0].to_string());
    }
    if lines.len() >= 2 {
        candidates.push(format!("{} {}", lines[0], lines[1]));
    }
    if lines.len() >= 3 {
        candidates.push(format!("{} {} {}", lines[0], lines[1], lines[2]));
    }
    candidates
}

fn rune_type_from_id(id: &str) -> Option<String> {
    if id.contains("_Condition_") {
        Some("condition".to_string())
    } else if id.contains("_Effect_") {
        Some("effect".to_string())
    } else {
        None
    }
}

fn sigil_type_from_id(id: &str) -> Option<String> {
    if id.starts_with("World_DGN_") {
        Some("Dungeon".to_string())
    } else if id.starts_with("DungeonAffix_Major") {
        Some("Major".to_string())
    } else if id.starts_with("DungeonAffix_Minor") {
        Some("Minor".to_string())
    } else if id.starts_with("DungeonAffix_Positive") {
        Some("Positive".to_string())
    } else {
        None
    }
}

fn convert_to_power(
    raw_text: &str,
    indexes: &FuzzyIndexes,
    category: FuzzyCategory,
    upstream_method: &'static str,
) -> PowerConversion {
    // Upstream v5.4.0: ConvertToAspect/ConvertToUnique strip digits, brackets
    // and punctuation like ConvertToAffix (previously raw text).
    let text_clean = upstream_convert_to_affix_clean_text(raw_text);
    let fuzzy = indexes.match_text_cached(category, &text_clean);
    PowerConversion {
        upstream_method,
        raw_text: raw_text.to_string(),
        text_clean,
        power_id: fuzzy.accepted.then(|| fuzzy.winner_id.clone()).flatten(),
        power_label: fuzzy.accepted.then(|| fuzzy.winner_label.clone()).flatten(),
        fuzzy,
    }
}

fn split_non_blank_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn find_power_line_in_lines(lines: &[String]) -> Option<(usize, String, u32)> {
    for (index, line) in lines.iter().enumerate().rev() {
        if let Some((digits, value)) = parse_leading_item_power(line) {
            return Some((index, digits, value));
        }
    }
    None
}

fn parse_leading_item_power(line: &str) -> Option<(String, u32)> {
    let regex = LEADING_ITEM_POWER_REGEX.as_ref()?;
    let digits = regex.find(line)?.as_str().to_string();
    if digits.len() < 3 {
        return None;
    }
    let value = digits.parse::<u32>().ok()?;
    if value < MIN_ITEM_POWER {
        return None;
    }
    Some((digits, value))
}

fn build_item_type_candidates(lines: &[String], power_index: Option<usize>) -> Vec<String> {
    if let Some(power_index) = power_index {
        let mut candidates = Vec::new();
        if power_index >= 1 {
            candidates.push(lines[power_index - 1].clone());
        }
        if power_index >= 2 {
            candidates.push(format!(
                "{} {}",
                lines[power_index - 2],
                lines[power_index - 1]
            ));
        }
        return candidates;
    }

    let count = lines.len();
    let mut candidates = Vec::new();
    if count >= 2 {
        candidates.push(lines[count - 2].clone());
    }
    if count >= 3 {
        candidates.push(format!("{} {}", lines[count - 3], lines[count - 2]));
    }
    if count >= 1 {
        candidates.push(lines[count - 1].clone());
    }
    if count >= 2 {
        candidates.push(format!("{} {}", lines[count - 2], lines[count - 1]));
    }
    candidates
}

#[cfg(test)]
mod tests;
