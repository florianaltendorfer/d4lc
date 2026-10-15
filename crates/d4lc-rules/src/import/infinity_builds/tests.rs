use super::*;
use crate::test_support::{
    infinity_builds_fixture_dir, infinity_builds_share_fixture_dir, workspace_root,
};

#[test]
fn available_builds_infinity_builds_lists_cached_build_with_date() {
    let builds = load_available_infinity_builds_builds(&infinity_builds_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].id, "barb-infinity-whirlwind");
    assert_eq!(builds[0].name, "Infinity Whirlwind");
    assert_eq!(builds[0].variant_names, vec!["Endgame".to_string()]);
    assert_eq!(builds[0].date, "2025-08-01T12:00:00Z");
}

#[test]
fn available_builds_infinity_cache_export_rejects_hostile_ids() {
    // The stored id becomes the `{id}.json` file name — a
    // hostile id from a captured share page must fail the safety gate
    // before any file is written, never escape the builds directory.
    let dir = std::env::temp_dir().join("d4lc-rules-hostile-infinity-export");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("decoy.json"), b"{}").unwrap();

    let build = InfinityBuildsBuild {
        date: String::new(),
        id: "../decoy".to_string(),
        name: "Hostile".to_string(),
        url: String::new(),
        variants: Vec::new(),
    };
    let error = export_infinity_builds_build_cache(&build, &dir).expect_err("hostile id must fail");
    assert!(
        error.contains("cannot be used as a cache file name"),
        "{error}"
    );
    assert!(
        dir.join("decoy.json").is_file(),
        "the decoy above the builds dir must survive"
    );
    assert!(!dir.join("..").join("decoy.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

fn infinity_builds_source_fixture_dir() -> std::path::PathBuf {
    infinity_builds_fixture_dir().join("source")
}

fn infinity_builds_expected_fixture_dir() -> std::path::PathBuf {
    infinity_builds_fixture_dir().join("expected")
}

fn assert_matches_infinity_builds_expected_import_fixture(
    result: &ImportWorkflowResult,
    file_name: &str,
) {
    let mut actual = serde_json::to_value(result).unwrap_or_else(|error| panic!("{error}"));
    actual["source_url_or_file"] = serde_json::Value::String(format!(
        "tests/fixtures/imports/infinitybuilds/source/{file_name}"
    ));
    if std::env::var_os("D4LC_WRITE_IMPORT_FIXTURES").is_some() {
        std::fs::create_dir_all(infinity_builds_expected_fixture_dir())
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            infinity_builds_expected_fixture_dir().join(file_name),
            serde_json::to_string_pretty(&actual).unwrap_or_else(|error| panic!("{error}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(infinity_builds_expected_fixture_dir().join(file_name))
            .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(actual, expected);
}

fn infinity_builds_full_round_trip(
    share_fixture: &str,
    cache_file_name: &str,
    write_cache_fixture: bool,
    repo: &AssetRepository,
    settings: &AppSettings,
) -> (ImportWorkflowResult, InfinityBuildsShareImport) {
    let html = std::fs::read_to_string(infinity_builds_share_fixture_dir().join(share_fixture))
        .unwrap_or_else(|error| panic!("{error}"));
    let share_import = import_infinity_builds_share_html(
        repo,
        &html,
        "https://infinitybuilds.gg/build/x",
        settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    // Local JSON cache round-trip (ExportBuildVariant / LoadAvailable).
    // The cache dir is unique per call so concurrent tests never share
    // or overwrite each other's cached build files.
    let cache_dir = workspace_root().join(format!(
        "target/tmp/infinity-builds-cache/{cache_file_name}"
    ));
    let cache_path = export_infinity_builds_build_cache(&share_import.build, &cache_dir)
        .unwrap_or_else(|error| panic!("{error}"));
    let loaded = load_available_infinity_builds_variants(&cache_dir)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.len(), share_import.build.variants.len());

    let result =
        import_infinity_builds_build_variant(repo, &cache_path, None, &AppSettings::default())
            .unwrap_or_else(|error| panic!("{error}"));
    // Keep the committed cache fixture in sync with the share import so
    // the app-side offline dialog lists the same variants.
    if write_cache_fixture && std::env::var_os("D4LC_WRITE_IMPORT_FIXTURES").is_some() {
        std::fs::create_dir_all(infinity_builds_source_fixture_dir())
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            infinity_builds_source_fixture_dir().join(cache_file_name),
            serde_json::to_string_pretty(&share_import.build)
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    }
    (result, share_import)
}

#[test]
fn import_infinity_builds_share_round_trips_through_multisegment_payload() {
    // Live share pages have this shape: the flight push
    // string packs a leading module-reference segment before the data
    // segment, and an earlier script can mention `shareSlug` without
    // carrying the build. The navigation must skip both and still
    // reach the same build as the single-segment fixture.
    let repo = AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let (result, _) = infinity_builds_full_round_trip(
        "whirlwind_endgame_multisegment.html",
        "whirlwind_endgame_multisegment_cache",
        false,
        &repo,
        &settings,
    );
    assert_eq!(result.source_build_id, "barb-infinity-whirlwind");
    assert_eq!(result.source_build_name, "Infinity Whirlwind");
    assert_eq!(result.variant_name.as_deref(), Some("Endgame"));
    assert_eq!(result.affix_preset.affixes.len(), 6);
    assert_eq!(result.uniques.len(), 1);
    assert!(result.warnings.is_empty());
}

#[test]
fn import_infinity_builds_share_reads_wrapper_array_at_top_level() {
    // Live share pages whose flight payload opens with the build wrapper
    // array itself (`[…, {"build": {…}}]`) instead of a container with
    // `children` used to fail with "failed to parse nested share array:
    // EOF while parsing a value" and produce no model
    // (`blazing-scream-warlock-VVnCwkWiey`). The navigation must accept
    // the wrapper candidate directly.
    let repo = AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let (result, share_import) = infinity_builds_full_round_trip(
        "whirlwind_endgame_direct_wrapper.html",
        "whirlwind_endgame_direct_wrapper",
        false,
        &repo,
        &settings,
    );
    assert_eq!(share_import.build.id, "barb-infinity-whirlwind");
    assert_eq!(result.source_build_id, "barb-infinity-whirlwind");
    assert_eq!(result.source_build_name, "Infinity Whirlwind");
    assert_eq!(result.variant_name.as_deref(), Some("Endgame"));
    assert_eq!(result.affix_preset.affixes.len(), 6);
}

#[test]
fn import_infinity_builds_skips_an_empty_build_candidate_before_the_real_one() {
    // A flight payload can carry an empty `{"build": {}}` candidate before the
    // real one. It must be skipped instead of shadowing the real candidate.
    let json = concat!(
        r#"["$","$L58",null,{"build":{}}]"#,
        r#"["$","$L58",null,{"build":{"shareSlug":"barb-infinity-whirlwind","title":"Infinity Whirlwind"}}]"#,
    );
    let build = parse_infinity_builds_json(json).expect("the real candidate must be reached");
    assert_eq!(build.share_slug, "barb-infinity-whirlwind");
    assert_eq!(build.title, "Infinity Whirlwind");
}

#[test]
fn import_infinity_builds_share_round_trips_through_cache_and_matches_expected_fixture() {
    let repo = AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let (result, share_import) = infinity_builds_full_round_trip(
        "whirlwind_endgame.html",
        "whirlwind_endgame.json",
        true,
        &repo,
        &settings,
    );

    assert_eq!(result.provider, INFINITY_BUILDS_PROVIDER);
    assert_eq!(result.source_build_id, "barb-infinity-whirlwind");
    assert_eq!(result.source_build_name, "Infinity Whirlwind");
    assert_eq!(result.variant_name.as_deref(), Some("Endgame"));
    // Affixes: 2x helm (greater life + tempered life max), 2x weapon
    // (core stat + CDR), 1x ring1 (greater life), 1x ring2 (CDR).
    assert_eq!(result.affix_preset.affixes.len(), 6);
    // Aspect spans all equipment item types.
    assert_eq!(result.aspects.len(), EQUIPMENT_ITEM_TYPES.len());
    assert_eq!(result.uniques.len(), 1);
    assert_eq!(result.runes.len(), 1);
    assert!(result.warnings.is_empty());
    // TuningPrisms attach through the resolved IdName.
    let prisms_by_id = load_affix_tuning_prisms(&repo, INFINITY_BUILDS_LANGUAGE)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(result.affix_preset.affixes.iter().all(|affix| {
        affix.tuning_prisms == affix_tuning_prisms(&prisms_by_id, &affix.affix_id)
    }));
    // Tempered sorts last; greater affix keeps its flag and color.
    let last = result
        .affix_preset
        .affixes
        .last()
        .unwrap_or_else(|| panic!("expected at least one affix"));
    assert!(last.is_tempered);
    let greater = result
        .affix_preset
        .affixes
        .iter()
        .find(|affix| affix.is_greater)
        .unwrap_or_else(|| panic!("expected a greater affix"));
    assert_eq!(
        greater.color,
        AppSettings::default().default_colors.greater,
        "a greater affix takes the configured greater color"
    );

    // Paragon: starter board (rotation 0) + one 90°-rotated board with
    // localized names and transformed node locations.
    assert_eq!(result.paragon_boards.len(), 2);
    assert_eq!(result.paragon_boards[0].name, "Start");
    assert_eq!(result.paragon_boards[0].rotation, "0°");
    assert_eq!(result.paragon_boards[0].active_nodes, vec![105]);
    assert_eq!(result.paragon_boards[1].name, "Hemorrhage");
    assert_eq!(result.paragon_boards[1].glyph, "Enchanter");
    assert_eq!(result.paragon_boards[1].rotation, "90°");
    assert_eq!(result.paragon_boards[1].active_nodes, vec![36, 57]);

    // Cache file keeps the upstream PascalCase shape.
    assert_eq!(share_import.build.url, "https://infinitybuilds.gg/build/x");
    let cache_text =
        serde_json::to_value(&share_import.build).unwrap_or_else(|error| panic!("{error}"));
    assert!(cache_text["Variants"][0]["AffixPreset"].is_object());
    // The InfinityBuilds wrapper keys are PascalCase; the nested Rust
    // `AffixPreset` keeps its native serde field names.
    assert_eq!(cache_text["Variants"][0]["AffixPreset"]["name"], "Endgame");

    assert_matches_infinity_builds_expected_import_fixture(&result, "whirlwind_endgame.json");
}

#[test]
fn import_infinity_builds_share_reports_unknown_source_ids_as_structured_warnings() {
    let repo = AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let html =
        std::fs::read_to_string(infinity_builds_share_fixture_dir().join("warning_cases.html"))
            .unwrap_or_else(|error| panic!("{error}"));
    let share_import = import_infinity_builds_share_html(
        &repo,
        &html,
        "https://infinitybuilds.gg/build/y",
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    // Unknown affix + unknown aspect warn; unknown rune and unique do
    // not (upstream ConvertItemRune / ConvertItemUnique emit no warning).
    assert_eq!(share_import.warnings.len(), 2);
    assert_eq!(share_import.unmapped_entries.len(), 2);
    assert!(share_import.warnings.iter().any(|warning| {
        warning.source_field == "helm[0].AffixText"
            && warning.source_value == "affix-totally-unknown-affix"
            && warning.message
                == "Imported InfinityBuild build contains unknown affix: affix-totally-unknown-affix."
            && warning.upstream_method == INFINITY_BUILDS_CONVERT_AFFIX_METHOD
    }));
    assert!(share_import.warnings.iter().any(|warning| {
        warning.source_field == "Aspect[0]"
            && warning.message
                == "Imported InfinityBuild build contains unknown aspect: aspect-asp-legendary-notreal-999-asp."
            && warning.upstream_method == INFINITY_BUILDS_CONVERT_ASPECT_METHOD
    }));
    let variant = &share_import.build.variants[0];
    assert!(variant.item_runes.is_empty());
    // ExtractOne-style unique matching always resolves a best candidate.
    assert_eq!(variant.item_uniques.len(), 1);
    assert!(variant.affix_preset.affixes.is_empty());
}

#[test]
fn import_infinity_builds_variant_matches_committed_cache_fixture() {
    let repo = AssetRepository::new(workspace_root());
    let result = import_infinity_builds_build_variant(
        &repo,
        &infinity_builds_source_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &AppSettings::default(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_matches_infinity_builds_expected_import_fixture(&result, "whirlwind_endgame.json");
}

#[test]
fn import_infinity_builds_can_disable_paragon_import() {
    let repo = AssetRepository::new(workspace_root());
    let settings = AppSettings {
        is_import_paragon_infinity_builds_enabled: false,
        ..AppSettings::default()
    };
    let (_, share_import) = infinity_builds_full_round_trip(
        "whirlwind_endgame.html",
        "whirlwind_endgame.json",
        false,
        &repo,
        &settings,
    );
    let variant = &share_import.build.variants[0];
    assert!(variant.paragon_boards.is_empty());
}

#[test]
fn load_available_infinity_builds_variants_reads_committed_cache() {
    let variants = load_available_infinity_builds_variants(&infinity_builds_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].source_build_id, "barb-infinity-whirlwind");
    assert_eq!(variants[0].source_build_name, "Infinity Whirlwind");
    assert_eq!(variants[0].variant_name.as_deref(), Some("Endgame"));
}

#[test]
fn infinity_builds_script_extraction_finds_share_slug_script() {
    let html = "<html><script>noop()</script><script>self.__next_f.push([1,\"x\"])</script></html>";
    assert!(extract_infinity_builds_share_script(html).is_none());
    let with_share = "<script>self.__next_f.push([1,\"4:shareSlug\"])</script>";
    let extracted = extract_infinity_builds_share_script(with_share)
        .unwrap_or_else(|| panic!("expected script extraction to find shareSlug"));
    assert!(extracted.contains("shareSlug"));
}

#[test]
fn infinity_builds_parse_handles_nested_children_container() {
    // Nested fallback: the outer container has no Id, so the parser
    // descends into Children[3] and scans its children again.
    let inner_wrapper = serde_json::json!([null, null, null, {"build": {
        "id": "x", "title": "Nested", "shareSlug": "nested-slug",
        "updatedAt": "t", "variants": []
    }}]);
    let nested_container = serde_json::json!([null, null, null, {
        "children": [inner_wrapper]
    }]);
    let outer_container = serde_json::json!([null, null, null, {
        "id": "",
        "children": [nested_container]
    }]);
    let json = outer_container.to_string();
    let build = parse_infinity_builds_json(&json).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(build.share_slug, "nested-slug");
    assert_eq!(build.title, "Nested");
    assert!(INFINITY_BUILDS_DOWNLOAD_METHOD.contains("DownloadInfinityBuildsBuild"));
    assert!(INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD.contains("ParseJsonBuild"));
    assert!(INFINITY_BUILDS_EXPORT_VARIANT_METHOD.contains("ExportBuildVariant"));
    assert!(INFINITY_BUILDS_CREATE_PRESET_METHOD.contains("CreatePresetFromInfinityBuildsBuild"));
}

#[test]
fn infinity_builds_parse_rejects_payload_without_share_script() {
    assert!(parse_infinity_builds_share_html("<html><script>idle()</script></html>").is_err());
}
