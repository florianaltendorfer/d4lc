//! One canonical identity space for
//! affixes and item types across catalog, OCR, imports and rules.
//!
//! The oracle is the pinned local catalog state (`assets/original/data/`,
//! enUS, 894 affix rows / 432 item type label rows): the composite-`IdName`
//! case (bare `AttackSpeed` alias versus its composite
//! canonical id) and the canonical `helm` type id that imports carry.

use d4lc_assets::catalog::{IdentityCatalog, ItemTypeCatalog};
use d4lc_assets::AssetRepository;
use d4lc_fuzzy::FuzzyIndexes;
use d4lc_parse::{
    convert_to_affix, convert_to_item_type, parse_item_tooltip_from_ocr_segments,
    parse_item_tooltip_from_ocr_segments_with_fuzzy, TooltipOcrLineInput, TooltipOcrSegmentInput,
    TooltipOcrSegmentKind,
};
use d4lc_rules::{
    evaluate_affix_preset_rows, AffixPreset, AffixRequirement, AffixRowVerdict, AffixRuleRow,
    ParsedAffixKind,
};

/// The OCR label path and the catalog lookup keys land on the same
/// canonical identity — the complete `IdName` of the pinned affix row, not
/// the bare `IdNameList` alias.
#[test]
fn ocr_label_and_catalog_keys_resolve_to_one_canonical_affix_id() {
    let repo = AssetRepository::new(workspace_root());
    let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
    let catalog = IdentityCatalog::affixes(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

    // OCR side: the tooltip affix line the parser feeds the fuzzy index
    // (`OcrHandler.ConvertToAffix` -> `OcrHandler.TextToAffix`).
    let conversion = convert_to_affix("+32.0% Attack Speed", &indexes);
    let ocr_id = conversion
        .affix_id
        .unwrap_or_else(|| panic!("OCR label must resolve an affix id"));
    assert!(conversion.fuzzy.accepted);

    // Provider keys of the same affix, resolved through the catalog.
    let by_alias = catalog
        .resolve_alias("AttackSpeed")
        .resolved()
        .unwrap_or_else(|| panic!("bare alias must resolve"));
    let by_sno = catalog
        .resolve_sno("1341731")
        .resolved()
        .unwrap_or_else(|| panic!("SNO must resolve"));

    assert_eq!(by_alias.id_name, by_sno.id_name);
    assert_eq!(ocr_id, by_alias.id_name);
    assert_ne!(
        ocr_id, "AttackSpeed",
        "the bare first alias is not the canonical identity"
    );
}

/// The canonical type id of the parsed tooltip reaches the rule
/// evaluation unchanged, and an item without a resolved type is never typed
/// from the display derivation of the tooltip line.
#[test]
fn canonical_item_type_id_reaches_evaluation_and_unknown_type_stays_unknown() {
    let repo = AssetRepository::new(workspace_root());
    let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
    let catalog = IdentityCatalog::affixes(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
    let item_types = ItemTypeCatalog::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

    // Parse side: the upper tooltip section resolves the canonical type id
    // (`ScreenProcessHandler.FindItemTypePower` -> `OcrHandler.ConvertToItemType`).
    let parsed = parse_item_tooltip_from_ocr_segments_with_fuzzy(
        &[upper(&["Ancestral Legendary Helm", "900 Item Power"])],
        &indexes,
    );
    let type_id = convert_to_item_type("Ancestral Legendary Helm\n900 Item Power", &indexes)
        .type_id
        .unwrap_or_else(|| panic!("the upper tooltip must resolve the canonical type id"));
    assert_eq!(type_id, "helm");
    assert!(item_types.is_canonical(&type_id));
    assert_eq!(
        parsed.item_type_id.as_deref(),
        Some("helm"),
        "the parse result carries the canonical type id of the upper tooltip section"
    );
    // The display derivation stays separate: it is not an evaluation id.
    assert_eq!(parsed.base_type.as_deref(), Some("Helm"));
    assert!(!item_types.is_canonical(parsed.base_type.as_deref().unwrap_or_default()));

    let affix_id = catalog
        .resolve_alias("AttackSpeed")
        .resolved()
        .unwrap_or_else(|| panic!("alias must resolve"))
        .id_name
        .clone();
    let preset = AffixPreset {
        name: "Canonical identity".to_string(),
        affixes: vec![AffixRequirement {
            affix_id: affix_id.clone(),
            label: Some("Attack Speed".to_string()),
            item_type: type_id.clone(),
            color: "Green".to_string(),
            is_any_type: false,
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        }],
    };
    let row = |item_type: &str| AffixRuleRow {
        affix_id: affix_id.clone(),
        item_type: item_type.to_string(),
        kind: ParsedAffixKind::Normal,
        row_id: Some(d4lc_core::AffixRowId(0)),
        value_text: Some("+32.0%".to_string()),
    };

    let matched = evaluate_affix_preset_rows(&[row(&type_id)], &preset);
    assert!(
        matches!(matched[0], AffixRowVerdict::Matched { .. }),
        "the canonical helm id must match the imported helm requirement"
    );

    // A typeless item stays untyped: neither the display derivation nor an
    // empty type is silently turned into `helm`.
    for unknown in ["", parsed.base_type.as_deref().unwrap_or_default()] {
        let verdicts = evaluate_affix_preset_rows(&[row(unknown)], &preset);
        assert!(
            matches!(verdicts[0], AffixRowVerdict::Unmatched { .. }),
            "type '{unknown}' must not match the helm requirement"
        );
    }

    // Without session indexes the canonical type stays unknown while the
    // display derivation of the same section is still parsed — the derivation
    // never becomes the evaluation id.
    let without_indexes = parse_item_tooltip_from_ocr_segments(&[upper(&[
        "Ancestral Legendary Helm",
        "900 Item Power",
    ])]);
    assert_eq!(without_indexes.base_type.as_deref(), Some("Helm"));
    assert!(
        without_indexes.item_type_id.is_none(),
        "an unresolved type stays unknown"
    );
}

fn upper(lines: &[&str]) -> TooltipOcrSegmentInput {
    TooltipOcrSegmentInput {
        index: 0,
        kind: TooltipOcrSegmentKind::UpperTooltip,
        raw_text: lines.join("\n"),
        converted_text: String::new(),
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

fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match manifest_dir.parent().and_then(|crates| crates.parent()) {
        Some(root) => root.to_path_buf(),
        None => panic!("could not resolve workspace root"),
    }
}
