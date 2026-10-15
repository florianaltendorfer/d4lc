use super::*;
use crate::overlay::{RuleOverlayMarkerType, RuleOverlayTarget, RuleOverlayZOrderGroup};
use d4lc_core::Rect;

#[test]
fn rule_result_model_records_all_required_decision_surfaces() {
    let affix_source = RuleSourceNote::new(
        "D4Companion.Services/AffixManager.cs",
        "AffixManager.GetAffix",
        "preset affix match",
    );
    let overlay_source = RuleSourceNote::new(
        "D4Companion.Services/OverlayHandler.cs",
        "OverlayHandler.DrawGraphicsAffixes",
        "normal affix circle marker",
    );
    let trade_source = RuleSourceNote::new(
        "D4Companion.Services/TradeItemManager.cs",
        "TradeItemManager.FindTradeItem",
        "trade item type and affix match",
    );

    let result = RuleEvaluationResult {
        final_decision: RuleDecision::Wanted,
        final_source: Some(affix_source.clone()),
        matched_affixes: vec![RuleAffixMatch {
            affix_id: "Affix_AttackSpeed".to_string(),
            label: Some("Attack Speed".to_string()),
            color: "Green".to_string(),
            value_text: Some("+32.0%".to_string()),
            source_text: "+32.0% Attack Speed".to_string(),
            row_id: Some(AffixRowId(3)),
            required_by: vec!["Speed Farm".to_string()],
            is_greater_required: false,
            is_any_type_required: false,
            value_status: RuleCheck::passed(
                RuleCheckKind::MinimalAffixValue,
                Some(32.0),
                Some(30.0),
                affix_source.clone(),
            ),
            tuning_prisms: Vec::new(),
            source: affix_source.clone(),
            column_offset: 0,
        }],
        missing_affixes: vec![RuleMissingAffix {
            affix_id: "Affix_CriticalStrikeChance".to_string(),
            label: Some("Critical Strike Chance".to_string()),
            color: "Red".to_string(),
            required_by: vec!["Speed Farm".to_string()],
            source: affix_source.clone(),
        }],
        item_power_status: RuleCheck::failed(
            RuleCheckKind::ItemPowerLimit,
            Some(795.0),
            Some(800.0),
            RuleSourceNote::new(
                "D4Companion.Entities/SettingsD4.cs",
                "SettingsD4.ItemPowerLimit",
                "item power below configured limit",
            ),
        ),
        minimal_value_status: RuleCheck::passed(
            RuleCheckKind::MinimalAffixValue,
            Some(32.0),
            Some(30.0),
            affix_source.clone(),
        ),
        matched_power: Some(RulePowerMatch {
            power_id: "Aspect_Example".to_string(),
            power_kind: RulePowerKind::Aspect,
            source: RuleSourceNote::new(
                "D4Companion.Services/AffixManager.cs",
                "AffixManager.GetAspect",
                "aspect match",
            ),
        }),
        matched_powers: Vec::new(),
        matched_runes: vec!["Item_Rune_Condition_OnMove".to_string()],
        matched_sigil: Some("World_DGN_Kehj_AbandonedMineworks".to_string()),
        matched_sigils: Vec::new(),
        trade_marker: Some(RuleTradeMarker {
            trade_item_id: "trade-amulet-speed".to_string(),
            value_text: "5b".to_string(),
            source: trade_source,
        }),
        overlay_descriptors: vec![RuleOverlayDescriptor {
            target: RuleOverlayTarget::Affix {
                affix_id: "Affix_AttackSpeed".to_string(),
                row_id: Some(AffixRowId(3)),
            },
            marker_type: RuleOverlayMarkerType::Affix,
            shape: RuleOverlayShape::Circle,
            geometry: Rect {
                x: 100,
                y: 200,
                width: 20,
                height: 20,
            },
            color: "Green".into(),
            text: None,
            text_origin: None,
            z_order_group: RuleOverlayZOrderGroup::Marker,
            geometry_source: "item_affix_location".into(),
            source: overlay_source,
        }],
        explanation: RuleExplanation {
            active_preset_names: vec!["Speed Farm".to_string()],
            matched_build_names: Vec::new(),
            selected_build_name: Some("Pit Push".to_string()),
            item_type: Some("amulet".to_string()),
            rarity: Some("legendary".to_string()),
            branches: vec![RuleBranchExplanation {
                decision: RuleDecision::Wanted,
                message: "required affix matched".to_string(),
                source: affix_source,
            }],
        },
        affix_traces: Vec::new(),
    };

    assert_eq!(result.final_decision.as_str(), "wanted");
    assert_eq!(result.item_power_status.status, RuleCheckStatus::Failed);
    assert_eq!(result.minimal_value_status.status, RuleCheckStatus::Passed);
    assert_eq!(
        result.matched_affixes[0].value_status.kind,
        RuleCheckKind::MinimalAffixValue
    );
    assert_eq!(result.missing_affixes[0].required_by, vec!["Speed Farm"]);
    assert_eq!(
        result.overlay_descriptors[0].source.upstream_method,
        "OverlayHandler.DrawGraphicsAffixes"
    );
    assert_eq!(
        result
            .trade_marker
            .as_ref()
            .map(|marker| marker.value_text.as_str()),
        Some("5b")
    );
    assert_eq!(
        result.explanation.branches[0].source.branch,
        "preset affix match"
    );
}
