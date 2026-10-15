use super::*;

#[test]
fn evaluate_non_affix_preset_rows_mirrors_upstream_id_and_type_lookups() {
    // Aspects and sigils match Id + item type (AffixManager.cs:632 and
    // :726), uniques and runes match Id only (:787 and :836); unmatched
    // rows get the red default, sigils per display mode (:716-721).
    let preset = non_affix_preset("Solo");
    let row = |id: &str, item_type: &str, kind: RuleNonAffixKind, is_unique_item: bool| {
        ParsedNonAffixForRules {
            id: id.to_string(),
            item_type: item_type.to_string(),
            kind,
            is_unique_item,
        }
    };
    let rows = vec![
        row(
            "Aspect_BoldChieftain",
            "ring",
            RuleNonAffixKind::Aspect,
            false,
        ),
        row(
            "Aspect_BoldChieftain",
            "helm",
            RuleNonAffixKind::Aspect,
            false,
        ),
        row(
            "Unique_HarlequinCrest",
            "helm",
            RuleNonAffixKind::Unique,
            true,
        ),
        row("Rune_Yul", "rune", RuleNonAffixKind::Rune, false),
        row("Rune_Anchor", "rune", RuleNonAffixKind::Rune, false),
        row("Sigil_Pit", "sigil", RuleNonAffixKind::Sigil, false),
        row("Sigil_Undercity", "sigil", RuleNonAffixKind::Sigil, false),
    ];

    let whitelisting =
        evaluate_non_affix_preset_rows(&rows, &preset, SigilDisplayMode::Whitelisting);

    assert_eq!(
        whitelisting,
        vec![
            NonAffixRowVerdict::Matched {
                requirement_index: 0,
                id: "Aspect_BoldChieftain".to_string(),
                color: "Gold".to_string(),
            },
            NonAffixRowVerdict::Unmatched {
                id: "Aspect_BoldChieftain".to_string(),
                color: "Red".to_string(),
            },
            NonAffixRowVerdict::Matched {
                requirement_index: 1,
                id: "Unique_HarlequinCrest".to_string(),
                color: "Orange".to_string(),
            },
            NonAffixRowVerdict::Matched {
                requirement_index: 0,
                id: "Rune_Yul".to_string(),
                color: "Purple".to_string(),
            },
            NonAffixRowVerdict::Unmatched {
                id: "Rune_Anchor".to_string(),
                color: "Red".to_string(),
            },
            NonAffixRowVerdict::Matched {
                requirement_index: 0,
                id: "Sigil_Pit".to_string(),
                color: "Teal".to_string(),
            },
            NonAffixRowVerdict::Unmatched {
                id: "Sigil_Undercity".to_string(),
                color: "Red".to_string(),
            },
        ]
    );

    let blacklisting =
        evaluate_non_affix_preset_rows(&rows, &preset, SigilDisplayMode::Blacklisting);
    assert_eq!(
        blacklisting[6],
        NonAffixRowVerdict::Unmatched {
            id: "Sigil_Undercity".to_string(),
            color: "Green".to_string(),
        }
    );
}

#[test]
fn non_affix_lookup_matches_aspect_unique_rune_and_sigil_defaults() {
    let preset = non_affix_preset("Solo");

    assert_eq!(
        find_power_match(
            "Solo",
            "Aspect_BoldChieftain",
            "ring",
            RulePowerKind::Aspect,
            std::slice::from_ref(&preset)
        )
        .map(|matched| matched.color),
        Some("Gold".to_string())
    );
    assert_eq!(
        find_power_match(
            "Solo",
            "Unique_HarlequinCrest",
            "helm",
            RulePowerKind::Unique,
            std::slice::from_ref(&preset)
        )
        .map(|matched| matched.color),
        Some("Orange".to_string())
    );
    assert_eq!(
        find_rune_match("Solo", "Rune_Yul", std::slice::from_ref(&preset))
            .map(|matched| matched.color),
        Some("Purple".to_string())
    );
    assert_eq!(
        find_sigil_match(
            "Missing",
            "Sigil_Pit",
            "sigil",
            SigilDisplayMode::Whitelisting,
            std::slice::from_ref(&preset)
        )
        .color,
        "Red"
    );
    assert_eq!(
        find_sigil_match(
            "Missing",
            "Sigil_Pit",
            "sigil",
            SigilDisplayMode::Blacklisting,
            &[preset]
        )
        .color,
        "Green"
    );
}

#[test]
fn multi_build_non_affix_matches_use_build_color() {
    let result = evaluate_non_affix_multi_build(
        &[
            ParsedNonAffixForRules {
                id: "Rune_Yul".to_string(),
                item_type: "rune".to_string(),
                kind: RuleNonAffixKind::Rune,
                is_unique_item: false,
            },
            ParsedNonAffixForRules {
                id: "Aspect_BoldChieftain".to_string(),
                item_type: "ring".to_string(),
                kind: RuleNonAffixKind::Aspect,
                is_unique_item: false,
            },
        ],
        &MultiBuildSettings {
            enabled: true,
            selected_preset: "Solo".to_string(),
            builds: vec![MultiBuildEntry {
                build_name: "Speed".to_string(),
                preset_name: "Speed".to_string(),
                color: "Blue".to_string(),
            }],
        },
        &[non_affix_preset("Speed")],
    );

    assert_eq!(result.matched_build_names, vec!["Speed"]);
    assert!(result.matches.iter().all(|matched| matched.color == "Blue"));
    assert_eq!(
        result
            .matches
            .iter()
            .map(|matched| matched.kind)
            .collect::<Vec<_>>(),
        vec![RuleNonAffixKind::Rune, RuleNonAffixKind::Aspect]
    );
}

fn non_affix_preset(name: &str) -> NonAffixPreset {
    NonAffixPreset {
        name: name.to_string(),
        powers: vec![
            NonAffixRequirement {
                id: "Aspect_BoldChieftain".to_string(),
                item_type: Some("ring".to_string()),
                color: "Gold".to_string(),
                kind: RuleNonAffixKind::Aspect,
            },
            NonAffixRequirement {
                id: "Unique_HarlequinCrest".to_string(),
                item_type: None,
                color: "Orange".to_string(),
                kind: RuleNonAffixKind::Unique,
            },
        ],
        runes: vec![NonAffixRequirement {
            id: "Rune_Yul".to_string(),
            item_type: None,
            color: "Purple".to_string(),
            kind: RuleNonAffixKind::Rune,
        }],
        sigils: vec![NonAffixRequirement {
            id: "Sigil_Pit".to_string(),
            item_type: Some("sigil".to_string()),
            color: "Teal".to_string(),
            kind: RuleNonAffixKind::Sigil,
        }],
    }
}
