use super::*;

#[test]
fn raw_text_to_ocr_text_keeps_non_empty_lines_without_tsv() {
    let text = raw_text_to_ocr_text(" Sword \n\n 925 Item Power \n", 375, 420);
    assert_eq!(text.raw_text, " Sword \n\n 925 Item Power \n");
    assert_eq!(
        text.bounding_box_source,
        OcrBoundingBoxSource::SyntheticFullImage
    );
    assert_eq!(text.lines.len(), 2);
    assert_eq!(text.lines[0].text, "Sword");
    assert_eq!(text.lines[0].rect.width, 375);
    assert_eq!(text.lines[0].rect.height, 420);
    assert_eq!(text.lines[0].words.len(), 1);
    assert_eq!(text.lines[0].words[0].text, "Sword");
}

#[test]
fn maps_upstream_languages_to_tesseract_codes() {
    assert_eq!(tesseract_language_code("enUS"), "eng");
    assert_eq!(tesseract_language_code("deDE"), "deu");
    assert_eq!(tesseract_language_code("zhCN"), "chi_sim");
    assert_eq!(tesseract_language_code("unknown"), "eng");
}

#[test]
fn tsv_rows_preserve_line_and_word_boxes_with_fallback_grouping() {
    let tsv = concat!(
        "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
        "4\t1\t1\t1\t1\t0\t12\t18\t70\t8\t-1\t\n",
        "5\t1\t1\t1\t1\t1\t10\t20\t30\t8\t95\tItem\n",
        "5\t1\t1\t1\t1\t2\t45\t20\t40\t8\t92\tPower\n",
        "4\t1\t1\t1\t2\t0\t12\t38\t45\t8\t-1\t\n",
        "5\t1\t1\t1\t2\t1\t10\t40\t50\t8\t90\tLegendary\n"
    );

    let text = parse_tesseract_tsv_lines(tsv, "fallback");

    assert_eq!(text.raw_text, "Item Power\nLegendary");
    assert_eq!(text.bounding_box_source, OcrBoundingBoxSource::TesseractTsv);
    assert_eq!(text.lines.len(), 2);
    assert_eq!(text.lines[0].text, "Item Power");
    assert_eq!(text.lines[0].rect.x, 12);
    assert_eq!(text.lines[0].rect.width, 70);
    assert_eq!(text.lines[0].words.len(), 2);
    assert_eq!(text.lines[0].words[0].rect.x, 10);
    assert_eq!(text.lines[0].words[1].rect.x, 45);
    assert!(text.lines[0].words[0].rect.x < text.lines[0].rect.x);
    assert_eq!(text.lines[1].words.len(), 1);
}

#[test]
fn upstream_convert_to_text_filters_right_aligned_lines() {
    let lines = vec![
        OcrLine {
            text: "Damage".to_string(),
            rect: Rect {
                x: 10,
                y: 10,
                width: 40,
                height: 10,
            },
            words: Vec::new(),
        },
        OcrLine {
            text: "Requires Level".to_string(),
            rect: Rect {
                x: 80,
                y: 12,
                width: 70,
                height: 10,
            },
            words: Vec::new(),
        },
        OcrLine {
            text: "Close".to_string(),
            rect: Rect {
                x: 15,
                y: 25,
                width: 35,
                height: 10,
            },
            words: Vec::new(),
        },
    ];

    assert_eq!(upstream_convert_to_text(&lines, "fallback"), "Damage Close");
}

#[test]
fn convert_to_text_upper_tooltip_section_keeps_raw_text() {
    let raw = "]ADE RELIC ®F v\n| FATHOMLESS DARK (‘ ' »\n| &\n900 Item Power\n";

    assert_eq!(
        upstream_converted_text(OcrSegmentKind::UpperTooltip, raw),
        raw
    );
}

#[test]
fn convert_to_aspect_strips_digits_brackets_and_punctuation_like_upstream() {
    let raw = "  Your Abyss Skills\ndeal increased damage.\n";

    assert_eq!(
        upstream_converted_text(OcrSegmentKind::Aspect, raw),
        "Your Abyss Skills deal increased damage"
    );
}

#[test]
fn convert_to_aspect_keeps_footer_control_text_because_upstream_has_no_blacklist() {
    let raw = "Aspect text
Equip
Hide Comparison
Scroll Down";

    assert_eq!(
        upstream_converted_text(OcrSegmentKind::Aspect, raw),
        "Aspect text Equip Hide Comparison Scroll Down"
    );
}

#[test]
fn convert_to_unique_strips_digits_and_punctuation_like_upstream() {
    let raw = "\nLucky Hit:\nup to 10% chance\n";

    // Exact upstream filter output: ':' is kept, stripped digits/punctuation
    // leave double spaces (no whitespace collapse upstream).
    assert_eq!(
        upstream_converted_text(OcrSegmentKind::UniqueAspect, raw),
        "Lucky Hit: up to  chance"
    );
}

#[test]
fn convert_to_affix_matches_upstream_digit_symbol_removal() {
    let raw = "+12.5% [8 - 15]% Damage, (Close)\n";

    assert_eq!(
        upstream_converted_text(OcrSegmentKind::Affix, raw),
        "Damage Close"
    );
}

#[test]
fn affix_conversion_route_switches_with_socket_detection() {
    assert_eq!(
        upstream_conversion_route(OcrSegmentKind::Affix, true).source_method,
        "OcrHandler.ConvertToAffix"
    );
    assert_eq!(
        upstream_conversion_route(OcrSegmentKind::Affix, true).branch,
        "socket_detection_enabled"
    );
    assert_eq!(
        upstream_conversion_route(OcrSegmentKind::Affix, false).source_method,
        "OcrHandler.ConvertToAffixMultiline"
    );
    assert_eq!(
        upstream_conversion_route(OcrSegmentKind::Affix, false).branch,
        "socket_detection_disabled"
    );
}

#[test]
fn convert_to_affix_multiline_joins_cleaned_leading_lines() {
    let raw = "+12.5% Damage\n(While Healthy)\n[8 - 15]%\n";

    assert_eq!(
        upstream_convert_to_affix_multiline_text(raw),
        "Damage While Healthy"
    );
}

fn word_row(left: i32, top: i32, width: u32, text: &str) -> String {
    format!("5\t1\t1\t1\t1\t1\t{left}\t{top}\t{width}\t12\t95\t{text}\n")
}

fn words_tsv(rows: &[String]) -> String {
    let mut tsv = String::from(
        "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
    );
    for row in rows {
        tsv.push_str(row);
    }
    tsv
}

#[test]
fn upper_tooltip_words_segment_by_geometry_and_apply_upstream_artifact_removal() {
    // Mirrors upstream ParseHOcrText: same-line join (gap 5), power line
    // "925 Item Power" at x1=20, artifacts removed by the power-x1 rule
    // (|35-20|>10), the <3 char rule, X1>100 and X1==0 rules.
    let tsv = words_tsv(&[
        word_row(20, 20, 60, "Ancestral"),
        word_row(85, 20, 60, "Legendary"),
        word_row(35, 45, 30, "ab"),
        word_row(20, 70, 30, "925"),
        word_row(55, 70, 30, "Item"),
        word_row(90, 70, 40, "Power"),
        word_row(20, 95, 30, "x"),
        word_row(0, 120, 60, "artifact0"),
        word_row(150, 145, 40, "side"),
    ]);

    let text = parse_upper_tooltip_text(&tsv, "enUS", "fallback");

    assert_eq!(text.raw_text, "Ancestral Legendary\n925 Item Power");
    assert_eq!(text.lines.len(), 2);
    assert_eq!(text.lines[0].text, "Ancestral Legendary");
    assert_eq!(text.lines[0].rect.x, 20);
    // Upstream HOcrLine zeroes Y1/X2/Y2 and only keeps X1.
    assert_eq!(text.lines[0].rect.y, 0);
    assert_eq!(text.lines[0].rect.width, 0);
    assert_eq!(text.lines[0].rect.height, 0);
    assert_eq!(text.lines[1].text, "925 Item Power");
    assert_eq!(
        text.bounding_box_source,
        OcrBoundingBoxSource::TesseractTsvWordsGrouped
    );
}

#[test]
fn upper_tooltip_words_truncate_lines_below_item_power_line() {
    let tsv = words_tsv(&[
        word_row(20, 20, 60, "Ancestral"),
        word_row(20, 45, 30, "925"),
        word_row(55, 45, 30, "Item"),
        word_row(90, 45, 40, "Power"),
        word_row(20, 70, 50, "Below"),
    ]);

    let text = parse_upper_tooltip_text(&tsv, "enUS", "fallback");

    assert_eq!(text.raw_text, "Ancestral\n925 Item Power");
    assert_eq!(text.lines.len(), 2);
}

#[test]
fn upper_tooltip_words_start_new_line_on_dy_over_twenty_gap_over_twenty() {
    // dy == 20 exactly and dx == 5 exactly stay on the same line (strict
    // upstream comparisons); a whitespace gap > 20 starts a new line.
    let tsv = words_tsv(&[
        word_row(50, 10, 20, "aaaa"),
        word_row(70, 10, 20, "bbbb"),
        word_row(95, 10, 10, "cccc"),
        word_row(100, 30, 10, "dddd"),
        word_row(150, 30, 10, "eeee"),
    ]);

    let text = parse_upper_tooltip_text(&tsv, "enUS", "fallback");

    // The second geometry line ("eeee", x1=150) is removed by X1 > 100.
    assert_eq!(text.raw_text, "aaaa bbbb cccc dddd");
}

#[test]
fn upper_tooltip_words_start_new_line_when_words_share_start_position() {
    // Case 2: |dx| < 5 with |dy| <= 20 means the word re-uses the line
    // start position, so it belongs to the next reconstructed line.
    let tsv = words_tsv(&[word_row(20, 20, 100, "aaaa"), word_row(22, 35, 30, "bbbb")]);

    let text = parse_upper_tooltip_text(&tsv, "enUS", "fallback");

    assert_eq!(text.raw_text, "aaaa\nbbbb");
}

#[test]
fn upper_tooltip_words_join_without_spaces_for_zhcn() {
    let tsv = words_tsv(&[word_row(20, 20, 30, "传奇"), word_row(55, 20, 30, "护甲")]);

    let text = parse_upper_tooltip_text(&tsv, "zhCN", "fallback");

    assert_eq!(text.raw_text, "传奇护甲");
}

#[test]
fn upper_tooltip_words_without_word_rows_fall_back_to_plain_text() {
    let text = parse_upper_tooltip_text("", "enUS", "fallback");

    assert_eq!(text.raw_text, "fallback");
    assert_eq!(text.bounding_box_source, OcrBoundingBoxSource::Unavailable);
}
