//! OCR text cleanup shared by the OCR trace and the parser.

/// Mirrors upstream `OcrHandler.ConvertToAffix` text-cleaning before fuzzy matching.
pub fn upstream_convert_to_affix_clean_text(raw_text: &str) -> String {
    raw_text
        .replace('\n', " ")
        .chars()
        .filter(|character| {
            !character.is_ascii_digit()
                && !matches!(
                    character,
                    '[' | ']' | '(' | ')' | '+' | '-' | '.' | ',' | '%'
                )
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Mirrors the one-, two- and three-line candidates of upstream
/// `OcrHandler.ConvertToAffixMultiline`.
pub fn upstream_affix_multiline_candidates(raw_text: &str) -> Vec<String> {
    let lines = raw_text
        .split('\n')
        .map(upstream_convert_to_affix_clean_text)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    let mut candidates = Vec::new();
    if !lines.is_empty() {
        candidates.push(lines[0].trim().to_string());
    }
    if lines.len() >= 2 {
        candidates.push(format!("{} {}", lines[0].trim(), lines[1].trim()));
    }
    if lines.len() >= 3 {
        candidates.push(format!(
            "{} {} {}",
            lines[0].trim(),
            lines[1].trim(),
            lines[2].trim()
        ));
    }
    candidates
}
