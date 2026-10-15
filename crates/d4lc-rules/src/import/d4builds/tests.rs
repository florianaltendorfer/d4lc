use super::*;
use crate::test_support::{d4builds_fixture_dir, workspace_root};

#[test]
fn load_available_d4builds_variants_reads_saved_build_json_without_network() {
    let variants = load_available_d4builds_variants(&d4builds_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(variants.len(), 3);
    assert_eq!(variants[0].source_build_id, "barb-whirlwind-endgame");
    assert_eq!(variants[0].source_build_name, "Whirlwind Endgame");
    assert_eq!(variants[0].variant_name.as_deref(), Some("Endgame"));
    assert_eq!(variants[1].variant_name.as_deref(), Some("Speedfarm"));
    assert_eq!(variants[2].source_build_id, "sorc-arc-lash-starter");
    assert_eq!(variants[2].variant_name.as_deref(), Some("Starter"));
}

#[test]
fn available_builds_d4builds_lists_stored_builds_and_skips_raw_captures() {
    // A raw browser capture must never surface as an importable build
    // row.
    let dir = std::env::temp_dir().join("d4lc-rules-available-builds-d4builds");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        d4builds_fixture_dir().join("whirlwind_endgame.json"),
        dir.join("whirlwind_endgame.json"),
    )
    .unwrap();
    std::fs::write(
        dir.join("660881f7-cb6a-4162-be62-29f0afeb37bf.raw.json"),
        r#"{"format":"raw-capture","responses":[]}"#,
    )
    .unwrap();

    let builds = load_available_d4builds_builds(&dir).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].id, "barb-whirlwind-endgame");
    assert_eq!(builds[0].name, "Whirlwind Endgame");
    assert_eq!(
        builds[0].variant_names,
        vec!["Endgame".to_string(), "Speedfarm".to_string()]
    );
    assert!(!builds[0].date.is_empty(), "the scraped date is carried");
    let _ = std::fs::remove_dir_all(&dir);
}

// -- D4Builds site-text conversion. ---------------------------------------

/// The recorded DOM extraction of a real d4builds.gg build page
/// (`D4BuildsSiteBuild` JSON, texts/labels only — a trimmed recording:
/// the leading entries of every stat group and a bounded tile sample
/// per paragon board). Recorded live 2026-09-21.
fn d4builds_site_fixture() -> D4BuildsSiteBuild {
    let path = d4builds_fixture_dir().join("site-extraction/ancients-barbarian-endgame.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn d4builds_conversion_lookups() -> D4BuildsConversionLookups {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    D4BuildsConversionLookups::load(&repo)
        .unwrap_or_else(|error| panic!("conversion lookups: {error}"))
}

#[test]
fn convert_d4builds_site_fixture_round_trips_through_the_import_path() {
    // converted entity → save → load_available_d4builds_variants →
    // import_d4builds_build_variant, exactly the chain the download
    // worker and the dialog feed.
    let site = d4builds_site_fixture();
    let converted = convert_d4builds_site_variants(&d4builds_conversion_lookups(), &site)
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(converted.build.id, "ancients-barbarian-endgame");
    assert_eq!(converted.build.name, "Singer Ancient \"Bro\" Barb (S14) ");
    assert_eq!(converted.build.date, "Updated on July 4, 2026.");
    assert_eq!(converted.build.variants.len(), 2);
    assert_eq!(
        converted
            .build
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Base", "Melted Heart Version"]
    );

    // The scraped paragon boards parse into the entity boards
    // (multi-line board names, paren-stripped glyphs, rotations).
    let boards = &converted.build.variants[0].paragon_boards;
    assert_eq!(boards.len(), 5);
    assert_eq!(boards[0].name, "Starting Board");
    assert_eq!(boards[0].glyph, "Might");
    assert_eq!(boards[0].rotation, "0°");
    assert_eq!(boards[1].name, "Blood Rage");
    assert_eq!(boards[1].rotation, "180°");
    assert_eq!(boards[3].rotation, "270°");
    assert!(
        boards
            .iter()
            .all(|board| board.nodes.len() == 21 * 21 && board.nodes.iter().any(|node| *node)),
        "every parsed board carries active nodes on the 21x21 grid"
    );

    // Round trip: persist, enumerate, import.
    let dir = std::env::temp_dir().join("d4lc-rules-d4builds-site-roundtrip");
    let _ = std::fs::remove_dir_all(&dir);
    let path = export_d4builds_build_cache(&converted.build, &dir)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(path, dir.join("ancients-barbarian-endgame.json"));
    let variants = load_available_d4builds_variants(&dir).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0].variant_name.as_deref(), Some("Base"));

    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let result = import_d4builds_build_variant(&repo, &path, Some("Base"), &AppSettings::default())
        .unwrap_or_else(|error| panic!("import of the converted entity failed: {error}"));
    assert!(
        !result.affix_preset.affixes.is_empty(),
        "the scraped affix texts must map to canonical ids"
    );
    assert!(
        !result.paragon_routes.is_empty(),
        "the paragon boards import"
    );
    assert_eq!(
        result.paragon_routes[0].boards.len(),
        5,
        "all scraped boards import"
    );
    // The recorded page carries site texts the thresholded lookup
    // refuses (upstream's always-matching ExtractOne would import
    // them); they surface as structured warnings, never as failures.
    assert!(
        !converted.warnings.is_empty(),
        "unknown site texts become structured warnings"
    );
    assert!(
        converted
            .warnings
            .iter()
            .all(|warning| warning.provider == D4BUILDS_PROVIDER
                && !warning.source_value.is_empty()
                && !warning.upstream_method.is_empty()),
        "warnings carry the structured import-warning shape"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn convert_d4builds_site_fixture_without_paragon_strips_the_boards() {
    // A scrape with the paragon import off carries no boards; the
    // conversion must mirror that and the import yields no routes.
    let mut site = d4builds_site_fixture();
    for variant in &mut site.variants {
        variant.paragon_boards.clear();
    }
    let converted = convert_d4builds_site_variants(&d4builds_conversion_lookups(), &site)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        converted
            .build
            .variants
            .iter()
            .all(|variant| variant.paragon_boards.is_empty()),
        "no variant may carry paragon boards"
    );

    let dir = std::env::temp_dir().join("d4lc-rules-d4builds-site-noparagon");
    let _ = std::fs::remove_dir_all(&dir);
    let path = export_d4builds_build_cache(&converted.build, &dir)
        .unwrap_or_else(|error| panic!("{error}"));
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let result = import_d4builds_build_variant(
        &repo,
        &path,
        Some("Melted Heart Version"),
        &AppSettings::default(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(result.paragon_routes.is_empty());
    assert!(
        !result.affix_preset.affixes.is_empty(),
        "the gear conversion is unaffected by the paragon gate"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn convert_d4builds_site_variant_warns_on_unknown_site_texts() {
    // Unknown affix / aspect / rune / unique texts become structured
    // warnings; the known texts still convert and nothing panics.
    let site = D4BuildsSiteBuild {
        build_id: "whirlwind-endgame".to_string(),
        name: "Unknown Text Build".to_string(),
        date: "Updated on now.".to_string(),
        variants: vec![D4BuildsSiteVariant {
            name: "Only".to_string(),
            affix_groups: vec![D4BuildsSiteAffixGroup {
                slot: "Helm".to_string(),
                affixes: vec![D4BuildsSiteAffix {
                    text: "Totally Unknown Affix Text".to_string(),
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                }],
            }],
            aspects: vec!["Totally Unknown Aspect".to_string()],
            uniques: vec!["Totally Unknown Unique".to_string()],
            runes: vec!["Qzz".to_string()],
            paragon_boards: Vec::new(),
        }],
    };
    let converted = convert_d4builds_site_variants(&d4builds_conversion_lookups(), &site)
        .unwrap_or_else(|error| panic!("{error}"));
    let fields: Vec<String> = converted
        .warnings
        .iter()
        .map(|warning| warning.source_field.clone())
        .collect();
    assert_eq!(
        fields,
        vec![
            "Helm[0].AffixText".to_string(),
            "Aspect[0]".to_string(),
            "Runes[0]".to_string(),
            "Uniques[0]".to_string(),
        ],
        "every unknown text produces one structured warning"
    );
    assert_eq!(
        converted.build.variants[0].helm.len(),
        0,
        "an unmapped affix is not stored as a mapped entry"
    );
}

#[test]
fn parse_d4builds_site_paragon_boards_applies_the_upstream_transforms() {
    // The upstream doc example "paragon__board__tile r2 c10 active
    // enabled" per rotation, on the 21x21 grid.
    let board_for = |style: &str| -> Vec<D4BuildsSiteParagonBoard> {
        vec![D4BuildsSiteParagonBoard {
            name: "Example\nBoard".to_string(),
            glyph: "(Glyph)".to_string(),
            style: style.to_string(),
            tiles: vec![
                "paragon__board__tile r2 c10 active enabled".to_string(),
                "paragon__board__tile r1 c1".to_string(),
            ],
        }]
    };
    let active_index = |boards: &[D4BuildsParagonBoardFile]| -> usize {
        boards[0]
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, active)| active.then_some(index))
            .unwrap_or_else(|| panic!("no active node"))
    };
    let mut boards = parse_d4builds_site_paragon_boards(&board_for("")).unwrap();
    assert_eq!(boards[0].name, "Example");
    assert_eq!(boards[0].glyph, "Glyph");
    assert_eq!(boards[0].rotation, "0°");
    assert_eq!(active_index(&boards), 21 + 9, "r2/c10 at 0°");
    boards = parse_d4builds_site_paragon_boards(&board_for("transform: rotate(90deg);")).unwrap();
    assert_eq!(boards[0].rotation, "90°");
    assert_eq!(active_index(&boards), 9 * 21 + 19, "r2/c10 at 90°");
    boards = parse_d4builds_site_paragon_boards(&board_for("transform: rotate(180deg);")).unwrap();
    assert_eq!(active_index(&boards), 19 * 21 + 11, "r2/c10 at 180°");
    boards = parse_d4builds_site_paragon_boards(&board_for("transform: rotate(270deg);")).unwrap();
    assert_eq!(active_index(&boards), 11 * 21 + 1, "r2/c10 at 270°");
    // Non-standard rotations fall through with unchanged coordinates
    // (upstream if/else chain) and render the "?°" text.
    boards = parse_d4builds_site_paragon_boards(&board_for("transform: rotate(45deg);")).unwrap();
    assert_eq!(boards[0].rotation, "?°");
    assert_eq!(active_index(&boards), 2 * 21 + 10);

    // Malformed structures fail like upstream's unguarded int.Parse /
    // index access; an all-short board name degrades to "unknown".
    let unknown_name = vec![D4BuildsSiteParagonBoard {
        name: String::new(),
        glyph: String::new(),
        style: String::new(),
        tiles: Vec::new(),
    }];
    assert_eq!(
        parse_d4builds_site_paragon_boards(&unknown_name).unwrap()[0].name,
        "unknown"
    );
    let no_digits = vec![D4BuildsSiteParagonBoard {
        name: "Board".to_string(),
        glyph: String::new(),
        style: String::new(),
        tiles: vec!["paragon__board__tile active".to_string()],
    }];
    assert!(parse_d4builds_site_paragon_boards(&no_digits).is_err());
    let broken_rotate = vec![D4BuildsSiteParagonBoard {
        name: "Board".to_string(),
        glyph: String::new(),
        style: "transform: rotate(abcdeg);".to_string(),
        tiles: Vec::new(),
    }];
    assert!(parse_d4builds_site_paragon_boards(&broken_rotate).is_err());
}

#[test]
fn import_d4builds_variant_maps_presets_non_affixes_and_paragon() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings {
        selected_affix_language: "deDE".to_string(),
        ..AppSettings::default()
    };
    let result = import_d4builds_build_variant(
        &repo,
        &d4builds_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_matches_d4builds_expected_import_fixture(&result, "whirlwind_endgame.json");
}

#[test]
fn import_d4builds_variant_reports_unknown_source_ids_as_structured_warnings() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let result = import_d4builds_build_variant(
        &repo,
        &d4builds_fixture_dir().join("arc_lash_unknown.json"),
        Some("Starter"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_matches_d4builds_expected_import_fixture(&result, "arc_lash_unknown.json");
}

fn d4builds_expected_fixture_dir() -> std::path::PathBuf {
    d4builds_fixture_dir().join("expected")
}

fn assert_matches_d4builds_expected_import_fixture(result: &ImportWorkflowResult, file_name: &str) {
    let actual = normalized_d4builds_import_fixture_value(result, file_name);
    if std::env::var_os("D4LC_WRITE_IMPORT_FIXTURES").is_some() {
        std::fs::create_dir_all(d4builds_expected_fixture_dir())
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            d4builds_expected_fixture_dir().join(file_name),
            serde_json::to_string_pretty(&actual).unwrap_or_else(|error| panic!("{error}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(d4builds_expected_fixture_dir().join(file_name))
            .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(actual, expected);
}

fn normalized_d4builds_import_fixture_value(
    result: &ImportWorkflowResult,
    file_name: &str,
) -> serde_json::Value {
    let mut actual = serde_json::to_value(result).unwrap_or_else(|error| panic!("{error}"));
    actual["source_url_or_file"] =
        serde_json::Value::String(format!("tests/fixtures/imports/d4builds/{file_name}"));
    actual
}
