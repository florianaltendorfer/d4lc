use super::*;
use crate::{upstream_conversion_method, OcrPageSegMode};

fn region_config() -> TooltipOcrRegionConfig {
    TooltipOcrRegionConfig {
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
    }
}

#[test]
fn upper_tooltip_region_uses_first_splitter_and_max_height() {
    let region = upper_tooltip_region(
        region_config(),
        &[Rect {
            x: 0,
            y: 500,
            width: 500,
            height: 2,
        }],
    )
    .unwrap_or_else(|| panic!("missing upper tooltip region"));

    assert_eq!(region.kind, OcrSegmentKind::UpperTooltip);
    assert_eq!(region.rect.y, 80);
    assert_eq!(region.rect.height, 420);
}

#[test]
fn upper_tooltip_region_applies_type_area_offset_left() {
    let mut cfg = region_config();
    cfg.tooltip_width = 500;
    let region = upper_tooltip_region(
        cfg,
        &[Rect {
            x: 0,
            y: 500,
            width: 500,
            height: 2,
        }],
    )
    .unwrap_or_else(|| panic!("missing upper tooltip region"));

    assert_eq!(
        region.rect.x, 5,
        "rect.x should equal type_area_offset_left"
    );
    assert_eq!(
        region.rect.width, 490,
        "rect.width should be tooltip_width minus type_area_offset_left and type_area_offset_right"
    );
}

#[test]
fn upper_tooltip_region_clamps_oversized_type_area_offset() {
    let mut cfg = region_config();
    cfg.tooltip_width = 100;
    cfg.type_area_offset_left = 200;
    let region = upper_tooltip_region(
        cfg,
        &[Rect {
            x: 0,
            y: 500,
            width: 500,
            height: 2,
        }],
    );

    assert!(
        region.is_none()
            || (region
                .as_ref()
                .is_some_and(|r| r.rect.x >= 0 && r.rect.width > 0)),
        "oversized offset must not panic: either region is None or rect.x >= 0 and width > 0"
    );
}

#[test]
fn upper_tooltip_region_zero_offset_behaves_as_before() {
    let mut cfg = region_config();
    cfg.type_area_offset_left = 0;
    cfg.tooltip_width = 500;
    let region = upper_tooltip_region(
        cfg,
        &[Rect {
            x: 0,
            y: 500,
            width: 500,
            height: 2,
        }],
    )
    .unwrap_or_else(|| panic!("missing upper tooltip region"));

    assert_eq!(region.rect.x, 0, "rect.x should be 0 when offset is 0");
    assert_eq!(
        region.rect.width, 495,
        "rect.width should be tooltip_width minus type_area_offset_right when offset_left is 0"
    );
}

#[test]
fn marker_defined_regions_follow_upstream_affix_area_offsets() {
    let affix_markers = vec![
        OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 120,
                width: 10,
                height: 10,
            },
        },
        OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_greater.png".to_string(),
            rect: Rect {
                x: 5,
                y: 200,
                width: 10,
                height: 10,
            },
        },
    ];
    let splitter = Rect {
        x: 0,
        y: 260,
        width: 500,
        height: 2,
    };

    let regions = marker_defined_regions(region_config(), &affix_markers, None, &[], &[splitter]);

    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].rect.x, 17);
    assert_eq!(regions[0].rect.y, 114);
    assert_eq!(regions[0].rect.height, 82);
    assert_eq!(regions[1].rect.height, 62);
}

#[test]
fn aspect_region_follows_upstream_offsets() {
    let aspect_marker = OcrMarker {
        kind: OcrSegmentKind::Aspect,
        name: "dot-aspects_legendary.png".to_string(),
        rect: Rect {
            x: 5,
            y: 240,
            width: 12,
            height: 12,
        },
    };
    let splitter = Rect {
        x: 0,
        y: 330,
        width: 500,
        height: 2,
    };

    let region = aspect_region(
        region_config(),
        &[],
        &aspect_marker,
        &[],
        &[splitter],
        false,
    )
    .unwrap_or_else(|| panic!("missing aspect region"));

    assert_eq!(region.kind, OcrSegmentKind::Aspect);
    assert_eq!(region.rect.x, 19);
    assert_eq!(region.rect.y, 232);
    assert_eq!(region.rect.width, 461);
    assert_eq!(region.rect.height, 94);
    assert_eq!(region.boundary_source, "splitter_marker");
    assert_eq!(region.boundary_rect, Some(splitter));
}

#[test]
fn last_aspect_region_uses_tooltip_bottom_when_no_later_split_point_exists() {
    let aspect_marker = OcrMarker {
        kind: OcrSegmentKind::Aspect,
        name: "dot-aspects_legendary.png".to_string(),
        rect: Rect {
            x: 5,
            y: 820,
            width: 12,
            height: 12,
        },
    };

    let region = aspect_region(region_config(), &[], &aspect_marker, &[], &[], false)
        .unwrap_or_else(|| panic!("missing aspect region"));

    assert_eq!(region.boundary_source, "tooltip_bottom");
    assert_eq!(region.boundary_rect, None);
    assert_eq!(region.rect.y, 812);
    assert_eq!(region.rect.height, 88);
}

#[test]
fn upper_tooltip_conversion_method_uses_upstream_sparse_text_plan() {
    let input = OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: Vec::new(),
        aspect_marker: None,
        socket_markers: Vec::new(),
        splitter_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Splitter,
            name: "dot-splitter_top.png".to_string(),
            rect: Rect {
                x: 0,
                y: 300,
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
        item_type: None,
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let plans = plan_ocr_segments(input);

    assert_eq!(plans[0].kind, OcrSegmentKind::UpperTooltip);
    assert_eq!(plans[0].source, "tooltip_filter");
    assert_eq!(plans[0].page_seg_mode, OcrPageSegMode::SparseText);
    assert_eq!(
        upstream_conversion_method(plans[0].kind),
        "OcrHandler.ConvertToTextUpperTooltipSection"
    );
}

#[test]
fn affix_split_points_use_last_affix_y_and_small_socket_offset_without_aspect() {
    let affix_markers = vec![
        OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 120,
                width: 10,
                height: 10,
            },
        },
        OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 200,
                width: 10,
                height: 10,
            },
        },
    ];

    let split_points = split_points_for_affixes(
        &affix_markers,
        None,
        &[Rect {
            x: 4,
            y: 12,
            width: 16,
            height: 20,
        }],
        &[],
    );

    let socket = split_points
        .iter()
        .find(|point| point.source == SplitPointSource::Socket)
        .unwrap_or_else(|| panic!("missing socket split point"));
    assert_eq!(socket.rect.y, 216);
    assert_eq!(socket.rect.height, 18);
}

#[test]
fn segment_plan_uses_upstream_order_and_psm() {
    let input = OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: vec![
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_greater.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 200,
                    width: 10,
                    height: 10,
                },
            },
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_normal.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 120,
                    width: 10,
                    height: 10,
                },
            },
        ],
        aspect_marker: None,
        socket_markers: Vec::new(),
        splitter_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Splitter,
            name: "dot-splitter_top.png".to_string(),
            rect: Rect {
                x: 0,
                y: 260,
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
        item_type: None,
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let plans = plan_ocr_segments(input);

    assert_eq!(plans.len(), 3);
    assert_eq!(plans[0].kind, OcrSegmentKind::UpperTooltip);
    assert_eq!(plans[0].page_seg_mode, OcrPageSegMode::SparseText);
    assert_eq!(plans[1].rect.y, 114);
    assert_eq!(plans[2].rect.y, 194);
    assert_eq!(plans[1].page_seg_mode, OcrPageSegMode::Default);
    assert_eq!(plans[1].boundary_source, "affix_marker");
    assert_eq!(plans[2].boundary_source, "splitter_marker");
}

#[test]
fn segment_plan_does_not_read_full_tooltip_when_splitter_exists() {
    let input = OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: Vec::new(),
        aspect_marker: None,
        socket_markers: Vec::new(),
        splitter_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Splitter,
            name: "dot-splitter_top.png".to_string(),
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
        item_type: None,
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let plans = plan_ocr_segments(input);

    assert!(!plans.is_empty());
    assert!(plans.iter().all(|plan| !plan.is_full_tooltip(500, 900)));
}

#[test]
fn upper_tooltip_region_applies_upstream_type_area_offset_right_width_guard() {
    // Upstream FindItemTypesPower: width = (Width - left - right) <= 0
    // ? Width - left : Width - left - right.
    let mut cfg = region_config();
    cfg.tooltip_width = 100;
    cfg.type_area_offset_left = 60;
    cfg.type_area_offset_right = 50;

    let region =
        upper_tooltip_region(cfg, &[]).unwrap_or_else(|| panic!("missing upper tooltip region"));

    assert_eq!(region.rect.x, 60);
    assert_eq!(
        region.rect.width, 40,
        "width guard falls back to tooltip_width - offset_left"
    );
}

#[test]
fn aspect_region_clamps_height_to_aspect_area_max_height() {
    let aspect_marker = OcrMarker {
        kind: OcrSegmentKind::Aspect,
        name: "dot-aspects_legendary.png".to_string(),
        rect: Rect {
            x: 5,
            y: 240,
            width: 12,
            height: 12,
        },
    };

    let mut cfg = region_config();
    cfg.aspect_area_max_height = 20;
    let region = aspect_region(cfg, &[], &aspect_marker, &[], &[], false)
        .unwrap_or_else(|| panic!("missing aspect region"));
    assert!(
        region.rect.height <= 20,
        "aspect region height must be clamped to AspectAreaMaxHeight"
    );

    cfg.aspect_area_max_height = 10_000;
    let unclamped = aspect_region(cfg, &[], &aspect_marker, &[], &[], false)
        .unwrap_or_else(|| panic!("missing aspect region"));
    assert_eq!(unclamped.rect.height, 668);
}

#[test]
fn plan_drops_affix_areas_for_charm_and_horadric_seal_item_types() {
    let base_input = |item_type: Option<&str>| OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 120,
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
        rune_detection_enabled: true,
        item_type: item_type.map(ToOwned::to_owned),
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let plans = plan_ocr_segments(base_input(Some("charm")));
    assert!(
        plans.iter().all(|plan| plan.kind != OcrSegmentKind::Affix),
        "charm item type must clear affix areas (upstream Not implemented)"
    );

    let plans = plan_ocr_segments(base_input(Some("horadricseal")));
    assert!(
        plans.iter().all(|plan| plan.kind != OcrSegmentKind::Affix),
        "horadricseal item type must clear affix areas (upstream Not implemented)"
    );

    let plans = plan_ocr_segments(base_input(Some("amulet")));
    assert!(
        plans.iter().any(|plan| plan.kind == OcrSegmentKind::Affix),
        "other item types keep affix areas"
    );
}

#[test]
fn plan_drops_affix_areas_for_seasonal_sigil_item_types() {
    // Upstream ScreenProcessHandler.FindItemAffix "Skip ignored item
    // types - Seasonal": witchersigil, dungeonescalation, and
    // bloodiedlair tooltips clear ItemAffixLocations, so their affix
    // areas are never classified as Sigil or Affix segments.
    let base_input = |item_type: Option<&str>| OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: vec![
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_normal.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 120,
                    width: 10,
                    height: 10,
                },
            },
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_normal.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 200,
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
        rune_detection_enabled: true,
        item_type: item_type.map(ToOwned::to_owned),
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    for item_type in ["witchersigil", "dungeonescalation", "bloodiedlair"] {
        let plans = plan_ocr_segments(base_input(Some(item_type)));
        assert!(
            plans.iter().all(
                |plan| plan.kind != OcrSegmentKind::Sigil && plan.kind != OcrSegmentKind::Affix
            ),
            "{item_type} item type must clear affix areas (upstream seasonal skip)"
        );
    }

    let plans = plan_ocr_segments(base_input(Some("sigil")));
    assert_eq!(
        plans
            .iter()
            .filter(|plan| plan.kind == OcrSegmentKind::Sigil)
            .count(),
        2,
        "non-seasonal sigil item type keeps its Sigil segments"
    );
}

#[test]
fn plan_classifies_affix_areas_as_sigil_segments_for_sigil_item_type() {
    // Upstream ScreenProcessHandler.FindItemAffix: itemType == Sigil
    // converts every affix area with OcrHandler.ConvertToSigil; sigils
    // have no dedicated marker template, so the item type decides.
    let base_input = |item_type: Option<&str>| OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: vec![
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_normal.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 120,
                    width: 10,
                    height: 10,
                },
            },
            OcrMarker {
                kind: OcrSegmentKind::Affix,
                name: "dot-affixes_normal.png".to_string(),
                rect: Rect {
                    x: 5,
                    y: 200,
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
        rune_detection_enabled: true,
        item_type: item_type.map(ToOwned::to_owned),
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let plans = plan_ocr_segments(base_input(Some("sigil")));
    assert_eq!(
        plans
            .iter()
            .filter(|plan| plan.kind == OcrSegmentKind::Sigil)
            .count(),
        2,
        "sigil item type must classify affix areas as Sigil segments"
    );
    assert!(
        plans.iter().all(|plan| plan.kind != OcrSegmentKind::Affix),
        "sigil item type must not emit plain Affix segments"
    );
    assert_eq!(
        upstream_conversion_method(OcrSegmentKind::Sigil),
        "OcrHandler.ConvertToSigil"
    );

    let plans = plan_ocr_segments(base_input(Some("amulet")));
    assert_eq!(
        plans
            .iter()
            .filter(|plan| plan.kind == OcrSegmentKind::Affix)
            .count(),
        2,
        "non-sigil item types keep Affix segments"
    );
    let plans = plan_ocr_segments(base_input(None));
    assert_eq!(
        plans
            .iter()
            .filter(|plan| plan.kind == OcrSegmentKind::Affix)
            .count(),
        2,
        "unresolved item type keeps Affix segments"
    );
}

#[test]
fn segment_plan_does_not_read_full_tooltip_when_marker_data_exists() {
    let input = OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 120,
                width: 10,
                height: 10,
            },
        }],
        aspect_marker: Some(OcrMarker {
            kind: OcrSegmentKind::Aspect,
            name: "dot-aspects_legendary.png".to_string(),
            rect: Rect {
                x: 5,
                y: 240,
                width: 12,
                height: 12,
            },
        }),
        socket_markers: Vec::new(),
        splitter_markers: Vec::new(),
        language: "enUS".to_string(),
        socket_detection_enabled: true,
        tempered_detection_enabled: true,
        aspect_detection_enabled: true,
        unique_detection_enabled: true,
        rune_detection_enabled: true,
        item_type: None,
        is_unique_item: false,
        has_tooltip_top_splitter: false,
    };

    let plans = plan_ocr_segments(input);

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].kind, OcrSegmentKind::Affix);
    assert_eq!(plans[1].kind, OcrSegmentKind::Aspect);
    assert!(plans.iter().all(|plan| !plan.is_full_tooltip(500, 900)));
}

#[test]
fn legendary_boots_do_not_reintroduce_implicit_segment_categories() {
    let input = OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 120,
                width: 10,
                height: 10,
            },
        }],
        aspect_marker: None,
        socket_markers: Vec::new(),
        splitter_markers: vec![
            OcrMarker {
                kind: OcrSegmentKind::Splitter,
                name: "dot-splitter_top.png".to_string(),
                rect: Rect {
                    x: 0,
                    y: 90,
                    width: 500,
                    height: 2,
                },
            },
            OcrMarker {
                kind: OcrSegmentKind::Splitter,
                name: "dot-splitter_1.png".to_string(),
                rect: Rect {
                    x: 0,
                    y: 180,
                    width: 500,
                    height: 2,
                },
            },
        ],
        language: "enUS".to_string(),
        socket_detection_enabled: true,
        tempered_detection_enabled: true,
        aspect_detection_enabled: true,
        unique_detection_enabled: true,
        rune_detection_enabled: true,
        item_type: Some("Boots".to_string()),
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let plans = plan_ocr_segments(input);
    let kinds = plans.into_iter().map(|plan| plan.kind).collect::<Vec<_>>();

    assert_eq!(
        kinds,
        vec![OcrSegmentKind::UpperTooltip, OcrSegmentKind::Affix]
    );
}

#[test]
fn segment_plan_respects_type_area_offset() {
    let mut cfg = region_config();
    cfg.type_area_offset_left = 7;
    cfg.tooltip_width = 400;
    let input = OcrSegmentPlanInput {
        config: cfg,
        affix_markers: vec![],
        aspect_marker: None,
        socket_markers: vec![],
        splitter_markers: vec![OcrMarker {
            kind: OcrSegmentKind::Splitter,
            name: "dot-splitter_top.png".to_string(),
            rect: Rect {
                x: 0,
                y: 260,
                width: 400,
                height: 2,
            },
        }],
        language: "enUS".to_string(),
        socket_detection_enabled: true,
        tempered_detection_enabled: true,
        aspect_detection_enabled: true,
        unique_detection_enabled: true,
        rune_detection_enabled: true,
        item_type: None,
        is_unique_item: false,
        has_tooltip_top_splitter: true,
    };

    let plans = plan_ocr_segments(input);
    let upper = plans
        .iter()
        .find(|p| p.kind == OcrSegmentKind::UpperTooltip)
        .unwrap_or_else(|| panic!("missing upper tooltip plan"));

    assert_eq!(
        upper.rect.x, 7,
        "UpperTooltip rect.x should equal type_area_offset_left"
    );
    assert_eq!(
        upper.rect.width, 388,
        "UpperTooltip rect.width should be tooltip_width minus type_area_offset_left and type_area_offset_right"
    );
}

fn detection_gate_input(item_type: Option<&str>, is_unique_item: bool) -> OcrSegmentPlanInput {
    let marker = |kind: OcrSegmentKind, name: &str, y: i32| OcrMarker {
        kind,
        name: name.to_string(),
        rect: Rect {
            x: 5,
            y,
            width: 10,
            height: 10,
        },
    };
    OcrSegmentPlanInput {
        config: region_config(),
        affix_markers: vec![
            marker(OcrSegmentKind::Affix, "dot-affixes_normal.png", 120),
            marker(OcrSegmentKind::Rune, "dot-affixes_rune.png", 200),
        ],
        aspect_marker: Some(marker(
            OcrSegmentKind::Aspect,
            "dot-aspects_legendary.png",
            300,
        )),
        socket_markers: Vec::new(),
        splitter_markers: Vec::new(),
        language: "enUS".to_string(),
        socket_detection_enabled: true,
        tempered_detection_enabled: true,
        aspect_detection_enabled: true,
        unique_detection_enabled: true,
        rune_detection_enabled: true,
        item_type: item_type.map(ToOwned::to_owned),
        is_unique_item,
        has_tooltip_top_splitter: false,
    }
}

fn has_kind(plans: &[OcrSegmentPlan], kind: OcrSegmentKind) -> bool {
    plans.iter().any(|plan| plan.kind == kind)
}

#[test]
fn aspect_detection_off_skips_aspect_ocr_for_non_unique_items() {
    // Upstream ScreenProcessHandler.cs:471-484.
    let plans = plan_ocr_segments(detection_gate_input(Some("amulet"), false));
    assert!(has_kind(&plans, OcrSegmentKind::Aspect));

    let mut input = detection_gate_input(Some("amulet"), false);
    input.aspect_detection_enabled = false;
    let plans = plan_ocr_segments(input);
    assert!(!has_kind(&plans, OcrSegmentKind::Aspect));
    assert!(has_kind(&plans, OcrSegmentKind::Affix));

    // The unique toggle does not gate a non-unique aspect.
    let mut input = detection_gate_input(Some("amulet"), false);
    input.unique_detection_enabled = false;
    assert!(has_kind(&plan_ocr_segments(input), OcrSegmentKind::Aspect));
}

#[test]
fn unique_detection_off_skips_unique_ocr_for_unique_items() {
    // Upstream ScreenProcessHandler.cs:476-484.
    let plans = plan_ocr_segments(detection_gate_input(Some("amulet"), true));
    assert!(has_kind(&plans, OcrSegmentKind::UniqueAspect));

    let mut input = detection_gate_input(Some("amulet"), true);
    input.unique_detection_enabled = false;
    let plans = plan_ocr_segments(input);
    assert!(!has_kind(&plans, OcrSegmentKind::UniqueAspect));
    assert!(!has_kind(&plans, OcrSegmentKind::Aspect));

    // The aspect toggle does not gate a unique aspect.
    let mut input = detection_gate_input(Some("amulet"), true);
    input.aspect_detection_enabled = false;
    assert!(has_kind(
        &plan_ocr_segments(input),
        OcrSegmentKind::UniqueAspect
    ));
}

#[test]
fn aspect_detection_off_still_bounds_the_affix_area_by_the_aspect_marker() {
    // Upstream finds the aspect location before the gated Parallel.Invoke
    // (ScreenProcessHandler.cs:471), so the affix area keeps its boundary.
    let affix_rects = |input: OcrSegmentPlanInput| {
        plan_ocr_segments(input)
            .into_iter()
            .filter(|plan| plan.kind == OcrSegmentKind::Affix)
            .map(|plan| plan.rect)
            .collect::<Vec<_>>()
    };
    let enabled = affix_rects(detection_gate_input(Some("amulet"), false));
    let mut input = detection_gate_input(Some("amulet"), false);
    input.aspect_detection_enabled = false;
    assert_eq!(affix_rects(input), enabled);
}

#[test]
fn rune_detection_off_skips_affix_ocr_for_rune_items_only() {
    // Upstream ScreenProcessHandler.cs:418-421.
    let plans = plan_ocr_segments(detection_gate_input(Some("rune"), false));
    assert!(has_kind(&plans, OcrSegmentKind::Rune));

    let mut input = detection_gate_input(Some("rune"), false);
    input.rune_detection_enabled = false;
    let plans = plan_ocr_segments(input);
    assert!(!has_kind(&plans, OcrSegmentKind::Rune));
    assert!(!has_kind(&plans, OcrSegmentKind::Affix));

    // Other item types keep their rune-socket affix areas.
    let mut input = detection_gate_input(Some("amulet"), false);
    input.rune_detection_enabled = false;
    assert!(has_kind(&plan_ocr_segments(input), OcrSegmentKind::Affix));
}
