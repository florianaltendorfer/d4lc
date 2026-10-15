use super::*;
use crate::ocr_parity::test_support::{
    canned_registry, enus_profile, load_enus_fuzzy_indexes, reference_capture,
    rune_sigil_execution, segment_result, workspace_root,
};
use crate::ocr_parity::{runtime_parity_result, PreparedTooltipOcr};
use d4lc_core::coords::{FramePlacement, TooltipPlacement};
use d4lc_ocr::OcrSegmentKind;
use d4lc_parse::TooltipOcrSegmentKind;

/// The chain of one real reference capture stays observable from the
/// tooltip crop through the recognized pixels, the raw text, the TSV and
/// the normalized lines to the item name, and the name keeps its own
/// provenance next to the affix rows.
#[test]
fn reference_capture_chain_stays_observable_from_crop_to_item_name() {
    // frame-CTRL.png is the controller capture of the reference corpus, so
    // the session runs the controller mode with the controller tooltip
    // template. Both are created at runtime in a real install, which is why
    // the test supplies them through its own config root.
    let config_root = std::env::temp_dir().join(format!(
        "d4lc-chain-export-controller-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&config_root);
    std::fs::create_dir_all(&config_root)
        .unwrap_or_else(|error| panic!("could not create the temp root: {error}"));
    // The geometry values are the ones the 1080p_SMF preset seeds into the
    // settings (assets/original/systempresets/1080p_SMF/config.json).
    let settings = d4lc_config::AppSettings {
        selected_system_preset: "1080p_SMF".to_string(),
        controller_mode: true,
        tooltip_width: 375,
        affix_area_height_offset_top: 8,
        affix_area_height_offset_bottom: 8,
        affix_aspect_area_width_offset: 18,
        aspect_area_height_offset_top: 8,
        ..d4lc_config::AppSettings::default()
    };
    d4lc_config::save_app_settings(&config_root, &settings)
        .unwrap_or_else(|error| panic!("could not write the settings: {error}"));
    d4lc_config::save_controller_config(&config_root, &["tooltip_gc_x.png".to_string()])
        .unwrap_or_else(|error| panic!("could not write the controller config: {error}"));
    let prepared = PreparedTooltipOcr::prepare_with_game_data_root(
        &config_root,
        &workspace_root(),
        "1080p_SMF".to_string(),
        enus_profile(),
    )
    .unwrap_or_else(|error| panic!("prepared OCR session failed: {error}"));
    let _ = std::fs::remove_dir_all(&config_root);
    let frame = reference_capture("frame-CTRL.png");
    let registry = canned_registry();

    let execution = prepared
        .execute(&registry, &frame)
        .unwrap_or_else(|error| panic!("reference capture execution failed: {error}"));
    assert!(
        execution.results.len() > 1,
        "the reference capture must plan the full segment set"
    );
    for segment in &execution.results {
        assert_eq!(segment.ocr_input.variant, prepared.preprocess_variant());
        assert_eq!(segment.ocr_input.payload, "gray");
        assert_eq!(segment.ocr_input.width, segment.plan.rect.width);
        assert_eq!(segment.ocr_input.height, segment.plan.rect.height);
        assert!(
            segment.tsv.is_some(),
            "segment {} must carry its structured output",
            segment.plan.index
        );
    }

    let placement =
        TooltipPlacement::whole_frame(FramePlacement::default(), frame.width, frame.height);
    let result = runtime_parity_result(&execution, 80, prepared.fuzzy_indexes(), &placement);
    assert_eq!(result.item.display_name.as_deref(), Some("Storm Amulet"));

    let name_source = result
        .item
        .name_line_sources
        .first()
        .unwrap_or_else(|| panic!("the name line must keep its provenance"));
    assert_eq!(
        name_source.segment_kind,
        TooltipOcrSegmentKind::UpperTooltip
    );
    let name_segment = execution
        .results
        .get(name_source.source_segment_index)
        .unwrap_or_else(|| panic!("the name segment must exist"));
    assert_eq!(name_segment.plan.kind, OcrSegmentKind::UpperTooltip);
    assert!(
        !result.item.affixes.is_empty(),
        "the reference capture must parse affix rows"
    );
    for affix in &result.item.affixes {
        assert_eq!(affix.source.segment_kind, TooltipOcrSegmentKind::Affix);
        assert_ne!(
            affix.source.source_segment_index, name_source.source_segment_index,
            "no affix row may be attributed to the name's segment"
        );
        let affix_segment = &execution.results[affix.source.source_segment_index];
        assert_eq!(affix_segment.plan.kind, OcrSegmentKind::Affix);
    }

    let dir = workspace_root().join("debug-output/ocr-chain");
    let artifacts = export_tooltip_ocr_chain(&dir, "frame-CTRL", &execution, &result)
        .unwrap_or_else(|error| panic!("chain export failed: {error}"));
    assert_eq!(artifacts.segment_inputs.len(), execution.results.len());
    for path in artifacts
        .segment_inputs
        .iter()
        .chain(std::iter::once(&artifacts.tooltip_crop))
    {
        assert!(path.exists(), "{} must exist", path.display());
    }
    let chain: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&artifacts.chain_json)
            .unwrap_or_else(|error| panic!("chain json unreadable: {error}")),
    )
    .unwrap_or_else(|error| panic!("chain json invalid: {error}"));
    assert_eq!(chain["display_name"], "Storm Amulet");
    assert_eq!(chain["preprocess_variant"], "upstream_gray_binary_inv");
    assert!(chain["name_line_sources"].is_array());
    assert!(chain["affix_rows"].is_array());
    assert!(chain["segments"][0]["tsv"]
        .as_str()
        .is_some_and(|tsv| tsv.starts_with("level")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An exported segment image that does not match the recognized pixels is
/// refused: the chain artifacts are evidence, not decoration.
#[test]
fn chain_export_refuses_a_segment_image_that_does_not_match_the_trace() {
    let mut execution = rune_sigil_execution(vec![
        segment_result(0, OcrSegmentKind::UpperTooltip, "Storm Amulet"),
        segment_result(1, OcrSegmentKind::Affix, "+740 Maximum Life"),
    ]);
    let result = runtime_parity_result(
        &execution,
        80,
        Some(&load_enus_fuzzy_indexes()),
        &TooltipPlacement::default(),
    );
    let dir = std::env::temp_dir().join(format!(
        "d4lc-runtime-chain-mismatch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    // The recorded pixels of the first segment no longer describe the
    // image the execution cropped.
    execution.results[0].ocr_input.digest ^= 0xffff;
    match export_tooltip_ocr_chain(&dir, "mismatch", &execution, &result) {
        Err(message) => assert!(
            message.contains("differs from the recognized image"),
            "{message}"
        ),
        Ok(_) => panic!("a mismatching segment image must not be exported as evidence"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
