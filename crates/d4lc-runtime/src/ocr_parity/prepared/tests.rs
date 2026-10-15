use super::*;
use crate::ocr_parity::test_support::{
    canned_registry, enus_profile, load_enus_fuzzy_indexes, rune_sigil_execution, segment_result,
    white_frame, workspace_root,
};
use d4lc_core::Rect;
use d4lc_ocr::{
    BoxedOcrEngine, OcrBackendKind, OcrEngine, OcrError, OcrImage, OcrPageSegMode,
    OcrStructuredOutput,
};

/// Fails every recognition while reading the structured output, the way a
/// backend with an unreadable TSV does.
struct StructuredOutputFailureEngine;

impl OcrEngine for StructuredOutputFailureEngine {
    fn recognize_lines(
        &mut self,
        _image: &OcrImage,
        _options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        Ok(OcrText {
            raw_text: "text-only fallback".to_string(),
            lines: Vec::new(),
            bounding_box_source: d4lc_core::OcrBoundingBoxSource::Unavailable,
        })
    }

    fn recognize_structured(
        &mut self,
        _image: &OcrImage,
        _options: &OcrOptions,
    ) -> Result<OcrStructuredOutput, OcrError> {
        Err(d4lc_ocr::structured_output_error(
            "invalid utf-8 sequence of 1 bytes from index 0",
        ))
    }
}

/// The tail segments are re-planned with the item type
/// resolved from the OCR'd upper tooltip section (upstream
/// FindItemTypesPower -> ConvertToItemType runs before FindItemAffixes),
/// so a live sigil tooltip ("Nightmare Sigil" header, regular affix
/// markers, no marker of its own) gets Sigil-kind segments instead of
/// plain Affix segments. Without indexes or a matching header the
/// Affix classification is kept (fail-soft).
#[test]
fn tail_segments_replanned_from_upper_tooltip_text_classify_sigil_areas() {
    let indexes = load_enus_fuzzy_indexes();
    let segment_input = |item_type: Option<String>| OcrSegmentPlanInput {
        config: TooltipOcrRegionConfig {
            tooltip_width: 500,
            tooltip_height: 900,
            tooltip_max_height: 420,
            affix_area_height_offset_top: 6,
            affix_area_height_offset_bottom: 4,
            affix_aspect_area_width_offset: 20,
            aspect_area_height_offset_top: 8,
            affix_marker_width: 10,
            aspect_marker_width: 12,
            type_area_offset_left: 5,
            type_area_offset_right: 5,
            aspect_area_max_height: 300,
        },
        affix_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 620,
                width: 10,
                height: 10,
            },
        }],
        aspect_marker: None,
        socket_markers: Vec::new(),
        splitter_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Splitter,
            name: "dot-splitter_top_1.png".to_string(),
            rect: Rect {
                x: 0,
                y: 500,
                width: 500,
                height: 2,
            },
        }],
        language: "enUS".to_string(),
        socket_detection_enabled: true,
        tempered_detection_enabled: true,
        aspect_detection_enabled: true,
        unique_detection_enabled: true,
        rune_detection_enabled: true,
        item_type,
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let initial = plan_ocr_segments(segment_input(None));
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0].kind, OcrSegmentKind::UpperTooltip);
    assert_eq!(initial[1].kind, OcrSegmentKind::Affix);

    let tail = replan_tail_segments_with_resolved_item_type(
        segment_input,
        "Nightmare Sigil",
        Some(&indexes),
    );
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].kind, OcrSegmentKind::Sigil);

    let tail = replan_tail_segments_with_resolved_item_type(segment_input, "Nightmare Sigil", None);
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].kind, OcrSegmentKind::Affix);
}

/// The desktop app's live runtime drove the OCR session from
/// the app config root, which carries the settings, the builds, and the
/// extracted user presets but never the vendored `assets/` tree. The
/// session then loaded no fuzzy candidate indexes, every parsed affix
/// stayed without an `affix_id`, and the app's affix rule rows (and with
/// them the overlay descriptors) dropped the whole hovered tooltip. The
/// session must resolve its localized data from the game-data root.
#[test]
fn session_game_data_root_seam_keeps_affix_ids_when_the_config_root_carries_no_assets() {
    let workspace = workspace_root();
    let data_less_root = std::env::temp_dir().join(format!(
        "d4lc-runtime-data-less-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&data_less_root)
        .unwrap_or_else(|error| panic!("could not create the temp root: {error}"));

    let profile = OcrProfile {
        backend: OcrBackendKind::Auto,
        language: "enUS".to_string(),
        tessdata_path: None,
    };
    let affix_execution = || {
        rune_sigil_execution(vec![
            segment_result(0, OcrSegmentKind::UpperTooltip, "Storm Amulet"),
            segment_result(1, OcrSegmentKind::Affix, "+740 Maximum Life"),
        ])
    };

    // A game-data root without `assets/original/data`: there is no
    // candidate index, so the affix conversion cannot resolve an id.
    let without_data = PreparedTooltipOcr::prepare_with_game_data_root(
        &workspace,
        &data_less_root,
        "1080p_SMF".to_string(),
        profile.clone(),
    )
    .unwrap_or_else(|error| panic!("session without a data root failed: {error}"));
    assert!(
        without_data.catalog_error().is_some(),
        "a missing catalog is named, never silently tolerated"
    );
    let result = runtime_parity_result(
        &affix_execution(),
        80,
        without_data.fuzzy_indexes.as_ref(),
        &TooltipPlacement::default(),
    );
    assert_eq!(result.item.affixes.len(), 1);
    assert!(
        result.item.affixes[0].affix_id.is_none(),
        "without the data root there is no candidate index: {:?}",
        result.item.affixes[0]
    );

    // The resolved game-data root supplies the index the id comes from.
    let with_data = PreparedTooltipOcr::prepare_with_game_data_root(
        &workspace,
        &workspace,
        "1080p_SMF".to_string(),
        profile,
    )
    .unwrap_or_else(|error| panic!("session with a data root failed: {error}"));
    assert_eq!(with_data.catalog_error(), None);
    let result = runtime_parity_result(
        &affix_execution(),
        80,
        with_data.fuzzy_indexes.as_ref(),
        &TooltipPlacement::default(),
    );
    assert_eq!(result.item.affixes.len(), 1);
    let affix_id = result.item.affixes[0].affix_id.clone().unwrap_or_else(|| {
        panic!(
            "the resolved data root must supply the affix id: {:?}",
            result.item.affixes[0]
        )
    });

    // Oracle: the id the repository's own enUS affix index assigns to the
    // line, loaded independently from the same vendored tree.
    let enus_indexes = load_enus_fuzzy_indexes();
    assert_eq!(
        d4lc_parse::convert_to_affix("+740 Maximum Life", &enus_indexes).affix_id,
        Some(affix_id)
    );

    let _ = std::fs::remove_dir_all(&data_less_root);
}

/// The prepared OCR session loads the fuzzy candidate indexes
/// once (upstream InitRuneData/InitSigilData session shape) from the
/// settings affix language, so per-frame jobs only pay conversion cost.
#[test]
fn prepared_ocr_session_loads_fuzzy_indexes_for_settings_language() {
    let prepared = PreparedTooltipOcr::prepare(
        &workspace_root(),
        "1080p_SMF".to_string(),
        OcrProfile {
            backend: OcrBackendKind::Auto,
            language: "enUS".to_string(),
            tessdata_path: None,
        },
    )
    .unwrap_or_else(|error| panic!("prepared OCR session failed: {error}"));

    let indexes = match prepared.fuzzy_indexes.as_ref() {
        Some(indexes) => indexes,
        None => {
            panic!("session must load fuzzy indexes for the default enUS settings language")
        }
    };
    assert!(
        indexes
            .category_index(d4lc_fuzzy::FuzzyCategory::Rune)
            .candidate_count()
            > 0
    );
    assert!(
        indexes
            .category_index(d4lc_fuzzy::FuzzyCategory::Sigil)
            .candidate_count()
            > 0
    );
}

/// A structured-output read failure fails the segment: the text-only
/// fallback never reaches the parse as a normal success.
#[test]
fn structured_output_failure_fails_the_segment_instead_of_degrading() {
    let prepared =
        PreparedTooltipOcr::prepare(&workspace_root(), "1080p_SMF".to_string(), enus_profile())
            .unwrap_or_else(|error| panic!("prepared OCR session failed: {error}"));
    let registry = OcrEngineRegistry::new(|_profile| {
        Ok(Box::new(StructuredOutputFailureEngine) as BoxedOcrEngine)
    });
    let frame = white_frame(64, 16);
    let plan = OcrSegmentPlan {
        index: 0,
        kind: OcrSegmentKind::Affix,
        source: "tooltip_filter".to_string(),
        rect: Rect {
            x: 0,
            y: 0,
            width: 32,
            height: 16,
        },
        source_marker_rect: None,
        boundary_source: "tooltip_bottom".to_string(),
        boundary_rect: None,
        upstream_rule: "test".to_string(),
        marker_name: None,
        marker_index: None,
        page_seg_mode: OcrPageSegMode::Default,
        language: "enUS".to_string(),
        tooltip_width: 0,
        clamp_reason: None,
    };

    match prepared.execute_segment(&frame, &registry, plan) {
        Err(message) => assert!(
            message.contains("failed to read OCR structured output (TSV)"),
            "{message}"
        ),
        Ok(result) => panic!(
            "a failed structured-output read must not produce a segment result: {:?}",
            result.text.raw_text
        ),
    }
}

/// A session without the catalog data fails every execution
/// instead of producing an item that only looks evaluated.
#[test]
fn execute_fails_visibly_when_the_catalog_data_is_missing() {
    let data_less_root = std::env::temp_dir().join(format!(
        "d4lc-runtime-catalog-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&data_less_root)
        .unwrap_or_else(|error| panic!("could not create the temp root: {error}"));

    let prepared = PreparedTooltipOcr::prepare_with_game_data_root(
        &workspace_root(),
        &data_less_root,
        "1080p_SMF".to_string(),
        enus_profile(),
    )
    .unwrap_or_else(|error| panic!("session without a data root failed: {error}"));
    assert!(
        prepared.catalog_error().is_some(),
        "a missing catalog is named, never silently tolerated"
    );

    let registry = canned_registry();
    match prepared.execute(&registry, &white_frame(16, 16)) {
        Err(message) => assert!(
            message.contains("catalog data unavailable"),
            "the failure must name the missing catalog: {message}"
        ),
        Ok(execution) => panic!(
            "an execution without catalog data must fail, got {} segments",
            execution.results.len()
        ),
    }

    let _ = std::fs::remove_dir_all(&data_less_root);
}

/// Upstream clears the affix locations of a rune tooltip when
/// `IsRuneDetectionEnabled` is off (ScreenProcessHandler.cs:418-421). The
/// item type is only known from the upper tooltip text, so the gate sits in
/// the tail re-plan.
#[test]
fn tail_segments_replanned_for_a_rune_tooltip_honor_the_rune_detection_setting() {
    let indexes = load_enus_fuzzy_indexes();
    let segment_input = |rune_detection_enabled: bool| {
        move |item_type: Option<String>| OcrSegmentPlanInput {
            config: TooltipOcrRegionConfig {
                tooltip_width: 500,
                tooltip_height: 900,
                tooltip_max_height: 420,
                affix_area_height_offset_top: 6,
                affix_area_height_offset_bottom: 4,
                affix_aspect_area_width_offset: 20,
                aspect_area_height_offset_top: 8,
                affix_marker_width: 10,
                aspect_marker_width: 12,
                type_area_offset_left: 5,
                type_area_offset_right: 5,
                aspect_area_max_height: 300,
            },
            affix_markers: vec![OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_rune.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 620,
                    width: 10,
                    height: 10,
                },
            }],
            aspect_marker: None,
            socket_markers: Vec::new(),
            splitter_markers: Vec::new(),
            language: "enUS".to_string(),
            socket_detection_enabled: true,
            tempered_detection_enabled: true,
            aspect_detection_enabled: true,
            unique_detection_enabled: true,
            rune_detection_enabled,
            item_type,
            is_unique_item: false,
            has_tooltip_top_splitter: false,
        }
    };

    let tail = replan_tail_segments_with_resolved_item_type(
        segment_input(true),
        "Legendary Rune of Ritual",
        Some(&indexes),
    );
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].kind, OcrSegmentKind::Rune);

    let tail = replan_tail_segments_with_resolved_item_type(
        segment_input(false),
        "Legendary Rune of Ritual",
        Some(&indexes),
    );
    assert!(tail.is_empty());
}

#[test]
fn first_pass_leaves_rune_areas_to_the_replan_when_rune_detection_is_off() {
    let input = |rune_detection_enabled: bool| OcrSegmentPlanInput {
        config: TooltipOcrRegionConfig {
            tooltip_width: 500,
            tooltip_height: 900,
            tooltip_max_height: 420,
            affix_area_height_offset_top: 6,
            affix_area_height_offset_bottom: 4,
            affix_aspect_area_width_offset: 20,
            aspect_area_height_offset_top: 8,
            affix_marker_width: 10,
            aspect_marker_width: 12,
            type_area_offset_left: 5,
            type_area_offset_right: 5,
            aspect_area_max_height: 300,
        },
        affix_markers: vec![
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_normal.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 620,
                    width: 10,
                    height: 10,
                },
            },
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_rune.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 700,
                    width: 10,
                    height: 10,
                },
            },
        ],
        aspect_marker: None,
        socket_markers: Vec::new(),
        splitter_markers: Vec::new(),
        language: "enUS".to_string(),
        socket_detection_enabled: true,
        tempered_detection_enabled: true,
        aspect_detection_enabled: true,
        unique_detection_enabled: true,
        rune_detection_enabled,
        item_type: None,
        is_unique_item: false,
        has_tooltip_top_splitter: false,
    };
    let kinds = |plans: Vec<OcrSegmentPlan>| plans.into_iter().map(|p| p.kind).collect::<Vec<_>>();

    assert!(kinds(first_pass_tail_plans(input(true))).contains(&OcrSegmentKind::Rune));
    let off = kinds(first_pass_tail_plans(input(false)));
    assert!(!off.contains(&OcrSegmentKind::Rune));
    assert!(off.contains(&OcrSegmentKind::Affix));

    // A non-rune item type brings the rune areas back through the re-plan.
    let replanned = kinds(plan_ocr_segments(OcrSegmentPlanInput {
        item_type: Some("amulet".to_string()),
        ..input(false)
    }));
    assert!(replanned.contains(&OcrSegmentKind::Rune));
}
