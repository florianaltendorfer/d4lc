use super::*;
use crate::test_support::{load_enus_fuzzy_indexes, workspace_root};
use d4lc_assets::AssetRepository;

#[test]
fn convert_to_item_power_uses_last_leading_value_at_or_above_150() {
    let result = convert_to_item_power("Legendary Amulet\n149 Item Power\n900 Item Power");
    assert_eq!(result.upstream_method, "OcrHandler.ConvertToPower");
    assert_eq!(result.matched_text.as_deref(), Some("900 Item Power"));
    assert_eq!(result.text_clean.as_deref(), Some("900"));
    assert_eq!(result.item_power, Some(900));
}

#[test]
fn convert_to_item_power_ignores_non_leading_and_sub_150_values() {
    let result = convert_to_item_power("Item Power 925\n149 Item Power\n99");
    assert_eq!(result.matched_text, None);
    assert_eq!(result.text_clean, None);
    assert_eq!(result.item_power, None);
}

#[test]
fn convert_to_item_type_uses_pre_power_candidates_without_double_space_truncation() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_item_type(
        "Ancestral Legendary  Two-Handed Sword\n900 Item Power",
        &indexes,
    );

    assert_eq!(result.upstream_method, "OcrHandler.ConvertToItemType");
    assert_eq!(result.item_power, Some(900));
    assert_eq!(
        result.fuzzy.as_ref().map(|trace| trace.upstream_method),
        Some("OcrHandler.TextToItemType")
    );
    assert_eq!(
        result.type_label.as_deref(),
        Some("Ancestral Legendary Two-Handed Sword")
    );
    assert_eq!(result.type_id.as_deref(), Some("weapon"));
    assert!(result
        .candidate_texts
        .iter()
        .any(|candidate| candidate == "Ancestral Legendary  Two-Handed Sword"));
}

#[test]
fn convert_to_item_type_uses_no_power_candidates_for_sigils() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_item_type("Nightmare Sigil\nAbandoned Mineworks", &indexes);

    assert_eq!(result.item_power, None);
    assert_eq!(result.type_label.as_deref(), Some("Nightmare Sigil"));
    assert_eq!(result.type_id.as_deref(), Some("sigil"));
    assert!(result.fuzzy.as_ref().is_some_and(|trace| trace.accepted));
}

#[test]
fn convert_to_item_type_prefers_best_scoring_candidate_over_first_accepted_one() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_item_type(
        "Ancestral Legendary\nTwo-Handed Sword\n900 Item Power",
        &indexes,
    );

    assert_eq!(
        result.type_label.as_deref(),
        Some("Ancestral Legendary Two-Handed Sword")
    );
    assert_eq!(
        result.fuzzy.as_ref().map(|trace| trace.raw_text.as_str()),
        Some("Ancestral Legendary Two-Handed Sword")
    );
}

#[test]
fn convert_to_item_type_keeps_short_word_lines_after_heuristic_removal_upstream() {
    // Upstream v5.4.0 removed the "all words < 3 chars" artifact-line
    // filter: the short line is kept as the pre-power candidate and
    // rejected by the item-type similarity threshold instead.
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_item_type("ab cd\n900 Item Power", &indexes);

    assert_eq!(result.item_power, Some(900));
    assert_eq!(result.candidate_texts, vec!["ab cd".to_string()]);
    assert!(result.fuzzy.as_ref().is_some_and(|trace| !trace.accepted));
    assert_eq!(result.type_id, None);
    assert_eq!(result.type_label, None);
}

#[test]
fn convert_to_item_type_still_matches_type_after_short_line_removal_removal() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_item_type(
        "ab cd\nAncestral Legendary Two-Handed Sword\n900 Item Power",
        &indexes,
    );

    assert_eq!(result.type_id.as_deref(), Some("weapon"));
    assert_eq!(
        result.type_label.as_deref(),
        Some("Ancestral Legendary Two-Handed Sword")
    );
}

#[test]
fn convert_to_aspect_strips_digits_brackets_and_punctuation() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_aspect(
        "Critical Strikes with Core Skills increase your Attack Speed by [30]%, for seconds.",
        &indexes,
    );

    // Exact upstream filter output: no whitespace collapse, so stripped
    // tokens leave double spaces.
    assert_eq!(
        result.text_clean,
        "Critical Strikes with Core Skills increase your Attack Speed by  for seconds"
    );
    assert_eq!(result.power_id.as_deref(), Some("Legendary_Generic_110"));
}

#[test]
fn convert_to_unique_strips_digits_brackets_and_punctuation() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_unique("Ring of [2] Starless (1) Skies - 100%", &indexes);

    assert_eq!(result.text_clean, "Ring of  Starless  Skies");
}

#[test]
fn fixture_upper_tooltip_ocr_text_maps_to_expected_item_power_and_type() {
    let indexes = load_enus_fuzzy_indexes();
    let text = fixture_weapon_upper_tooltip_ocr_text();

    let power = convert_to_item_power(text);
    let item_type = convert_to_item_type(text, &indexes);

    assert_eq!(power.item_power, Some(900));
    assert_eq!(item_type.item_power, Some(900));
    assert_eq!(
        item_type.type_label.as_deref(),
        Some("Ancestral Legendary Two-Handed Sword")
    );
    assert_eq!(item_type.type_id.as_deref(), Some("weapon"));
}

#[test]
fn convert_to_affix_matches_upstream_cleanup_value_and_fuzzy_trace() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_affix("+32.0% Attack Speed [32.0]%", &indexes);

    assert_eq!(result.upstream_method, "OcrHandler.ConvertToAffix");
    assert_eq!(result.raw_text, "+32.0% Attack Speed [32.0]%");
    assert_eq!(result.text_clean, "Attack Speed");
    assert_eq!(result.text_value, 32.0);
    assert_eq!(result.affix_label.as_deref(), Some("Attack Speed"));
    assert!(result
        .affix_id
        .as_deref()
        .is_some_and(|id| id.contains("AttackSpeed")));
    assert_eq!(result.fuzzy.upstream_method, "OcrHandler.TextToAffix");
    assert_eq!(result.fuzzy.raw_text, "Attack Speed");
    assert!(result.fuzzy.accepted);
}

#[test]
fn convert_to_affix_multiline_uses_first_three_cleaned_candidates_without_threshold() {
    let indexes = load_enus_fuzzy_indexes();
    let result = convert_to_affix_multiline(
        "+26.0% Chance for Rapid Fire\nProjectiles to Cast Twice\n[26.0 - 35.0]% (Rogue Only)",
        &indexes,
    );

    assert_eq!(result.upstream_method, "OcrHandler.ConvertToAffixMultiline");
    assert_eq!(
        result.candidate_texts,
        vec![
            "Chance for Rapid Fire".to_string(),
            "Chance for Rapid Fire Projectiles to Cast Twice".to_string(),
            "Chance for Rapid Fire Projectiles to Cast Twice Rogue Only".to_string(),
        ]
    );
    assert_eq!(
        result.text_clean,
        "Chance for Rapid Fire Projectiles to Cast Twice"
    );
    assert_eq!(result.text_value, 26.0);
    assert_eq!(
        result.affix_label.as_deref(),
        Some("Chance for Rapid Fire Projectiles to Cast Twice")
    );
}

#[test]
fn text_to_affix_value_matches_upstream_regex_fixtures() {
    let cases = [
        ("+32.0% Vulnerable Damage [32.0]%", 32.0),
        ("+32,0% Vulnerable Damage [32,0]%", 32.0),
        ("+174 Dexterity +[152 - 180]", 174.0),
        ("+1,394 Maximum Life [1,276 - 1,746]", 1394.0),
        ("+1.394 Maximum Life [1.276 - 1.746]", 1394.0),
        ("+96.0% Critical Strike Damage [80.0 - 100.0]%", 96.0),
        ("+96,0% Critical Strike Damage [80,0 - 100,0]%", 96.0),
    ];

    for (input, expected) in cases {
        assert_eq!(text_to_affix_value(input), expected, "input: {input}");
    }
    assert_eq!(text_to_affix_value("No numeric affix"), 0.0);
}

#[test]
fn aspect_and_unique_indexes_expose_baseline_counts() {
    let indexes = load_enus_fuzzy_indexes();

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
}

#[test]
fn convert_to_aspect_matches_exact_and_fuzzy_text() {
    let indexes = load_enus_fuzzy_indexes();
    let exact = convert_to_aspect(
        "Critical Strikes with Core Skills increase your Attack Speed by for seconds.",
        &indexes,
    );
    let fuzzy = convert_to_aspect(
        "Critcal Strikes with Core Skills increase Attack Speed for seconds",
        &indexes,
    );

    assert_eq!(exact.upstream_method, "OcrHandler.ConvertToAspect");
    // Upstream v5.4.0 strips digits/brackets/punctuation from aspect text
    // (the trailing '.' is removed; no whitespace collapse).
    assert_eq!(
        exact.text_clean,
        "Critical Strikes with Core Skills increase your Attack Speed by for seconds"
    );
    assert_eq!(
        exact.raw_text,
        "Critical Strikes with Core Skills increase your Attack Speed by for seconds."
    );
    assert_eq!(exact.power_id.as_deref(), Some("Legendary_Generic_110"));
    assert_eq!(
        exact.power_label.as_deref(),
        Some("Critical Strikes with Core Skills increase your Attack Speed by for seconds.")
    );
    assert_eq!(exact.fuzzy.upstream_method, "OcrHandler.TextToAspect");
    assert_eq!(fuzzy.power_id.as_deref(), Some("Legendary_Generic_110"));
    assert!(fuzzy.fuzzy.score.is_some_and(|score| score < 100.0));
}

#[test]
fn convert_to_unique_matches_exact_and_fuzzy_text() {
    let indexes = load_enus_fuzzy_indexes();
    let exact = convert_to_unique(
        "Ground Stomp's damage is increased by plus an additional amount based on the distance traveled since the last casted Ground Stomp's, up to.",
        &indexes,
    );
    let fuzzy = convert_to_unique(
        "Ground Stomp damage increased based on distance traveled since last cast",
        &indexes,
    );

    assert_eq!(exact.upstream_method, "OcrHandler.ConvertToUnique");
    assert_eq!(exact.power_id.as_deref(), Some("Boots_Unique_Barb_100_x2"));
    assert_eq!(exact.power_label.as_deref(), Some("Ground Stomp's damage is increased by plus an additional amount based on the distance traveled since the last casted Ground Stomp's, up to."));
    assert_eq!(exact.fuzzy.upstream_method, "OcrHandler.TextToUnique");
    assert_eq!(fuzzy.power_id.as_deref(), Some("Boots_Unique_Barb_100_x2"));
    assert!(fuzzy.fuzzy.score.is_some_and(|score| score < 100.0));
}

#[test]
fn below_threshold_aspect_match_keeps_rejection_trace() {
    let repo = AssetRepository::new(workspace_root());
    let thresholds = d4lc_fuzzy::FuzzyMatchThresholds {
        aspect: 101.0,
        ..Default::default()
    };
    let indexes = FuzzyIndexes::load_with_thresholds(&repo, "enUS", thresholds)
        .unwrap_or_else(|error| panic!("{error}"));

    let result = convert_to_aspect(
        "Critical Strikes with Core Skills increase your Attack Speed by for seconds.",
        &indexes,
    );

    assert_eq!(result.power_id, None);
    assert!(!result.fuzzy.accepted);
    assert_eq!(
        result.fuzzy.rejection_reason.as_deref(),
        Some("best score below threshold")
    );
}

#[test]
fn rune_and_sigil_indexes_expose_baseline_counts() {
    let indexes = load_enus_fuzzy_indexes();

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
fn convert_to_rune_splits_condition_and_effect_candidates() {
    let indexes = load_enus_fuzzy_indexes();
    let condition = convert_to_rune("Travel meters.\nCooldown text\nignored", &indexes);
    let effect = convert_to_rune(
        "Summon a Spirit Wolf Companion to attack enemies for 8 seconds.",
        &indexes,
    );

    assert_eq!(condition.upstream_method, "OcrHandler.ConvertToRune");
    assert_eq!(condition.fuzzy.upstream_method, "OcrHandler.TextToRune");
    assert_eq!(effect.fuzzy.upstream_method, "OcrHandler.TextToRune");
    assert_eq!(
        condition.candidate_texts,
        vec![
            "Travel meters.".to_string(),
            "Travel meters. Cooldown text".to_string(),
            "Travel meters. Cooldown text ignored".to_string(),
        ]
    );
    assert_eq!(
        condition.rune_id.as_deref(),
        Some("Item_Rune_Condition_OnMove")
    );
    assert_eq!(condition.rune_type.as_deref(), Some("condition"));
    assert_eq!(
        effect.rune_id.as_deref(),
        Some("Item_Rune_Effect_SummonSpiritWolf")
    );
    assert_eq!(effect.rune_type.as_deref(), Some("effect"));
    assert!(condition.fuzzy.accepted);
    assert!(effect.fuzzy.accepted);
}

#[test]
fn rejected_rune_keeps_trace_when_below_minimal_match_threshold() {
    let repo = AssetRepository::new(workspace_root());
    let thresholds = d4lc_fuzzy::FuzzyMatchThresholds {
        rune: 101.0,
        ..Default::default()
    };
    let indexes = FuzzyIndexes::load_with_thresholds(&repo, "enUS", thresholds)
        .unwrap_or_else(|error| panic!("{error}"));

    let result = convert_to_rune("Travel meters.", &indexes);

    assert_eq!(result.rune_id, None);
    assert!(!result.fuzzy.accepted);
    assert_eq!(
        result.fuzzy.rejection_reason.as_deref(),
        Some("best score below threshold")
    );
}

#[test]
fn convert_to_sigil_matches_dungeon_and_affix_types() {
    let indexes = load_enus_fuzzy_indexes();
    let dungeon = convert_to_sigil("Abandoned Mineworks in Kehjistan\nTier 100", &indexes);
    let minor = convert_to_sigil("monster cold resist", &indexes);

    assert_eq!(dungeon.upstream_method, "OcrHandler.ConvertToSigil");
    assert_eq!(dungeon.fuzzy.upstream_method, "OcrHandler.TextToSigil");
    assert_eq!(minor.fuzzy.upstream_method, "OcrHandler.TextToSigil");
    assert_eq!(dungeon.text_clean, "Abandoned Mineworks in Kehjistan");
    assert_eq!(
        dungeon.sigil_id.as_deref(),
        Some("World_DGN_Kehj_AbandonedMineworks")
    );
    assert_eq!(dungeon.sigil_type.as_deref(), Some("Dungeon"));
    assert_eq!(
        minor.sigil_id.as_deref(),
        Some("DungeonAffix_Minor_Monster_LessCold")
    );
    assert_eq!(minor.sigil_type.as_deref(), Some("Minor"));
}

#[test]
fn rejected_sigil_trace_can_be_reported_by_threshold_override() {
    let repo = AssetRepository::new(workspace_root());
    let thresholds = d4lc_fuzzy::FuzzyMatchThresholds {
        sigil: 101.0,
        ..Default::default()
    };
    let indexes = FuzzyIndexes::load_with_thresholds(&repo, "enUS", thresholds)
        .unwrap_or_else(|error| panic!("{error}"));

    let result = convert_to_sigil("Abandoned Mineworks in Kehjistan", &indexes);

    assert_eq!(result.sigil_id, None);
    assert!(!result.fuzzy.accepted);
    assert_eq!(
        result.fuzzy.rejection_reason.as_deref(),
        Some("best score below threshold")
    );
}

fn fixture_weapon_upper_tooltip_ocr_text() -> &'static str {
    "HELLBENT\nCOMMANDER\nDEMONBLADE\nAncestral Legendary Two- }\nHanded Sword\n900 Item Power\n25 (% +25) Quality"
}
