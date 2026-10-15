use super::*;
use crate::model::{MultiBuildEntry, ParsedAffixForRules, RuleCheckStatus};
use crate::overlay::RuleOverlayShape;
use crate::test_support::{affix_requirement, parsed_affix, prism_requirement, prism_test_item};
use d4lc_core::AffixRowId;

#[test]
fn evaluate_affix_preset_matches_selected_type_and_preserves_source_lines() {
    let item = ParsedItemForRules {
        item_type: "amulet".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![ParsedAffixForRules {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            value_text: Some("+32.0%".to_string()),
            source_text: "+32.0% Attack Speed".to_string(),
            row_id: Some(AffixRowId(4)),
            kind: ParsedAffixKind::Normal,
        }],
    };
    let preset = AffixPreset {
        name: "Speed Farm".to_string(),
        affixes: vec![AffixRequirement {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            item_type: "amulet".to_string(),
            color: "Green".to_string(),
            is_any_type: false,
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        }],
    };

    let result = evaluate_affix_preset(&item, "Speed Farm", &[preset]);

    assert_eq!(result.final_decision, RuleDecision::Wanted);
    assert!(result.missing_affixes.is_empty());
    assert_eq!(result.matched_affixes.len(), 1);
    assert_eq!(result.matched_affixes[0].color, "Green");
    assert_eq!(result.matched_affixes[0].row_id, Some(AffixRowId(4)));
    assert_eq!(result.matched_affixes[0].source_text, "+32.0% Attack Speed");
    assert_eq!(result.explanation.active_preset_names, vec!["Speed Farm"]);
    assert_eq!(
        result.explanation.branches[0].decision,
        RuleDecision::Wanted
    );
}

#[test]
fn evaluate_affix_preset_carries_requirement_tuning_prisms_into_matches() {
    let item = prism_test_item("Affix_AttackSpeed");
    let presets = vec![AffixPreset {
        name: "Speed Farm".to_string(),
        affixes: vec![
            prism_requirement("Affix_AttackSpeed", &["TuningStone_1"]),
            prism_requirement("Affix_Life", &[]),
        ],
    }];

    let result = evaluate_affix_preset(&item, "Speed Farm", &presets);

    assert_eq!(result.matched_affixes.len(), 1);
    assert_eq!(
        result.matched_affixes[0].tuning_prisms,
        vec!["TuningStone_1".to_string()]
    );
    // The unmatched requirement is reported missing with no prisms.
    assert_eq!(result.missing_affixes.len(), 1);
    assert_eq!(result.missing_affixes[0].affix_id, "Affix_Life");
}

#[test]
fn evaluate_affix_preset_reports_missing_and_type_restricted_affixes_as_red() {
    let item = ParsedItemForRules {
        item_type: "ring".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![ParsedAffixForRules {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            value_text: Some("+32.0%".to_string()),
            source_text: "+32.0% Attack Speed".to_string(),
            row_id: Some(AffixRowId(2)),
            kind: ParsedAffixKind::Normal,
        }],
    };
    let preset = AffixPreset {
        name: "Speed Farm".to_string(),
        affixes: vec![
            AffixRequirement {
                affix_id: "Affix_AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                item_type: "amulet".to_string(),
                color: "Green".to_string(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
            AffixRequirement {
                affix_id: "Affix_CriticalStrikeChance".to_string(),
                label: Some("Critical Strike Chance".to_string()),
                item_type: "ring".to_string(),
                color: "Gold".to_string(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
        ],
    };

    let result = evaluate_affix_preset(&item, "Speed Farm", &[preset]);

    assert_eq!(result.final_decision, RuleDecision::Unwanted);
    assert!(result.matched_affixes.is_empty());
    assert_eq!(
        result
            .missing_affixes
            .iter()
            .map(|affix| affix.affix_id.as_str())
            .collect::<Vec<_>>(),
        vec!["Affix_AttackSpeed", "Affix_CriticalStrikeChance"]
    );
    assert!(result
        .missing_affixes
        .iter()
        .all(|affix| affix.color == "Red"));
    assert!(result
        .explanation
        .branches
        .iter()
        .any(|branch| branch.message == "missing affix Affix_CriticalStrikeChance"));
}

#[test]
fn evaluate_affix_preset_supports_any_type_and_tempered_greater_fallback() {
    let item = ParsedItemForRules {
        item_type: "gloves".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![
            ParsedAffixForRules {
                affix_id: "Affix_AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                value_text: Some("+32.0%".to_string()),
                source_text: "+32.0% Attack Speed".to_string(),
                row_id: Some(AffixRowId(1)),
                kind: ParsedAffixKind::Normal,
            },
            ParsedAffixForRules {
                affix_id: "Affix_TemperedDamage".to_string(),
                label: Some("Damage".to_string()),
                value_text: Some("+12.0%".to_string()),
                source_text: "★ +12.0% Damage".to_string(),
                row_id: Some(AffixRowId(2)),
                kind: ParsedAffixKind::Greater,
            },
        ],
    };
    let preset = AffixPreset {
        name: "Any Type".to_string(),
        affixes: vec![
            AffixRequirement {
                affix_id: "Affix_AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                item_type: "amulet".to_string(),
                color: "Blue".to_string(),
                is_any_type: true,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
            AffixRequirement {
                affix_id: "Affix_TemperedDamage".to_string(),
                label: Some("Damage".to_string()),
                item_type: "gloves".to_string(),
                color: "Purple".to_string(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms: Vec::new(),
            },
        ],
    };

    let result = evaluate_affix_preset(&item, "Any Type", &[preset]);

    assert_eq!(result.final_decision, RuleDecision::Wanted);
    assert_eq!(
        result
            .matched_affixes
            .iter()
            .map(|affix| affix.color.as_str())
            .collect::<Vec<_>>(),
        vec!["Blue", "Purple"]
    );
}

fn flagged_affix_requirement(
    affix_id: &str,
    item_type: &str,
    color: &str,
    is_any_type: bool,
    is_implicit: bool,
    is_tempered: bool,
) -> AffixRequirement {
    AffixRequirement {
        affix_id: affix_id.to_string(),
        label: None,
        item_type: item_type.to_string(),
        color: color.to_string(),
        is_any_type,
        is_greater: false,
        is_implicit,
        is_tempered,
        tuning_prisms: Vec::new(),
    }
}

fn affix_rule_row(
    affix_id: &str,
    item_type: &str,
    kind: ParsedAffixKind,
    row_id: Option<AffixRowId>,
) -> AffixRuleRow {
    AffixRuleRow {
        affix_id: affix_id.to_string(),
        item_type: item_type.to_string(),
        kind,
        row_id,
        value_text: None,
    }
}

fn affix_rule_row_with_value(
    affix_id: &str,
    item_type: &str,
    kind: ParsedAffixKind,
    row_id: Option<AffixRowId>,
    value_text: Option<&str>,
) -> AffixRuleRow {
    AffixRuleRow {
        value_text: value_text.map(str::to_string),
        ..affix_rule_row(affix_id, item_type, kind, row_id)
    }
}

#[test]
fn evaluate_affix_preset_rows_follows_upstream_get_affix_passes() {
    // Upstream `AffixManager.GetAffix`
    // (D4Companion.Services/AffixManager.cs:530-560): a tempered preset
    // entry matches a greater roll (:548-549) but not a normal roll,
    // implicit rows match implicit entries through the strict pass, an
    // `IsAnyType` entry matches any item type through the fallback
    // (:552-556), and unmatched rows get the red default (:532-538,
    // :558).
    let preset = AffixPreset {
        name: "Endgame".to_string(),
        affixes: vec![
            flagged_affix_requirement("AttackSpeed", "amulet", "Gold", true, false, false),
            flagged_affix_requirement("AttackSpeed", "gloves", "Green", false, false, true),
            flagged_affix_requirement(
                "Barrier_Strength_Percent",
                "chest",
                "Blue",
                false,
                false,
                true,
            ),
            flagged_affix_requirement("Berserk_Damage", "amulet", "Purple", false, true, false),
        ],
    };

    let rows = vec![
        affix_rule_row(
            "AttackSpeed",
            "gloves",
            ParsedAffixKind::Greater,
            Some(AffixRowId(1)),
        ),
        affix_rule_row(
            "Barrier_Strength_Percent",
            "chest",
            ParsedAffixKind::Tempered,
            Some(AffixRowId(2)),
        ),
        affix_rule_row(
            "Barrier_Strength_Percent",
            "chest",
            ParsedAffixKind::Normal,
            Some(AffixRowId(3)),
        ),
        affix_rule_row(
            "AttackSpeed",
            "helm",
            ParsedAffixKind::Normal,
            Some(AffixRowId(4)),
        ),
        affix_rule_row(
            "Berserk_Damage",
            "amulet",
            ParsedAffixKind::Implicit,
            Some(AffixRowId(5)),
        ),
        affix_rule_row(
            "Life_On_Hit",
            "gloves",
            ParsedAffixKind::Normal,
            Some(AffixRowId(6)),
        ),
    ];

    let verdicts = evaluate_affix_preset_rows(&rows, &preset);

    assert_eq!(
        verdicts,
        vec![
            // Greater roll matched by the tempered gloves entry (S11 rule,
            // AffixManager.cs:548-549); the strict pass wins over the
            // any-type entry sharing the same Id.
            AffixRowVerdict::Matched {
                requirement_index: 1,
                affix_id: "AttackSpeed".to_string(),
                color: "Green".to_string(),
                pass: AffixMatchPass::Strict,
                is_greater: false,
                is_any_type: false,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms: Vec::new(),
                below_minimal_value: false,
            },
            AffixRowVerdict::Matched {
                requirement_index: 2,
                affix_id: "Barrier_Strength_Percent".to_string(),
                color: "Blue".to_string(),
                pass: AffixMatchPass::Strict,
                is_greater: false,
                is_any_type: false,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms: Vec::new(),
                below_minimal_value: false,
            },
            // A normal roll does not satisfy tempered flag equality
            // (:548-549) and the entry is not any-type, so the fallback
            // keeps the red default.
            AffixRowVerdict::Unmatched {
                affix_id: "Barrier_Strength_Percent".to_string(),
                color: "Red".to_string(),
            },
            // The fallback ignores the item type (AffixManager.cs:552-556).
            AffixRowVerdict::Matched {
                requirement_index: 0,
                affix_id: "AttackSpeed".to_string(),
                color: "Gold".to_string(),
                pass: AffixMatchPass::AnyTypeFallback,
                is_greater: false,
                is_any_type: true,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
                below_minimal_value: false,
            },
            // Implicit rows match implicit entries in the strict pass.
            AffixRowVerdict::Matched {
                requirement_index: 3,
                affix_id: "Berserk_Damage".to_string(),
                color: "Purple".to_string(),
                pass: AffixMatchPass::Strict,
                is_greater: false,
                is_any_type: false,
                is_implicit: true,
                is_tempered: false,
                tuning_prisms: Vec::new(),
                below_minimal_value: false,
            },
            AffixRowVerdict::Unmatched {
                affix_id: "Life_On_Hit".to_string(),
                color: "Red".to_string(),
            },
        ]
    );
}

#[test]
fn evaluate_affix_preset_rows_with_thresholds_flags_below_minimal_matches() {
    // Upstream below-minimal comparison (OverlayHandler.cs:311-315):
    // while the minimal-value filter is enabled, a matched row whose
    // parsed OCR value is below the affix's minimal value draws the
    // rectangle. `GetAffixMinimalValue` defaults a missing id to 0
    // (AffixManager.cs:610-613), and the filter-off verdict keeps
    // `below_minimal_value: false` because the comparison did not run.
    let preset = AffixPreset {
        name: "Thresholds".to_string(),
        affixes: vec![affix_requirement("AttackSpeed", "gloves", "Green")],
    };
    let thresholds = vec![AffixValueThreshold {
        affix_id: "AttackSpeed".to_string(),
        threshold: 12.0,
    }];
    let rows = |value: Option<&str>| {
        vec![affix_rule_row_with_value(
            "AttackSpeed",
            "gloves",
            ParsedAffixKind::Normal,
            Some(AffixRowId(0)),
            value,
        )]
    };

    // Filter disabled: the comparison never runs.
    assert!(
        !matches!(
            evaluate_affix_preset_rows_with_thresholds(
                &rows(Some("+3.0%")),
                &preset,
                false,
                &thresholds
            )[0],
            AffixRowVerdict::Matched {
                below_minimal_value: true,
                ..
            }
        ),
        "filter off must not flag below-minimal"
    );

    // Below the threshold: flagged.
    assert!(matches!(
        evaluate_affix_preset_rows_with_thresholds(
            &rows(Some("+3.0%")),
            &preset,
            true,
            &thresholds
        )[0],
        AffixRowVerdict::Matched {
            below_minimal_value: true,
            ..
        }
    ));
    // At or above the threshold: not flagged.
    assert!(!matches!(
        evaluate_affix_preset_rows_with_thresholds(
            &rows(Some("+12.0%")),
            &preset,
            true,
            &thresholds
        )[0],
        AffixRowVerdict::Matched {
            below_minimal_value: true,
            ..
        }
    ));
    // Unparsable OCR value: below every threshold.
    assert!(matches!(
        evaluate_affix_preset_rows_with_thresholds(
            &rows(Some("garbage")),
            &preset,
            true,
            &thresholds
        )[0],
        AffixRowVerdict::Matched {
            below_minimal_value: true,
            ..
        }
    ));
    // No threshold configured: the upstream default 0 passes any
    // parsable value.
    assert!(!matches!(
        evaluate_affix_preset_rows_with_thresholds(&rows(Some("+5.0%")), &preset, true, &[])[0],
        AffixRowVerdict::Matched {
            below_minimal_value: true,
            ..
        }
    ));
}

#[test]
fn evaluate_affix_preset_rows_with_thresholds_tracks_threshold_edits() {
    // The SetAffix dialog's minimal-value field edits the threshold the
    // rule evaluation consumes (`AffixManager.SetAffixMinimalValue`,
    // AffixManager.cs:1087-1092 → `GetAffixMinimalValue`,
    // AffixManager.cs:610-613). The same detected row must therefore
    // follow the edit: raising the threshold above the row's value
    // flags it, lowering it back clears the flag, a threshold for a
    // different affix id leaves it alone, and the reset (no thresholds
    // at all) falls back to the upstream `0` default.
    let preset = AffixPreset {
        name: "Edited".to_string(),
        affixes: vec![
            affix_requirement("AttackSpeed", "gloves", "Green"),
            affix_requirement("CritChance", "gloves", "Green"),
        ],
    };
    let rows = vec![affix_rule_row_with_value(
        "AttackSpeed",
        "gloves",
        ParsedAffixKind::Normal,
        Some(AffixRowId(0)),
        Some("+10.0%"),
    )];
    let below_minimal = |thresholds: &[AffixValueThreshold]| {
        matches!(
            evaluate_affix_preset_rows_with_thresholds(&rows, &preset, true, thresholds)[0],
            AffixRowVerdict::Matched {
                below_minimal_value: true,
                ..
            }
        )
    };
    let threshold = |affix_id: &str, threshold: f64| AffixValueThreshold {
        affix_id: affix_id.to_string(),
        threshold,
    };

    // Stored threshold below the detected value: passing.
    assert!(!below_minimal(&[threshold("AttackSpeed", 5.0)]));

    // The user raises the threshold above the value: the row flips.
    assert!(below_minimal(&[threshold("AttackSpeed", 15.0)]));

    // The user lowers it again: the row passes once more.
    assert!(!below_minimal(&[threshold("AttackSpeed", 10.0)]));

    // Another affix's threshold never touches this row.
    assert!(!below_minimal(&[
        threshold("AttackSpeed", 5.0),
        threshold("CritChance", 99.0),
    ]));

    // Reset (the dialog's confirmed reset clears every threshold): the
    // upstream `0` default passes the parsable value.
    assert!(!below_minimal(&[]));
}

#[test]
fn evaluate_affix_preset_rows_gate_fallback_on_first_id_match_and_exact_type() {
    // Upstream keeps only the FIRST entry with a matching Id for the
    // IsAnyType fallback (AffixManager.cs:554-555), so a later any-type
    // entry cannot rescue a row the first Id match rejected, and item
    // types compare exactly like `Type.Equals` (:548).
    let preset = AffixPreset {
        name: "Gated".to_string(),
        affixes: vec![
            affix_requirement("AttackSpeed", "helm", "Green"),
            flagged_affix_requirement("AttackSpeed", "gloves", "Blue", true, false, false),
        ],
    };

    // The strict pass rejects both entries on implicit flag equality
    // (:548-549) and the fallback gates on the first Id match, which is
    // not any-type.
    let verdicts = evaluate_affix_preset_rows(
        &[affix_rule_row(
            "AttackSpeed",
            "gloves",
            ParsedAffixKind::Implicit,
            Some(AffixRowId(2)),
        )],
        &preset,
    );
    assert_eq!(
        verdicts,
        vec![AffixRowVerdict::Unmatched {
            affix_id: "AttackSpeed".to_string(),
            color: "Red".to_string(),
        }]
    );

    // Slot strings compare exactly, without case folding.
    let verdicts = evaluate_affix_preset_rows(
        &[affix_rule_row(
            "AttackSpeed",
            "Helm",
            ParsedAffixKind::Normal,
            Some(AffixRowId(3)),
        )],
        &preset,
    );
    assert_eq!(
        verdicts,
        vec![AffixRowVerdict::Unmatched {
            affix_id: "AttackSpeed".to_string(),
            color: "Red".to_string(),
        }]
    );
}

#[test]
fn evaluate_affix_preset_marks_a_row_through_one_entry_only() {
    // Row-oriented upstream matching (AffixManager.cs:530-560): a row
    // claims exactly one preset entry, so a strict entry shadowing an
    // any-type duplicate cannot mark the same tooltip line twice.
    let item = ParsedItemForRules {
        item_type: "gloves".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![parsed_affix("AttackSpeed", ParsedAffixKind::Normal)],
    };
    let preset = AffixPreset {
        name: "Duplicated".to_string(),
        affixes: vec![
            affix_requirement("AttackSpeed", "gloves", "Green"),
            flagged_affix_requirement("AttackSpeed", "ring", "Gold", true, false, false),
        ],
    };

    let result = evaluate_affix_preset(&item, "Duplicated", &[preset]);

    assert_eq!(result.matched_affixes.len(), 1);
    assert_eq!(result.matched_affixes[0].color, "Green");
    assert_eq!(result.missing_affixes.len(), 1);
    assert_eq!(result.missing_affixes[0].affix_id, "AttackSpeed");
}

#[test]
fn evaluate_affix_preset_rows_evaluates_imported_whirlwind_preset() {
    // Entries mirroring the imported maxroll whirlwind preset
    // (tests/fixtures/imports/maxroll/expected/whirlwind_endgame.json):
    // per-slot canonical IdName entries with greater, implicit and
    // tempered flags, evaluated against a synthetic chest item.
    let preset = AffixPreset {
        name: "Whirlwind Endgame".to_string(),
        affixes: vec![
            AffixRequirement {
                affix_id: "AttackSpeed".to_string(),
                label: None,
                item_type: "helm".to_string(),
                color: "Green".to_string(),
                is_any_type: false,
                // Imported greater-roll entry; the flag is metadata and
                // not part of the match predicate.
                is_greater: true,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            },
            affix_requirement("AttackSpeed", "ring", "Green"),
            flagged_affix_requirement("AttackSpeed_Basics", "ring", "Green", false, true, false),
            affix_requirement("Barrier_Strength_Percent", "chest", "Green"),
            flagged_affix_requirement(
                "Barrier_Strength_Percent",
                "chest",
                "Green",
                false,
                false,
                true,
            ),
            affix_requirement("Berserk_Damage", "amulet", "Green"),
        ],
    };

    let rows = vec![
        affix_rule_row(
            "Barrier_Strength_Percent",
            "chest",
            ParsedAffixKind::Normal,
            Some(AffixRowId(3)),
        ),
        affix_rule_row(
            "AttackSpeed",
            "chest",
            ParsedAffixKind::Greater,
            Some(AffixRowId(4)),
        ),
        affix_rule_row(
            "Barrier_Strength_Percent",
            "chest",
            ParsedAffixKind::Tempered,
            Some(AffixRowId(5)),
        ),
        affix_rule_row(
            "Berserk_Damage",
            "chest",
            ParsedAffixKind::Normal,
            Some(AffixRowId(6)),
        ),
    ];

    let verdicts = evaluate_affix_preset_rows(&rows, &preset);

    // The explicit chest barrier roll and its tempered roll each claim
    // their own preset entry.
    assert_eq!(
        verdicts[0],
        AffixRowVerdict::Matched {
            requirement_index: 3,
            affix_id: "Barrier_Strength_Percent".to_string(),
            color: "Green".to_string(),
            pass: AffixMatchPass::Strict,
            is_greater: false,
            is_any_type: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
            below_minimal_value: false,
        }
    );
    assert_eq!(
        verdicts[2],
        AffixRowVerdict::Matched {
            requirement_index: 4,
            affix_id: "Barrier_Strength_Percent".to_string(),
            color: "Green".to_string(),
            pass: AffixMatchPass::Strict,
            is_greater: false,
            is_any_type: false,
            is_implicit: false,
            is_tempered: true,
            tuning_prisms: Vec::new(),
            below_minimal_value: false,
        }
    );
    // AttackSpeed only lists the helm/ring slots, so even a greater chest
    // roll stays red — item types compare exactly.
    assert_eq!(
        verdicts[1],
        AffixRowVerdict::Unmatched {
            affix_id: "AttackSpeed".to_string(),
            color: "Red".to_string(),
        }
    );
    assert_eq!(
        verdicts[3],
        AffixRowVerdict::Unmatched {
            affix_id: "Berserk_Damage".to_string(),
            color: "Red".to_string(),
        }
    );
}

#[test]
fn apply_item_power_filter_records_enabled_disabled_and_sigil_bypass() {
    let mut result = evaluate_affix_preset(
        &ParsedItemForRules {
            item_type: "amulet".to_string(),
            rarity: Some("legendary".to_string()),
            affixes: Vec::new(),
        },
        "Empty",
        &[AffixPreset {
            name: "Empty".to_string(),
            affixes: Vec::new(),
        }],
    );
    apply_item_power_and_value_filters(
        &mut result,
        Some(795),
        "amulet",
        &RuleFilterSettings {
            is_item_power_limit_enabled: true,
            item_power_limit: 800,
            is_minimal_affix_value_filter_enabled: false,
        },
        &[],
    );

    assert_eq!(result.item_power_status.status, RuleCheckStatus::Failed);
    assert_eq!(result.item_power_status.actual_value, Some(795.0));
    assert_eq!(result.item_power_status.threshold_value, Some(800.0));
    assert_eq!(result.final_decision, RuleDecision::Unwanted);
    assert!(result
        .explanation
        .branches
        .iter()
        .any(
            |branch| branch.source.upstream_method == "OverlayHandler.DrawGraphics"
                && branch.message == "item power 795 below limit 800"
        ));

    apply_item_power_and_value_filters(
        &mut result,
        Some(1),
        "sigil",
        &RuleFilterSettings {
            is_item_power_limit_enabled: true,
            item_power_limit: 800,
            is_minimal_affix_value_filter_enabled: false,
        },
        &[],
    );
    assert_eq!(result.item_power_status.status, RuleCheckStatus::Passed);

    apply_item_power_and_value_filters(
        &mut result,
        Some(1),
        "amulet",
        &RuleFilterSettings {
            is_item_power_limit_enabled: false,
            item_power_limit: 800,
            is_minimal_affix_value_filter_enabled: false,
        },
        &[],
    );
    assert_eq!(
        result.item_power_status.status,
        RuleCheckStatus::NotEvaluated
    );
}

#[test]
fn minimal_affix_value_filter_parses_parser_value_text_and_records_failures() {
    assert_eq!(parse_affix_value_text("+32.0%"), Some(32.0));
    assert_eq!(parse_affix_value_text("-1,234%"), Some(1234.0));
    assert_eq!(parse_affix_value_text("[10.5 - 20.5]%"), Some(10.5));

    let item = ParsedItemForRules {
        item_type: "amulet".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![ParsedAffixForRules {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            value_text: Some("[10.5 - 20.5]%".to_string()),
            source_text: "[10.5 - 20.5]% Attack Speed".to_string(),
            row_id: Some(AffixRowId(7)),
            kind: ParsedAffixKind::Normal,
        }],
    };
    let preset = AffixPreset {
        name: "Speed Farm".to_string(),
        affixes: vec![AffixRequirement {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            item_type: "amulet".to_string(),
            color: "Green".to_string(),
            is_any_type: false,
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        }],
    };
    let mut result = evaluate_affix_preset(&item, "Speed Farm", &[preset]);

    apply_item_power_and_value_filters(
        &mut result,
        Some(925),
        "amulet",
        &RuleFilterSettings {
            is_item_power_limit_enabled: true,
            item_power_limit: 800,
            is_minimal_affix_value_filter_enabled: true,
        },
        &[AffixValueThreshold {
            affix_id: "Affix_AttackSpeed".to_string(),
            threshold: 12.0,
        }],
    );

    assert_eq!(result.item_power_status.status, RuleCheckStatus::Passed);
    assert_eq!(result.minimal_value_status.status, RuleCheckStatus::Failed);
    assert_eq!(
        result.matched_affixes[0].value_status.status,
        RuleCheckStatus::Failed
    );
    assert_eq!(
        result.matched_affixes[0].value_status.actual_value,
        Some(10.5)
    );
    assert_eq!(
        result.matched_affixes[0].value_status.threshold_value,
        Some(12.0)
    );
    assert!(result
        .explanation
        .branches
        .iter()
        .any(|branch| branch.message == "affix Affix_AttackSpeed value 10.5 below threshold 12"));
}

#[test]
fn multi_build_disabled_uses_selected_preset_without_build_winner() {
    let item = item_with_attack_speed("amulet");
    let presets = vec![speed_preset("Solo", "amulet", "Green")];
    let result = evaluate_multi_build_affix_presets(
        &item,
        &MultiBuildSettings {
            enabled: false,
            selected_preset: "Solo".to_string(),
            builds: Vec::new(),
        },
        &presets,
    );

    assert_eq!(result.final_decision, RuleDecision::Wanted);
    assert_eq!(result.explanation.active_preset_names, vec!["Solo"]);
    assert!(result.explanation.matched_build_names.is_empty());
    assert_eq!(result.explanation.selected_build_name, None);
}

#[test]
fn multi_build_reports_no_matching_build() {
    let item = item_with_attack_speed("ring");
    let presets = vec![speed_preset("Amulet Build", "amulet", "Green")];
    let result = evaluate_multi_build_affix_presets(
        &item,
        &MultiBuildSettings {
            enabled: true,
            selected_preset: "Solo".to_string(),
            builds: vec![MultiBuildEntry {
                build_name: "Amulet Build".to_string(),
                preset_name: "Amulet Build".to_string(),
                color: "Blue".to_string(),
            }],
        },
        &presets,
    );

    assert_eq!(result.final_decision, RuleDecision::Unwanted);
    assert!(result.explanation.matched_build_names.is_empty());
    assert_eq!(result.explanation.selected_build_name, None);
    assert!(result
        .explanation
        .branches
        .iter()
        .any(|branch| branch.message == "no matching multi-build preset"));
}

#[test]
fn multi_build_selects_first_winner_and_preserves_all_matching_build_names() {
    let item = item_with_attack_speed("amulet");
    let presets = vec![
        speed_preset("Speed", "amulet", "Green"),
        speed_preset("Boss", "amulet", "Gold"),
    ];
    let result = evaluate_multi_build_affix_presets(
        &item,
        &MultiBuildSettings {
            enabled: true,
            selected_preset: "Solo".to_string(),
            builds: vec![
                MultiBuildEntry {
                    build_name: "Speed".to_string(),
                    preset_name: "Speed".to_string(),
                    color: "Blue".to_string(),
                },
                MultiBuildEntry {
                    build_name: "Boss".to_string(),
                    preset_name: "Boss".to_string(),
                    color: "Purple".to_string(),
                },
            ],
        },
        &presets,
    );

    assert_eq!(result.final_decision, RuleDecision::Wanted);
    assert_eq!(
        result.explanation.selected_build_name.as_deref(),
        Some("Speed")
    );
    assert_eq!(
        result.explanation.matched_build_names,
        vec!["Speed", "Boss"]
    );
    assert_eq!(result.matched_affixes[0].color, "Blue");
    assert!(result
        .explanation
        .branches
        .iter()
        .any(|branch| branch.message == "selected multi-build winner Speed"));
}

#[test]
fn selected_preset_not_found_is_explicitly_reported() {
    let item = item_with_attack_speed("amulet");
    let result = evaluate_affix_preset(&item, "Missing", &[]);

    assert_eq!(result.final_decision, RuleDecision::Unknown);
    assert!(result.explanation.active_preset_names.is_empty());
    assert!(result
        .explanation
        .branches
        .iter()
        .any(|branch| branch.message == "selected preset 'Missing' was not found"));
}

fn item_with_attack_speed(item_type: &str) -> ParsedItemForRules {
    ParsedItemForRules {
        item_type: item_type.to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![ParsedAffixForRules {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            value_text: Some("+32.0%".to_string()),
            source_text: "+32.0% Attack Speed".to_string(),
            row_id: Some(AffixRowId(2)),
            kind: ParsedAffixKind::Normal,
        }],
    }
}

fn speed_preset(name: &str, item_type: &str, color: &str) -> AffixPreset {
    AffixPreset {
        name: name.to_string(),
        affixes: vec![AffixRequirement {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            item_type: item_type.to_string(),
            color: color.to_string(),
            is_any_type: false,
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        }],
    }
}

fn traced_row(affix_id: &str, item_type: &str, kind: ParsedAffixKind, row: u32) -> AffixRuleRow {
    affix_rule_row(affix_id, item_type, kind, Some(AffixRowId(row)))
}

fn greater_requirement(affix_id: &str, item_type: &str, color: &str) -> AffixRequirement {
    AffixRequirement {
        is_greater: true,
        ..affix_requirement(affix_id, item_type, color)
    }
}

const CRIT_INJURED: &str = "CritChance_To_LowLife;CritChance_To_LowLife_Jewelry";

const ATTACK_SPEED: &str = "AttackSpeed;AttackSpeed_Jewelry";

#[test]
fn trace_follows_the_upstream_strict_search_for_every_affix_kind() {
    // AffixManager.cs:548-549: id, exact item type, implicit equality,
    // tempered equality or (tempered entry and greater roll).
    let preset = AffixPreset {
        name: "Kinds".to_string(),
        affixes: vec![
            affix_requirement(CRIT_INJURED, "ring", "Green"),
            flagged_affix_requirement(ATTACK_SPEED, "ring", "Blue", false, true, false),
            flagged_affix_requirement("Damage_to_LowLife", "ring", "Yellow", false, false, true),
            greater_requirement("DamageHealthy", "ring", "Purple"),
        ],
    };
    let presets = [preset];
    let rows = [
        traced_row(CRIT_INJURED, "ring", ParsedAffixKind::Normal, 0),
        traced_row(ATTACK_SPEED, "ring", ParsedAffixKind::Implicit, 1),
        traced_row("Damage_to_LowLife", "ring", ParsedAffixKind::Tempered, 2),
        // Season 11: a tempered entry also accepts a greater roll.
        traced_row("Damage_to_LowLife", "ring", ParsedAffixKind::Greater, 3),
        // A greater roll is not tempered, so it satisfies a plain entry.
        traced_row(CRIT_INJURED, "ring", ParsedAffixKind::Greater, 4),
        // The greater flag of an entry is not part of the search.
        traced_row("DamageHealthy", "ring", ParsedAffixKind::Normal, 5),
        // Implicit and tempered flags must be equal.
        traced_row(CRIT_INJURED, "ring", ParsedAffixKind::Implicit, 6),
        traced_row(ATTACK_SPEED, "ring", ParsedAffixKind::Normal, 7),
        traced_row(CRIT_INJURED, "ring", ParsedAffixKind::Tempered, 8),
    ];

    let traces = trace_affix_rows(&rows, "Kinds", &presets, AffixValueFilter::DISABLED);

    let summary: Vec<_> = traces
        .iter()
        .map(|trace| {
            (
                trace.row_id,
                trace.pass,
                trace.entry.as_ref().map(|entry| entry.requirement_index),
                trace.color.as_str(),
                trace.shape,
                trace.rejection,
            )
        })
        .collect();
    let strict = Some(AffixMatchPass::Strict);
    assert_eq!(
        summary,
        vec![
            (
                Some(AffixRowId(0)),
                strict,
                Some(0),
                "Green",
                RuleOverlayShape::Circle,
                None
            ),
            (
                Some(AffixRowId(1)),
                strict,
                Some(1),
                "Blue",
                RuleOverlayShape::Circle,
                None
            ),
            (
                Some(AffixRowId(2)),
                strict,
                Some(2),
                "Yellow",
                RuleOverlayShape::Circle,
                None
            ),
            (
                Some(AffixRowId(3)),
                strict,
                Some(2),
                "Yellow",
                RuleOverlayShape::Circle,
                None
            ),
            (
                Some(AffixRowId(4)),
                strict,
                Some(0),
                "Green",
                RuleOverlayShape::Circle,
                None
            ),
            (
                Some(AffixRowId(5)),
                strict,
                Some(3),
                "Purple",
                RuleOverlayShape::Triangle,
                None
            ),
            (
                Some(AffixRowId(6)),
                None,
                None,
                "Red",
                RuleOverlayShape::Circle,
                Some(RuleRejection::FirstSameIdEntryNotAnyType {
                    requirement_index: 0
                })
            ),
            (
                Some(AffixRowId(7)),
                None,
                None,
                "Red",
                RuleOverlayShape::Circle,
                Some(RuleRejection::FirstSameIdEntryNotAnyType {
                    requirement_index: 1
                })
            ),
            (
                Some(AffixRowId(8)),
                None,
                None,
                "Red",
                RuleOverlayShape::Circle,
                Some(RuleRejection::FirstSameIdEntryNotAnyType {
                    requirement_index: 0
                })
            ),
        ]
    );
    let traced = &traces[3];
    assert_eq!(traced.preset_name, "Kinds");
    assert_eq!(traced.affix_id, "Damage_to_LowLife");
    assert_eq!(traced.item_type, "ring");
    assert_eq!(traced.row_kind, ParsedAffixKind::Greater);
    let entry = traced.entry.as_ref().expect("matched entry");
    assert!(entry.is_tempered && !entry.is_greater && !entry.is_any_type);
    assert_eq!(traced.value_check, RuleValueCheck::NotEvaluated);
}

#[test]
fn any_type_fallback_consults_only_the_first_entry_with_the_id() {
    // AffixManager.cs:552-556: `FirstOrDefault(a => a.Id == id)`, then
    // its `IsAnyType`. A later any-type entry with the same id is never
    // reached; an index over "any any-type entry" would be wrong.
    let hidden_any_type = AffixPreset {
        name: "HiddenAnyType".to_string(),
        affixes: vec![
            affix_requirement(ATTACK_SPEED, "gloves", "Green"),
            flagged_affix_requirement(ATTACK_SPEED, "helm", "Gold", true, false, false),
        ],
    };
    let first_any_type = AffixPreset {
        name: "FirstAnyType".to_string(),
        affixes: vec![
            flagged_affix_requirement(ATTACK_SPEED, "helm", "Gold", true, false, false),
            affix_requirement(ATTACK_SPEED, "ring", "Green"),
        ],
    };
    let presets = [hidden_any_type, first_any_type];
    let ring = [traced_row(ATTACK_SPEED, "ring", ParsedAffixKind::Normal, 0)];
    let amulet = [traced_row(
        ATTACK_SPEED,
        "amulet",
        ParsedAffixKind::Implicit,
        0,
    )];
    let helm = [traced_row(ATTACK_SPEED, "helm", ParsedAffixKind::Normal, 0)];
    let trace = |rows: &[AffixRuleRow], preset: &str| {
        trace_affix_rows(rows, preset, &presets, AffixValueFilter::DISABLED).remove(0)
    };

    let rejected = trace(&ring, "HiddenAnyType");
    assert_eq!(
        rejected.rejection,
        Some(RuleRejection::FirstSameIdEntryNotAnyType {
            requirement_index: 0
        })
    );
    assert_eq!(
        (rejected.color.as_str(), rejected.shape),
        ("Red", RuleOverlayShape::Circle)
    );

    // The strict pass runs over every entry before the fallback: the
    // ring entry wins over the earlier any-type entry.
    let strict = trace(&ring, "FirstAnyType");
    assert_eq!(strict.pass, Some(AffixMatchPass::Strict));
    assert_eq!(
        (strict.color.as_str(), strict.shape),
        ("Green", RuleOverlayShape::Circle)
    );

    // The fallback ignores item type and the implicit/tempered flags.
    let fallback = trace(&amulet, "FirstAnyType");
    assert_eq!(fallback.pass, Some(AffixMatchPass::AnyTypeFallback));
    assert_eq!(
        fallback.entry.as_ref().map(|entry| entry.requirement_index),
        Some(0)
    );
    assert_eq!(
        (fallback.color.as_str(), fallback.shape),
        ("Gold", RuleOverlayShape::Rectangle)
    );

    // An any-type entry of the item's own type matches in the strict
    // pass and still draws the any-type rectangle (OverlayHandler.cs:300).
    let own_type = trace(&helm, "FirstAnyType");
    assert_eq!(own_type.pass, Some(AffixMatchPass::Strict));
    assert_eq!(own_type.shape, RuleOverlayShape::Rectangle);
}

#[test]
fn deliberately_non_matching_rules_stay_red_with_their_reason() {
    let preset = AffixPreset {
        name: "Helm".to_string(),
        affixes: vec![affix_requirement(CRIT_INJURED, "helm", "Green")],
    };
    let presets = [preset];
    let rows = [
        // Display label instead of the canonical type id.
        traced_row(CRIT_INJURED, "Legendary Helm", ParsedAffixKind::Normal, 0),
        // Shortened alias instead of the catalog IdName.
        traced_row("CritChance_To_LowLife", "helm", ParsedAffixKind::Normal, 1),
        traced_row(ATTACK_SPEED, "helm", ParsedAffixKind::Normal, 2),
    ];

    let traces = trace_affix_rows(&rows, "Helm", &presets, AffixValueFilter::DISABLED);
    assert_eq!(
        traces
            .iter()
            .map(|trace| trace.rejection)
            .collect::<Vec<_>>(),
        vec![
            Some(RuleRejection::FirstSameIdEntryNotAnyType {
                requirement_index: 0
            }),
            Some(RuleRejection::NoEntryForAffix),
            Some(RuleRejection::NoEntryForAffix),
        ]
    );
    assert!(traces
        .iter()
        .all(|trace| trace.color == "Red" && !trace.is_match()));

    let missing = trace_affix_rows(&rows, "Renamed", &presets, AffixValueFilter::DISABLED);
    assert!(missing.iter().all(
        |trace| trace.rejection == Some(RuleRejection::PresetNotFound)
            && trace.color == "Red"
            && trace.preset_name == "Renamed"
    ));
}

#[test]
fn minimal_value_check_uses_the_upstream_affix_value_for_every_row() {
    // ScreenProcessHandler.cs:1078-1085 stores value and threshold for
    // every area; OverlayHandler.cs:296-319 checks any-type, greater, then
    // the minimal value.
    let preset = AffixPreset {
        name: "Minimal".to_string(),
        affixes: vec![
            affix_requirement(CRIT_INJURED, "ring", "Green"),
            flagged_affix_requirement(ATTACK_SPEED, "ring", "Gold", true, false, false),
            greater_requirement("DamageHealthy", "ring", "Purple"),
        ],
    };
    let presets = [preset];
    let thresholds = [
        AffixValueThreshold {
            affix_id: CRIT_INJURED.to_string(),
            threshold: 10.0,
        },
        AffixValueThreshold {
            affix_id: ATTACK_SPEED.to_string(),
            threshold: 10.0,
        },
        AffixValueThreshold {
            affix_id: "DamageHealthy".to_string(),
            threshold: 10.0,
        },
        AffixValueThreshold {
            affix_id: "Unlisted".to_string(),
            threshold: 10.0,
        },
    ];
    let row = |affix_id: &str, value: Option<&str>| {
        affix_rule_row_with_value(affix_id, "ring", ParsedAffixKind::Normal, None, value)
    };
    let rows = [
        row(CRIT_INJURED, Some("12.5%")),
        row(CRIT_INJURED, Some("9.5%")),
        row(ATTACK_SPEED, Some("3%")),
        row("DamageHealthy", Some("3%")),
        // Not in the preset: the red default is still compared.
        row("Unlisted", Some("3%")),
        // No number: upstream `TextToAffixValue` yields 0.0.
        row(CRIT_INJURED, None),
        row("NoThreshold", Some("garbage")),
    ];
    let enabled = AffixValueFilter {
        enabled: true,
        thresholds: &thresholds,
    };

    let traces = trace_affix_rows(&rows, "Minimal", &presets, enabled);
    assert_eq!(
        traces
            .iter()
            .map(|trace| (trace.value_check, trace.color.as_str(), trace.shape))
            .collect::<Vec<_>>(),
        vec![
            (
                RuleValueCheck::AtOrAbove {
                    value: 12.5,
                    threshold: 10.0
                },
                "Green",
                RuleOverlayShape::Circle
            ),
            (
                RuleValueCheck::Below {
                    value: 9.5,
                    threshold: 10.0
                },
                "Green",
                RuleOverlayShape::Rectangle
            ),
            (
                RuleValueCheck::Below {
                    value: 3.0,
                    threshold: 10.0
                },
                "Gold",
                RuleOverlayShape::Rectangle
            ),
            (
                RuleValueCheck::Below {
                    value: 3.0,
                    threshold: 10.0
                },
                "Purple",
                RuleOverlayShape::Triangle
            ),
            (
                RuleValueCheck::Below {
                    value: 3.0,
                    threshold: 10.0
                },
                "Red",
                RuleOverlayShape::Rectangle
            ),
            (
                RuleValueCheck::Below {
                    value: 0.0,
                    threshold: 10.0
                },
                "Green",
                RuleOverlayShape::Rectangle
            ),
            (
                RuleValueCheck::AtOrAbove {
                    value: 0.0,
                    threshold: 0.0
                },
                "Red",
                RuleOverlayShape::Circle
            ),
        ]
    );
    // Below the minimal value never changes the color (red is
    // not "value too low").
    assert_eq!(traces[1].color, "Green");

    let off = trace_affix_rows(&rows, "Minimal", &presets, AffixValueFilter::DISABLED);
    assert!(off
        .iter()
        .all(|trace| trace.value_check == RuleValueCheck::NotEvaluated));
    assert_eq!(off[1].shape, RuleOverlayShape::Circle);
}

#[test]
fn a_build_trace_recolors_matches_only() {
    let preset = AffixPreset {
        name: "Build".to_string(),
        affixes: vec![affix_requirement(CRIT_INJURED, "ring", "Green")],
    };
    let presets = [preset];
    let rows = [
        traced_row(CRIT_INJURED, "ring", ParsedAffixKind::Normal, 0),
        traced_row(ATTACK_SPEED, "ring", ParsedAffixKind::Normal, 1),
    ];
    let traces: Vec<_> = trace_affix_rows(&rows, "Build", &presets, AffixValueFilter::DISABLED)
        .into_iter()
        .map(|trace| trace.for_build(2, "#FFAA00"))
        .collect();

    assert_eq!(traces[0].color, "#FFAA00");
    assert_eq!(traces[0].build_column, Some(2));
    assert_eq!(traces[1].color, "Red");
    assert_eq!(traces[1].build_column, Some(2));
}
