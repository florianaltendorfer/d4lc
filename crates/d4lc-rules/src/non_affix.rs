use crate::model::{
    MultiBuildEntry, MultiBuildSettings, NonAffixMatch, NonAffixMultiBuildResult, NonAffixPreset,
    NonAffixRequirement, NonAffixRowVerdict, ParsedNonAffixForRules, RuleNonAffixKind,
    RulePowerKind, RuleSourceNote, SigilDisplayMode,
};

/// Evaluate detected aspect/unique/rune/sigil rows against one selected
/// preset and return one verdict per row, in row order.
///
/// The row kind selects the preset bucket and the lookup rule, exactly like
/// the dedicated upstream lookups: aspects and sigils match Id + item type
/// (`AffixManager.cs:632` and `:726`), uniques and runes match Id only
/// (`:787` and `:836`). Unmatched rows get the red default
/// (`:622-627`, `:777-782`, `:826-831`) except sigils, whose default follows
/// the selected sigil display mode like upstream `GetSigil` (`:716-721`) and
/// the crate's `find_sigil_match`.
pub fn evaluate_non_affix_preset_rows(
    rows: &[ParsedNonAffixForRules],
    preset: &NonAffixPreset,
    sigil_display_mode: SigilDisplayMode,
) -> Vec<NonAffixRowVerdict> {
    rows.iter()
        .map(|row| {
            match non_affix_requirement_in_preset(preset, &row.id, &row.item_type, row.kind) {
                Some((requirement_index, requirement)) => NonAffixRowVerdict::Matched {
                    requirement_index,
                    id: row.id.clone(),
                    color: requirement.color.clone(),
                },
                None => {
                    let color = match row.kind {
                        RuleNonAffixKind::Sigil => match sigil_display_mode {
                            SigilDisplayMode::Whitelisting => "Red",
                            SigilDisplayMode::Blacklisting => "Green",
                        },
                        _ => "Red",
                    };
                    NonAffixRowVerdict::Unmatched {
                        id: row.id.clone(),
                        color: color.to_string(),
                    }
                }
            }
        })
        .collect()
}

pub fn find_power_match(
    selected_preset: &str,
    power_id: &str,
    item_type: &str,
    power_kind: RulePowerKind,
    presets: &[NonAffixPreset],
) -> Option<NonAffixMatch> {
    let kind = match power_kind {
        RulePowerKind::Aspect => RuleNonAffixKind::Aspect,
        RulePowerKind::Unique => RuleNonAffixKind::Unique,
    };
    find_non_affix_requirement(selected_preset, power_id, item_type, kind, presets).map(
        |requirement| NonAffixMatch {
            id: power_id.to_string(),
            item_type: item_type.to_string(),
            kind,
            color: requirement.color.clone(),
            source: non_affix_source(kind, "selected preset match"),
        },
    )
}

pub fn find_rune_match(
    selected_preset: &str,
    rune_id: &str,
    presets: &[NonAffixPreset],
) -> Option<NonAffixMatch> {
    find_non_affix_requirement(
        selected_preset,
        rune_id,
        "",
        RuleNonAffixKind::Rune,
        presets,
    )
    .map(|requirement| NonAffixMatch {
        id: rune_id.to_string(),
        item_type: "rune".to_string(),
        kind: RuleNonAffixKind::Rune,
        color: requirement.color.clone(),
        source: non_affix_source(RuleNonAffixKind::Rune, "selected preset match"),
    })
}

pub fn find_sigil_match(
    selected_preset: &str,
    sigil_id: &str,
    item_type: &str,
    display_mode: SigilDisplayMode,
    presets: &[NonAffixPreset],
) -> NonAffixMatch {
    if let Some(requirement) = find_non_affix_requirement(
        selected_preset,
        sigil_id,
        item_type,
        RuleNonAffixKind::Sigil,
        presets,
    ) {
        return NonAffixMatch {
            id: sigil_id.to_string(),
            item_type: item_type.to_string(),
            kind: RuleNonAffixKind::Sigil,
            color: requirement.color.clone(),
            source: non_affix_source(RuleNonAffixKind::Sigil, "selected preset match"),
        };
    }

    NonAffixMatch {
        id: sigil_id.to_string(),
        item_type: item_type.to_string(),
        kind: RuleNonAffixKind::Sigil,
        color: match display_mode {
            SigilDisplayMode::Whitelisting => "Red",
            SigilDisplayMode::Blacklisting => "Green",
        }
        .to_string(),
        source: non_affix_source(RuleNonAffixKind::Sigil, "default sigil color"),
    }
}

pub fn evaluate_non_affix_multi_build(
    parsed: &[ParsedNonAffixForRules],
    settings: &MultiBuildSettings,
    presets: &[NonAffixPreset],
) -> NonAffixMultiBuildResult {
    let selected_builds: Vec<MultiBuildEntry> = if settings.enabled {
        settings
            .builds
            .iter()
            .filter(|build| !build.build_name.trim().is_empty())
            .cloned()
            .collect()
    } else {
        vec![MultiBuildEntry {
            build_name: settings.selected_preset.clone(),
            preset_name: settings.selected_preset.clone(),
            color: "Green".to_string(),
        }]
    };
    let mut matched_build_names = Vec::new();
    let mut matches = Vec::new();

    for build in selected_builds {
        let mut build_matches = Vec::new();
        for parsed in parsed {
            let matched = match parsed.kind {
                RuleNonAffixKind::Aspect if !parsed.is_unique_item => find_power_match(
                    &build.preset_name,
                    &parsed.id,
                    &parsed.item_type,
                    RulePowerKind::Aspect,
                    presets,
                ),
                RuleNonAffixKind::Unique if parsed.is_unique_item => find_power_match(
                    &build.preset_name,
                    &parsed.id,
                    &parsed.item_type,
                    RulePowerKind::Unique,
                    presets,
                ),
                RuleNonAffixKind::Rune => find_rune_match(&build.preset_name, &parsed.id, presets),
                RuleNonAffixKind::Sigil => Some(find_sigil_match(
                    &build.preset_name,
                    &parsed.id,
                    &parsed.item_type,
                    SigilDisplayMode::Whitelisting,
                    presets,
                ))
                .filter(|matched| matched.color != "Red"),
                _ => None,
            };
            if let Some(mut matched) = matched {
                matched.color = build.color.clone();
                build_matches.push(matched);
            }
        }
        if !build_matches.is_empty() {
            matched_build_names.push(build.build_name);
            matches.extend(build_matches);
        }
    }

    NonAffixMultiBuildResult {
        matched_build_names,
        matches,
    }
}

fn non_affix_requirement_in_preset<'a>(
    preset: &'a NonAffixPreset,
    id: &str,
    item_type: &str,
    kind: RuleNonAffixKind,
) -> Option<(usize, &'a NonAffixRequirement)> {
    let requirements = match kind {
        RuleNonAffixKind::Aspect | RuleNonAffixKind::Unique => &preset.powers,
        RuleNonAffixKind::Rune => &preset.runes,
        RuleNonAffixKind::Sigil => &preset.sigils,
    };
    requirements.iter().enumerate().find(|(_, requirement)| {
        requirement.kind == kind
            && requirement.id == id
            && requirement
                .item_type
                .as_deref()
                .map(|required_type| required_type == item_type)
                .unwrap_or(true)
    })
}

fn find_non_affix_requirement<'a>(
    selected_preset: &str,
    id: &str,
    item_type: &str,
    kind: RuleNonAffixKind,
    presets: &'a [NonAffixPreset],
) -> Option<&'a NonAffixRequirement> {
    let preset = presets
        .iter()
        .find(|preset| preset.name == selected_preset)?;
    non_affix_requirement_in_preset(preset, id, item_type, kind).map(|(_, requirement)| requirement)
}

fn non_affix_source(kind: RuleNonAffixKind, branch: &'static str) -> RuleSourceNote {
    let method = match kind {
        RuleNonAffixKind::Aspect => "AffixManager.GetAspect",
        RuleNonAffixKind::Unique => "AffixManager.GetUnique",
        RuleNonAffixKind::Rune => "AffixManager.GetRune",
        RuleNonAffixKind::Sigil => "AffixManager.GetSigil",
    };
    RuleSourceNote::new("D4Companion.Services/AffixManager.cs", method, branch)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
