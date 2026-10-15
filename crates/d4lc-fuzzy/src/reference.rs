//! The former full-scoring matcher, kept verbatim as the oracle the cutoff
//! matcher in `match_text` is proven identical against (same winner, same
//! score bits) over a seeded corpus.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use d4lc_assets::AssetRepository;
use std::path::{Path, PathBuf};

pub(crate) fn reference_match_text(
    indexes: &FuzzyIndexes,
    category: FuzzyCategory,
    raw_text: &str,
) -> FuzzyMatchResult {
    let index = indexes.category_index(category);
    let threshold = indexes.threshold_for(category);
    let normalized_text = normalize(raw_text, index.normalization.disable_preprocessor);

    let mut winner: Option<&Candidate> = None;
    let mut winner_score: Option<f64> = None;
    for candidate in &index.candidates {
        let normalized_label: String = candidate.text.chars.iter().collect();
        let score = score_match(index.scorer, &normalized_text, &normalized_label);
        if winner_score.is_none_or(|current| score > current) {
            winner = Some(candidate);
            winner_score = Some(score);
        }
    }

    let accepted = winner_score.is_some_and(|score| score >= threshold);
    let rejection_reason = if accepted {
        None
    } else if winner_score.is_some() {
        Some("best score below threshold".to_string())
    } else {
        Some("no candidates available".to_string())
    };
    FuzzyMatchResult {
        category,
        upstream_method: upstream_match_method(category),
        raw_text: raw_text.to_string(),
        normalized_text,
        winner_id: winner.map(|c| c.id.clone()),
        winner_label: winner.map(|c| c.label.clone()),
        score: winner_score,
        threshold,
        candidate_count: index.candidates.len(),
        accepted,
        rejection_reason,
        normalization: index.normalization.clone(),
    }
}

fn score_match(scorer: Scorer, left: &str, right: &str) -> f64 {
    match scorer {
        Scorer::Ratio => ratio_score(left, right),
        Scorer::TokenSet => token_set_score(left, right),
    }
}

fn ratio_score(left: &str, right: &str) -> f64 {
    if left == right {
        return 100.0;
    }
    let max_len = left.chars().count().max(right.chars().count());
    if max_len == 0 {
        return 100.0;
    }
    let dist = levenshtein(left, right);
    ((max_len.saturating_sub(dist)) as f64 / max_len as f64) * 100.0
}

fn token_set_score(left: &str, right: &str) -> f64 {
    if left == right {
        return 100.0;
    }
    let left_tokens: Vec<&str> = left.split_whitespace().collect();
    let right_tokens: Vec<&str> = right.split_whitespace().collect();
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return ratio_score(left, right);
    }

    let mut shared = Vec::new();
    let mut left_only = Vec::new();
    let mut right_only = Vec::new();

    for token in &left_tokens {
        if right_tokens.contains(token) {
            shared.push(*token);
        } else {
            left_only.push(*token);
        }
    }
    for token in &right_tokens {
        if !left_tokens.contains(token) {
            right_only.push(*token);
        }
    }

    let shared_s = shared.join(" ");
    let left_join = if left_only.is_empty() {
        shared_s.clone()
    } else {
        format!("{shared_s} {}", left_only.join(" "))
            .trim()
            .to_string()
    };
    let right_join = if right_only.is_empty() {
        shared_s.clone()
    } else {
        format!("{shared_s} {}", right_only.join(" "))
            .trim()
            .to_string()
    };

    ratio_score(&shared_s, &left_join)
        .max(ratio_score(&shared_s, &right_join))
        .max(ratio_score(&left_join, &right_join))
}

fn levenshtein(left: &str, right: &str) -> usize {
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    if left_chars.is_empty() {
        return right_chars.len();
    }
    if right_chars.is_empty() {
        return left_chars.len();
    }

    let mut prev: Vec<usize> = (0..=right_chars.len()).collect();
    let mut curr = vec![0; right_chars.len() + 1];

    for (i, left_char) in left_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, right_char) in right_chars.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[right_chars.len()]
}

const CATEGORIES: [FuzzyCategory; 6] = [
    FuzzyCategory::ItemType,
    FuzzyCategory::Affix,
    FuzzyCategory::Aspect,
    FuzzyCategory::Unique,
    FuzzyCategory::Rune,
    FuzzyCategory::Sigil,
];

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

/// Applies `edits` random single-char insertions, deletions, substitutions
/// (letters, digits, spaces, punctuation), like OCR misreads.
fn edited(text: &str, edits: usize, rng: &mut XorShift64) -> String {
    const ALPHABET: &[char] = &[
        'a', 'e', 'i', 'l', 'o', 'r', 's', 't', 'x', 'z', '1', '0', '5', ' ', '.', '%', '+', 'é',
    ];
    let mut chars: Vec<char> = text.chars().collect();
    for _ in 0..edits {
        let replacement = ALPHABET[rng.below(ALPHABET.len())];
        match rng.below(3) {
            0 if !chars.is_empty() => {
                let at = rng.below(chars.len());
                chars.remove(at);
            }
            1 if !chars.is_empty() => {
                let at = rng.below(chars.len());
                chars[at] = replacement;
            }
            _ => {
                let at = rng.below(chars.len() + 1);
                chars.insert(at, replacement);
            }
        }
    }
    chars.into_iter().collect()
}

/// Token-level variants: shuffled, duplicated, truncated, joined with
/// another label — the shapes TokenSet treats specially.
fn token_variant(text: &str, other: &str, rng: &mut XorShift64) -> String {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    match rng.below(4) {
        0 => {
            for index in (1..tokens.len()).rev() {
                tokens.swap(index, rng.below(index + 1));
            }
            tokens.join(" ")
        }
        1 => {
            if let Some(&token) = tokens.get(rng.below(tokens.len())) {
                tokens.insert(rng.below(tokens.len() + 1), token);
            }
            tokens.join(" ")
        }
        2 => {
            tokens.truncate(rng.below(tokens.len() + 1));
            tokens.join(" ")
        }
        _ => format!("{text} {other}"),
    }
}

/// Every label of the category, 1-, 2- and 3-edit variants of each, plus
/// token-level variants and edge inputs.
fn category_corpus(
    indexes: &FuzzyIndexes,
    category: FuzzyCategory,
    variants_per_label: usize,
    seed: u64,
) -> Vec<String> {
    let labels: Vec<&str> = indexes.category_index(category).labels().collect();
    let mut rng = XorShift64(seed);
    let mut corpus: Vec<String> = ["", "   ", "+.%", "x", "the the the", "Legendary Amulet"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    for label in &labels {
        corpus.push((*label).to_string());
        for variant in 0..variants_per_label {
            let other = labels[rng.below(labels.len())];
            corpus.push(match variant % 5 {
                0..=2 => edited(label, variant % 5 + 1, &mut rng),
                3 => token_variant(label, other, &mut rng),
                _ => edited(&token_variant(label, other, &mut rng), 2, &mut rng),
            });
        }
    }
    corpus
}

fn assert_identical(indexes: &FuzzyIndexes, category: FuzzyCategory, text: &str) {
    let fast = indexes.match_text(category, text);
    let reference = reference_match_text(indexes, category, text);
    assert_eq!(
        fast.winner_id, reference.winner_id,
        "{category:?} winner for {text:?}"
    );
    assert_eq!(
        fast.score.map(f64::to_bits),
        reference.score.map(f64::to_bits),
        "{category:?} score for {text:?}"
    );
    assert_eq!(fast, reference, "{category:?} result for {text:?}");
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// OCR text recorded by earlier live and fixture runs (`text`, `raw_text`
/// and `normalized_lines` strings of the exported chain and bundle JSON).
/// These are not committed; they widen the corpus where present.
fn recorded_ocr_lines(root: &Path) -> Vec<String> {
    fn collect(value: &Value, key: Option<&str>, out: &mut Vec<String>) {
        match value {
            Value::String(text)
                if matches!(key, Some("text" | "raw_text" | "normalized_lines")) =>
            {
                out.extend(
                    text.lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(ToOwned::to_owned),
                );
            }
            Value::Array(values) => values.iter().for_each(|value| collect(value, key, out)),
            Value::Object(map) => map
                .iter()
                .for_each(|(key, value)| collect(value, Some(key), out)),
            _ => {}
        }
    }
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                walk(&path, files);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("debug-output/ocr-preprocess"), &mut files);
    walk(&root.join("debug-output/manual-diablo-iv"), &mut files);
    files.sort();
    let mut lines = Vec::new();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        if let Ok(json) = serde_json::from_str::<Value>(&text) {
            collect(&json, None, &mut lines);
        }
    }
    lines.sort();
    lines.dedup();
    lines
}

#[test]
fn cutoff_matcher_equals_reference_on_a_sampled_corpus() {
    let repo = AssetRepository::new(workspace_root());
    let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap();
    for (seed, category) in (1_u64..).zip(CATEGORIES) {
        // Every label and one edited variant; the TokenSet categories are
        // sampled (every third label) to keep the default suite fast. The
        // ignored test below runs the full corpus.
        let step = match indexes.category_index(category).scorer {
            Scorer::Ratio => 1,
            Scorer::TokenSet => 3,
        };
        for text in category_corpus(&indexes, category, 1, seed)
            .iter()
            .step_by(step)
        {
            assert_identical(&indexes, category, text);
        }
    }
}

#[test]
fn bounded_levenshtein_equals_the_full_distance_or_exceeds_the_limit() {
    let mut rng = XorShift64(0x5eed);
    let (mut prev, mut curr) = (Vec::new(), Vec::new());
    for _ in 0..4000 {
        let base = edited("the quick brown fox", rng.below(6), &mut rng);
        let other = edited(&base, rng.below(12), &mut rng);
        let full = levenshtein(&base, &other);
        let left: Vec<char> = base.chars().collect();
        let right: Vec<char> = other.chars().collect();
        for limit in 0..=full + 2 {
            let bounded = bounded_levenshtein(&left, &right, limit, &mut prev, &mut curr);
            assert_eq!(
                bounded,
                (full <= limit).then_some(full),
                "{base:?} vs {other:?} limit {limit}"
            );
        }
    }
}

#[test]
fn max_winning_distance_is_the_exact_ratio_boundary() {
    for max_len in 0..60 {
        for bar in [
            0.0,
            1.0,
            33.333_333_333_333_33,
            50.0,
            66.666_666_666_666_67,
            75.0,
            99.9,
            100.0,
        ]
        .into_iter()
        .chain((0..=max_len).map(|d| ratio_from_distance(max_len, d)))
        {
            let expected = (0..=max_len)
                .filter(|&d| ratio_from_distance(max_len, d) > bar)
                .max();
            assert_eq!(max_winning_distance(max_len, Some(bar)), expected);
        }
        assert_eq!(max_winning_distance(max_len, None), Some(max_len));
    }
}

#[test]
#[ignore = "full seeded corpus (~20k inputs per language) against the reference; run with `cargo test -p d4lc-fuzzy --release full_corpus -- --ignored --nocapture`"]
fn cutoff_matcher_equals_reference_on_the_full_corpus() {
    let root = workspace_root();
    let repo = AssetRepository::new(root.clone());
    let recorded = recorded_ocr_lines(&root);
    for (language, variants) in [("enUS", 7), ("zhCN", 2), ("deDE", 2)] {
        let indexes = FuzzyIndexes::load(&repo, language).unwrap();
        let mut total = 0;
        for (seed, category) in (100_u64..).zip(CATEGORIES) {
            let mut corpus = category_corpus(&indexes, category, variants, seed);
            if language == "enUS" {
                corpus.extend(recorded.iter().cloned());
            }
            for text in &corpus {
                assert_identical(&indexes, category, text);
            }
            total += corpus.len();
        }
        println!(
            "{language}: {total} inputs identical ({} recorded OCR lines per category)",
            if language == "enUS" {
                recorded.len()
            } else {
                0
            }
        );
    }
}
