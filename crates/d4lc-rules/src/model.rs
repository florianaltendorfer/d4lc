use crate::overlay::{RuleOverlayDescriptor, RuleOverlayShape};
use d4lc_core::AffixRowId;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleDecision {
    Wanted,
    Unwanted,
    Neutral,
    Unknown,
}

impl RuleDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wanted => "wanted",
            Self::Unwanted => "unwanted",
            Self::Neutral => "neutral",
            Self::Unknown => "unknown",
        }
    }
}

/// Where a rule or descriptor decision comes from upstream. The three parts
/// are usually string literals, which clone for free; only a computed part
/// (a branch with an index, say) owns a `String`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleSourceNote {
    pub upstream_file: Cow<'static, str>,
    pub upstream_method: Cow<'static, str>,
    pub branch: Cow<'static, str>,
}

impl RuleSourceNote {
    pub fn new(
        upstream_file: impl Into<Cow<'static, str>>,
        upstream_method: impl Into<Cow<'static, str>>,
        branch: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            upstream_file: upstream_file.into(),
            upstream_method: upstream_method.into(),
            branch: branch.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleCheckKind {
    ItemPowerLimit,
    MinimalAffixValue,
    AffixPreset,
    AspectPreset,
    UniquePreset,
    RunePreset,
    SigilPreset,
    TradeLookup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleCheckStatus {
    Passed,
    Failed,
    NotEvaluated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleCheck {
    pub kind: RuleCheckKind,
    pub status: RuleCheckStatus,
    pub actual_value: Option<f64>,
    pub threshold_value: Option<f64>,
    pub source: RuleSourceNote,
}

impl RuleCheck {
    pub fn passed(
        kind: RuleCheckKind,
        actual_value: Option<f64>,
        threshold_value: Option<f64>,
        source: RuleSourceNote,
    ) -> Self {
        Self {
            kind,
            status: RuleCheckStatus::Passed,
            actual_value,
            threshold_value,
            source,
        }
    }

    pub fn failed(
        kind: RuleCheckKind,
        actual_value: Option<f64>,
        threshold_value: Option<f64>,
        source: RuleSourceNote,
    ) -> Self {
        Self {
            kind,
            status: RuleCheckStatus::Failed,
            actual_value,
            threshold_value,
            source,
        }
    }

    pub fn not_evaluated(kind: RuleCheckKind, source: RuleSourceNote) -> Self {
        Self {
            kind,
            status: RuleCheckStatus::NotEvaluated,
            actual_value: None,
            threshold_value: None,
            source,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleAffixMatch {
    pub affix_id: String,
    pub label: Option<String>,
    pub color: String,
    pub value_text: Option<String>,
    pub source_text: String,
    pub row_id: Option<AffixRowId>,
    pub required_by: Vec<String>,
    pub is_greater_required: bool,
    pub is_any_type_required: bool,
    pub value_status: RuleCheck,
    /// Tuning prism identifiers attached to the matched requirement
    /// (e.g. `TuningStone_1`), carried from the imported affix data.
    pub tuning_prisms: Vec<String>,
    /// Horizontal shift of this marker's column from the row's marker
    /// anchor: `0` for the selected preset, `5 - (i * 20)` for multi-build
    /// column `i` (OverlayHandler.cs:337, :357).
    pub column_offset: i32,
    pub source: RuleSourceNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMissingAffix {
    pub affix_id: String,
    pub label: Option<String>,
    pub color: String,
    pub required_by: Vec<String>,
    pub source: RuleSourceNote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RulePowerKind {
    Aspect,
    Unique,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulePowerMatch {
    pub power_id: String,
    pub power_kind: RulePowerKind,
    pub source: RuleSourceNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleTradeMarker {
    pub trade_item_id: String,
    pub value_text: String,
    pub source: RuleSourceNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleBranchExplanation {
    pub decision: RuleDecision,
    pub message: String,
    pub source: RuleSourceNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleExplanation {
    pub active_preset_names: Vec<String>,
    pub matched_build_names: Vec<String>,
    pub selected_build_name: Option<String>,
    pub item_type: Option<String>,
    pub rarity: Option<String>,
    pub branches: Vec<RuleBranchExplanation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleEvaluationResult {
    pub final_decision: RuleDecision,
    pub final_source: Option<RuleSourceNote>,
    pub matched_affixes: Vec<RuleAffixMatch>,
    pub missing_affixes: Vec<RuleMissingAffix>,
    pub item_power_status: RuleCheck,
    pub minimal_value_status: RuleCheck,
    pub matched_power: Option<RulePowerMatch>,
    /// Per-build power matches for the multi-build draw (upstream
    /// `ItemAspectBuildList`, `DrawGraphicsAspectsMulti`,
    /// OverlayHandler.cs:487-522): one entry per build column that matched
    /// an aspect/unique row, in column order. When non-empty they replace
    /// the single slot for the marker draw; the single slot keeps the
    /// first claim for the current-item views.
    pub matched_powers: Vec<RulePowerMatch>,
    pub matched_runes: Vec<String>,
    pub matched_sigil: Option<String>,
    /// Per-build sigil matches for the multi-build draw (upstream draws
    /// each build's dungeon square in its own column through
    /// `DrawGraphicsAffixesMulti`, OverlayHandler.cs:331-419): one entry
    /// per build column that matched a sigil row, in column order.
    pub matched_sigils: Vec<String>,
    pub trade_marker: Option<RuleTradeMarker>,
    pub overlay_descriptors: Vec<RuleOverlayDescriptor>,
    pub explanation: RuleExplanation,
    /// One trace per evaluated affix row (per build in multi-build mode):
    /// the selected entry, the search pass, the value check, color, shape
    /// and rejection reason behind every affix marker.
    pub affix_traces: Vec<RuleMatchTrace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedAffixKind {
    Normal,
    Greater,
    Implicit,
    Tempered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAffixForRules {
    pub affix_id: String,
    pub label: Option<String>,
    pub value_text: Option<String>,
    pub source_text: String,
    pub row_id: Option<AffixRowId>,
    pub kind: ParsedAffixKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedItemForRules {
    pub item_type: String,
    pub rarity: Option<String>,
    pub affixes: Vec<ParsedAffixForRules>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffixRequirement {
    pub affix_id: String,
    pub label: Option<String>,
    pub item_type: String,
    pub color: String,
    pub is_any_type: bool,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
    #[serde(default)]
    pub tuning_prisms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffixPreset {
    pub name: String,
    pub affixes: Vec<AffixRequirement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleNonAffixKind {
    Aspect,
    Unique,
    Rune,
    Sigil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigilDisplayMode {
    Whitelisting,
    Blacklisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonAffixRequirement {
    pub id: String,
    pub item_type: Option<String>,
    pub color: String,
    pub kind: RuleNonAffixKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonAffixPreset {
    pub name: String,
    pub powers: Vec<NonAffixRequirement>,
    pub runes: Vec<NonAffixRequirement>,
    pub sigils: Vec<NonAffixRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonAffixMatch {
    pub id: String,
    pub item_type: String,
    pub kind: RuleNonAffixKind,
    pub color: String,
    pub source: RuleSourceNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedNonAffixForRules {
    pub id: String,
    pub item_type: String,
    pub kind: RuleNonAffixKind,
    pub is_unique_item: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonAffixMultiBuildResult {
    pub matched_build_names: Vec<String>,
    pub matches: Vec<NonAffixMatch>,
}

/// One detected tooltip affix row handed to the per-row verdict API.
///
/// Plain-data projection of a parsed affix row: the canonical `affix_id`
/// (`IdName`), the owning item's type, the detected kind, and the tooltip
/// line the row was parsed from. Mirrors the per-row inputs of upstream
/// `D4Companion.Services/AffixManager.cs::GetAffix` (AffixManager.cs:530-560).
/// `value_text` carries the row's parsed OCR value ("+32.0%") so the
/// minimal-value verdict can compare it against the preset threshold the
/// way upstream compares `ItemAffixArea.AffixValue` against
/// `AffixThresholdValue` (OverlayHandler.cs:311-315).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixRuleRow {
    pub affix_id: String,
    pub item_type: String,
    pub kind: ParsedAffixKind,
    pub row_id: Option<AffixRowId>,
    pub value_text: Option<String>,
}

/// Which lookup pass produced an affix row match.
///
/// Upstream `GetAffix` tries the strict Id+Type+flag pass first
/// (AffixManager.cs:548-549) and only then the `IsAnyType` fallback
/// (AffixManager.cs:552-556).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffixMatchPass {
    Strict,
    AnyTypeFallback,
}

/// Verdict for one detected affix row (see `evaluate_affix_preset_rows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AffixRowVerdict {
    Matched {
        /// Index into `AffixPreset::affixes` of the entry upstream `GetAffix`
        /// would return for this row.
        requirement_index: usize,
        affix_id: String,
        color: String,
        pass: AffixMatchPass,
        is_greater: bool,
        is_any_type: bool,
        is_implicit: bool,
        is_tempered: bool,
        tuning_prisms: Vec<String>,
        /// Upstream below-minimal comparison
        /// (`OverlayHandler.cs:311-315`: `AffixValue < AffixThresholdValue`
        /// draws the rectangle): `true` when the minimal-value filter is
        /// enabled and the row's parsed OCR value is below the preset's
        /// minimal value for the affix (an unparsable value is below every
        /// threshold, mirroring `apply_minimal_affix_value_filter`).
        /// Always `false` while the filter is disabled — the comparison
        /// did not run.
        below_minimal_value: bool,
    },
    Unmatched {
        affix_id: String,
        /// Upstream red default (AffixManager.cs:532-538 and :558).
        color: String,
    },
}

/// Outcome of the minimal-value comparison for one affix row.
///
/// Upstream stores `AffixValue` (the first number of the OCR text,
/// `OcrHandler.TextToAffixValue`, `0.0` when the text carries none,
/// OcrHandler.cs:955-978) and `AffixThresholdValue`
/// (`AffixManager.GetAffixMinimalValue`, a missing id is `0`,
/// AffixManager.cs:610-613) for every affix area regardless of the preset
/// match (ScreenProcessHandler.cs:1078-1085), and the draw compares them only
/// while the filter is enabled (OverlayHandler.cs:311-315).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuleValueCheck {
    /// The minimal-value filter is off: the comparison did not run.
    NotEvaluated,
    AtOrAbove {
        value: f64,
        threshold: f64,
    },
    Below {
        value: f64,
        threshold: f64,
    },
}

impl RuleValueCheck {
    pub const fn is_below(self) -> bool {
        matches!(self, Self::Below { .. })
    }
}

/// Why no preset entry was selected for a row. Every rejection draws the
/// upstream red default (AffixManager.cs:532-538 and :558).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleRejection {
    /// No preset carries the selected name (`preset == null`,
    /// AffixManager.cs:540-541).
    PresetNotFound,
    /// No entry passes the strict search and no entry carries the id.
    NoEntryForAffix,
    /// The strict search found nothing and the first entry with the id is
    /// not set to any type (AffixManager.cs:552-556). Later entries with
    /// the same id are never consulted, even when they are any-type.
    FirstSameIdEntryNotAnyType { requirement_index: usize },
}

/// The preset entry a row was matched to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleTraceEntry {
    /// Index into `AffixPreset::affixes`.
    pub requirement_index: usize,
    pub affix_id: String,
    pub item_type: String,
    pub color: String,
    pub is_any_type: bool,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
    pub tuning_prisms: Vec<String>,
}

/// Everything the evaluation of one affix row decided, and why.
///
/// Built from the canonical row (catalog `IdName`, canonical item type id,
/// row kind) and one preset of the rules snapshot; the verdict, the marker
/// color and the marker shape are all read from here.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleMatchTrace {
    /// The preset the row was evaluated against (the selected preset, or
    /// the build's preset in multi-build mode).
    pub preset_name: String,
    /// The multi-build column (upstream `MultiBuild.Index` order), `None`
    /// for the single selected preset.
    pub build_column: Option<usize>,
    pub row_id: Option<AffixRowId>,
    pub affix_id: String,
    pub item_type: String,
    pub row_kind: ParsedAffixKind,
    /// The search pass that selected `entry`.
    pub pass: Option<AffixMatchPass>,
    pub entry: Option<RuleTraceEntry>,
    pub value_check: RuleValueCheck,
    pub color: String,
    pub shape: RuleOverlayShape,
    pub rejection: Option<RuleRejection>,
}

impl RuleMatchTrace {
    pub fn is_match(&self) -> bool {
        self.entry.is_some()
    }

    /// The multi-build view of this trace: a matched row draws in the
    /// build's color, an unmatched row keeps the red default
    /// (ScreenProcessHandler.cs:1765-1818).
    pub fn for_build(mut self, column: usize, build_color: &str) -> Self {
        self.build_column = Some(column);
        if self.is_match() {
            self.color = build_color.to_string();
        }
        self
    }
}

/// The minimal-value filter inputs of a rules snapshot.
#[derive(Debug, Clone, Copy)]
pub struct AffixValueFilter<'a> {
    pub enabled: bool,
    pub thresholds: &'a [AffixValueThreshold],
}

impl AffixValueFilter<'_> {
    pub const DISABLED: AffixValueFilter<'static> = AffixValueFilter {
        enabled: false,
        thresholds: &[],
    };
}

/// Verdict for one detected aspect/unique/rune/sigil row (see
/// `evaluate_non_affix_preset_rows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonAffixRowVerdict {
    Matched {
        /// Index of the matched entry inside the preset's `powers`, `runes`,
        /// or `sigils` bucket selected by the row kind.
        requirement_index: usize,
        id: String,
        color: String,
    },
    Unmatched {
        id: String,
        color: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleFilterSettings {
    pub is_item_power_limit_enabled: bool,
    pub item_power_limit: u32,
    pub is_minimal_affix_value_filter_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AffixValueThreshold {
    pub affix_id: String,
    pub threshold: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiBuildEntry {
    pub build_name: String,
    pub preset_name: String,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiBuildSettings {
    pub enabled: bool,
    pub selected_preset: String,
    pub builds: Vec<MultiBuildEntry>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
