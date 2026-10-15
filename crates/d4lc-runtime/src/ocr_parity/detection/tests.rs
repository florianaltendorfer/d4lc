use super::*;
use crate::ocr_parity::test_support::{put_black_pixel, white_frame};

fn one_pixel_tooltip_template() -> TooltipTemplate {
    TooltipTemplate {
        name: "tooltip_kb_all.png".to_string(),
        image: GrayImage {
            width: 1,
            height: 1,
            pixels: vec![0],
        },
    }
}

/// A 2x1 marker: background then the marker pixel. Upstream paints each
/// found rectangle white, so a single all-white template would match its
/// own painted rectangle again; this one does not.
fn small_affix_marker_template() -> MarkerTemplate {
    MarkerTemplate {
        name: "dot-affix.png".to_string(),
        kind: TemplateKind::AffixMarker,
        image: GrayImage {
            width: 2,
            height: 1,
            pixels: vec![0, 255],
        },
    }
}

fn splitter_template(name: &str, pixels: Vec<u8>, width: u32, height: u32) -> MarkerTemplate {
    MarkerTemplate {
        name: name.to_string(),
        kind: TemplateKind::SplitterMarker,
        image: GrayImage {
            width,
            height,
            pixels,
        },
    }
}

fn splitter_marker(name: &str, y: i32) -> OcrMarker {
    OcrMarker {
        kind: OcrSegmentKind::Splitter,
        name: name.to_string(),
        rect: Rect {
            x: 10,
            y,
            width: 8,
            height: 3,
        },
    }
}

// --- Tooltip crop tests ---

#[test]
fn broad_roi_with_horizontal_padding_is_trimmed_to_upstream_final_crop_before_ocr() {
    let mut frame = white_frame(12, 10);
    put_black_pixel(&mut frame, 3, 8);
    let templates = vec![one_pixel_tooltip_template()];

    let (final_frame, trace) = prepare_tooltip_ocr_input(&frame, &templates, 6, 0.01)
        .unwrap_or_else(|error| panic!("prepare tooltip OCR input failed: {error}"));

    assert_eq!(trace.planned_tooltip_roi.width, 12);
    assert_eq!(trace.original_input_size, (12, 10));
    assert_eq!(
        trace.detected_tooltip_rect,
        Some(Rect {
            x: 3,
            y: 0,
            width: 6,
            height: 8
        })
    );
    assert_eq!(
        trace.final_tooltip_crop_rect,
        Rect {
            x: 3,
            y: 0,
            width: 6,
            height: 8
        }
    );
    assert_eq!(trace.crop_source, "upstream_tooltip_template_match");
    assert_eq!(final_frame.width, 6);
    assert_eq!(final_frame.height, 8);
}

#[test]
fn final_tooltip_crop_width_drives_width_divided_by_seven_marker_roi() {
    let templates = vec![small_affix_marker_template()];
    let settings = d4lc_config::AppSettings {
        threshold_similarity_affix_location: 0.01,
        ..Default::default()
    };

    let mut broad = white_frame(70, 10);
    put_black_pixel(&mut broad, 31, 5);
    let broad_markers = detect_upstream_markers(&broad, &templates, 0, 255, &settings)
        .unwrap_or_else(|error| panic!("broad marker detection failed: {error}"));

    let mut final_crop = white_frame(14, 10);
    put_black_pixel(&mut final_crop, 1, 5);
    let final_markers = detect_upstream_markers(&final_crop, &templates, 0, 255, &settings)
        .unwrap_or_else(|error| panic!("final marker detection failed: {error}"));

    assert!(broad_markers.affix_markers.is_empty());
    assert_eq!(final_markers.affix_markers.len(), 1);
    // The 2px ROI (14 / 7) holds the template at x = 0.
    assert_eq!(final_markers.affix_markers[0].rect.x, 0);
}

// --- Socket tests ---

/// Upstream `FindItemSocketLocation`: the `_mask` files are partners,
/// never searched themselves; a template without its mask finds nothing;
/// the masked match ignores the socket interior (a gem-filled socket is
/// found by its ring).
#[test]
fn socket_search_uses_the_mask_partner_and_ignores_the_interior() {
    let socket = |name: &str, pixels: Vec<u8>| MarkerTemplate {
        name: name.to_string(),
        kind: TemplateKind::SocketMarker,
        image: GrayImage {
            width: 3,
            height: 3,
            pixels,
        },
    };
    // An empty socket: a patterned ring around an empty interior (255).
    let ring = vec![255, 0, 255, 0, 255, 0, 255, 0, 255];
    let ring_mask = vec![255, 255, 255, 255, 0, 255, 255, 255, 255];
    // Background 0, a ring at (2, 4) holding a "gem" (interior 0).
    let mut roi = GrayImage {
        width: 6,
        height: 8,
        pixels: vec![0; 48],
    };
    for y in 4..7 {
        for x in 2..5 {
            // The same ring, the interior holding a gem (0).
            roi.pixels[y * 6 + x] = if (y + x) % 2 == 0 && (y, x) != (5, 3) {
                255
            } else {
                0
            };
        }
    }

    let with_mask = vec![
        socket("dot-socket_1.png", ring.clone()),
        socket("dot-socket_1_mask.png", ring_mask),
    ];
    let found = find_socket_locations(&roi, &with_mask, 0.05);
    assert_eq!(
        found,
        vec![Rect {
            x: 2,
            y: 4,
            width: 3,
            height: 3
        }],
        "only the template is searched, and the filled socket matches"
    );

    let without_mask = vec![socket("dot-socket_1.png", ring)];
    assert!(find_socket_locations(&roi, &without_mask, 0.05).is_empty());
}

// --- Splitter tests ---

#[test]
fn splitter_roi_matches_upstream_width_and_center_calculation() {
    let roi = splitter_roi_rect(375, 749);

    assert_eq!(
        roi,
        Rect {
            x: 161,
            y: 0,
            width: 53,
            height: 749
        }
    );
}

#[test]
fn splitter_marker_coordinates_translate_from_roi_to_tooltip_coordinates() {
    let tooltip = GrayImage {
        width: 70,
        height: 8,
        pixels: vec![128; 70 * 8],
    };
    let mut tooltip = tooltip;
    // Upstream splitter ROI for width 70 is x=30,width=10. Put a 2px template at ROI-local x=3.
    tooltip.pixels[4 * 70 + 33] = 255;
    tooltip.pixels[4 * 70 + 34] = 0;
    let templates = vec![splitter_template("dot-splitter_1.png", vec![255, 0], 2, 1)];

    let (markers, trace) = detect_splitter_markers(&tooltip, &templates, 0.01)
        .unwrap_or_else(|error| panic!("splitter detection failed: {error}"));

    assert_eq!(trace.roi.x, 30);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].rect.x, 33);
    assert_eq!(markers[0].rect.y, 4);
    assert_eq!(trace.accepted_matches[0].score, 0.0);
}

#[test]
fn splitter_repeated_matching_masks_accepted_rects_white_and_finds_multiple_matches() {
    let mut roi = GrayImage {
        width: 6,
        height: 1,
        pixels: vec![255, 0, 128, 255, 0, 128],
    };
    let template = splitter_template("dot-splitter_1.png", vec![255, 0], 2, 1);
    mask_gray_rect_white(
        &mut roi,
        Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        },
    );
    assert_eq!(roi.pixels, vec![255, 255, 128, 255, 0, 128]);

    let tooltip = GrayImage {
        width: 42,
        height: 1,
        pixels: {
            let mut pixels = vec![128; 42];
            // Upstream splitter ROI for width 42 is x=18,width=6.
            pixels[18..24].copy_from_slice(&[255, 0, 128, 255, 0, 128]);
            pixels
        },
    };
    let (markers, trace) = detect_splitter_markers(&tooltip, &[template], 0.01)
        .unwrap_or_else(|error| panic!("splitter detection failed: {error}"));

    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].rect.x, 18);
    assert_eq!(markers[1].rect.x, 21);
    assert_eq!(trace.template_results[0].accepted_candidate_count, 2);
    assert!(trace.template_results[0]
        .rejected_best_score
        .is_some_and(|score| score >= 0.01));
}

#[test]
fn top_splitter_cleanup_removes_only_non_top_splitters_within_two_y_of_first_top() {
    let mut splitters = vec![
        splitter_marker("dot-splitter_top_1.png", 100),
        splitter_marker("dot-splitter_1.png", 98),
        splitter_marker("dot-splitter_1.png", 102),
        splitter_marker("dot-splitter_1.png", 103),
        splitter_marker("dot-splitter_top_2.png", 101),
    ];

    let (applied, removed) = remove_overlapping_non_top_splitters(&mut splitters);

    assert!(applied);
    assert_eq!(removed.len(), 2);
    assert!(removed
        .iter()
        .all(|marker| marker.name == "dot-splitter_1.png"));
    assert!(splitters.iter().any(|marker| marker.rect.y == 103));
    assert!(splitters
        .iter()
        .any(|marker| marker.name == "dot-splitter_top_2.png"));
}

// --- Aspect area trace tests ---

#[test]
fn aspect_trace_uses_first_split_point_after_aspect_marker() {
    let aspect = OcrMarker {
        kind: OcrSegmentKind::Aspect,
        name: "dot-aspects_legendary.png".to_string(),
        rect: Rect {
            x: 5,
            y: 240,
            width: 12,
            height: 12,
        },
    };
    let trace = trace_aspect_area_split_points(
        &[],
        &aspect,
        &[],
        &[
            splitter_marker("dot-splitter_1.png", 330),
            splitter_marker("dot-splitter_1.png", 300),
        ],
    );

    assert_eq!(trace.selected_boundary_source, "splitter_marker");
    assert_eq!(trace.selected_boundary_rect.map(|rect| rect.y), Some(300));
    assert!(!trace.fallback_to_tooltip_bottom);
}

#[test]
fn aspect_trace_falls_back_to_tooltip_bottom_without_later_split_point() {
    let aspect = OcrMarker {
        kind: OcrSegmentKind::Aspect,
        name: "dot-aspects_legendary.png".to_string(),
        rect: Rect {
            x: 5,
            y: 240,
            width: 12,
            height: 12,
        },
    };
    let trace = trace_aspect_area_split_points(
        &[OcrMarker {
            kind: OcrSegmentKind::Affix,
            name: "dot-affixes_normal.png".to_string(),
            rect: Rect {
                x: 5,
                y: 120,
                width: 10,
                height: 10,
            },
        }],
        &aspect,
        &[],
        &[],
    );

    assert_eq!(trace.selected_boundary_source, "tooltip_bottom");
    assert_eq!(trace.selected_boundary_rect, None);
    assert!(trace.fallback_to_tooltip_bottom);
}
