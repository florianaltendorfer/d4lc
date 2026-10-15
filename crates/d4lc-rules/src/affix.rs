use crate::model::{
    AffixMatchPass, AffixPreset, AffixRequirement, AffixRowVerdict, AffixRuleRow, AffixValueFilter,
    AffixValueThreshold, MultiBuildSettings, ParsedAffixKind, ParsedItemForRules, RuleAffixMatch,
    RuleBranchExplanation, RuleCheck, RuleCheckKind, RuleDecision, RuleEvaluationResult,
    RuleExplanation, RuleFilterSettings, RuleMatchTrace, RuleMissingAffix, RuleRejection,
    RuleSourceNote, RuleTraceEntry, RuleValueCheck,
};
use crate::overlay::upstream_affix_marker_shape;

pub fn evaluate_affix_preset(
    item: &ParsedItemForRules,
    selected_preset: &str,
    presets: &[AffixPreset],
) -> RuleEvaluationResult {
    let source = RuleSourceNote::new(
        "D4Companion.Services/AffixManager.cs",
        "AffixManager.GetAffix",
        "selected preset affix lookup",
    );
    let preset = presets.iter().find(|preset| preset.name == selected_preset);
    let mut matched_affixes = Vec::new();
    let mut missing_affixes = Vec::new();
    let mut branches = Vec::new();

    if let Some(preset) = preset {
        // Upstream `AffixManager.GetAffix` evaluates each DETECTED row
        // (D4Companion.Services/AffixManager.cs:530-560), so each row claims
        // at most one preset entry: the strict pass wins first and only then
        // the IsAnyType fallback runs. Computing the row matches up front
        // keeps two preset entries from marking the same tooltip line.
        let row_matches: Vec<Option<usize>> = item
            .affixes
            .iter()
            .map(|affix| {
                affix_match_for_row(&affix.affix_id, affix.kind, &item.item_type, preset)
                    .map(|(index, _)| index)
            })
            .collect();
        for (requirement_index, requirement) in preset.affixes.iter().enumerate() {
            let affix = item
                .affixes
                .iter()
                .zip(&row_matches)
                .find(|(_, row_match)| {
                    row_match
                        .as_ref()
                        .is_some_and(|matched_index| *matched_index == requirement_index)
                })
                .map(|(affix, _)| affix);
            if let Some(affix) = affix {
                matched_affixes.push(RuleAffixMatch {
                    affix_id: requirement.affix_id.clone(),
                    label: affix.label.clone().or_else(|| requirement.label.clone()),
                    color: requirement.color.clone(),
                    value_text: affix.value_text.clone(),
                    source_text: affix.source_text.clone(),
                    row_id: affix.row_id,
                    required_by: vec![preset.name.clone()],
                    is_greater_required: requirement.is_greater,
                    is_any_type_required: requirement.is_any_type,
                    tuning_prisms: requirement.tuning_prisms.clone(),
                    value_status: RuleCheck::not_evaluated(
                        RuleCheckKind::MinimalAffixValue,
                        source.clone(),
                    ),
                    column_offset: 0,
                    source: source.clone(),
                });
                branches.push(RuleBranchExplanation {
                    decision: RuleDecision::Wanted,
                    message: format!("matched affix {}", requirement.affix_id),
                    source: source.clone(),
                });
            } else {
                missing_affixes.push(RuleMissingAffix {
                    affix_id: requirement.affix_id.clone(),
                    label: requirement.label.clone(),
                    color: "Red".to_string(),
                    required_by: vec![preset.name.clone()],
                    source: RuleSourceNote::new(
                        "D4Companion.Services/AffixManager.cs",
                        "AffixManager.GetAffix",
                        "default red affix when selected preset has no match",
                    ),
                });
                branches.push(RuleBranchExplanation {
                    decision: RuleDecision::Unwanted,
                    message: format!("missing affix {}", requirement.affix_id),
                    source: RuleSourceNote::new(
                        "D4Companion.Services/AffixManager.cs",
                        "AffixManager.GetAffix",
                        "default red affix when selected preset has no match",
                    ),
                });
            }
        }
    } else {
        branches.push(RuleBranchExplanation {
            decision: RuleDecision::Unknown,
            message: format!("selected preset '{selected_preset}' was not found"),
            source: RuleSourceNote::new(
                "D4Companion.Services/AffixManager.cs",
                "AffixManager.GetAffix",
                "selected preset not found",
            ),
        });
    }

    let final_decision = if preset.is_none() {
        RuleDecision::Unknown
    } else if missing_affixes.is_empty() {
        RuleDecision::Wanted
    } else {
        RuleDecision::Unwanted
    };

    RuleEvaluationResult {
        final_decision,
        final_source: branches.first().map(|branch| branch.source.clone()),
        matched_affixes,
        missing_affixes,
        item_power_status: RuleCheck::not_evaluated(RuleCheckKind::ItemPowerLimit, source.clone()),
        minimal_value_status: RuleCheck::not_evaluated(
            RuleCheckKind::MinimalAffixValue,
            source.clone(),
        ),
        matched_power: None,
        matched_powers: Vec::new(),
        matched_runes: Vec::new(),
        matched_sigil: None,
        matched_sigils: Vec::new(),
        trade_marker: None,
        overlay_descriptors: Vec::new(),
        affix_traces: Vec::new(),
        explanation: RuleExplanation {
            active_preset_names: preset
                .map(|preset| vec![preset.name.clone()])
                .unwrap_or_default(),
            matched_build_names: Vec::new(),
            selected_build_name: None,
            item_type: Some(item.item_type.clone()),
            rarity: item.rarity.clone(),
            branches,
        },
    }
}

/// Evaluate detected tooltip affix rows against one selected preset and
/// return one verdict per row, in row order.
///
/// Per-row mirror of upstream `AffixManager.GetAffix`
/// (D4Companion.Services/AffixManager.cs:530-560): each detected row matches
/// at most one preset entry (strict pass first, then the `IsAnyType`
/// fallback), rows whose affix the preset does not list get the red default,
/// and preset entries absent from the item produce no marker. An empty
/// preset behaves like upstream's missing selected preset: every row is
/// unmatched red. The minimal-value comparison is not run: every matched
/// verdict reports `below_minimal_value: false` — use
/// [`evaluate_affix_preset_rows_with_thresholds`] to compare the rows'
/// parsed OCR values against per-affix thresholds.
pub fn evaluate_affix_preset_rows(
    rows: &[AffixRuleRow],
    preset: &AffixPreset,
) -> Vec<AffixRowVerdict> {
    evaluate_affix_preset_rows_with_thresholds(rows, preset, false, &[])
}

/// [`evaluate_affix_preset_rows`] plus the upstream below-minimal
/// comparison (`OverlayHandler.cs:311-315`): while
/// `is_minimal_affix_value_filter_enabled` is on, each matched verdict
/// compares the row's parsed OCR value against the affix's
/// `AffixValueThreshold` (missing thresholds default to `0.0`, upstream
/// `AffixManager.GetAffixMinimalValue` AffixManager.cs:610-613). The
/// verdict's `below_minimal_value` flag then drives the rectangle marker
/// shape through `RuleAffixMatch::value_status` — the frozen
/// `affix_overlay_shape` consumer. Unparsable values are below every
/// threshold, the same semantics `apply_minimal_affix_value_filter`
/// established for the whole-item path.
pub fn evaluate_affix_preset_rows_with_thresholds(
    rows: &[AffixRuleRow],
    preset: &AffixPreset,
    is_minimal_affix_value_filter_enabled: bool,
    thresholds: &[AffixValueThreshold],
) -> Vec<AffixRowVerdict> {
    let filter = AffixValueFilter {
        enabled: is_minimal_affix_value_filter_enabled,
        thresholds,
    };
    rows.iter()
        .map(|row| AffixRowVerdict::from(&trace_affix_row(row, &preset.name, Some(preset), filter)))
        .collect()
}

/// Evaluate detected affix rows against the preset named `selected_preset`
/// and return one [`RuleMatchTrace`] per row, in row order.
///
/// Per-row mirror of upstream `AffixManager.GetAffix`
/// (AffixManager.cs:530-560): the preset is the first one with the name; a
/// missing preset leaves every row on the red default.
pub fn trace_affix_rows(
    rows: &[AffixRuleRow],
    selected_preset: &str,
    presets: &[AffixPreset],
    filter: AffixValueFilter<'_>,
) -> Vec<RuleMatchTrace> {
    let preset = presets.iter().find(|preset| preset.name == selected_preset);
    rows.iter()
        .map(|row| trace_affix_row(row, selected_preset, preset, filter))
        .collect()
}

/// Evaluate one affix row against one preset (see [`trace_affix_rows`]).
pub fn trace_affix_row(
    row: &AffixRuleRow,
    preset_name: &str,
    preset: Option<&AffixPreset>,
    filter: AffixValueFilter<'_>,
) -> RuleMatchTrace {
    let search = match preset {
        Some(preset) => search_affix_entry(&row.affix_id, row.kind, &row.item_type, preset)
            .map(|(index, pass)| (&preset.affixes[index], index, pass)),
        None => Err(RuleRejection::PresetNotFound),
    };
    let value_check = affix_value_check(row, filter);
    let (pass, entry, rejection) = match search {
        Ok((requirement, requirement_index, pass)) => (
            Some(pass),
            Some(RuleTraceEntry {
                requirement_index,
                affix_id: requirement.affix_id.clone(),
                item_type: requirement.item_type.clone(),
                color: requirement.color.clone(),
                is_any_type: requirement.is_any_type,
                is_greater: requirement.is_greater,
                is_implicit: requirement.is_implicit,
                is_tempered: requirement.is_tempered,
                tuning_prisms: requirement.tuning_prisms.clone(),
            }),
            None,
        ),
        Err(rejection) => (None, None, Some(rejection)),
    };
    let (color, is_any_type, is_greater) = match &entry {
        Some(entry) => (entry.color.clone(), entry.is_any_type, entry.is_greater),
        // Upstream `affixDefault` (AffixManager.cs:532-538): red, neither
        // any-type nor greater.
        None => (UPSTREAM_DEFAULT_AFFIX_COLOR.to_string(), false, false),
    };
    RuleMatchTrace {
        preset_name: preset_name.to_string(),
        build_column: None,
        row_id: row.row_id,
        affix_id: row.affix_id.clone(),
        item_type: row.item_type.clone(),
        row_kind: row.kind,
        pass,
        entry,
        value_check,
        color,
        shape: upstream_affix_marker_shape(is_any_type, is_greater, value_check.is_below()),
        rejection,
    }
}

/// Upstream `affixDefault.Color` (AffixManager.cs:536).
const UPSTREAM_DEFAULT_AFFIX_COLOR: &str = "Red";

/// The minimal-value comparison of one row (see [`RuleValueCheck`]). A value
/// text without a number compares as `0.0`, like upstream's
/// `TextToAffixValue`.
fn affix_value_check(row: &AffixRuleRow, filter: AffixValueFilter<'_>) -> RuleValueCheck {
    if !filter.enabled {
        return RuleValueCheck::NotEvaluated;
    }
    let threshold = filter
        .thresholds
        .iter()
        .find(|threshold| threshold.affix_id == row.affix_id)
        .map(|threshold| threshold.threshold)
        .unwrap_or(0.0);
    let value = row
        .value_text
        .as_deref()
        .and_then(parse_affix_value_text)
        .unwrap_or(0.0);
    if value < threshold {
        RuleValueCheck::Below { value, threshold }
    } else {
        RuleValueCheck::AtOrAbove { value, threshold }
    }
}

impl From<&RuleMatchTrace> for AffixRowVerdict {
    fn from(trace: &RuleMatchTrace) -> Self {
        match (&trace.entry, trace.pass) {
            (Some(entry), Some(pass)) => AffixRowVerdict::Matched {
                requirement_index: entry.requirement_index,
                affix_id: trace.affix_id.clone(),
                color: entry.color.clone(),
                pass,
                is_greater: entry.is_greater,
                is_any_type: entry.is_any_type,
                is_implicit: entry.is_implicit,
                is_tempered: entry.is_tempered,
                tuning_prisms: entry.tuning_prisms.clone(),
                below_minimal_value: trace.value_check.is_below(),
            },
            _ => AffixRowVerdict::Unmatched {
                affix_id: trace.affix_id.clone(),
                color: trace.color.clone(),
            },
        }
    }
}

/// The display text of one parsed affix row, carried onto its marker entry.
#[derive(Debug, Clone, Copy)]
pub struct AffixRowText<'a> {
    pub label: Option<&'a str>,
    pub value_text: Option<&'a str>,
    pub source_text: &'a str,
}

impl RuleAffixMatch {
    /// The marker entry of one evaluated row: color, shape flags and value
    /// status all come from the trace, so the drawn marker is exactly what
    /// the evaluation decided. `required_by` names the preset (or build)
    /// that matched; unmatched rows name none.
    pub fn from_trace(
        trace: &RuleMatchTrace,
        text: AffixRowText<'_>,
        column_offset: i32,
        source: RuleSourceNote,
    ) -> Self {
        let value_status = match trace.value_check {
            RuleValueCheck::NotEvaluated => {
                RuleCheck::not_evaluated(RuleCheckKind::MinimalAffixValue, source.clone())
            }
            RuleValueCheck::AtOrAbove { value, threshold } => RuleCheck::passed(
                RuleCheckKind::MinimalAffixValue,
                Some(value),
                Some(threshold),
                source.clone(),
            ),
            RuleValueCheck::Below { value, threshold } => RuleCheck::failed(
                RuleCheckKind::MinimalAffixValue,
                Some(value),
                Some(threshold),
                source.clone(),
            ),
        };
        let entry = trace.entry.as_ref();
        Self {
            affix_id: trace.affix_id.clone(),
            label: text.label.map(str::to_string),
            color: trace.color.clone(),
            value_text: text.value_text.map(str::to_string),
            source_text: text.source_text.to_string(),
            row_id: trace.row_id,
            required_by: entry
                .map(|_| vec![trace.preset_name.clone()])
                .unwrap_or_default(),
            is_greater_required: entry.is_some_and(|entry| entry.is_greater),
            is_any_type_required: entry.is_some_and(|entry| entry.is_any_type),
            value_status,
            tuning_prisms: entry
                .map(|entry| entry.tuning_prisms.clone())
                .unwrap_or_default(),
            column_offset,
            source,
        }
    }
}

pub fn evaluate_multi_build_affix_presets(
    item: &ParsedItemForRules,
    settings: &MultiBuildSettings,
    presets: &[AffixPreset],
) -> RuleEvaluationResult {
    if !settings.enabled {
        return evaluate_affix_preset(item, &settings.selected_preset, presets);
    }

    let source = RuleSourceNote::new(
        "D4Companion.Services/ScreenProcessHandler.cs",
        "ScreenProcessHandler.SetMultiBuildMode",
        "multi-build preset evaluation",
    );
    let active_build_names: Vec<String> = settings
        .builds
        .iter()
        .filter(|build| !build.build_name.trim().is_empty())
        .map(|build| build.build_name.clone())
        .collect();
    let mut matching_build_names = Vec::new();
    let mut winning_result: Option<RuleEvaluationResult> = None;

    for build in settings
        .builds
        .iter()
        .filter(|build| !build.build_name.trim().is_empty())
    {
        let mut candidate = evaluate_affix_preset(item, &build.preset_name, presets);
        if candidate.final_decision == RuleDecision::Wanted {
            for affix in &mut candidate.matched_affixes {
                affix.color = build.color.clone();
            }
            matching_build_names.push(build.build_name.clone());
            if winning_result.is_none() {
                candidate.explanation.selected_build_name = Some(build.build_name.clone());
                winning_result = Some(candidate);
            }
        }
    }

    if let Some(mut result) = winning_result {
        result.explanation.active_preset_names = active_build_names;
        result.explanation.matched_build_names = matching_build_names;
        if let Some(winner) = result.explanation.selected_build_name.clone() {
            result.explanation.branches.push(RuleBranchExplanation {
                decision: RuleDecision::Wanted,
                message: format!("selected multi-build winner {winner}"),
                source,
            });
        }
        return result;
    }

    RuleEvaluationResult {
        final_decision: RuleDecision::Unwanted,
        final_source: Some(source.clone()),
        matched_affixes: Vec::new(),
        missing_affixes: Vec::new(),
        item_power_status: RuleCheck::not_evaluated(RuleCheckKind::ItemPowerLimit, source.clone()),
        minimal_value_status: RuleCheck::not_evaluated(
            RuleCheckKind::MinimalAffixValue,
            source.clone(),
        ),
        matched_power: None,
        matched_powers: Vec::new(),
        matched_runes: Vec::new(),
        matched_sigil: None,
        matched_sigils: Vec::new(),
        trade_marker: None,
        overlay_descriptors: Vec::new(),
        affix_traces: Vec::new(),
        explanation: RuleExplanation {
            active_preset_names: active_build_names,
            matched_build_names: Vec::new(),
            selected_build_name: None,
            item_type: Some(item.item_type.clone()),
            rarity: item.rarity.clone(),
            branches: vec![RuleBranchExplanation {
                decision: RuleDecision::Unwanted,
                message: "no matching multi-build preset".to_string(),
                source,
            }],
        },
    }
}

pub fn apply_item_power_and_value_filters(
    result: &mut RuleEvaluationResult,
    item_power: Option<u32>,
    item_type: &str,
    settings: &RuleFilterSettings,
    affix_thresholds: &[AffixValueThreshold],
) {
    apply_item_power_filter(result, item_power, item_type, settings);
    apply_minimal_affix_value_filter(result, settings, affix_thresholds);
}

pub fn parse_affix_value_text(text: &str) -> Option<f64> {
    let cleaned: String = text
        .chars()
        .filter(|character| !matches!(character, '[' | ']' | '(' | ')' | '+' | '-' | '%'))
        .collect();
    let mut value = String::new();
    let mut started = false;
    for character in cleaned.chars() {
        if character.is_ascii_digit() || (started && matches!(character, '.' | ',')) {
            started = true;
            value.push(character);
        } else if started {
            break;
        }
    }
    if value.is_empty() {
        return None;
    }
    let normalized = normalize_upstream_number(&value);
    normalized.parse::<f64>().ok()
}

fn apply_item_power_filter(
    result: &mut RuleEvaluationResult,
    item_power: Option<u32>,
    item_type: &str,
    settings: &RuleFilterSettings,
) {
    let source = RuleSourceNote::new(
        "D4Companion.Services/OverlayHandler.cs",
        "OverlayHandler.DrawGraphics",
        "itemPowerLimitCheckOk",
    );
    if !settings.is_item_power_limit_enabled {
        result.item_power_status =
            RuleCheck::not_evaluated(RuleCheckKind::ItemPowerLimit, source.clone());
        result.explanation.branches.push(RuleBranchExplanation {
            decision: result.final_decision,
            message: "item power limit disabled".to_string(),
            source,
        });
        return;
    }

    let actual = item_power.map(f64::from);
    let threshold = f64::from(settings.item_power_limit);
    let sigil_bypass = item_type.eq_ignore_ascii_case("sigil");
    let passed = sigil_bypass || item_power.is_some_and(|power| power >= settings.item_power_limit);
    result.item_power_status = if passed {
        RuleCheck::passed(
            RuleCheckKind::ItemPowerLimit,
            actual,
            Some(threshold),
            source.clone(),
        )
    } else {
        RuleCheck::failed(
            RuleCheckKind::ItemPowerLimit,
            actual,
            Some(threshold),
            source.clone(),
        )
    };
    let message = if sigil_bypass {
        "item power limit bypassed for sigil".to_string()
    } else if let Some(power) = item_power {
        if passed {
            format!(
                "item power {power} meets limit {}",
                settings.item_power_limit
            )
        } else {
            format!(
                "item power {power} below limit {}",
                settings.item_power_limit
            )
        }
    } else {
        format!(
            "item power missing with limit {}",
            settings.item_power_limit
        )
    };
    if !passed {
        result.final_decision = RuleDecision::Unwanted;
    }
    result.explanation.branches.push(RuleBranchExplanation {
        decision: if passed {
            RuleDecision::Wanted
        } else {
            RuleDecision::Unwanted
        },
        message,
        source,
    });
}

fn apply_minimal_affix_value_filter(
    result: &mut RuleEvaluationResult,
    settings: &RuleFilterSettings,
    affix_thresholds: &[AffixValueThreshold],
) {
    let source = RuleSourceNote::new(
        "D4Companion.Services/ScreenProcessHandler.cs",
        "ScreenProcessHandler.UpdateItemAffixAreasWithOcrResults",
        "AffixValue and AffixThresholdValue",
    );
    if !settings.is_minimal_affix_value_filter_enabled {
        result.minimal_value_status =
            RuleCheck::not_evaluated(RuleCheckKind::MinimalAffixValue, source.clone());
        for affix in &mut result.matched_affixes {
            affix.value_status =
                RuleCheck::not_evaluated(RuleCheckKind::MinimalAffixValue, source.clone());
        }
        result.explanation.branches.push(RuleBranchExplanation {
            decision: result.final_decision,
            message: "minimal affix value filter disabled".to_string(),
            source,
        });
        return;
    }

    let mut any_failed = false;
    for affix in &mut result.matched_affixes {
        let threshold = affix_thresholds
            .iter()
            .find(|threshold| threshold.affix_id == affix.affix_id)
            .map(|threshold| threshold.threshold)
            .unwrap_or(0.0);
        let actual = affix
            .value_text
            .as_deref()
            .and_then(parse_affix_value_text)
            .or_else(|| parse_affix_value_text(&affix.source_text));
        let passed = actual.is_some_and(|actual| actual >= threshold);
        affix.value_status = if passed {
            RuleCheck::passed(
                RuleCheckKind::MinimalAffixValue,
                actual,
                Some(threshold),
                source.clone(),
            )
        } else {
            any_failed = true;
            RuleCheck::failed(
                RuleCheckKind::MinimalAffixValue,
                actual,
                Some(threshold),
                source.clone(),
            )
        };
        let actual_text = actual
            .map(|value| value.to_string())
            .unwrap_or_else(|| "missing".to_string());
        result.explanation.branches.push(RuleBranchExplanation {
            decision: if passed {
                RuleDecision::Wanted
            } else {
                RuleDecision::Unwanted
            },
            message: if passed {
                format!(
                    "affix {} value {actual_text} meets threshold {threshold}",
                    affix.affix_id
                )
            } else {
                format!(
                    "affix {} value {actual_text} below threshold {threshold}",
                    affix.affix_id
                )
            },
            source: source.clone(),
        });
    }

    result.minimal_value_status = if any_failed {
        result.final_decision = RuleDecision::Unwanted;
        RuleCheck::failed(RuleCheckKind::MinimalAffixValue, None, None, source)
    } else {
        RuleCheck::passed(RuleCheckKind::MinimalAffixValue, None, None, source)
    };
}

/// Strict lookup pass of upstream `AffixManager.GetAffix`
/// (D4Companion.Services/AffixManager.cs:548-549): Id equality, exact item
/// type equality, implicit flag equality, and tempered flag equality with the
/// season-11 rule that a tempered preset entry also matches a greater roll.
/// `IsGreater` and `IsAnyType` are not part of the predicate: an any-type
/// entry whose type equals the item's type matches here like any other
/// entry, and every other item type reaches it only through the fallback pass
/// in `search_affix_entry` (AffixManager.cs:552-556).
fn requirement_matches_affix(
    requirement: &AffixRequirement,
    affix_id: &str,
    kind: ParsedAffixKind,
    item_type: &str,
) -> bool {
    if requirement.affix_id != affix_id || requirement.item_type != item_type {
        return false;
    }

    let is_greater = kind == ParsedAffixKind::Greater;
    let is_implicit = kind == ParsedAffixKind::Implicit;
    let is_tempered = kind == ParsedAffixKind::Tempered;

    is_implicit == requirement.is_implicit
        && (is_tempered == requirement.is_tempered || (requirement.is_tempered && is_greater))
}

/// Full per-row preset lookup of upstream `AffixManager.GetAffix`
/// (D4Companion.Services/AffixManager.cs:530-560). The strict pass
/// (`requirement_matches_affix`) runs over every preset entry first; only if
/// no entry matches does the fallback take the FIRST entry with a matching Id
/// and keep it when its `IsAnyType` flag is set — there the item type and the
/// implicit/tempered flags are ignored (AffixManager.cs:552-556). Returns the
/// matched entry index and the pass that produced it.
fn affix_match_for_row(
    affix_id: &str,
    kind: ParsedAffixKind,
    item_type: &str,
    preset: &AffixPreset,
) -> Option<(usize, AffixMatchPass)> {
    search_affix_entry(affix_id, kind, item_type, preset).ok()
}

/// [`affix_match_for_row`] with the reason when no entry is selected.
///
/// The fallback deliberately looks at the first entry with the id only: an
/// any-type entry behind a non-any-type entry with the same id is never
/// reached (AffixManager.cs:553-554, `FirstOrDefault` then the flag check).
fn search_affix_entry(
    affix_id: &str,
    kind: ParsedAffixKind,
    item_type: &str,
    preset: &AffixPreset,
) -> Result<(usize, AffixMatchPass), RuleRejection> {
    if let Some(index) = preset
        .affixes
        .iter()
        .position(|requirement| requirement_matches_affix(requirement, affix_id, kind, item_type))
    {
        return Ok((index, AffixMatchPass::Strict));
    }

    let index = preset
        .affixes
        .iter()
        .position(|requirement| requirement.affix_id == affix_id)
        .ok_or(RuleRejection::NoEntryForAffix)?;
    if preset.affixes[index].is_any_type {
        Ok((index, AffixMatchPass::AnyTypeFallback))
    } else {
        Err(RuleRejection::FirstSameIdEntryNotAnyType {
            requirement_index: index,
        })
    }
}

fn normalize_upstream_number(value: &str) -> String {
    let mut normalized = value.to_string();
    if normalized.find('.') != Some(normalized.len().saturating_sub(2)) {
        normalized = normalized.replace('.', "");
    }
    if normalized.find(',') != Some(normalized.len().saturating_sub(2)) {
        normalized = normalized.replace(',', "");
    }
    normalized.replace(',', ".")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
