use super::*;
use crate::affix::evaluate_affix_preset;
use crate::model::AffixPreset;
use crate::test_support::parsed_affix;

#[test]
fn trade_item_lookup_matches_type_and_required_affixes() {
    let item = ParsedItemForRules {
        item_type: "amulet".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![
            parsed_affix("Affix_AttackSpeed", ParsedAffixKind::Normal),
            parsed_affix("Affix_CriticalStrikeChance", ParsedAffixKind::Normal),
        ],
    };
    let mut result = evaluate_affix_preset(
        &item,
        "Empty",
        &[AffixPreset {
            name: "Empty".to_string(),
            affixes: Vec::new(),
        }],
    );

    apply_trade_item_lookup(
        &mut result,
        &item,
        &[
            TradeItemRule {
                trade_item_id: "trade-ring-nope".to_string(),
                item_type: "ring".to_string(),
                value_text: "1b".to_string(),
                affixes: vec![TradeAffixRequirement {
                    affix_id: "Affix_AttackSpeed".to_string(),
                    is_greater: false,
                }],
            },
            TradeItemRule {
                trade_item_id: "trade-amulet-speed-crit".to_string(),
                item_type: "amulet".to_string(),
                value_text: "5b".to_string(),
                affixes: vec![
                    TradeAffixRequirement {
                        affix_id: "Affix_AttackSpeed".to_string(),
                        is_greater: false,
                    },
                    TradeAffixRequirement {
                        affix_id: "Affix_CriticalStrikeChance".to_string(),
                        is_greater: false,
                    },
                ],
            },
        ],
    );

    assert_eq!(
        result
            .trade_marker
            .as_ref()
            .map(|marker| marker.trade_item_id.as_str()),
        Some("trade-amulet-speed-crit")
    );
    assert_eq!(
        result
            .trade_marker
            .as_ref()
            .map(|marker| marker.value_text.as_str()),
        Some("5b")
    );
    assert_eq!(
        result
            .trade_marker
            .as_ref()
            .map(|marker| marker.source.upstream_method.as_ref()),
        Some("TradeItemManager.FindTradeItem")
    );
    assert!(result.explanation.branches.iter().any(|branch| {
        branch.message == "trade item trade-amulet-speed-crit matched"
            && branch.decision == RuleDecision::Wanted
    }));
}

#[test]
fn trade_item_lookup_reports_no_match_for_type_or_missing_affix() {
    let item = ParsedItemForRules {
        item_type: "amulet".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![parsed_affix("Affix_AttackSpeed", ParsedAffixKind::Normal)],
    };
    let mut result = evaluate_affix_preset(
        &item,
        "Empty",
        &[AffixPreset {
            name: "Empty".to_string(),
            affixes: Vec::new(),
        }],
    );

    apply_trade_item_lookup(
        &mut result,
        &item,
        &[
            TradeItemRule {
                trade_item_id: "trade-ring-speed".to_string(),
                item_type: "ring".to_string(),
                value_text: "1b".to_string(),
                affixes: vec![TradeAffixRequirement {
                    affix_id: "Affix_AttackSpeed".to_string(),
                    is_greater: false,
                }],
            },
            TradeItemRule {
                trade_item_id: "trade-amulet-crit".to_string(),
                item_type: "amulet".to_string(),
                value_text: "2b".to_string(),
                affixes: vec![TradeAffixRequirement {
                    affix_id: "Affix_CriticalStrikeChance".to_string(),
                    is_greater: false,
                }],
            },
        ],
    );

    assert_eq!(result.trade_marker, None);
    assert!(result.explanation.branches.iter().any(|branch| {
        branch.message == "no trade item matched" && branch.decision == RuleDecision::Neutral
    }));
}

#[test]
fn trade_item_lookup_requires_greater_affix_when_configured() {
    let item = ParsedItemForRules {
        item_type: "gloves".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![
            parsed_affix("Affix_AttackSpeed", ParsedAffixKind::Normal),
            parsed_affix("Affix_CriticalStrikeChance", ParsedAffixKind::Greater),
        ],
    };
    let mut result = evaluate_affix_preset(
        &item,
        "Empty",
        &[AffixPreset {
            name: "Empty".to_string(),
            affixes: Vec::new(),
        }],
    );

    apply_trade_item_lookup(
        &mut result,
        &item,
        &[
            TradeItemRule {
                trade_item_id: "trade-gloves-speed-greater".to_string(),
                item_type: "gloves".to_string(),
                value_text: "3b".to_string(),
                affixes: vec![TradeAffixRequirement {
                    affix_id: "Affix_AttackSpeed".to_string(),
                    is_greater: true,
                }],
            },
            TradeItemRule {
                trade_item_id: "trade-gloves-crit-greater".to_string(),
                item_type: "gloves".to_string(),
                value_text: "4b".to_string(),
                affixes: vec![TradeAffixRequirement {
                    affix_id: "Affix_CriticalStrikeChance".to_string(),
                    is_greater: true,
                }],
            },
        ],
    );

    assert_eq!(
        result
            .trade_marker
            .as_ref()
            .map(|marker| marker.trade_item_id.as_str()),
        Some("trade-gloves-crit-greater")
    );
    assert!(result.explanation.branches.iter().any(|branch| {
        branch
            .message
            .contains("trade item trade-gloves-crit-greater matched")
    }));
}
