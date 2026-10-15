//! Tooltip OCR input and item tooltip parse result model.

use d4lc_core::coords::WindowRect;
use d4lc_core::{AffixRowId, Rect};
use d4lc_fuzzy::FuzzyMatchResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TooltipOcrSegmentKind {
    #[default]
    Generic,
    UpperTooltip,
    Affix,
    Aspect,
    UniqueAspect,
    Rune,
    Sigil,
    Socket,
    Splitter,
}

impl TooltipOcrSegmentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::UpperTooltip => "upper_tooltip",
            Self::Affix => "affix",
            Self::Aspect => "aspect",
            Self::UniqueAspect => "unique_aspect",
            Self::Rune => "rune",
            Self::Sigil => "sigil",
            Self::Socket => "socket",
            Self::Splitter => "splitter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TooltipOcrLineInput {
    pub text: String,
    pub rect: Option<Rect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TooltipOcrSegmentInput {
    pub index: usize,
    pub kind: TooltipOcrSegmentKind,
    pub raw_text: String,
    pub converted_text: String,
    pub lines: Vec<TooltipOcrLineInput>,
    pub marker_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemTooltipParseStatus {
    Parsed,
    Partial,
    NotItem,
    Error,
}

impl ItemTooltipParseStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Parsed => "parsed",
            Self::Partial => "partial",
            Self::NotItem => "not_item",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemTooltipRarity {
    Normal,
    Magic,
    Rare,
    Legendary,
    Unique,
}

impl ItemTooltipRarity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Magic => "magic",
            Self::Rare => "rare",
            Self::Legendary => "legendary",
            Self::Unique => "unique",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ItemTooltipSourceLine {
    pub source_segment_index: usize,
    pub source_line_index: usize,
    pub global_line_index: usize,
    pub segment_kind: TooltipOcrSegmentKind,
    pub raw_text: String,
    pub converted_text: Option<String>,
    pub normalized_text: String,
    pub rect: Option<Rect>,
    pub marker_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ItemTooltipStat {
    pub name: String,
    pub raw_text: String,
    pub value_text: Option<String>,
    pub source: ItemTooltipSourceLine,
}

/// One parsed affix row.
///
/// Three numbers live around an affix row and are never interchangeable:
///
/// - [`Self::value_text`] / [`Self::text_value`]: the value the tooltip shows
///   for this row (upstream `OcrResultAffix.TextValue`).
/// - [`Self::range_text`]: the `[min - max]` range the game displays next to
///   it — a display range, never the value.
/// - the comparison value: the minimum the imported preset asks for
///   (upstream `AffixManager.GetAffixMinimalValue` ->
///   `ItemAffixAreaDescriptor.AffixThresholdValue`, compared as
///   `AffixValue < AffixThresholdValue` in `OverlayHandler.DrawGraphicsAffixes`).
///   It is not part of the parsed row: the runtime rule evaluation takes it
///   from the active preset.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ItemTooltipAffix {
    pub name: String,
    pub raw_text: String,
    /// The leading value token of the row as OCR'd it ("32.0%", "1,250"),
    /// with its unit but without the leading `+`/`-`/`x` sign. A row without
    /// a leading number keeps `None`.
    pub value_text: Option<String>,
    /// The bracketed display range ("5 - 12.5" of "[5 - 12.5]"), when the
    /// tooltip shows one.
    pub range_text: Option<String>,
    pub text_clean: Option<String>,
    /// The numeric comparison value of the row, exactly as upstream
    /// `OcrHandler.TextToAffixValue` derives it, quirks included:
    ///
    /// - It is the first number of the whole row text after removing
    ///   `[ ] ( ) + - %`, not the [`Self::value_text`] token and never the
    ///   [`Self::range_text`] range.
    /// - A row whose leading value is missing therefore reports the first
    ///   number it contains at all — for an implicit row that carries only a
    ///   range, that is the range's lower bound.
    /// - A single `,` or `.` in the second-to-last position is read as a
    ///   decimal separator; every other `,`/`.` is stripped as a thousands
    ///   separator (`1,250` -> 1250, `12,5` -> 12.5).
    /// - A row without any digit reports `0.0`, the same value a genuinely
    ///   zero row reports.
    ///
    /// The field is `None` when the parse ran without fuzzy indexes: the value
    /// is computed by the same conversion that resolves the affix identity, so
    /// a session without catalog data carries no comparison value either.
    pub text_value: Option<f64>,
    pub affix_id: Option<String>,
    pub affix_label: Option<String>,
    pub fuzzy: Option<FuzzyMatchResult>,
    pub conversion_method: Option<&'static str>,
    pub is_greater_affix: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
    /// Tuning prism ids recorded for the affix in the imported data
    /// (upstream `ItemAffix.TuningPrisms`, v5.4.0); the overlay draws them
    /// as tuning-prism hints.
    pub tuning_prisms: Vec<String>,
    /// Tooltip-wide identity of this row (its position in
    /// [`ItemTooltipParseResult::affixes`]). Rules, geometry and overlay
    /// placement join on this, never on the segment-local line index.
    pub row_id: AffixRowId,
    pub source: ItemTooltipSourceLine,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ItemTooltipLegendaryPower {
    pub raw_text: String,
    pub joined_text: String,
    pub is_imprinted: bool,
    pub text_clean: Option<String>,
    pub power_id: Option<String>,
    pub power_label: Option<String>,
    pub fuzzy: Option<FuzzyMatchResult>,
    pub conversion_method: Option<&'static str>,
    pub is_unique: bool,
    pub sources: Vec<ItemTooltipSourceLine>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ItemTooltipRune {
    pub raw_text: String,
    pub text_clean: String,
    pub candidate_texts: Vec<String>,
    pub rune_id: Option<String>,
    pub rune_label: Option<String>,
    pub rune_type: Option<String>,
    pub fuzzy: FuzzyMatchResult,
    pub conversion_method: &'static str,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ItemTooltipSigil {
    pub raw_text: String,
    pub text_clean: String,
    pub sigil_id: Option<String>,
    pub sigil_label: Option<String>,
    pub sigil_type: Option<String>,
    pub fuzzy: FuzzyMatchResult,
    pub conversion_method: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ItemTooltipSocket {
    pub raw_text: String,
    pub source: ItemTooltipSourceLine,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct ItemTooltipMetadata {
    pub requires_level: Option<ItemTooltipMetadataValue<u32>>,
    pub account_bound: Option<ItemTooltipMetadataValue<bool>>,
    pub sell_value: Option<ItemTooltipMetadataValue<String>>,
    pub durability: Option<ItemTooltipMetadataValue<String>>,
    pub tempers: Option<ItemTooltipMetadataValue<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ItemTooltipMetadataValue<T> {
    pub value: T,
    pub source: ItemTooltipSourceLine,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct ItemTooltipSourceSummary {
    pub segment_count: usize,
    pub line_count: usize,
}

/// Window-space geometry of one parsed affix row.
///
/// Transport for overlay row anchoring: the pure parser leaves
/// [`ItemTooltipParseResult::affix_row_geometry`] empty. The runtime OCR
/// parity path fills one entry per affix whose source line carries an OCR
/// rect. It composes each rect inside the tooltip image (segment plan
/// offset, then the OCR line inside the segment) and converts it to window
/// space exactly once through the job's
/// [`TooltipPlacement`](d4lc_core::coords::TooltipPlacement).
///
/// Three different rects, never interchangeable:
/// the recognized text, the affix area the text was read from, and the
/// marker anchor the overlay places its marker on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ItemTooltipAffixRowGeometry {
    /// The row this geometry belongs to ([`ItemTooltipAffix::row_id`]); the
    /// join key between parsed affixes, rule verdicts and overlay locations.
    pub row_id: AffixRowId,
    /// Provenance: the OCR segment the row was read from
    /// ([`ItemTooltipSourceLine::source_segment_index`]).
    pub source_segment_index: usize,
    /// Provenance: line index within that segment
    /// ([`ItemTooltipSourceLine::source_line_index`]). Restarts at zero in
    /// every segment — never an identity.
    pub source_line_index: usize,
    /// Provenance: zero-based index of the line across all parsed tooltip
    /// lines ([`ItemTooltipSourceLine::global_line_index`]).
    pub global_line_index: usize,
    /// Provenance: ordinal of the affix marker the row's segment was planned
    /// from, in marker order (upstream `ItemAffixLocations` index), when the
    /// segment came from a marker.
    pub marker_ordinal: Option<usize>,
    /// The OCR text line of the row.
    pub text_rect: WindowRect,
    /// The affix area the row was read from (upstream `ItemAffixAreas`
    /// entry): the OCR segment's planned rect.
    pub affix_area: WindowRect,
    /// Where the row's marker goes, following upstream
    /// `OverlayHandler.DrawGraphicsAffixes` (OverlayHandler.cs:251-318): the
    /// marker column is the tooltip's left edge, the row supplies the top and
    /// the height of its affix marker (`ItemAffixLocation`). `None` when the
    /// row's segment was not planned from an affix marker.
    pub marker_anchor: Option<WindowRect>,
}

/// Classified non-affix tooltip row (aspect, unique aspect, rune, sigil).
///
/// Transport for overlay row anchoring of the non-affix parsed rows: the
/// pure parser fills the classification natively — the segment kind it
/// classified, the canonical id and label from the fuzzy conversion, and
/// the first source line of the classified row — and leaves
/// [`ItemTooltipNonAffixRow::rect`] empty. The runtime OCR parity path
/// composes the window-relative row rect from the tooltip crop origin, the
/// OCR segment plan offset, and the source line rect, mirroring
/// [`ItemTooltipAffixRowGeometry`]. Rows without an anchor line keep out of
/// the list entirely, and rows whose anchor line carries no OCR rect keep
/// `rect: None` (absence, no fabricated geometry).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ItemTooltipNonAffixRow {
    /// Segment classification of the row (`Aspect`, `UniqueAspect`, `Rune`,
    /// or `Sigil`) — the same classification that produced `id`.
    pub kind: TooltipOcrSegmentKind,
    /// Canonical id as classified today (aspect/unique `power_id`, rune
    /// `rune_id`, sigil `sigil_id`); `None` when the fuzzy conversion did
    /// not resolve one.
    pub id: Option<String>,
    /// Matched display label (`power_label`, `rune_label`, `sigil_label`),
    /// when the fuzzy conversion resolved one.
    pub label: Option<String>,
    /// First source line of the classified row; the anchor line the runtime
    /// composes the row rect from and the overlay joins on.
    pub source: ItemTooltipSourceLine,
    /// Window-relative row rect: tooltip crop origin + segment plan offset +
    /// OCR line rect. Filled by the runtime OCR parity path; `None` for pure
    /// parser results.
    pub rect: Option<Rect>,
    /// Window-relative marker anchor: the tooltip's left edge and the row
    /// of the aspect/rune/sigil marker the segment was planned from
    /// (upstream `ItemAspectLocation`/`ItemAffixLocations`,
    /// OverlayHandler.cs:263-264, :471-475). Filled by the runtime OCR
    /// parity path; `None` when the segment came from no marker.
    pub marker_anchor: Option<Rect>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ItemTooltipParseResult {
    pub status: ItemTooltipParseStatus,
    pub name_lines: Vec<String>,
    /// Provenance of every entry in [`Self::name_lines`], in the same order:
    /// the OCR line the name text was read from (segment index, line index,
    /// rect, marker). The display name is derived from these lines only, so a
    /// divergent name can be attributed to the segment and line it came from
    /// Never an affix row: name lines are collected from the upper
    /// tooltip section before any affix, stat or power line appears.
    pub name_line_sources: Vec<ItemTooltipSourceLine>,
    pub display_name: Option<String>,
    pub rarity: Option<ItemTooltipRarity>,
    pub is_ancestral: Option<bool>,
    pub is_sacred: Option<bool>,
    /// Display-side type derivation of the rarity/base-type line ("Helm",
    /// "Two-Handed Sword"). Display only — never an evaluation id: the rule
    /// evaluation compares the canonical item type id of the pinned catalog
    /// (`helm`, `chest`, ...) that imports carry and that
    /// [`convert_to_item_type`](crate::convert_to_item_type) resolves.
    pub base_type: Option<String>,
    /// Canonical item type id of the pinned catalog (`helm`, `chest`, ...),
    /// resolved from the upper tooltip section text through
    /// [`convert_to_item_type`](crate::convert_to_item_type) (`ScreenProcessHandler.FindItemTypePower` ->
    /// `OcrHandler.ConvertToItemType`). This is the evaluation identity the
    /// rules compare exactly; it is never re-derived from the display text.
    /// `None` when the section text did not resolve or no fuzzy indexes were
    /// supplied — an unresolved type stays unknown.
    pub item_type_id: Option<String>,
    pub item_power: Option<u32>,
    pub quality: Option<u32>,
    pub primary_stats: Vec<ItemTooltipStat>,
    pub weapon_details: Vec<ItemTooltipStat>,
    pub affixes: Vec<ItemTooltipAffix>,
    pub legendary_power: Option<ItemTooltipLegendaryPower>,
    pub runes: Vec<ItemTooltipRune>,
    pub sigils: Vec<ItemTooltipSigil>,
    pub sockets: Vec<ItemTooltipSocket>,
    pub metadata: ItemTooltipMetadata,
    pub comparison_delta: Vec<ItemTooltipSourceLine>,
    pub ui_chrome_ignored: Vec<ItemTooltipSourceLine>,
    pub right_aligned_filtered: Vec<ItemTooltipSourceLine>,
    pub unclassified_item_lines: Vec<ItemTooltipSourceLine>,
    pub source: ItemTooltipSourceSummary,
    /// Window-relative per-affix row geometry for overlay anchoring. Filled
    /// by the runtime OCR parity path; empty for pure parser results.
    pub affix_row_geometry: Vec<ItemTooltipAffixRowGeometry>,
    /// Classified non-affix rows (aspect, unique aspect, rune, sigil) in
    /// source order. The pure parser fills the classification and each
    /// row's anchor line; `rect` stays empty there. The runtime OCR parity
    /// path composes the window-relative row rects (see
    /// [`ItemTooltipNonAffixRow::rect`]).
    pub non_affix_rows: Vec<ItemTooltipNonAffixRow>,
}

impl Default for ItemTooltipParseResult {
    fn default() -> Self {
        Self {
            status: ItemTooltipParseStatus::NotItem,
            name_lines: Vec::new(),
            name_line_sources: Vec::new(),
            display_name: None,
            rarity: None,
            is_ancestral: None,
            is_sacred: None,
            base_type: None,
            item_type_id: None,
            item_power: None,
            quality: None,
            primary_stats: Vec::new(),
            weapon_details: Vec::new(),
            affixes: Vec::new(),
            legendary_power: None,
            runes: Vec::new(),
            sigils: Vec::new(),
            sockets: Vec::new(),
            metadata: ItemTooltipMetadata::default(),
            comparison_delta: Vec::new(),
            ui_chrome_ignored: Vec::new(),
            right_aligned_filtered: Vec::new(),
            unclassified_item_lines: Vec::new(),
            source: ItemTooltipSourceSummary::default(),
            affix_row_geometry: Vec::new(),
            non_affix_rows: Vec::new(),
        }
    }
}
