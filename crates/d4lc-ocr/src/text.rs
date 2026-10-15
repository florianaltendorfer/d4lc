use crate::{upstream_convert_to_affix_clean_text, OcrSegmentKind};
use d4lc_core::text::upstream_affix_multiline_candidates;
use d4lc_core::{OcrBoundingBoxSource, OcrLine, OcrText, OcrWord, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrConversionRoute {
    pub source_method: &'static str,
    pub branch: &'static str,
}

/// Upstream OCR conversion method selected after raw Tesseract text extraction.
pub fn upstream_conversion_route(
    segment_kind: OcrSegmentKind,
    socket_detection_enabled: bool,
) -> OcrConversionRoute {
    match segment_kind {
        OcrSegmentKind::UpperTooltip => OcrConversionRoute {
            source_method: "OcrHandler.ConvertToTextUpperTooltipSection",
            branch: "fixed_segment_route",
        },
        OcrSegmentKind::Affix => {
            if socket_detection_enabled {
                OcrConversionRoute {
                    source_method: "OcrHandler.ConvertToAffix",
                    branch: "socket_detection_enabled",
                }
            } else {
                OcrConversionRoute {
                    source_method: "OcrHandler.ConvertToAffixMultiline",
                    branch: "socket_detection_disabled",
                }
            }
        }
        OcrSegmentKind::Aspect => OcrConversionRoute {
            source_method: "OcrHandler.ConvertToAspect",
            branch: "fixed_segment_route",
        },
        OcrSegmentKind::UniqueAspect => OcrConversionRoute {
            source_method: "OcrHandler.ConvertToUnique",
            branch: "fixed_segment_route",
        },
        OcrSegmentKind::Rune => OcrConversionRoute {
            source_method: "OcrHandler.ConvertToRune",
            branch: "fixed_segment_route",
        },
        OcrSegmentKind::Sigil => OcrConversionRoute {
            source_method: "OcrHandler.ConvertToSigil",
            branch: "fixed_segment_route",
        },
        OcrSegmentKind::Socket => OcrConversionRoute {
            source_method: "ScreenProcessHandler.FindItemSocketLocations: no OCR text conversion",
            branch: "fixed_segment_route",
        },
        OcrSegmentKind::Splitter => OcrConversionRoute {
            source_method: "ScreenProcessHandler.FindItemSplitterLocations: no OCR text conversion",
            branch: "fixed_segment_route",
        },
        OcrSegmentKind::Generic => OcrConversionRoute {
            source_method: "OcrHandler.ConvertToText",
            branch: "fixed_segment_route",
        },
    }
}

pub fn upstream_conversion_method(segment_kind: OcrSegmentKind) -> &'static str {
    upstream_conversion_route(segment_kind, true).source_method
}

/// Mirrors upstream `OcrHandler.ConvertToAspect` and `ConvertToUnique` text
/// cleaning. Since v5.4.0 both strip digits/brackets/punctuation like
/// `ConvertToAffix` (previously the raw text was kept).
pub fn upstream_convert_aspect_unique_clean_text(raw_text: &str) -> String {
    upstream_convert_to_affix_clean_text(raw_text)
}

pub fn upstream_convert_to_affix_multiline_text(raw_text: &str) -> String {
    upstream_affix_multiline_candidates(raw_text)
        .into_iter()
        .max_by_key(|candidate| candidate.len())
        .unwrap_or_default()
}

/// Mirrors the text-cleaning stage of upstream conversion methods.
///
/// This deliberately does not perform fuzzy matching or item-data lookup; it
/// only exposes the upstream-equivalent conversion decision in traces.
pub fn upstream_converted_text(segment_kind: OcrSegmentKind, raw_text: &str) -> String {
    upstream_converted_text_for_route(upstream_conversion_route(segment_kind, true), raw_text)
}

pub fn upstream_converted_text_for_route(route: OcrConversionRoute, raw_text: &str) -> String {
    match route.source_method {
        "OcrHandler.ConvertToAffixMultiline" => upstream_convert_to_affix_multiline_text(raw_text),
        _ => upstream_converted_text_for_method(route.source_method, raw_text),
    }
}

fn upstream_converted_text_for_method(source_method: &str, raw_text: &str) -> String {
    match source_method {
        "OcrHandler.ConvertToAffix" => upstream_convert_to_affix_clean_text(raw_text),
        "OcrHandler.ConvertToAspect" | "OcrHandler.ConvertToUnique" => {
            upstream_convert_aspect_unique_clean_text(raw_text)
        }
        "OcrHandler.ConvertToSigil" => raw_text.split('\n').next().unwrap_or_default().to_string(),
        "OcrHandler.ConvertToRune" => raw_text
            .split("\n\n")
            .next()
            .unwrap_or_default()
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect::<Vec<_>>()
            .join(" "),
        _ => raw_text.to_string(),
    }
}

pub fn tesseract_language_code(language: &str) -> &'static str {
    match language {
        "deDE" => "deu",
        "enUS" => "eng",
        "esES" | "esMX" => "spa",
        "frFR" => "fra",
        "itIT" => "ita",
        "jaJP" => "jpn",
        "koKR" => "kor",
        "plPL" => "pol",
        "ptBR" => "por",
        "ruRU" => "rus",
        "trTR" => "tur",
        "zhCN" => "chi_sim",
        "zhTW" => "chi_tra",
        _ => "eng",
    }
}

pub fn text_from_lines(lines: &[OcrLine]) -> String {
    lines
        .iter()
        .map(|line| line.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn upstream_convert_to_text(lines: &[OcrLine], fallback_raw_text: &str) -> String {
    if lines.is_empty() {
        return fallback_raw_text.to_string();
    }

    let first_line_x = lines[0].rect.x;
    let max_left_x = first_line_x.saturating_add(20);
    lines
        .iter()
        .filter(|line| line.rect == Rect::default() || line.rect.x <= max_left_x)
        .map(|line| line.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_tesseract_tsv_lines(tsv: &str, fallback_raw_text: &str) -> OcrText {
    let mut grouped: Vec<TsvLineGroup> = Vec::new();
    for row in tsv.lines().skip(1) {
        let fields = row.splitn(12, '\t').collect::<Vec<_>>();
        if fields.len() < 12 {
            continue;
        }
        let level = fields[0];
        if level != "4" && level != "5" {
            continue;
        }
        let key = tsv_line_key(&fields);
        let Some(rect) = parse_tsv_rect(&fields) else {
            continue;
        };
        if level == "4" {
            if let Some(group) = grouped.iter_mut().find(|group| group.key == key) {
                group.rect = rect;
                group.has_explicit_line_rect = true;
            } else {
                grouped.push(TsvLineGroup {
                    key,
                    text: String::new(),
                    rect,
                    words: Vec::new(),
                    has_explicit_line_rect: true,
                });
            }
            continue;
        }

        let text = fields[11].trim();
        if text.is_empty() {
            continue;
        }
        let word = OcrWord {
            text: text.to_string(),
            rect,
        };
        if let Some(group) = grouped.iter_mut().find(|group| group.key == key) {
            if !group.text.is_empty() {
                group.text.push(' ');
            }
            group.text.push_str(text);
            if !group.has_explicit_line_rect {
                group.rect = union_rect(group.rect, rect);
            }
            group.words.push(word);
        } else {
            grouped.push(TsvLineGroup {
                key,
                text: text.to_string(),
                rect,
                words: vec![word],
                has_explicit_line_rect: false,
            });
        }
    }

    // Upstream v5.4.0 dropped the hOcr line-regex plus offsetBoundingBox=10
    // containment matching from ParseHOcrText; line membership now comes purely
    // from the structural TSV grouping here (equivalent of `page.Layout`
    // iteration in ConvertToText) and from word geometry in
    // `parse_upper_tooltip_text` (the ParseHOcrText port).
    let bounding_box_source = if grouped.is_empty() {
        OcrBoundingBoxSource::Unavailable
    } else if grouped.iter().all(|group| group.has_explicit_line_rect) {
        OcrBoundingBoxSource::TesseractTsv
    } else {
        OcrBoundingBoxSource::TesseractTsvWordsGrouped
    };
    let lines = grouped
        .into_iter()
        .map(|group| OcrLine {
            text: group.text,
            rect: group.rect,
            words: group.words,
        })
        .collect::<Vec<_>>();

    if lines.is_empty() {
        return fallback_text(fallback_raw_text);
    }

    let raw_text = text_from_lines(&lines);
    OcrText {
        raw_text,
        lines,
        bounding_box_source,
    }
}

fn fallback_text(raw_text: &str) -> OcrText {
    let lines = raw_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| OcrLine {
            text: line.to_string(),
            rect: Rect::default(),
            words: Vec::new(),
        })
        .collect::<Vec<_>>();
    OcrText {
        raw_text: raw_text.to_string(),
        lines,
        bounding_box_source: OcrBoundingBoxSource::Unavailable,
    }
}

fn parse_tsv_rect(fields: &[&str]) -> Option<Rect> {
    let x = fields.get(6)?.parse::<i32>().ok()?;
    let y = fields.get(7)?.parse::<i32>().ok()?;
    let width = fields.get(8)?.parse::<u32>().ok()?;
    let height = fields.get(9)?.parse::<u32>().ok()?;
    Some(Rect {
        x,
        y,
        width,
        height,
    })
}

/// Upstream `maxWordDistance` in `OcrHandler.ParseHOcrText`.
const UPSTREAM_MAX_WORD_DISTANCE: i64 = 20;

/// Upstream minimum Tier 1 item power used by the power-line scan.
const MIN_UPSTREAM_ITEM_POWER: u32 = 150;

/// One reconstructed tooltip line while mirroring upstream `HOcrLine`.
struct HocrLine {
    text: String,
    x1: i64,
}

/// Mirrors upstream `OcrHandler.ParseHOcrText` (v5.4.0 word-geometry rewrite).
///
/// Upstream parses `page.HOcrText()` word spans; the Rust OCR backend exposes
/// the same word stream through the Tesseract TSV (level 5 rows, identical
/// reading order). Lines are segmented purely from word geometry:
///
/// - new line when `|dy| > 20` (Y1 distance between neighbouring words)
/// - new line when both words start within `|dx| < 5` (same line start)
/// - new line when `dx < -5` (previous word is right of current word)
/// - new line when the word starts more than `maxWordDistance = 20` past the
///   previous word end (whitespace gap too large)
///
/// zhCN lines join words without spaces. The item-power line must satisfy
/// `X1 <= 100`; when found, lines below it are dropped and remaining lines
/// whose `X1` deviates by more than 10 from the power line `X1` are removed.
/// Generic artifact removal drops lines shorter than 3 characters, lines with
/// `X1 > 100` and lines with `X1 == 0`.
///
/// `language` mirrors upstream `_settingsManager.Settings.SelectedAffixLanguage`.
pub fn parse_upper_tooltip_text(tsv: &str, language: &str, fallback_raw_text: &str) -> OcrText {
    let mut words: Vec<OcrWord> = Vec::new();
    for row in tsv.lines().skip(1) {
        let fields = row.splitn(12, '\t').collect::<Vec<_>>();
        if fields.len() < 12 || fields[0] != "5" {
            continue;
        }
        let text = fields[11].trim();
        // hOCR never emits empty word spans; TSV emits alignment filler rows.
        if text.is_empty() {
            continue;
        }
        let Some(rect) = parse_tsv_rect(&fields) else {
            continue;
        };
        words.push(OcrWord {
            text: text.to_string(),
            rect,
        });
    }

    if words.is_empty() {
        return fallback_text(fallback_raw_text);
    }

    // Word-geometry line segmentation (upstream lineCounter + four cases).
    let mut line_counter = 1usize;
    let mut word_lines = Vec::with_capacity(words.len());
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            let previous = &words[index - 1];
            let x1 = i64::from(word.rect.x);
            let y1 = i64::from(word.rect.y);
            let previous_x1 = i64::from(previous.rect.x);
            let previous_y1 = i64::from(previous.rect.y);
            let previous_x2 = i64::from(previous.rect.x) + i64::from(previous.rect.width);
            let dy = y1 - previous_y1;
            let dx = x1 - previous_x1;
            if !(-UPSTREAM_MAX_WORD_DISTANCE..=UPSTREAM_MAX_WORD_DISTANCE).contains(&dy) {
                // Case 1: Previous word y-pos differs too much from current word.
                line_counter += 1;
            } else if (-5_i64..5_i64).contains(&dx) {
                // Case 2: Previous word is at the same start position as the current word.
                line_counter += 1;
            } else if dx < -5 {
                // Case 3: Previous word is on the right side of current word.
                line_counter += 1;
            } else if x1 > previous_x2 && x1 - previous_x2 > UPSTREAM_MAX_WORD_DISTANCE {
                // Case 4: Previous word is on the left side of current word.
                //         But there is too much whitespace between the words.
                line_counter += 1;
            }
        }
        word_lines.push(line_counter);
    }

    // Add words to their corresponding line. Upstream stores Y1/X2/Y2 as 0 and
    // keeps only X1 (the minimum word X1) for the artifact-removal rules.
    let mut lines: Vec<HocrLine> = Vec::with_capacity(line_counter);
    for line_number in 1..=line_counter {
        let selected = words
            .iter()
            .zip(&word_lines)
            .filter(|(_, word_line)| **word_line == line_number)
            .map(|(word, _)| word)
            .collect::<Vec<_>>();
        // Every line number was assigned to at least one word above.
        let x1 = selected
            .iter()
            .map(|word| i64::from(word.rect.x))
            .min()
            .unwrap_or_default();
        let text = if language.eq_ignore_ascii_case("zhCN") {
            selected
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>()
        } else {
            selected
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        lines.push(HocrLine { text, x1 });
    }

    // Find item power line.
    let mut power_index = None;
    let mut power_x1 = 0i64;
    for (index, line) in lines.iter().enumerate().rev() {
        if line.x1 > 100 {
            continue;
        }
        let digits = leading_digits(&line.text);
        // Item power 150 is the minimum value for Tier 1 items.
        if digits.len() >= 3
            && digits
                .parse::<u32>()
                .map(|value| value >= MIN_UPSTREAM_ITEM_POWER)
                .unwrap_or(false)
        {
            power_index = Some(index);
            power_x1 = line.x1;
            break;
        }
    }

    if let Some(power_index) = power_index {
        // Removed lines below item power.
        lines.truncate(power_index + 1);
        // Remove artifacts based on item power x-pos. The power line is the
        // last element while upstream evaluates `lines[powerIndex].X1`, so the
        // comparison target never shifts: a fixed powerX1 comparison.
        lines.retain(|line| (line.x1 - power_x1).abs() <= 10);
    }

    // Remove artifacts.
    // - Lines smaller than 3 characters.
    // - Lines not starting on the left side.
    // - Lines starting exactly at x-pos 0.
    lines.retain(|line| line.text.chars().count() >= 3);
    lines.retain(|line| line.x1 <= 100);
    lines.retain(|line| line.x1 != 0);

    let ocr_lines = lines
        .into_iter()
        .map(|line| OcrLine {
            text: line.text,
            rect: Rect {
                x: i32::try_from(line.x1).unwrap_or(i32::MAX),
                y: 0,
                width: 0,
                height: 0,
            },
            words: Vec::new(),
        })
        .collect::<Vec<_>>();

    OcrText {
        raw_text: ocr_lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        bounding_box_source: OcrBoundingBoxSource::TesseractTsvWordsGrouped,
        lines: ocr_lines,
    }
}

fn leading_digits(text: &str) -> &str {
    let end = text
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    &text[..end]
}

fn union_rect(left: Rect, right: Rect) -> Rect {
    let min_x = left.x.min(right.x);
    let min_y = left.y.min(right.y);
    let max_x = i64::from(left.x)
        .saturating_add(i64::from(left.width))
        .max(i64::from(right.x).saturating_add(i64::from(right.width)));
    let max_y = i64::from(left.y)
        .saturating_add(i64::from(left.height))
        .max(i64::from(right.y).saturating_add(i64::from(right.height)));
    Rect {
        x: min_x,
        y: min_y,
        width: u32::try_from(max_x.saturating_sub(i64::from(min_x))).unwrap_or(u32::MAX),
        height: u32::try_from(max_y.saturating_sub(i64::from(min_y))).unwrap_or(u32::MAX),
    }
}

#[cfg(any(feature = "leptess-backend", test))]
pub(crate) fn raw_text_to_ocr_text(raw_text: &str, width: u32, height: u32) -> OcrText {
    let lines = raw_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let rect = Rect {
                x: 0,
                y: 0,
                width,
                height,
            };
            OcrLine {
                text: line.to_string(),
                rect,
                words: vec![OcrWord {
                    text: line.to_string(),
                    rect,
                }],
            }
        })
        .collect();

    OcrText {
        raw_text: raw_text.to_string(),
        lines,
        bounding_box_source: OcrBoundingBoxSource::SyntheticFullImage,
    }
}

fn tsv_line_key(fields: &[&str]) -> String {
    format!(
        "{}:{}:{}:{}",
        fields.get(1).copied().unwrap_or_default(),
        fields.get(2).copied().unwrap_or_default(),
        fields.get(3).copied().unwrap_or_default(),
        fields.get(4).copied().unwrap_or_default()
    )
}

#[derive(Debug)]
struct TsvLineGroup {
    key: String,
    text: String,
    rect: Rect,
    words: Vec<OcrWord>,
    has_explicit_line_rect: bool,
}

#[cfg(test)]
mod tests;
