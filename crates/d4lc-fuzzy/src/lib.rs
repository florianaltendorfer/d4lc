//! d4lc-fuzzy crate.

use d4lc_assets::catalog::{IdentityCatalog, ItemTypeCatalog};
use d4lc_assets::AssetRepository;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FuzzyCategory {
    ItemType,
    Affix,
    Aspect,
    Unique,
    Rune,
    Sigil,
}

impl FuzzyCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ItemType => "item_type",
            Self::Affix => "affix",
            Self::Aspect => "aspect",
            Self::Unique => "unique",
            Self::Rune => "rune",
            Self::Sigil => "sigil",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatchThresholds {
    pub item_type: f64,
    pub affix: f64,
    pub aspect: f64,
    pub unique: f64,
    pub rune: f64,
    pub sigil: f64,
}

impl Default for FuzzyMatchThresholds {
    fn default() -> Self {
        Self {
            item_type: f64::from(d4lc_config::DEFAULT_MINIMAL_OCR_MATCH_TYPE),
            affix: 0.0,
            aspect: 0.0,
            unique: 0.0,
            rune: f64::from(d4lc_config::DEFAULT_MINIMAL_OCR_MATCH_TYPE),
            sigil: 0.0,
        }
    }
}

impl FuzzyMatchThresholds {
    pub fn from_settings(settings: &d4lc_config::AppSettings) -> Self {
        Self {
            item_type: f64::from(settings.minimal_ocr_match_type),
            rune: f64::from(settings.minimal_ocr_match_type),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FuzzyNormalization {
    pub language: String,
    pub disable_preprocessor: bool,
    pub source_value_field: &'static str,
    pub cleanup_steps: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FuzzyMatchResult {
    pub category: FuzzyCategory,
    pub upstream_method: &'static str,
    pub raw_text: String,
    pub normalized_text: String,
    pub winner_id: Option<String>,
    pub winner_label: Option<String>,
    pub score: Option<f64>,
    pub threshold: f64,
    pub candidate_count: usize,
    pub accepted: bool,
    pub rejection_reason: Option<String>,
    pub normalization: FuzzyNormalization,
}

#[derive(Debug, Clone)]
pub struct FuzzyCategoryIndex {
    candidates: Vec<Candidate>,
    normalization: FuzzyNormalization,
    scorer: Scorer,
}

impl FuzzyCategoryIndex {
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    pub fn normalization(&self) -> &FuzzyNormalization {
        &self.normalization
    }

    /// The catalog labels of this category, in load order (benchmark and
    /// diagnostics tooling replays them through [`FuzzyIndexes::match_text`]).
    pub fn labels(&self) -> impl Iterator<Item = &str> + '_ {
        self.candidates
            .iter()
            .map(|candidate| candidate.label.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct FuzzyIndexes {
    thresholds: FuzzyMatchThresholds,
    item_type: FuzzyCategoryIndex,
    affix: FuzzyCategoryIndex,
    aspect: FuzzyCategoryIndex,
    unique: FuzzyCategoryIndex,
    rune: FuzzyCategoryIndex,
    sigil: FuzzyCategoryIndex,
    /// Memo caches keyed per category and case-insensitive input text
    /// (upstream `_cacheAffixes/_cacheAspects/_cacheItemtypes/_cacheUniques`
    /// are `ConcurrentDictionary`s with `StringComparer.OrdinalIgnoreCase`).
    match_cache: Arc<Mutex<HashMap<(FuzzyCategory, String), FuzzyMatchResult>>>,
    /// Mirrors upstream `AffixManager._tuningPrisms` (`AffixInfo.TuningPrisms`
    /// keyed by `IdName`, v5.4.0 `InitTuningPrismData`).
    affix_tuning_prisms: HashMap<String, Vec<String>>,
}

impl FuzzyIndexes {
    pub fn load(repo: &AssetRepository, language: &str) -> Result<Self, String> {
        Self::load_with_thresholds(repo, language, FuzzyMatchThresholds::default())
    }

    pub fn load_with_settings(
        repo: &AssetRepository,
        language: &str,
        settings: &d4lc_config::AppSettings,
    ) -> Result<Self, String> {
        Self::load_with_thresholds(
            repo,
            language,
            FuzzyMatchThresholds::from_settings(settings),
        )
    }

    pub fn load_with_thresholds(
        repo: &AssetRepository,
        language: &str,
        thresholds: FuzzyMatchThresholds,
    ) -> Result<Self, String> {
        let disable_preprocessor = matches!(language, "zhCN" | "zhTW" | "ruRU");

        Ok(Self {
            thresholds,
            item_type: load_index(
                repo,
                language,
                FuzzyCategory::ItemType,
                "Name",
                disable_preprocessor,
                Scorer::Ratio,
            )?,
            affix: load_index(
                repo,
                language,
                FuzzyCategory::Affix,
                "DescriptionClean",
                disable_preprocessor,
                Scorer::Ratio,
            )?,
            aspect: load_index(
                repo,
                language,
                FuzzyCategory::Aspect,
                "DescriptionClean",
                disable_preprocessor,
                Scorer::TokenSet,
            )?,
            unique: load_index(
                repo,
                language,
                FuzzyCategory::Unique,
                "DescriptionClean",
                disable_preprocessor,
                Scorer::TokenSet,
            )?,
            rune: load_index(
                repo,
                language,
                FuzzyCategory::Rune,
                "DescriptionClean",
                disable_preprocessor,
                Scorer::TokenSet,
            )?,
            sigil: load_index(
                repo,
                language,
                FuzzyCategory::Sigil,
                "Name",
                disable_preprocessor,
                Scorer::TokenSet,
            )?,
            match_cache: Arc::new(Mutex::new(HashMap::new())),
            affix_tuning_prisms: load_affix_tuning_prisms(repo, language)?,
        })
    }

    pub fn category_index(&self, category: FuzzyCategory) -> &FuzzyCategoryIndex {
        match category {
            FuzzyCategory::ItemType => &self.item_type,
            FuzzyCategory::Affix => &self.affix,
            FuzzyCategory::Aspect => &self.aspect,
            FuzzyCategory::Unique => &self.unique,
            FuzzyCategory::Rune => &self.rune,
            FuzzyCategory::Sigil => &self.sigil,
        }
    }

    pub fn match_text(&self, category: FuzzyCategory, raw_text: &str) -> FuzzyMatchResult {
        let index = self.category_index(category);
        let threshold = self.threshold_for(category);
        let normalized_text = normalize(raw_text, index.normalization.disable_preprocessor);
        let input = MatchText::new(&normalized_text);
        let mut scratch = MatchScratch::default();

        // A later candidate wins only with a strictly higher score, so each
        // candidate is scored only as far as needed to tell whether it beats
        // the current winner; a winner's score is always computed in full.
        let mut winner: Option<&Candidate> = None;
        let mut winner_score: Option<f64> = None;
        for candidate in &index.candidates {
            if let Some(score) = score_above(
                index.scorer,
                &input,
                &candidate.text,
                winner_score,
                &mut scratch,
            ) {
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

    /// Memoized variant of [`Self::match_text`] mirroring the upstream
    /// `TextToAffixCached` / `TextToAspectCached` / `TextToUniqueCached` /
    /// `TextToItemTypeCached` wrappers (`ConcurrentDictionary.GetOrAdd`).
    ///
    /// The cache key is the category plus the input text compared
    /// case-insensitively (`StringComparer.OrdinalIgnoreCase` upstream). Like
    /// upstream, the first computed result is reused verbatim for later calls
    /// with an equal key, so traces (including the first caller's raw-text
    /// casing) are identical to the uncached computation. Rune and Sigil
    /// lookups are uncached upstream (`TextToRune`/`TextToSigil` are called
    /// directly), so they keep using [`Self::match_text`].
    pub fn match_text_cached(&self, category: FuzzyCategory, raw_text: &str) -> FuzzyMatchResult {
        let key = (category, raw_text.to_lowercase());
        if let Ok(cache) = self.match_cache.lock() {
            if let Some(hit) = cache.get(&key) {
                return hit.clone();
            }
        }
        let result = self.match_text(category, raw_text);
        if let Ok(mut cache) = self.match_cache.lock() {
            // GetOrAdd semantics: the first computed value wins on races.
            cache.entry(key).or_insert_with(|| result.clone());
        }
        result
    }

    pub fn cached_match_count(&self) -> usize {
        self.match_cache
            .lock()
            .map(|cache| cache.len())
            .unwrap_or(0)
    }

    /// Mirrors upstream `AffixManager.GetAffixTuningPrismsByIdName`: the
    /// `TuningPrisms` list recorded for the affix `IdName`, or an empty list.
    pub fn affix_tuning_prisms(&self, affix_id: &str) -> Vec<String> {
        self.affix_tuning_prisms
            .get(affix_id)
            .cloned()
            .unwrap_or_default()
    }

    fn threshold_for(&self, category: FuzzyCategory) -> f64 {
        match category {
            FuzzyCategory::ItemType => self.thresholds.item_type,
            FuzzyCategory::Affix => self.thresholds.affix,
            FuzzyCategory::Aspect => self.thresholds.aspect,
            FuzzyCategory::Unique => self.thresholds.unique,
            FuzzyCategory::Rune => self.thresholds.rune,
            FuzzyCategory::Sigil => self.thresholds.sigil,
        }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    label: String,
    /// The normalized label, prepared once at index load.
    text: MatchText,
}

/// A normalized text as the scorers read it: its chars and its
/// whitespace-separated tokens (`str::split_whitespace`), in order.
#[derive(Debug, Clone, Default)]
struct MatchText {
    chars: Vec<char>,
    /// Char ranges of the tokens in `chars`.
    tokens: Vec<(usize, usize)>,
}

impl MatchText {
    fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let mut tokens = Vec::new();
        let mut start = None;
        for (index, ch) in chars.iter().enumerate() {
            match (ch.is_whitespace(), start) {
                (true, Some(begin)) => {
                    tokens.push((begin, index));
                    start = None;
                }
                (false, None) => start = Some(index),
                _ => {}
            }
        }
        if let Some(begin) = start {
            tokens.push((begin, chars.len()));
        }
        Self { chars, tokens }
    }

    fn tokens(&self) -> impl Iterator<Item = &[char]> + '_ {
        self.tokens
            .iter()
            .map(|&(start, end)| self.chars.get(start..end).unwrap_or_default())
    }

    fn has_token(&self, token: &[char]) -> bool {
        self.tokens().any(|candidate| candidate == token)
    }
}

/// Buffers one `match_text` call reuses across all candidates.
#[derive(Debug, Default)]
struct MatchScratch {
    prev: Vec<usize>,
    curr: Vec<usize>,
    left_join: Vec<char>,
    right_join: Vec<char>,
}

#[derive(Debug, Clone, Copy)]
enum Scorer {
    Ratio,
    TokenSet,
}

fn load_index(
    repo: &AssetRepository,
    language: &str,
    category: FuzzyCategory,
    source_value_field: &'static str,
    disable_preprocessor: bool,
    scorer: Scorer,
) -> Result<FuzzyCategoryIndex, String> {
    // The affix and item type candidate identities come from the canonical
    // catalog index: an affix winner id is the complete `IdName` of
    // the pinned row, an item type winner id is the canonical `Type`
    // value while the per-rarity `Name` stays the display label.
    let entries: Vec<(String, String)> = match category {
        FuzzyCategory::Affix => {
            let catalog = IdentityCatalog::affixes(repo, language)?;
            catalog
                .rows()
                .iter()
                .filter_map(|row| Some((row.id_name.clone(), row.label.clone()?)))
                .collect()
        }
        FuzzyCategory::ItemType => {
            let catalog = ItemTypeCatalog::load(repo, language)?;
            catalog
                .rows()
                .iter()
                .map(|row| (row.type_id.clone(), row.name.clone()))
                .collect()
        }
        _ => json_entries(repo, language, category, source_value_field)?,
    };

    let mut candidates = Vec::with_capacity(entries.len());
    for (id, label) in entries {
        if label.trim().is_empty() {
            continue;
        }
        let text = MatchText::new(&normalize(&label, disable_preprocessor));
        candidates.push(Candidate { id, label, text });
    }

    Ok(FuzzyCategoryIndex {
        candidates,
        normalization: FuzzyNormalization {
            language: language.to_string(),
            disable_preprocessor,
            source_value_field,
            cleanup_steps: normalization_cleanup_steps(disable_preprocessor),
        },
        scorer,
    })
}

/// Candidates read straight from one localized data file — the categories
/// whose identity is already the full row `IdName` and whose candidates the
/// catalog does not index (aspects, uniques, runes, sigils).
fn json_entries(
    repo: &AssetRepository,
    language: &str,
    category: FuzzyCategory,
    source_value_field: &'static str,
) -> Result<Vec<(String, String)>, String> {
    let path = repo.data_dir().join(category_file_name(category, language));
    let text =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let json: Value = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    let items = json
        .as_array()
        .ok_or_else(|| format!("expected JSON array in {}", path.display()))?;

    let mut entries = Vec::with_capacity(items.len());
    let mut first_label_id: HashMap<String, String> = HashMap::new();
    for item in items {
        let label = candidate_label(category, item, source_value_field)?;
        if label.trim().is_empty() {
            continue;
        }
        let id = candidate_id(category, item, &label, &mut first_label_id)?;
        entries.push((id, label));
    }
    Ok(entries)
}

fn category_file_name(category: FuzzyCategory, language: &str) -> String {
    let stem = match category {
        FuzzyCategory::ItemType => "ItemTypes",
        FuzzyCategory::Affix => "Affixes",
        FuzzyCategory::Aspect => "Aspects",
        FuzzyCategory::Unique => "Uniques",
        FuzzyCategory::Rune => "Runes",
        FuzzyCategory::Sigil => "Sigils",
    };
    format!("{stem}.{language}.json")
}

fn load_affix_tuning_prisms(
    repo: &AssetRepository,
    language: &str,
) -> Result<HashMap<String, Vec<String>>, String> {
    let path = repo.data_dir().join(format!("Affixes.{language}.json"));
    let text =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let json: Value = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    let items = json
        .as_array()
        .ok_or_else(|| format!("expected JSON array in {}", path.display()))?;

    let mut prisms = HashMap::new();
    for item in items {
        let Some(id_name) = string_field(item, "IdName") else {
            continue;
        };
        let values = item
            .get("TuningPrisms")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        prisms.insert(id_name, values);
    }
    Ok(prisms)
}

fn candidate_label(
    category: FuzzyCategory,
    item: &Value,
    source_value_field: &str,
) -> Result<String, String> {
    let base = string_field(item, source_value_field)
        .ok_or_else(|| format!("missing '{}' in source item", source_value_field))?;
    if matches!(category, FuzzyCategory::Sigil)
        && string_field(item, "Type").is_some_and(|kind| kind == "Dungeon")
    {
        let suffix = string_field(item, "DungeonZoneInfo").unwrap_or_default();
        if suffix.is_empty() {
            Ok(base)
        } else {
            Ok(format!("{base} {suffix}"))
        }
    } else {
        Ok(base)
    }
}

fn candidate_id(
    category: FuzzyCategory,
    item: &Value,
    label: &str,
    first_label_id: &mut HashMap<String, String>,
) -> Result<String, String> {
    let raw_id = match category {
        FuzzyCategory::ItemType => string_field(item, "Type")
            .or_else(|| string_field(item, "Name"))
            .ok_or_else(|| "missing item type id".to_string())?,
        FuzzyCategory::Affix => string_field(item, "IdName")
            .or_else(|| string_field(item, "IdSno"))
            .ok_or_else(|| "missing affix id".to_string())?,
        FuzzyCategory::Aspect => string_field(item, "IdName")
            .or_else(|| string_field(item, "IdSno"))
            .ok_or_else(|| "missing aspect id".to_string())?,
        FuzzyCategory::Unique => string_field(item, "IdName")
            .or_else(|| string_field(item, "IdSno"))
            .ok_or_else(|| "missing unique id".to_string())?,
        FuzzyCategory::Rune => string_field(item, "IdName")
            .or_else(|| string_field(item, "IdSno"))
            .ok_or_else(|| "missing rune id".to_string())?,
        FuzzyCategory::Sigil => string_field(item, "IdName")
            .or_else(|| string_field(item, "IdSno"))
            .ok_or_else(|| "missing sigil id".to_string())?,
    };

    if matches!(category, FuzzyCategory::Unique | FuzzyCategory::Sigil) {
        let entry = first_label_id
            .entry(label.to_string())
            .or_insert_with(|| raw_id.clone());
        Ok(entry.clone())
    } else {
        Ok(raw_id)
    }
}

fn string_field(item: &Value, field: &str) -> Option<String> {
    item.get(field).map(|value| match value {
        Value::String(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    })
}

fn normalize(value: &str, disable_preprocessor: bool) -> String {
    let trimmed = value.trim();
    if disable_preprocessor {
        return trimmed.to_string();
    }
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            for lowered in ch.to_lowercase() {
                out.push(lowered);
            }
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalization_cleanup_steps(disable_preprocessor: bool) -> Vec<&'static str> {
    if disable_preprocessor {
        vec!["trim", "disable_preprocessor_passthrough"]
    } else {
        vec![
            "trim",
            "unicode_lowercase",
            "replace_punctuation_with_space",
            "collapse_whitespace",
        ]
    }
}

fn upstream_match_method(category: FuzzyCategory) -> &'static str {
    match category {
        FuzzyCategory::ItemType => "OcrHandler.TextToItemType",
        FuzzyCategory::Affix => "OcrHandler.TextToAffix",
        FuzzyCategory::Aspect => "OcrHandler.TextToAspect",
        FuzzyCategory::Unique => "OcrHandler.TextToUnique",
        FuzzyCategory::Rune => "OcrHandler.TextToRune",
        FuzzyCategory::Sigil => "OcrHandler.TextToSigil",
    }
}

/// The candidate's score when it is strictly greater than `bar` (any score
/// when there is no bar yet), else `None`. The returned score is
/// bit-identical to the full computation.
fn score_above(
    scorer: Scorer,
    input: &MatchText,
    candidate: &MatchText,
    bar: Option<f64>,
    scratch: &mut MatchScratch,
) -> Option<f64> {
    match scorer {
        Scorer::Ratio => ratio_above(&input.chars, &candidate.chars, bar, scratch),
        Scorer::TokenSet => token_set_above(input, candidate, bar, scratch),
    }
}

/// `(max_len - distance) / max_len * 100`, the ratio of an edit distance.
fn ratio_from_distance(max_len: usize, distance: usize) -> f64 {
    if max_len == 0 {
        return 100.0;
    }
    ((max_len.saturating_sub(distance)) as f64 / max_len as f64) * 100.0
}

/// The largest edit distance whose ratio is still strictly greater than
/// `bar`, or `None` when not even distance 0 beats it. The ratio falls
/// monotonically with the distance, so a binary search over the exact
/// floating-point ratio finds the boundary.
fn max_winning_distance(max_len: usize, bar: Option<f64>) -> Option<usize> {
    let Some(bar) = bar else {
        return Some(max_len);
    };
    if ratio_from_distance(max_len, 0) <= bar {
        return None;
    }
    // Invariant: `low` beats the bar; everything above `high` does not.
    let (mut low, mut high) = (0, max_len);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if ratio_from_distance(max_len, mid) > bar {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Some(low)
}

fn ratio_above(
    left: &[char],
    right: &[char],
    bar: Option<f64>,
    scratch: &mut MatchScratch,
) -> Option<f64> {
    let max_len = left.len().max(right.len());
    let limit = max_winning_distance(max_len, bar)?;
    if left == right {
        return Some(100.0);
    }
    // Every alignment needs at least the length difference in edits.
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }
    let distance = bounded_levenshtein(left, right, limit, &mut scratch.prev, &mut scratch.curr)?;
    Some(ratio_from_distance(max_len, distance))
}

fn token_set_above(
    input: &MatchText,
    candidate: &MatchText,
    bar: Option<f64>,
    scratch: &mut MatchScratch,
) -> Option<f64> {
    if input.chars == candidate.chars {
        return max_winning_distance(0, bar).map(|_| 100.0);
    }
    if input.tokens.is_empty() || candidate.tokens.is_empty() {
        return ratio_above(&input.chars, &candidate.chars, bar, scratch);
    }

    // shared = input tokens found in the candidate (input order),
    // left_join = shared + input-only tokens, right_join = shared +
    // candidate-only tokens, each joined by single spaces.
    let MatchScratch {
        prev,
        curr,
        left_join,
        right_join,
    } = scratch;
    left_join.clear();
    right_join.clear();
    for token in input.tokens().filter(|token| candidate.has_token(token)) {
        push_token(left_join, token);
    }
    let shared_len = left_join.len();
    right_join.extend_from_slice(left_join);
    for token in input.tokens().filter(|token| !candidate.has_token(token)) {
        push_token(left_join, token);
    }
    for token in candidate.tokens().filter(|token| !input.has_token(token)) {
        push_token(right_join, token);
    }

    // `shared` is a prefix of both joins, so its distance to either is the
    // appended length exactly.
    let prefix_ratio = |join_len: usize| {
        let max_len = shared_len.max(join_len);
        ratio_from_distance(max_len, max_len - shared_len.min(join_len))
    };
    // `best` is only ever set above `bar`, so it becomes the new bar.
    let mut best: Option<f64> = None;
    for score in [
        prefix_ratio(left_join.len()),
        prefix_ratio(right_join.len()),
    ] {
        if best.or(bar).is_none_or(|current| score > current) {
            best = Some(score);
        }
    }
    let current_bar = best.or(bar);
    let max_len = left_join.len().max(right_join.len());
    if let Some(limit) = max_winning_distance(max_len, current_bar) {
        let joins_ratio = if left_join == right_join {
            Some(100.0)
        } else if left_join.len().abs_diff(right_join.len()) > limit {
            None
        } else {
            bounded_levenshtein(left_join, right_join, limit, prev, curr)
                .map(|distance| ratio_from_distance(max_len, distance))
        };
        if joins_ratio.is_some() {
            best = joins_ratio;
        }
    }
    best
}

fn push_token(join: &mut Vec<char>, token: &[char]) {
    if !join.is_empty() {
        join.push(' ');
    }
    join.extend_from_slice(token);
}

/// Levenshtein distance of `left` and `right` when it is at most `limit`,
/// else `None`. Common prefixes and suffixes do not change the distance, and
/// no cell farther than `limit` from the diagonal can hold a value within
/// `limit`, so only that band is computed; a row whose minimum exceeds
/// `limit` ends the search (the final distance is at least every row
/// minimum).
fn bounded_levenshtein(
    left: &[char],
    right: &[char],
    limit: usize,
    prev: &mut Vec<usize>,
    curr: &mut Vec<usize>,
) -> Option<usize> {
    let prefix = left
        .iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count();
    let (left, right) = (&left[prefix..], &right[prefix..]);
    let suffix = left
        .iter()
        .rev()
        .zip(right.iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let (left, right) = (&left[..left.len() - suffix], &right[..right.len() - suffix]);
    if left.is_empty() || right.is_empty() {
        let distance = left.len().max(right.len());
        return (distance <= limit).then_some(distance);
    }

    let outside = limit.saturating_add(1);
    prev.clear();
    prev.extend((0..=right.len()).map(|j| j.min(outside)));
    curr.clear();
    curr.resize(right.len() + 1, outside);

    for (row, left_char) in left.iter().enumerate() {
        let i = row + 1;
        let low = i.saturating_sub(limit).max(1);
        let high = i.saturating_add(limit).min(right.len());
        if low > high {
            return None;
        }
        curr[low - 1] = if low == 1 { i.min(outside) } else { outside };
        let mut row_min = curr[low - 1];
        for j in low..=high {
            let cost = usize::from(*left_char != right[j - 1]);
            let value = (curr[j - 1] + 1)
                .min(prev[j] + 1)
                .min(prev[j - 1] + cost)
                .min(outside);
            curr[j] = value;
            row_min = row_min.min(value);
        }
        if high < right.len() {
            curr[high + 1] = outside;
        }
        if row_min > limit {
            return None;
        }
        std::mem::swap(prev, curr);
    }
    let distance = prev[right.len()];
    (distance <= limit).then_some(distance)
}

#[cfg(test)]
mod reference;

#[cfg(test)]
mod tests {
    use super::*;
    use d4lc_assets::AssetRepository;
    use std::path::PathBuf;

    #[test]
    fn item_type_threshold_defaults_to_upstream_value() {
        let thresholds = FuzzyMatchThresholds::default();
        assert_eq!(
            thresholds.item_type,
            f64::from(d4lc_config::DEFAULT_MINIMAL_OCR_MATCH_TYPE)
        );
    }

    #[test]
    fn loads_enus_candidate_counts_from_assets() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            indexes
                .category_index(FuzzyCategory::ItemType)
                .candidate_count(),
            432
        );
        assert_eq!(
            indexes
                .category_index(FuzzyCategory::Affix)
                .candidate_count(),
            894
        );
        assert_eq!(
            indexes
                .category_index(FuzzyCategory::Aspect)
                .candidate_count(),
            523
        );
        assert_eq!(
            indexes
                .category_index(FuzzyCategory::Unique)
                .candidate_count(),
            308
        );
        assert_eq!(
            indexes
                .category_index(FuzzyCategory::Rune)
                .candidate_count(),
            52
        );
        assert_eq!(
            indexes
                .category_index(FuzzyCategory::Sigil)
                .candidate_count(),
            340
        );
    }

    #[test]
    fn loads_parser_category_indexes_for_every_imported_language() {
        let repo = AssetRepository::new(workspace_root());
        let counts = repo
            .data_counts_by_category_language()
            .unwrap_or_else(|error| panic!("{error}"));
        let parser_categories = [
            ("ItemTypes", FuzzyCategory::ItemType, "Name"),
            ("Affixes", FuzzyCategory::Affix, "DescriptionClean"),
            ("Aspects", FuzzyCategory::Aspect, "DescriptionClean"),
            ("Uniques", FuzzyCategory::Unique, "DescriptionClean"),
            ("Runes", FuzzyCategory::Rune, "DescriptionClean"),
            ("Sigils", FuzzyCategory::Sigil, "Name"),
        ];
        let languages: Vec<String> = counts["Affixes"].keys().cloned().collect();

        assert_eq!(languages.len(), 14);
        for language in languages {
            let indexes = FuzzyIndexes::load(&repo, &language)
                .unwrap_or_else(|error| panic!("{language}: {error}"));
            for (category_name, category, source_value_field) in parser_categories {
                let raw_count = counts[category_name][&language];
                let expected = labelled_entry_count(
                    &repo,
                    &language,
                    category_name,
                    category,
                    source_value_field,
                );
                assert!(
                    expected <= raw_count,
                    "{language} {category_name}: labelled entries {expected} exceed raw {raw_count}"
                );
                assert_eq!(
                    indexes.category_index(category).candidate_count(),
                    expected,
                    "{language} {category_name}"
                );
            }
        }
    }

    #[test]
    fn item_type_match_result_includes_traceable_normalization_and_threshold() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let result = indexes.match_text(FuzzyCategory::ItemType, "Legendary Amulet");
        assert_eq!(result.category, FuzzyCategory::ItemType);
        assert_eq!(result.upstream_method, "OcrHandler.TextToItemType");
        assert_eq!(result.raw_text, "Legendary Amulet");
        assert_eq!(result.normalized_text, "legendary amulet");
        assert_eq!(result.winner_id.as_deref(), Some("amulet"));
        assert_eq!(result.winner_label.as_deref(), Some("Legendary Amulet"));
        assert_eq!(result.score, Some(100.0));
        assert_eq!(result.threshold, 75.0);
        assert_eq!(result.candidate_count, 432);
        assert!(result.accepted);
        assert_eq!(result.rejection_reason, None);
        assert_eq!(result.normalization.language, "enUS");
        assert!(!result.normalization.disable_preprocessor);
        assert_eq!(
            result.normalization.cleanup_steps,
            vec![
                "trim",
                "unicode_lowercase",
                "replace_punctuation_with_space",
                "collapse_whitespace"
            ]
        );
    }

    #[test]
    fn item_type_threshold_can_follow_loaded_settings_value() {
        let repo = AssetRepository::new(workspace_root());
        let settings = d4lc_config::AppSettings {
            minimal_ocr_match_type: 101,
            ..Default::default()
        };
        let thresholds = FuzzyMatchThresholds::from_settings(&settings);
        let indexes = FuzzyIndexes::load_with_thresholds(&repo, "enUS", thresholds)
            .unwrap_or_else(|error| panic!("{error}"));

        let result = indexes.match_text(FuzzyCategory::ItemType, "Legendary Amulet");

        assert_eq!(result.score, Some(100.0));
        assert_eq!(result.threshold, 101.0);
        assert!(!result.accepted);
    }

    #[test]
    fn zhcn_indexes_disable_default_preprocessor() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "zhCN").unwrap_or_else(|error| panic!("{error}"));

        let normalization = indexes.category_index(FuzzyCategory::Affix).normalization();
        assert_eq!(normalization.language, "zhCN");
        assert!(normalization.disable_preprocessor);
        assert_eq!(normalization.source_value_field, "DescriptionClean");
    }

    #[test]
    fn sigil_matches_include_dungeon_zone_suffix_in_label() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let result = indexes.match_text(FuzzyCategory::Sigil, "Abandoned Mineworks in Kehjistan");
        assert_eq!(
            result.winner_id.as_deref(),
            Some("World_DGN_Kehj_AbandonedMineworks")
        );
        assert_eq!(
            result.winner_label.as_deref(),
            Some("Abandoned Mineworks in Kehjistan")
        );
        assert_eq!(result.score, Some(100.0));
        assert!(result.accepted);
    }

    #[test]
    fn rejected_match_reports_category_candidate_count_and_reason() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let result = indexes.match_text(FuzzyCategory::ItemType, "zzzzzzzzzz");
        assert_eq!(result.category, FuzzyCategory::ItemType);
        assert_eq!(result.candidate_count, 432);
        assert!(!result.accepted);
        assert_eq!(
            result.rejection_reason.as_deref(),
            Some("best score below threshold")
        );
    }

    #[test]
    fn normalization_uses_unicode_lowercase_for_non_ascii_languages() {
        assert_eq!(normalize("ÉCHO FAIBLISSANT", false), "écho faiblissant");
    }

    #[test]
    fn cached_match_reuses_first_result_for_case_insensitive_key() {
        // Upstream GetOrAdd with StringComparer.OrdinalIgnoreCase: the second
        // lookup with different casing receives the first stored result,
        // including the first caller's raw-text casing.
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let first = indexes.match_text_cached(FuzzyCategory::ItemType, "Legendary Amulet");
        let second = indexes.match_text_cached(FuzzyCategory::ItemType, "LEGENDARY AMULET");

        assert_eq!(first.winner_id, second.winner_id);
        assert_eq!(first.score, second.score);
        assert_eq!(second.raw_text, "Legendary Amulet");
        assert_eq!(indexes.cached_match_count(), 1);
    }

    #[test]
    fn cached_match_keys_are_separate_per_category() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let item_type = indexes.match_text_cached(FuzzyCategory::ItemType, "Legendary Amulet");
        let affix = indexes.match_text_cached(FuzzyCategory::Affix, "Legendary Amulet");

        assert_eq!(item_type.category, FuzzyCategory::ItemType);
        assert_eq!(affix.category, FuzzyCategory::Affix);
        assert_ne!(item_type.threshold, affix.threshold);
        assert_eq!(indexes.cached_match_count(), 2);
    }

    #[test]
    fn cached_match_preserves_acceptance_and_rejection_traces() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let accepted = indexes.match_text_cached(FuzzyCategory::ItemType, "Legendary Amulet");
        let accepted_again = indexes.match_text_cached(FuzzyCategory::ItemType, "Legendary Amulet");
        assert_eq!(accepted.accepted, accepted_again.accepted);
        assert_eq!(accepted.rejection_reason, accepted_again.rejection_reason);

        let rejected = indexes.match_text_cached(FuzzyCategory::ItemType, "zzzzzzzzzz");
        let rejected_again = indexes.match_text_cached(FuzzyCategory::ItemType, "zzzzzzzzzz");
        assert!(!rejected.accepted);
        assert_eq!(rejected.rejection_reason, rejected_again.rejection_reason);
        assert_eq!(rejected.candidate_count, rejected_again.candidate_count);
    }

    #[test]
    fn affix_tuning_prisms_lookup_matches_imported_data_by_id_name() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        // The enUS v3.2.0 data records TuningPrisms for this affix entry.
        assert_eq!(
            indexes.affix_tuning_prisms("UNIQUE_AttackSpeed_CoreSpenders"),
            vec!["TuningStone_1".to_string()]
        );
        // Upstream GetAffixTuningPrismsByIdName falls back to an empty list.
        assert!(indexes.affix_tuning_prisms("Missing_Affix_Id").is_empty());
    }

    #[test]
    fn rune_and_sigil_lookups_stay_uncached_like_upstream() {
        let repo = AssetRepository::new(workspace_root());
        let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let _ = indexes.match_text(FuzzyCategory::Rune, "Some Rune");
        let _ = indexes.match_text(FuzzyCategory::Sigil, "Some Sigil");
        assert_eq!(indexes.cached_match_count(), 0);
    }

    fn workspace_root() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        match manifest_dir.parent().and_then(|crates| crates.parent()) {
            Some(root) => root.to_path_buf(),
            None => panic!("could not resolve workspace root"),
        }
    }

    /// Counts the data file entries that yield a non-empty candidate label,
    /// mirroring the `load_index` skip rule for blank upstream labels
    /// (v3.2.0 localized Aspects files carry one placeholder entry with an
    /// empty `DescriptionClean`).
    fn labelled_entry_count(
        repo: &AssetRepository,
        language: &str,
        category_name: &str,
        category: FuzzyCategory,
        source_value_field: &str,
    ) -> usize {
        let path = repo
            .data_dir()
            .join(format!("{category_name}.{language}.json"));
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let json: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        let items = json
            .as_array()
            .unwrap_or_else(|| panic!("expected JSON array in {}", path.display()));
        items
            .iter()
            .filter(
                |item| match candidate_label(category, item, source_value_field) {
                    Ok(label) => !label.trim().is_empty(),
                    Err(_) => false,
                },
            )
            .count()
    }
}
