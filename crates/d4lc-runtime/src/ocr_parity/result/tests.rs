use super::*;
use crate::ocr_parity::test_support::{
    load_enus_fuzzy_indexes, rune_sigil_execution, segment_result,
};
use d4lc_core::coords::FramePlacement;
use d4lc_ocr::OcrMarker;

fn text_line(text: &str, y: i32) -> d4lc_core::OcrLine {
    d4lc_core::OcrLine {
        text: text.to_string(),
        rect: Rect {
            x: 4,
            y,
            width: 200,
            height: 14,
        },
        words: Vec::new(),
    }
}

/// The compared region starts below the header: at the first splitter
/// under the item name, else at the first recognized line; blank lines
/// and empty segments do not pull it up.
#[test]
fn read_top_starts_at_the_first_recognized_line() {
    let mut upper = segment_result(0, OcrSegmentKind::Affix, "Item Name");
    upper.text.lines = vec![text_line("   ", 3), text_line("Item Name", 30)];
    let mut lower = segment_result(1, OcrSegmentKind::Affix, "+10 Strength");
    lower.text.lines = vec![text_line("+10 Strength", 2)];
    let execution = rune_sigil_execution(vec![upper.clone(), lower.clone()]);
    assert_eq!(segment_read_top(&execution), Some(28));

    // A splitter below the name ends the (possibly animated) header.
    let mut execution = rune_sigil_execution(vec![upper, lower]);
    execution.marker_plan.splitter_markers = vec![OcrMarker {
        kind: OcrSegmentKind::Affix,
        name: "dot-splitter_1".to_string(),
        rect: Rect {
            x: 0,
            y: 36,
            width: 100,
            height: 4,
        },
    }];
    assert_eq!(segment_read_top(&execution), Some(36));

    let blank = segment_result(0, OcrSegmentKind::Affix, "");
    let execution = rune_sigil_execution(vec![blank]);
    assert_eq!(
        segment_read_top(&execution),
        Some(0),
        "without recognized text the segment top is kept"
    );
}

/// The compared extent spans the recognized lines of all segments (in
/// input-crop coordinates, widened by the text margin) down to the lowest
/// line; blank lines do not widen it, and without a recognized line it is
/// unknown.
#[test]
fn read_extent_spans_the_recognized_lines() {
    let mut upper = segment_result(0, OcrSegmentKind::Affix, "Item Name");
    let mut wide_blank = text_line("   ", 3);
    wide_blank.rect.width = 900;
    upper.text.lines = vec![wide_blank, text_line("Item Name", 30)];
    let mut lower = segment_result(1, OcrSegmentKind::Affix, "+10 Strength");
    let mut indented = text_line("+10 Strength", 2);
    indented.rect.x = 20;
    indented.rect.width = 290;
    lower.text.lines = vec![indented];
    let mut execution = rune_sigil_execution(vec![upper, lower]);
    execution.crop_trace.final_tooltip_crop_rect.x = 10;
    assert_eq!(
        segment_read_extent(&execution, Some(28)),
        Some(crate::ReadExtent {
            left: 10,
            right: 324,
            bottom: 60
        })
    );

    let blank = segment_result(0, OcrSegmentKind::Affix, "");
    assert_eq!(
        segment_read_extent(&rune_sigil_execution(vec![blank]), None),
        None
    );
}

/// Header lines above the compared rows do not widen the columns: the
/// extent follows the text below `read_top` only.
#[test]
fn read_extent_ignores_lines_above_the_compared_rows() {
    let mut header = segment_result(0, OcrSegmentKind::Affix, "Item Name");
    let mut name = text_line("Item Name", 2);
    name.rect.width = 340;
    header.text.lines = vec![name];
    let mut affix = segment_result(1, OcrSegmentKind::Affix, "+10 Strength");
    let mut indented = text_line("+10 Strength", 2);
    indented.rect.x = 30;
    affix.text.lines = vec![indented];
    let execution = rune_sigil_execution(vec![header, affix]);
    assert_eq!(
        segment_read_extent(&execution, Some(36)),
        Some(crate::ReadExtent {
            left: 26,
            right: 234,
            bottom: 60
        })
    );
    assert_eq!(
        segment_read_extent(&execution, None),
        Some(crate::ReadExtent {
            left: 0,
            right: 348,
            bottom: 60
        })
    );
}

/// The shared snapshot builder (CLI read-tooltip, app live
/// loop, manual validation bundle) must run upstream
/// `OcrHandler.TextToRune`/`TextToSigil` fuzzy conversion when the session
/// indexes are present, and degrade to the pre-wiring empty rune/sigil
/// results without them (fail soft, no error, no panic).
#[test]
fn runtime_parity_result_converts_rune_and_sigil_segments_with_session_indexes() {
    let indexes = load_enus_fuzzy_indexes();
    let execution = rune_sigil_execution(vec![
        segment_result(0, OcrSegmentKind::UpperTooltip, "Magic Rune of Invocation"),
        segment_result(
            1,
            OcrSegmentKind::Rune,
            "Summon a Spirit Wolf Companion to attack enemies for 8 seconds.",
        ),
        segment_result(
            2,
            OcrSegmentKind::Sigil,
            "Abandoned Mineworks in Kehjistan\nTier 100",
        ),
    ]);

    let with_fuzzy =
        runtime_parity_result(&execution, 80, Some(&indexes), &TooltipPlacement::default());

    assert_eq!(with_fuzzy.item.runes.len(), 1);
    assert_eq!(
        with_fuzzy.item.runes[0].rune_id.as_deref(),
        Some("Item_Rune_Effect_SummonSpiritWolf")
    );
    assert_eq!(
        with_fuzzy.item.runes[0].conversion_method,
        "OcrHandler.ConvertToRune"
    );
    assert_eq!(with_fuzzy.item.sigils.len(), 1);
    assert_eq!(
        with_fuzzy.item.sigils[0].sigil_id.as_deref(),
        Some("World_DGN_Kehj_AbandonedMineworks")
    );

    // The classified rune/sigil rows ride the same parity result. The
    // test segments carry no OCR line rects, so the composed geometry
    // stays absent (absence) while the classification transports.
    assert_eq!(with_fuzzy.item.non_affix_rows.len(), 2);
    assert!(with_fuzzy
        .item
        .non_affix_rows
        .iter()
        .all(|row| row.rect.is_none()));

    let without_fuzzy = runtime_parity_result(&execution, 80, None, &TooltipPlacement::default());
    assert!(without_fuzzy.item.runes.is_empty());
    assert!(without_fuzzy.item.sigils.is_empty());
    // Without the session indexes nothing is classified, so no rows.
    assert!(without_fuzzy.item.non_affix_rows.is_empty());
}

fn affix_with_source(
    source_segment_index: usize,
    source_line_index: usize,
    global_line_index: usize,
    rect: Option<Rect>,
) -> d4lc_parse::ItemTooltipAffix {
    d4lc_parse::ItemTooltipAffix {
        name: "Maximum Life".to_string(),
        raw_text: "+740 Maximum Life".to_string(),
        value_text: Some("+740".to_string()),
        range_text: None,
        text_clean: None,
        text_value: None,
        affix_id: Some("Affix_MaxLife".to_string()),
        affix_label: None,
        fuzzy: None,
        conversion_method: None,
        is_greater_affix: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
        row_id: d4lc_parse::AffixRowId::from_ordinal(global_line_index),
        source: d4lc_parse::ItemTooltipSourceLine {
            source_segment_index,
            source_line_index,
            global_line_index,
            segment_kind: d4lc_parse::TooltipOcrSegmentKind::Affix,
            raw_text: "+740 Maximum Life".to_string(),
            converted_text: None,
            normalized_text: "+740 maximum life".to_string(),
            rect,
            marker_name: None,
        },
    }
}

/// Window-relative affix row geometry: tooltip crop origin + segment
/// plan offset + OCR line rect. Rows without a source line rect, or
/// whose segment plan is missing, are skipped instead of fabricated.
#[test]
fn affix_row_geometry_composes_crop_plan_and_line_rects() {
    let results = vec![
        segment_result(0, OcrSegmentKind::UpperTooltip, "Storm Amulet"),
        segment_result(1, OcrSegmentKind::Affix, "+740 Maximum Life"),
    ];
    let item = ItemTooltipParseResult {
        affixes: vec![
            affix_with_source(
                1,
                0,
                2,
                Some(Rect {
                    x: 5,
                    y: 6,
                    width: 100,
                    height: 12,
                }),
            ),
            // No OCR line rect on the source line: skipped.
            affix_with_source(1, 1, 3, None),
            // Segment plan missing for segment 9: skipped.
            affix_with_source(
                9,
                0,
                4,
                Some(Rect {
                    x: 5,
                    y: 6,
                    width: 100,
                    height: 12,
                }),
            ),
        ],
        ..Default::default()
    };

    let geometry = compose_affix_row_geometry(
        &results,
        TooltipRect(Rect {
            x: 100,
            y: 0,
            width: 375,
            height: 720,
        }),
        &TooltipPlacement::default(),
        &item,
    );

    assert_eq!(geometry.len(), 1);
    assert_eq!(geometry[0].row_id, d4lc_parse::AffixRowId(2));
    assert_eq!(geometry[0].source_line_index, 0);
    assert_eq!(geometry[0].global_line_index, 2);
    assert_eq!(
        geometry[0].text_rect.rect(),
        Rect {
            x: 105,
            y: 46,
            width: 100,
            height: 12,
        }
    );
}

/// Two OCR segments that each start at local line 0 keep
/// two distinct row identities and two distinct geometries through
/// parse and row-geometry composition.
#[test]
fn rows_with_local_line_zero_in_two_segments_stay_distinct() {
    let line = |text: &str| d4lc_core::OcrLine {
        text: text.to_string(),
        rect: Rect {
            x: 5,
            y: 6,
            width: 100,
            height: 12,
        },
        words: Vec::new(),
    };
    let mut first = segment_result(1, OcrSegmentKind::Affix, "+740 Maximum Life");
    first.text.lines = vec![line("+740 Maximum Life")];
    let mut second = segment_result(2, OcrSegmentKind::Affix, "+12.0% Movement Speed");
    second.text.lines = vec![line("+12.0% Movement Speed")];
    let results = vec![first, second];

    let item = parse_item_tooltip_from_ocr_segments(&runtime_item_segments(&results));
    assert_eq!(item.affixes.len(), 2, "both rows parse: {:?}", item.affixes);
    assert_eq!(item.affixes[0].source.source_line_index, 0);
    assert_eq!(item.affixes[1].source.source_line_index, 0);
    assert_ne!(item.affixes[0].row_id, item.affixes[1].row_id);

    let geometry = compose_affix_row_geometry(
        &results,
        TooltipRect(Rect {
            x: 100,
            y: 0,
            width: 375,
            height: 720,
        }),
        &TooltipPlacement::default(),
        &item,
    );
    assert_eq!(geometry.len(), 2);
    assert_eq!(geometry[0].row_id, item.affixes[0].row_id);
    assert_eq!(geometry[1].row_id, item.affixes[1].row_id);
    assert_eq!(
        (
            geometry[0].source_segment_index,
            geometry[1].source_segment_index
        ),
        (1, 2)
    );
    assert_ne!(geometry[0].text_rect, geometry[1].text_rect);
}

/// A wrapped affix is read from one marker's area: it stays one row whose
/// marker anchor is that marker, and the next affix keeps its own row and
/// its own marker instead of inheriting the wrapped line's.
#[test]
fn a_wrapped_affix_keeps_one_row_and_its_own_marker_anchor() {
    let line = |text: &str, y: i32, width: u32| d4lc_core::OcrLine {
        text: text.to_string(),
        rect: Rect {
            x: 4,
            y,
            width,
            height: 14,
        },
        words: Vec::new(),
    };
    let marker = |y: i32| Rect {
        x: 2,
        y,
        width: 18,
        height: 18,
    };
    let mut wrapped = segment_result(
        1,
        OcrSegmentKind::Affix,
        "+12.5% Critical Strike Chance Against\nInjured Enemies",
    );
    wrapped.text.lines = vec![
        line("+12.5% Critical Strike Chance Against", 2, 250),
        line("Injured Enemies", 18, 110),
    ];
    wrapped.plan.source_marker_rect = Some(marker(40));
    wrapped.plan.marker_index = Some(0);
    let mut next = segment_result(2, OcrSegmentKind::Affix, "+8.0% Attack Speed");
    next.text.lines = vec![line("+8.0% Attack Speed", 2, 160)];
    next.plan.source_marker_rect = Some(marker(80));
    next.plan.marker_index = Some(1);
    let results = vec![wrapped, next];
    let item = parse_item_tooltip_from_ocr_segments(&runtime_item_segments(&results));
    let parity_crop = TooltipRect(Rect {
        x: 300,
        y: 0,
        width: 375,
        height: 720,
    });

    let geometry =
        compose_affix_row_geometry(&results, parity_crop, &TooltipPlacement::default(), &item);

    assert_eq!(item.affixes.len(), 2, "the wrapped line is not a row");
    assert_eq!(geometry.len(), 2);
    let ids: Vec<_> = geometry.iter().map(|row| row.row_id).collect();
    assert_eq!(
        ids,
        vec![d4lc_core::AffixRowId(0), d4lc_core::AffixRowId(1)]
    );
    // Row 0: both text lines, one marker; row 1: its own marker.
    assert_eq!(geometry[0].text_rect.rect().height, 30);
    let anchors: Vec<_> = geometry
        .iter()
        .map(|row| row.marker_anchor.map(|anchor| anchor.rect()))
        .collect();
    assert_eq!(
        anchors,
        vec![
            Some(Rect {
                x: 300,
                y: 40,
                width: 18,
                height: 18,
            }),
            Some(Rect {
                x: 300,
                y: 80,
                width: 18,
                height: 18,
            }),
        ]
    );
}

/// A tooltip on a second monitor left of and above the primary
/// (negative desktop origin), captured at 1.5 frame pixels per window
/// pixel, lands in window space through exactly one conversion — and
/// moving the window afterwards changes only the desktop placement.
#[test]
fn row_geometry_is_converted_to_window_space_exactly_once() {
    use d4lc_core::coords::{DesktopRect, FrameRect, FrameScale, WindowRect};

    let mut affix = segment_result(1, OcrSegmentKind::Affix, "+740 Maximum Life");
    affix.text.lines = vec![d4lc_core::OcrLine {
        text: "+740 Maximum Life".to_string(),
        rect: Rect {
            x: 5,
            y: 6,
            width: 100,
            height: 12,
        },
        words: Vec::new(),
    }];
    affix.plan.source_marker_rect = Some(Rect {
        x: 2,
        y: 44,
        width: 18,
        height: 18,
    });
    affix.plan.marker_index = Some(0);
    let results = vec![affix];
    let item = parse_item_tooltip_from_ocr_segments(&runtime_item_segments(&results));
    let parity_crop = TooltipRect(Rect {
        x: 10,
        y: 0,
        width: 375,
        height: 720,
    });
    let placement = |window_x: i32, window_y: i32| TooltipPlacement {
        frame: FramePlacement {
            window_in_desktop: DesktopRect(Rect {
                x: window_x,
                y: window_y,
                width: 2560,
                height: 1440,
            }),
            frame_origin_in_window: d4lc_core::Point { x: 0, y: 0 },
            scale: FrameScale::from_milli(1500),
        },
        crop_in_frame: FrameRect(Rect {
            x: 900,
            y: 30,
            width: 400,
            height: 800,
        }),
    };
    let on_second_monitor = placement(-2560, -300);

    let geometry = compose_affix_row_geometry(&results, parity_crop, &on_second_monitor, &item);
    assert_eq!(geometry.len(), 1);
    let row = &geometry[0];
    // Text: tooltip (10 + 0 + 5, 0 + 40 + 6) → frame (915, 76) → window
    // (610, 50); the frame span [915, 1015) covers window [610, 677).
    assert_eq!(
        row.text_rect,
        WindowRect(Rect {
            x: 610,
            y: 50,
            width: 67,
            height: 9,
        })
    );
    assert_eq!(
        row.affix_area,
        WindowRect(Rect {
            x: 606,
            y: 46,
            width: 214,
            height: 22,
        })
    );
    // Marker column: the tooltip's left edge; top and height: the marker.
    assert_eq!(
        row.marker_anchor,
        Some(WindowRect(Rect {
            x: 606,
            y: 49,
            width: 13,
            height: 13,
        }))
    );
    assert_eq!(row.marker_ordinal, Some(0));
    assert_eq!(
        on_second_monitor
            .frame
            .window_to_desktop(row.text_rect)
            .rect(),
        Rect {
            x: -1950,
            y: -250,
            width: 67,
            height: 9,
        }
    );

    // The window moved between two captures: window-space rows are the
    // same, only their desktop position follows the window.
    let moved = placement(-1280, 0);
    let moved_geometry = compose_affix_row_geometry(&results, parity_crop, &moved, &item);
    assert_eq!(moved_geometry[0].text_rect, row.text_rect);
    assert_eq!(
        moved.frame.window_to_desktop(row.text_rect).origin(),
        d4lc_core::Point { x: -670, y: 50 }
    );
}

fn non_affix_row(
    source_segment_index: usize,
    source_line_index: usize,
    global_line_index: usize,
    kind: TooltipOcrSegmentKind,
    rect: Option<Rect>,
) -> ItemTooltipNonAffixRow {
    ItemTooltipNonAffixRow {
        kind,
        id: Some("Item_Rune_Condition_OnMove".to_string()),
        label: None,
        source: d4lc_parse::ItemTooltipSourceLine {
            source_segment_index,
            source_line_index,
            global_line_index,
            segment_kind: kind,
            raw_text: "Travel meters.".to_string(),
            converted_text: None,
            normalized_text: "travel meters.".to_string(),
            rect,
            marker_name: None,
        },
        rect: None,
        marker_anchor: None,
    }
}

/// Window-relative non-affix row rects mirror the affix geometry math:
/// tooltip crop origin + segment plan offset + OCR line rect. Rows
/// without an anchor line rect, or whose segment plan is missing, keep
/// `rect: None` instead of fabricated geometry.
#[test]
fn non_affix_row_geometry_composes_crop_plan_and_line_rects() {
    let mut rune = segment_result(1, OcrSegmentKind::Rune, "Travel meters.");
    rune.plan.source_marker_rect = Some(Rect {
        x: 2,
        y: 44,
        width: 18,
        height: 18,
    });
    let results = vec![
        segment_result(0, OcrSegmentKind::UpperTooltip, "Frost Rune of Summoning"),
        rune,
        segment_result(2, OcrSegmentKind::Sigil, "Aldurwood in Scosglen"),
    ];
    let mut rows = vec![
        non_affix_row(
            1,
            0,
            1,
            TooltipOcrSegmentKind::Rune,
            Some(Rect {
                x: 5,
                y: 6,
                width: 100,
                height: 12,
            }),
        ),
        // No OCR line rect on the anchor line: skipped.
        non_affix_row(2, 0, 2, TooltipOcrSegmentKind::Sigil, None),
        // Segment plan missing for segment 9: skipped.
        non_affix_row(
            9,
            0,
            3,
            TooltipOcrSegmentKind::Aspect,
            Some(Rect {
                x: 5,
                y: 6,
                width: 100,
                height: 12,
            }),
        ),
    ];

    compose_non_affix_row_geometry(
        &results,
        TooltipRect(Rect {
            x: 100,
            y: 0,
            width: 375,
            height: 720,
        }),
        &TooltipPlacement::default(),
        &mut rows,
    );

    assert_eq!(
        rows[0].rect,
        Some(Rect {
            x: 105,
            y: 46,
            width: 100,
            height: 12,
        })
    );
    assert_eq!(rows[1].rect, None);
    assert_eq!(rows[2].rect, None);
    // The marker anchor: the tooltip's left edge as the column, the
    // rune marker's row (OverlayHandler.cs:263-264).
    assert_eq!(
        rows[0].marker_anchor,
        Some(Rect {
            x: 100,
            y: 44,
            width: 18,
            height: 18,
        })
    );
    assert_eq!(rows[1].marker_anchor, None);
    assert_eq!(rows[2].marker_anchor, None);
}

/// Full pipeline with the live m5-sigil-retest fixture text.
/// Affix-kind segments (the pre-fix classification, seen live) produce no
/// sigils; the Sigil-kind classification emitted for item type "sigil"
/// runs OcrHandler.TextToSigil through the shared snapshot builder and
/// populates item.sigils.
#[test]
fn sigil_segment_classification_populates_item_sigils_through_parity_result() {
    let indexes = load_enus_fuzzy_indexes();
    let live_affix_classification = vec![
        segment_result(0, OcrSegmentKind::UpperTooltip, "Nightmare Sigil"),
        segment_result(1, OcrSegmentKind::Affix, "Aldurwood in Scosglen"),
    ];
    let sigil_classification = vec![
        segment_result(0, OcrSegmentKind::UpperTooltip, "Nightmare Sigil"),
        segment_result(1, OcrSegmentKind::Sigil, "Aldurwood in Scosglen"),
    ];

    let before = runtime_parity_result(
        &rune_sigil_execution(live_affix_classification),
        80,
        Some(&indexes),
        &TooltipPlacement::default(),
    );
    assert!(
        before.item.sigils.is_empty(),
        "Affix-kind classification must not populate sigils (live bug)"
    );

    let after = runtime_parity_result(
        &rune_sigil_execution(sigil_classification),
        80,
        Some(&indexes),
        &TooltipPlacement::default(),
    );
    assert_eq!(after.item.sigils.len(), 1);
    assert_eq!(
        after.item.sigils[0].sigil_id.as_deref(),
        Some("World_DGN_Scos_Aldurwood")
    );
    assert_eq!(
        after.item.sigils[0].conversion_method,
        "OcrHandler.ConvertToSigil"
    );
}
