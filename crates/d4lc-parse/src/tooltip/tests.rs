use super::*;
use crate::model::{ItemTooltipRarity, TooltipOcrLineInput};
use crate::test_support::load_enus_fuzzy_indexes;
use d4lc_core::Rect;

#[test]
fn parsed_affixes_carry_tuning_prisms_from_imported_data() {
    let indexes = load_enus_fuzzy_indexes();
    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[affix(1, "+32.0% Attack Speed", "Attack Speed")],
        &indexes,
    );

    let affix = result
        .affixes
        .first()
        .unwrap_or_else(|| panic!("missing affix"));
    // The enUS v3.2.0 data records TuningPrisms for the matched
    // "Attack Speed" affix entry.
    assert_eq!(affix.tuning_prisms, vec!["TuningStone_1".to_string()]);

    let json = serde_json::to_string(&result).unwrap_or_else(|error| panic!("{error}"));
    assert!(json.contains("\"tuning_prisms\":[\"TuningStone_1\"]"));
}

#[test]
fn parsed_affixes_without_tuning_prisms_emit_empty_json_array() {
    let indexes = load_enus_fuzzy_indexes();
    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[affix(1, "+715 Thorns", "Thorns")],
        &indexes,
    );

    let affix = result
        .affixes
        .first()
        .unwrap_or_else(|| panic!("missing affix"));
    let expected = indexes.affix_tuning_prisms(affix.affix_id.as_deref().unwrap_or_default());
    assert_eq!(affix.tuning_prisms, expected);

    let json = serde_json::to_string(&result).unwrap_or_else(|error| panic!("{error}"));
    assert!(json.contains("\"tuning_prisms\":["));
}

#[test]
fn parse_with_fuzzy_emits_canonical_affix_id_and_trace() {
    let indexes = load_enus_fuzzy_indexes();
    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[affix(1, "+715 Thorns", "Thorns")],
        &indexes,
    );

    let affix = result
        .affixes
        .first()
        .unwrap_or_else(|| panic!("missing affix"));
    assert_eq!(affix.name, "Thorns");
    assert_eq!(affix.text_clean.as_deref(), Some("Thorns"));
    assert_eq!(affix.text_value, Some(715.0));
    assert!(affix
        .affix_id
        .as_deref()
        .is_some_and(|id| id.contains("Thorns")));
    assert_eq!(
        affix.fuzzy.as_ref().map(|trace| trace.upstream_method),
        Some("OcrHandler.TextToAffix")
    );
    assert!(!affix.is_greater_affix);
    assert!(!affix.is_implicit);
    assert!(!affix.is_tempered);
}

#[test]
fn parser_classifies_greater_implicit_and_tempered_affix_markers() {
    let indexes = load_enus_fuzzy_indexes();
    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[
            affix_with_marker(
                1,
                "★ +32.0% Attack Speed",
                "Attack Speed",
                "dot-affixes_greater",
            ),
            affix_with_marker(2, "+715 Thorns", "Thorns", "dot-affixes_implicit"),
            affix_with_marker(3, "+10% Damage", "Damage", "dot-affixes_temper"),
        ],
        &indexes,
    );

    assert_eq!(result.affixes.len(), 3);
    assert!(result.affixes[0].is_greater_affix);
    assert!(result.affixes[1].is_implicit);
    assert!(result.affixes[2].is_tempered);
    assert!(result.affixes.iter().all(|affix| affix.fuzzy.is_some()));
}

#[test]
fn parse_with_fuzzy_emits_aspect_and_unique_ids() {
    let indexes = load_enus_fuzzy_indexes();
    let aspect_result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[aspect(
            6,
            "Critical Strikes with Core Skills increase your Attack Speed by for seconds.",
        )],
        &indexes,
    );
    let unique_result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[unique_aspect(
            7,
            "Ground Stomp's damage is increased by plus an additional amount based on the distance traveled since the last casted Ground Stomp's, up to.",
        )],
        &indexes,
    );

    let aspect = aspect_result
        .legendary_power
        .as_ref()
        .unwrap_or_else(|| panic!("missing aspect"));
    assert_eq!(aspect.power_id.as_deref(), Some("Legendary_Generic_110"));
    assert_eq!(aspect.conversion_method, Some("OcrHandler.ConvertToAspect"));
    assert!(!aspect.is_unique);

    let unique = unique_result
        .legendary_power
        .as_ref()
        .unwrap_or_else(|| panic!("missing unique"));
    assert_eq!(unique.power_id.as_deref(), Some("Boots_Unique_Barb_100_x2"));
    assert_eq!(unique.conversion_method, Some("OcrHandler.ConvertToUnique"));
    assert!(unique.is_unique);

    // The aspect/unique rows reuse the segment classification as their
    // kind and anchor to the legendary power's first source line.
    let aspect_rows = &aspect_result.non_affix_rows;
    assert_eq!(aspect_rows.len(), 1);
    assert_eq!(aspect_rows[0].kind, TooltipOcrSegmentKind::Aspect);
    assert_eq!(aspect_rows[0].id.as_deref(), Some("Legendary_Generic_110"));
    assert_eq!(aspect_rows[0].source.source_segment_index, 6);
    assert_eq!(aspect_rows[0].source.source_line_index, 0);
    assert_eq!(aspect_rows[0].source.global_line_index, 0);
    assert_eq!(aspect_rows[0].rect, None);

    let unique_rows = &unique_result.non_affix_rows;
    assert_eq!(unique_rows.len(), 1);
    assert_eq!(unique_rows[0].kind, TooltipOcrSegmentKind::UniqueAspect);
    assert_eq!(
        unique_rows[0].id.as_deref(),
        Some("Boots_Unique_Barb_100_x2")
    );
    assert_eq!(unique_rows[0].source.source_segment_index, 7);
    assert_eq!(unique_rows[0].source.global_line_index, 0);
    assert_eq!(unique_rows[0].rect, None);
}

#[test]
fn parse_with_fuzzy_emits_rune_and_sigil_results() {
    let indexes = load_enus_fuzzy_indexes();
    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[
            rune(1, &["Travel meters.", "Cooldown text", "ignored"]),
            sigil(2, "Abandoned Mineworks in Kehjistan\nTier 100"),
        ],
        &indexes,
    );

    assert_eq!(result.runes.len(), 1);
    assert_eq!(result.sigils.len(), 1);
    assert_eq!(
        result.runes[0].rune_id.as_deref(),
        Some("Item_Rune_Condition_OnMove")
    );
    assert_eq!(
        result.sigils[0].sigil_id.as_deref(),
        Some("World_DGN_Kehj_AbandonedMineworks")
    );
    assert!(result.unclassified_item_lines.is_empty());

    // The classified rows carry the same canonical ids anchored to the
    // first source line of their segment; the pure parser has no
    // window-relative geometry, so `rect` stays empty.
    assert_eq!(result.non_affix_rows.len(), 2);
    assert_eq!(result.non_affix_rows[0].kind, TooltipOcrSegmentKind::Rune);
    assert_eq!(
        result.non_affix_rows[0].id.as_deref(),
        Some("Item_Rune_Condition_OnMove")
    );
    assert_eq!(result.non_affix_rows[0].source.source_segment_index, 1);
    assert_eq!(result.non_affix_rows[0].source.source_line_index, 0);
    assert_eq!(result.non_affix_rows[0].source.global_line_index, 0);
    assert_eq!(result.non_affix_rows[0].rect, None);
    assert_eq!(result.non_affix_rows[1].kind, TooltipOcrSegmentKind::Sigil);
    assert_eq!(
        result.non_affix_rows[1].id.as_deref(),
        Some("World_DGN_Kehj_AbandonedMineworks")
    );
    assert_eq!(result.non_affix_rows[1].source.source_segment_index, 2);
    assert_eq!(result.non_affix_rows[1].source.global_line_index, 3);
    assert_eq!(result.non_affix_rows[1].rect, None);
}

#[test]
fn right_aligned_filter_drops_incidental_boxed_lines() {
    let result = parse_item_tooltip_from_ocr_segments(&[upper_with_rects(&[
        (
            "Ancestral Legendary Ring",
            Rect {
                x: 10,
                y: 0,
                width: 180,
                height: 20,
            },
        ),
        (
            "Requires Level 70",
            Rect {
                x: 72,
                y: 22,
                width: 140,
                height: 18,
            },
        ),
        (
            "900 Item Power",
            Rect {
                x: 11,
                y: 44,
                width: 120,
                height: 20,
            },
        ),
    ])]);

    assert_eq!(result.right_aligned_filtered.len(), 1);
    assert_eq!(
        result.right_aligned_filtered[0].raw_text,
        "Requires Level 70"
    );
    assert!(result.metadata.requires_level.is_none());
    assert_eq!(result.item_power, Some(900));
    assert_eq!(result.rarity, Some(ItemTooltipRarity::Legendary));
}

#[test]
fn a_wrapped_affix_stays_one_row_with_the_whole_area_text() {
    // One affix marker, one affix area (SPH.cs:1165-1186): the wrapped
    // second OCR line belongs to the same affix, it is not a second row.
    let first = Rect {
        x: 90,
        y: 300,
        width: 250,
        height: 20,
    };
    let second = Rect {
        x: 90,
        y: 322,
        width: 140,
        height: 20,
    };
    let indexes = load_enus_fuzzy_indexes();
    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[
            segment_with_rects(
                4,
                TooltipOcrSegmentKind::Affix,
                &[
                    ("+12.5% Critical Strike Chance Against", first),
                    ("Injured Enemies", second),
                ],
                "Critical Strike Chance Against\nInjured Enemies",
            ),
            affix_with_rect(
                5,
                "+8.0% Attack Speed",
                "Attack Speed",
                Rect {
                    x: 90,
                    y: 350,
                    width: 160,
                    height: 20,
                },
            ),
        ],
        &indexes,
    );

    assert_eq!(result.affixes.len(), 2);
    let wrapped = &result.affixes[0];
    assert_eq!(wrapped.row_id, AffixRowId(0));
    assert_eq!(
        wrapped.affix_id.as_deref(),
        Some("CritChance_To_LowLife;CritChance_To_LowLife_Jewelry")
    );
    assert_eq!(wrapped.value_text.as_deref(), Some("12.5%"));
    assert_eq!(wrapped.text_value, Some(12.5));
    assert_eq!(wrapped.source.source_line_index, 0);
    assert_eq!(
        wrapped.source.rect,
        Some(Rect {
            x: 90,
            y: 300,
            width: 250,
            height: 42,
        })
    );
    assert_eq!(result.affixes[1].row_id, AffixRowId(1));
    assert_eq!(result.affixes[1].source.source_segment_index, 5);
}

#[test]
fn right_aligned_filter_preserves_affix_and_aspect_segments() {
    let result = parse_item_tooltip_from_ocr_segments(&[
        affix_with_rect(
            1,
            "+32.0% Attack Speed",
            "Attack Speed",
            Rect {
                x: 90,
                y: 0,
                width: 160,
                height: 20,
            },
        ),
        aspect_with_rect(
            2,
            "Critical Strikes with Core Skills increase your Attack Speed by for seconds.",
            Rect {
                x: 95,
                y: 24,
                width: 360,
                height: 40,
            },
        ),
    ]);

    assert!(result.right_aligned_filtered.is_empty());
    assert_eq!(result.affixes.len(), 1);
    assert!(result.legendary_power.is_some());
}

#[test]
fn right_aligned_filter_is_reported_in_json_trace() {
    let result = parse_item_tooltip_from_ocr_segments(&[upper_with_rects(&[
        (
            "Ancestral Legendary Ring",
            Rect {
                x: 10,
                y: 0,
                width: 180,
                height: 20,
            },
        ),
        (
            "Requires Level 70",
            Rect {
                x: 72,
                y: 22,
                width: 140,
                height: 18,
            },
        ),
    ])]);
    let json = serde_json::to_string(&result).unwrap_or_else(|error| panic!("{error}"));

    assert!(json.contains("\"right_aligned_filtered\""));
    assert!(json.contains("Requires Level 70"));
    assert!(json.contains("\"rect\":{\"x\":72"));
}

#[test]
fn parses_weapon_item_core_from_full_segments() {
    let result = parse_item_tooltip_from_ocr_segments(&[upper(&[
        "HELLBENT",
        "COMMANDER",
        "DEMONBLADE",
        "Ancestral Legendary Two- }",
        "Handed Sword",
        "900 Item Power",
        "25 (% +25) Quality",
    ])]);

    assert_eq!(
        result.name_lines,
        vec![
            "HELLBENT".to_string(),
            "COMMANDER".to_string(),
            "DEMONBLADE".to_string(),
        ]
    );
    assert_eq!(
        result.display_name.as_deref(),
        Some("HELLBENT COMMANDER DEMONBLADE")
    );
    assert_eq!(result.rarity, Some(ItemTooltipRarity::Legendary));
    assert_eq!(result.is_ancestral, Some(true));
    assert_eq!(result.base_type.as_deref(), Some("Two-Handed Sword"));
    assert_eq!(result.item_power, Some(900));
    assert_eq!(result.quality, Some(25));
}

#[test]
fn parses_all_affixes_beyond_preview_limit() {
    let segments = vec![
        upper(&["HELLBENT", "COMMANDER", "DEMONBLADE", "900 Item Power"]),
        affix(1, "+293 Weapon Damage [207 - 345]", "Weapon Damage"),
        affix(2, "+428 Willpower +[300 - 360]", "Willpower"),
        affix(3, "+4,808 Maximum Life 3,662 - 4,400]", "Maximum Life"),
        affix(
            4,
            "x20% All Damage Multiplier [12 - 20%]",
            "All Damage Multiplier",
        ),
        affix(5, "★ +32.0% Attack Speed", "Attack Speed"),
        aspect(6, "Imprinted: You deal 124% increased Summon damage."),
    ];

    let result = parse_item_tooltip_from_ocr_segments(&segments);

    assert_eq!(result.source.segment_count, 7);
    assert_eq!(result.affixes.len(), 5);
    assert!(result
        .affixes
        .iter()
        .any(|affix| affix.name == "Attack Speed"));
}

/// An affix whose text names an upper-tooltip stat (`+1,667 Armor`,
/// `+… Damage Per Second`, `… Item Power`, `… Quality`, `Rare …`) stays an
/// affix row: only the unmarked base line is the primary armor stat.
#[test]
fn affixes_naming_upper_tooltip_stats_stay_affix_rows() {
    let indexes = load_enus_fuzzy_indexes();
    let segments = vec![
        upper(&[
            "MYRMIDON CLASH",
            "Rare Shield",
            "850 Item Power",
            "956 Armor (+41.8% Toughness)",
        ]),
        affix(1, "20.0% Block Chance [20.0]%", ""),
        affix(2, "+100% Main Hand Weapon Damage [100]%", ""),
        affix(3, "+148 Intelligence +[125 - 149] (+148)", ""),
        affix(4, "+475 Life On Hit [438 - 526] (+475)", ""),
        affix(5, "+1,667 Armor [1,561 - 1,962] (+1,667)", ""),
        affix(6, "+6.5% Critical Strike Chance [6.5 - 8.5]% (+6.5%)", ""),
    ];

    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(&segments, &indexes);

    assert_eq!(result.primary_stats.len(), 1);
    assert_eq!(
        result.primary_stats[0].raw_text,
        "956 Armor (+41.8% Toughness)"
    );
    assert_eq!(result.item_power, Some(850));
    assert_eq!(result.affixes.len(), 6);
    let armor = result
        .affixes
        .iter()
        .find(|affix| affix.source.source_segment_index == 5)
        .unwrap_or_else(|| panic!("the armor affix must be a row"));
    assert!(armor
        .affix_id
        .as_deref()
        .is_some_and(|id| id.split(';').any(|id| id == "S04_Armor")));

    // Marker-planned lines never feed the content-guessing classifiers.
    let result = parse_item_tooltip_from_ocr_segments(&[
        affix(1, "+12.5% Damage Per Second", ""),
        affix(2, "+20 Item Power", ""),
        affix(3, "Rare Quality Armor", ""),
    ]);
    assert!(result.primary_stats.is_empty());
    assert_eq!(result.item_power, None);
    assert_eq!(result.quality, None);
    assert_eq!(result.rarity, None);
    assert_eq!(result.affixes.len(), 3);

    // A marker-planned area without a readable name keeps its row.
    let result = parse_item_tooltip_from_ocr_segments(&[affix(1, "+1,667", "")]);
    assert_eq!(result.affixes.len(), 1);

    // Without marker provenance an affix-shaped line is still an affix.
    let result = parse_item_tooltip_from_ocr_segments(&[generic(&[
        "956 Armor",
        "+1,667 Armor [1,561 - 1,962]",
    ])]);
    assert_eq!(result.primary_stats.len(), 1);
    assert_eq!(result.affixes.len(), 1);
}

#[test]
fn keeps_attack_speed_greater_affix() {
    let result =
        parse_item_tooltip_from_ocr_segments(&[affix(5, "★ +32.0% Attack Speed", "Attack Speed")]);

    let attack_speed = result
        .affixes
        .iter()
        .find(|affix| affix.name == "Attack Speed")
        .unwrap_or_else(|| panic!("missing Attack Speed affix"));
    assert_eq!(attack_speed.value_text.as_deref(), Some("32.0%"));
    assert!(attack_speed.is_greater_affix);
}

#[test]
fn keeps_attack_speed_greater_affix_from_marker_name_when_ocr_drops_star() {
    let result = parse_item_tooltip_from_ocr_segments(&[affix_with_marker(
        5,
        "+32.0% Attack Speed",
        "Attack Speed",
        "dot-affixes_greater",
    )]);

    let attack_speed = result
        .affixes
        .iter()
        .find(|affix| affix.name == "Attack Speed")
        .unwrap_or_else(|| panic!("missing Attack Speed affix"));
    assert_eq!(attack_speed.value_text.as_deref(), Some("32.0%"));
    assert!(attack_speed.is_greater_affix);
    assert_eq!(
        attack_speed.source.marker_name.as_deref(),
        Some("dot-affixes_greater")
    );
}

#[test]
fn does_not_classify_ui_chrome_as_item_data() {
    let result = parse_item_tooltip_from_ocr_segments(&[generic(&[
        "Scroll Down",
        "Equip",
        "Link",
        "Drop",
        "Hide Comparison",
        "Unmark Item",
    ])]);

    assert_eq!(result.ui_chrome_ignored.len(), 6);
    assert!(result.affixes.is_empty());
    assert!(result.metadata.requires_level.is_none());
    assert!(result.legendary_power.is_none());
    assert!(result.display_name.is_none());
}

#[test]
fn separates_legendary_power_from_affixes() {
    let result = parse_item_tooltip_from_ocr_segments(&[aspect(
        6,
        "Imprinted: You deal 124% increased Summon damage.",
    )]);

    assert!(result.legendary_power.is_some());
    assert!(result.affixes.is_empty());
    assert_eq!(
        result
            .legendary_power
            .as_ref()
            .map(|power| power.is_imprinted),
        Some(true)
    );
}

#[test]
fn separates_sockets_from_affixes() {
    let result = parse_item_tooltip_from_ocr_segments(&[
        aspect(6, "Imprinted: You deal 124% increased Summon damage."),
        socket(7, "x24% Physical Damage Multiplier"),
        socket(8, "Empty Socket"),
    ]);

    assert_eq!(result.sockets.len(), 2);
    assert!(result.affixes.is_empty());
}

#[test]
fn parses_metadata_lines() {
    let result = parse_item_tooltip_from_ocr_segments(&[generic(&[
        "Requires Level 70",
        "Account Bound",
        "Sell Value: 85,069",
        "Durability: 30/100",
        "Tempers: 4/4",
    ])]);

    assert_eq!(
        result
            .metadata
            .requires_level
            .as_ref()
            .map(|value| value.value),
        Some(70)
    );
    assert_eq!(
        result
            .metadata
            .account_bound
            .as_ref()
            .map(|value| value.value),
        Some(true)
    );
    assert_eq!(
        result
            .metadata
            .sell_value
            .as_ref()
            .map(|value| value.value.as_str()),
        Some("85,069")
    );
    assert_eq!(
        result
            .metadata
            .durability
            .as_ref()
            .map(|value| value.value.as_str()),
        Some("30/100")
    );
    assert_eq!(
        result
            .metadata
            .tempers
            .as_ref()
            .map(|value| value.value.as_str()),
        Some("4/4")
    );
}

#[test]
fn comparison_lines_not_mixed_into_main_affixes() {
    let result = parse_item_tooltip_from_ocr_segments(&[generic(&[
        "Properties lost when equipped:",
        "+18 Maximum Resource",
        "+340 Resistance to All Elements",
        "Legendary Power",
    ])]);

    assert_eq!(result.comparison_delta.len(), 4);
    assert!(result.affixes.is_empty());
}

#[test]
fn item_parser_does_not_require_leptess_backend() {
    let result = parse_item_tooltip_from_ocr_segments(&[affix(
        1,
        "+293 Weapon Damage [207 - 345]",
        "Weapon Damage",
    )]);

    assert_eq!(result.affixes.len(), 1);
}

#[test]
fn item_result_json_contains_item_status_and_source_trace() {
    let result =
        parse_item_tooltip_from_ocr_segments(&[affix(5, "★ +32.0% Attack Speed", "Attack Speed")]);
    let json = serde_json::to_string(&result).unwrap_or_else(|error| panic!("{error}"));

    assert!(json.contains("\"status\":\"partial\""));
    assert!(json.contains("\"affixes\""));
    assert!(json.contains("\"Attack Speed\""));
    assert!(json.contains("\"is_greater_affix\":true"));
    assert!(json.contains("\"source_segment_index\":5"));
    assert!(json.contains("\"source_line_index\":0"));
}

fn upper(lines: &[&str]) -> TooltipOcrSegmentInput {
    segment(0, TooltipOcrSegmentKind::UpperTooltip, lines, "")
}

fn generic(lines: &[&str]) -> TooltipOcrSegmentInput {
    segment(0, TooltipOcrSegmentKind::Generic, lines, "")
}

fn affix(index: usize, raw_text: &str, converted_text: &str) -> TooltipOcrSegmentInput {
    segment(
        index,
        TooltipOcrSegmentKind::Affix,
        &[raw_text],
        converted_text,
    )
}

fn affix_with_marker(
    index: usize,
    raw_text: &str,
    converted_text: &str,
    marker_name: &str,
) -> TooltipOcrSegmentInput {
    let mut segment = segment(
        index,
        TooltipOcrSegmentKind::Affix,
        &[raw_text],
        converted_text,
    );
    segment.marker_name = Some(marker_name.to_string());
    segment
}

fn affix_with_rect(
    index: usize,
    raw_text: &str,
    converted_text: &str,
    rect: Rect,
) -> TooltipOcrSegmentInput {
    segment_with_rects(
        index,
        TooltipOcrSegmentKind::Affix,
        &[(raw_text, rect)],
        converted_text,
    )
}

fn aspect(index: usize, raw_text: &str) -> TooltipOcrSegmentInput {
    segment(index, TooltipOcrSegmentKind::Aspect, &[raw_text], raw_text)
}

fn aspect_with_rect(index: usize, raw_text: &str, rect: Rect) -> TooltipOcrSegmentInput {
    segment_with_rects(
        index,
        TooltipOcrSegmentKind::Aspect,
        &[(raw_text, rect)],
        raw_text,
    )
}

fn upper_with_rects(lines: &[(&str, Rect)]) -> TooltipOcrSegmentInput {
    segment_with_rects(0, TooltipOcrSegmentKind::UpperTooltip, lines, "")
}

fn unique_aspect(index: usize, raw_text: &str) -> TooltipOcrSegmentInput {
    segment(
        index,
        TooltipOcrSegmentKind::UniqueAspect,
        &[raw_text],
        raw_text,
    )
}

fn socket(index: usize, raw_text: &str) -> TooltipOcrSegmentInput {
    segment(index, TooltipOcrSegmentKind::Socket, &[raw_text], raw_text)
}

fn rune(index: usize, lines: &[&str]) -> TooltipOcrSegmentInput {
    segment(index, TooltipOcrSegmentKind::Rune, lines, &lines.join("\n"))
}

fn sigil(index: usize, raw_text: &str) -> TooltipOcrSegmentInput {
    segment(index, TooltipOcrSegmentKind::Sigil, &[raw_text], raw_text)
}

fn segment(
    index: usize,
    kind: TooltipOcrSegmentKind,
    lines: &[&str],
    converted_text: &str,
) -> TooltipOcrSegmentInput {
    TooltipOcrSegmentInput {
        index,
        kind,
        raw_text: lines.join("\n"),
        converted_text: converted_text.to_string(),
        lines: lines
            .iter()
            .map(|line| TooltipOcrLineInput {
                text: (*line).to_string(),
                rect: None,
            })
            .collect(),
        marker_name: None,
    }
}

fn segment_with_rects(
    index: usize,
    kind: TooltipOcrSegmentKind,
    lines: &[(&str, Rect)],
    converted_text: &str,
) -> TooltipOcrSegmentInput {
    TooltipOcrSegmentInput {
        index,
        kind,
        raw_text: lines
            .iter()
            .map(|(line, _)| *line)
            .collect::<Vec<_>>()
            .join("\n"),
        converted_text: converted_text.to_string(),
        lines: lines
            .iter()
            .map(|(line, rect)| TooltipOcrLineInput {
                text: (*line).to_string(),
                rect: Some(*rect),
            })
            .collect(),
        marker_name: None,
    }
}

/// The three numbers around an affix row stay distinguishable: the value
/// the tooltip shows, the displayed range and the upstream comparison
/// value. The cases pin the upstream `TextToAffixValue` quirks.
#[test]
fn affix_value_displayed_range_and_comparison_value_are_not_confused() {
    let cases: [(&str, Option<&str>, Option<&str>, f64); 5] = [
        // (row, displayed value, displayed range, upstream comparison value)
        (
            "+32.0% Attack Speed [15 - 32]",
            Some("32.0%"),
            Some("15 - 32"),
            32.0,
        ),
        (
            "+1,250 Maximum Life [1,000 - 2,000]",
            Some("1,250"),
            Some("1,000 - 2,000"),
            1250.0,
        ),
        // Comma as the decimal separator (second-to-last position).
        (
            "+12,5% Critical Strike Chance [5 - 12,5]",
            Some("12,5%"),
            Some("5 - 12,5"),
            12.5,
        ),
        // No leading value: the first number of the row is the range's
        // lower bound. Documented upstream quirk, not a value of its own.
        (
            "Lucky Hit: Up to a 15% Chance to Execute [10 - 15]",
            None,
            Some("10 - 15"),
            15.0,
        ),
        // No digit at all: upstream reports 0.0, the same value a
        // genuinely zero row reports.
        ("+ Maximum Life", None, None, 0.0),
    ];

    for (row, expected_value_text, expected_range, expected_text_value) in cases {
        let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
            &[segment(0, TooltipOcrSegmentKind::Affix, &[row], row)],
            &load_enus_fuzzy_indexes(),
        );

        let affix = result
            .affixes
            .first()
            .unwrap_or_else(|| panic!("{row} must parse as one affix row"));
        assert_eq!(affix.value_text.as_deref(), expected_value_text, "{row}");
        assert_eq!(affix.range_text.as_deref(), expected_range, "{row}");
        assert_eq!(affix.text_value, Some(expected_text_value), "{row}");
    }
}

/// A row whose text starts with a range carries no leading value token,
/// and the upstream comparison value follows the first number of the row:
/// here it is the range's lower bound. The three fields stay separate —
/// none of them is confused with another.
#[test]
fn affix_comparison_value_follows_the_first_number_even_when_it_is_the_range() {
    let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[segment(
            0,
            TooltipOcrSegmentKind::Affix,
            &["[5 - 12] +7.5% Attack Speed"],
            "[5 - 12] +7.5% Attack Speed",
        )],
        &load_enus_fuzzy_indexes(),
    );

    let affix = &result.affixes[0];
    assert_eq!(affix.range_text.as_deref(), Some("5 - 12"));
    assert_eq!(affix.value_text, None, "the row has no leading value token");
    assert_eq!(
        affix.text_value,
        Some(5.0),
        "upstream TextToAffixValue reads the first number of the whole row, which is the range's lower bound here"
    );
}

/// The display name and the affix rows keep their own provenance: the
/// name comes from the upper tooltip section, every affix row from its
/// affix segment. A name is never taken from an affix row and no affix
/// row is attributed to the name's segment.
#[test]
fn name_lines_and_affix_rows_keep_their_own_segment_provenance() {
    let result = parse_item_tooltip_from_ocr_segments(&[
        segment(
            0,
            TooltipOcrSegmentKind::UpperTooltip,
            &["Storm Amulet"],
            "Storm Amulet",
        ),
        segment(
            1,
            TooltipOcrSegmentKind::Affix,
            &["+32.0% Attack Speed"],
            "Attack Speed",
        ),
    ]);

    assert_eq!(result.display_name.as_deref(), Some("Storm Amulet"));
    assert_eq!(result.name_lines, vec!["Storm Amulet".to_string()]);
    assert_eq!(result.name_line_sources.len(), 1);
    assert_eq!(result.name_line_sources[0].source_segment_index, 0);
    assert_eq!(
        result.name_line_sources[0].segment_kind,
        TooltipOcrSegmentKind::UpperTooltip
    );
    assert_eq!(result.affixes.len(), 1);
    assert_eq!(result.affixes[0].source.source_segment_index, 1);
    assert_eq!(
        result.affixes[0].source.segment_kind,
        TooltipOcrSegmentKind::Affix
    );
}

/// Once an affix or item section line was read, a later upper-tooltip
/// line is no longer a name line: the display name never absorbs text
/// from another item section.
#[test]
fn a_name_line_is_never_collected_after_an_affix_row() {
    let result = parse_item_tooltip_from_ocr_segments(&[segment(
        0,
        TooltipOcrSegmentKind::UpperTooltip,
        &["+32.0% Attack Speed", "Storm Amulet"],
        "+32.0% Attack Speed\nStorm Amulet",
    )]);

    assert_eq!(result.affixes.len(), 1);
    assert!(
        result.name_lines.is_empty(),
        "the display name must stay empty instead of absorbing a later line: {:?}",
        result.name_lines
    );
    assert_eq!(result.display_name, None);
}
