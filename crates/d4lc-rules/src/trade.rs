use crate::model::{
    ParsedAffixForRules, ParsedAffixKind, ParsedItemForRules, RuleBranchExplanation, RuleDecision,
    RuleEvaluationResult, RuleSourceNote, RuleTradeMarker,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeAffixRequirement {
    pub affix_id: String,
    pub is_greater: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeItemRule {
    pub trade_item_id: String,
    pub item_type: String,
    pub value_text: String,
    pub affixes: Vec<TradeAffixRequirement>,
}

/// Find the first trade item whose item type matches the parsed item
/// and whose affix requirements are all satisfied (a `Greater`
/// requirement only matches a Greater affix).
///
/// Upstream: `TradeItemManager.FindTradeItem`
/// (TradeItemManager.cs:55-75). This is the pure match core of
/// [`apply_trade_item_lookup`], exposed for the GUI live projection
/// path where only the trade marker (not the full rule evaluation) is
/// re-derived per frame.
pub fn find_trade_marker(
    item: &ParsedItemForRules,
    trade_items: &[TradeItemRule],
) -> Option<RuleTradeMarker> {
    trade_items
        .iter()
        .find(|trade_item| trade_item_matches_item(trade_item, item))
        .map(|trade_item| RuleTradeMarker {
            trade_item_id: trade_item.trade_item_id.clone(),
            value_text: trade_item.value_text.clone(),
            source: RuleSourceNote::new(
                "D4Companion.Services/TradeItemManager.cs",
                "TradeItemManager.FindTradeItem",
                "trade item type and affix match",
            ),
        })
}

pub fn apply_trade_item_lookup(
    result: &mut RuleEvaluationResult,
    item: &ParsedItemForRules,
    trade_items: &[TradeItemRule],
) {
    result.trade_marker = None;

    if let Some(marker) = find_trade_marker(item, trade_items) {
        let message = format!("trade item {} matched", marker.trade_item_id);
        let source = marker.source.clone();
        result.trade_marker = Some(marker);
        result.explanation.branches.push(RuleBranchExplanation {
            decision: RuleDecision::Wanted,
            message,
            source,
        });
        return;
    }

    result.explanation.branches.push(RuleBranchExplanation {
        decision: RuleDecision::Neutral,
        message: "no trade item matched".to_string(),
        source: RuleSourceNote::new(
            "D4Companion.Services/TradeItemManager.cs",
            "TradeItemManager.FindTradeItem",
            "no trade item matched",
        ),
    });
}

fn trade_item_matches_item(trade_item: &TradeItemRule, item: &ParsedItemForRules) -> bool {
    trade_item.item_type == item.item_type
        && trade_item
            .affixes
            .iter()
            .all(|requirement| trade_requirement_matches_affixes(requirement, &item.affixes))
}

fn trade_requirement_matches_affixes(
    requirement: &TradeAffixRequirement,
    affixes: &[ParsedAffixForRules],
) -> bool {
    affixes.iter().any(|affix| {
        affix.affix_id == requirement.affix_id
            && (!requirement.is_greater || affix.kind == ParsedAffixKind::Greater)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
