use super::*;
use crate::import::{ImportLookupScorer, D4BUILDS_AFFIX_THRESHOLD};
use crate::test_support::{canonical_affix_id, mobalytics_source_fixture_dir, workspace_root};

#[test]
fn available_builds_mobalytics_lists_builds_and_profile_summaries() {
    let builds = load_available_mobalytics_builds(&mobalytics_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(
        builds.iter().any(|build| !build.id.is_empty()),
        "stored build / profile rows are listed"
    );
    assert!(
        builds
            .iter()
            .any(|build| build.variant_names.iter().any(|v| !v.is_empty())),
        "build rows carry their variant names"
    );
}

#[test]
fn load_available_mobalytics_variants_reads_saved_source_json_without_network() {
    let variants = load_available_mobalytics_variants(&mobalytics_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(variants.len(), 5);
    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-build-001"
            && variant.source_build_name == "Spiritborn Endgame"
            && variant.variant_name.as_deref() == Some("Spiritborn Endgame")
    }));
    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-build-002"
            && variant.variant_name.as_deref() == Some("Warlock Starter")
    }));
    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-build-001"
            && variant.source_build_name == "Spiritborn Leveling"
            && variant.variant_name.as_deref() == Some("Endgame")
    }));
    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-build-unknown"
            && variant.variant_name.as_deref() == Some("Starter")
    }));
    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-build-multi-token"
            && variant.source_build_name == "Multi Token Source IDs"
            && variant.variant_name.as_deref() == Some("Starter")
    }));
}

#[test]
fn import_mobalytics_build_variant_supports_slugified_name_fallback_and_rotation_over_360() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings {
        selected_affix_language: "deDE".to_string(),
        ..AppSettings::default()
    };
    let result = import_mobalytics_build_variant(
        &repo,
        &mobalytics_source_fixture_dir().join("build_slugified_rotation.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_matches_mobalytics_expected_import_fixture(&result, "build_slugified_rotation.json");
    assert_eq!(result.paragon_boards[0].rotation, "90°");
    assert_eq!(result.paragon_boards[0].active_nodes, vec![20, 39]);
    assert_eq!(result.paragon_boards[0].name, "spiritborn-starting-board");
    assert_eq!(result.paragon_boards[1].name, "warlock-starting-board");
}

#[test]
fn import_mobalytics_build_variant_reports_unknown_source_ids_as_structured_warnings() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings {
        selected_affix_language: "deDE".to_string(),
        ..AppSettings::default()
    };
    let result = import_mobalytics_build_variant(
        &repo,
        &mobalytics_source_fixture_dir().join("build_unknown_ids.json"),
        Some("Starter"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_matches_mobalytics_expected_import_fixture(&result, "build_unknown_ids.json");
}

#[test]
fn mobalytics_tail_join_prefers_best_score_then_longest_affix() {
    let index_for = |labels: &[&str]| ImportLookupIndex {
        candidates: labels
            .iter()
            .map(|label| ImportLookupCandidate {
                id: (*label).to_string(),
                label: (*label).to_string(),
                normalized_label: normalize_import_text(label),
            })
            .collect(),
        scorer: ImportLookupScorer::Ratio,
        threshold: D4BUILDS_AFFIX_THRESHOLD,
    };

    // Equal best scores: the longer joined affix text wins.
    let index = index_for(&["beta gamma", "alpha beta gamma"]);
    let winner = mobalytics_match_affix_tail_joins(&index, "alpha-beta-gamma");
    assert_eq!(
        winner.map(|candidate| candidate.id),
        Some("alpha beta gamma".to_string())
    );

    // Higher best score wins over the longer tail: only the tail-join
    // "attack speed" exactly matches, the junk prefix tails do not.
    let index = index_for(&["attack speed"]);
    let winner = mobalytics_match_affix_tail_joins(&index, "greater-attack-speed");
    assert_eq!(
        winner.map(|candidate| candidate.id),
        Some("attack speed".to_string())
    );

    // Every tail-join below the fuzzy threshold stays unmapped.
    let index = index_for(&["attack speed"]);
    let winner = mobalytics_match_affix_tail_joins(&index, "zzz-qqq");
    assert!(winner.is_none());
}

#[test]
fn import_mobalytics_build_variant_resolves_multi_token_ids_via_tail_joins() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let result = import_mobalytics_build_variant(
        &repo,
        &mobalytics_source_fixture_dir().join("build_multi_token_ids.json"),
        Some("Starter"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_matches_mobalytics_expected_import_fixture(&result, "build_multi_token_ids.json");

    // "greater-attack-speed" resolves through the tail-join "attack
    // speed"; the full text alone would stay below the fuzzy threshold.
    let attack_speed = result
        .affix_preset
        .affixes
        .iter()
        .find(|affix| affix.affix_id == canonical_affix_id("AttackSpeed"))
        .unwrap_or_else(|| panic!("tail-join should resolve AttackSpeed"));
    assert!(attack_speed.is_greater);
    assert_eq!(
        attack_speed.tuning_prisms,
        vec!["TuningStone_1".to_string()]
    );
    // "lucky-hit-vulnerable-damage" resolves through its exact tail.
    assert!(result
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.affix_id == canonical_affix_id("Damage_to_Vulnerable")));
    // The implicit tail-join maps with the implicit flag preserved.
    assert!(result
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.affix_id == canonical_affix_id("Movement_Speed") && affix.is_implicit));
    // The unmapped multi-token affix still reports a structured warning.
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(
        result.warnings[0].source_value,
        "imaginary-multi-token-affix"
    );
}

#[test]
fn mobalytics_stored_entity_round_trips_through_export_load_and_import() {
    // The stored converted entity is the file the download flow writes
    // and the import path consumes — no GraphQL payload involved.
    let entity = MobalyticsBuildFile {
        date: "2026-09-01T10:00:00Z".to_string(),
        id: "moba-round-trip-1".to_string(),
        name: "Round Trip Build".to_string(),
        url: "https://mobalytics.gg/diablo-4/builds/round-trip".to_string(),
        variants: vec![MobalyticsVariantFile {
            name: "Endgame".to_string(),
            gloves: vec![MobalyticsAffixEntry {
                affix_text: "greater-attack-speed".to_string(),
                affix_text_list: vec![
                    "greater".to_string(),
                    "attack".to_string(),
                    "speed".to_string(),
                ],
                is_greater: true,
                ..MobalyticsAffixEntry::default()
            }],
            aspect: vec!["Bold Chieftain's Aspect".to_string()],
            uniques: vec!["harlequin crest".to_string()],
            runes: vec!["Yul".to_string()],
            paragon_boards: vec![MobalyticsParagonBoardFile {
                name: "spiritborn-starting-board".to_string(),
                glyph: "marshal".to_string(),
                rotation: "90°".to_string(),
                nodes: {
                    let mut nodes = vec![false; 21 * 21];
                    nodes[20] = true;
                    nodes
                },
            }],
            ..MobalyticsVariantFile::default()
        }],
    };
    let dir = std::env::temp_dir().join("d4lc-rules-mobalytics-round-trip");
    let _ = std::fs::remove_dir_all(&dir);

    let path = export_mobalytics_build_cache(&entity, &dir).expect("export");
    assert_eq!(path, dir.join("moba-round-trip-1.json"));

    // The dialog variant list reads the stored entity.
    let variants = load_available_mobalytics_variants(&dir).expect("variants");
    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-round-trip-1"
            && variant.variant_name.as_deref() == Some("Endgame")
    }));

    // The dialog build list carries the stored Url + Date (the Update
    // flow re-downloads by the stored Url).
    let builds = load_available_mobalytics_builds(&dir).expect("builds");
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].id, "moba-round-trip-1");
    assert_eq!(builds[0].date, "2026-09-01T10:00:00Z");
    assert_eq!(
        builds[0].url,
        "https://mobalytics.gg/diablo-4/builds/round-trip"
    );
    assert_eq!(builds[0].variant_names, vec!["Endgame".to_string()]);

    // The import consumes the stored conversion directly.
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let result =
        import_mobalytics_build_variant(&repo, &path, Some("Endgame"), &AppSettings::default())
            .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.source_build_id, "moba-round-trip-1");
    assert!(result
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.affix_id == canonical_affix_id("AttackSpeed") && affix.is_greater));
    // The stored paragon board round-trips through the imported shape.
    assert_eq!(result.paragon_boards[0].name, "spiritborn-starting-board");
    assert_eq!(result.paragon_boards[0].rotation, "90°");
    assert_eq!(result.paragon_boards[0].active_nodes, vec![20]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mobalytics_conversion_reports_unknown_texts_as_structured_warnings() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let lookups = MobalyticsConversionLookups::load(&repo).expect("lookups");
    let graphql =
        std::fs::read_to_string(mobalytics_source_fixture_dir().join("build_unknown_ids.json"))
            .unwrap_or_else(|error| panic!("{error}"));

    let converted = convert_mobalytics_graphql_build(
        &graphql,
        "https://mobalytics.gg/diablo-4/builds/unknown",
        true,
        &lookups,
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .expect("the raw GraphQL fixture must parse");

    assert_eq!(converted.build.name, "Unknown Source IDs");
    assert_eq!(
        converted.build.url, "https://mobalytics.gg/diablo-4/builds/unknown",
        "the entity carries the page URL like upstream _buildUrl"
    );
    assert!(
        converted
            .warnings
            .iter()
            .any(|warning| warning.source_field == "Gloves[0].AffixText"
                && warning.upstream_method == MOBALYTICS_CONVERT_AFFIX_METHOD),
        "warnings: {:?}",
        converted.warnings
    );
}

#[test]
fn parse_mobalytics_graphql_build_gates_the_paragon_export() {
    let graphql = std::fs::read_to_string(
        mobalytics_source_fixture_dir().join("build_slugified_rotation.json"),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let with_paragon = parse_mobalytics_graphql_build(
        &graphql,
        "https://mobalytics.gg/diablo-4/builds/slug",
        true,
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .expect("parses");
    assert!(
        with_paragon
            .variants
            .iter()
            .any(|variant| !variant.paragon_boards.is_empty()),
        "the fixture carries paragon boards"
    );

    let without_paragon = parse_mobalytics_graphql_build(
        &graphql,
        "https://mobalytics.gg/diablo-4/builds/slug",
        false,
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .expect("parses");
    assert!(
        without_paragon
            .variants
            .iter()
            .all(|variant| variant.paragon_boards.is_empty()),
        "the paragon export is gated by the upstream import toggle"
    );
}

/// Empty the `nodes` list next to every `boards` list, so each exported
/// paragon board is left without a node slug.
fn clear_paragon_nodes(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if object.contains_key("boards") && object.contains_key("nodes") {
                object.insert("nodes".to_string(), serde_json::json!([]));
            }
            object.values_mut().for_each(clear_paragon_nodes);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(clear_paragon_nodes),
        _ => {}
    }
}

#[test]
fn boards_without_nodes_report_upstreams_message_when_the_paragon_export_is_on() {
    // BuildsManagerMobalytics.cs:967-974.
    let graphql = std::fs::read_to_string(
        mobalytics_source_fixture_dir().join("build_slugified_rotation.json"),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let url = "https://mobalytics.gg/diablo-4/builds/slug";

    let (_, complete) = parse_mobalytics_graphql_build_with_warnings(&graphql, url, true)
        .unwrap_or_else(|error| panic!("{error}"))
        .expect("parses");
    assert!(
        complete.is_empty(),
        "boards with nodes report nothing: {complete:?}"
    );

    let mut document: serde_json::Value = serde_json::from_str(&graphql).expect("fixture JSON");
    clear_paragon_nodes(&mut document);
    let emptied = document.to_string();
    let (build, warnings) = parse_mobalytics_graphql_build_with_warnings(&emptied, url, true)
        .unwrap_or_else(|error| panic!("{error}"))
        .expect("parses");
    let board_names: Vec<&str> = build
        .variants
        .iter()
        .flat_map(|variant| {
            variant
                .paragon_boards
                .iter()
                .map(|board| board.name.as_str())
        })
        .collect();
    assert!(!board_names.is_empty());
    assert_eq!(
        warnings,
        board_names
            .iter()
            .map(|name| format!("No nodes found for paragon board {name}."))
            .collect::<Vec<_>>()
    );

    let (_, gated) = parse_mobalytics_graphql_build_with_warnings(&emptied, url, false)
        .unwrap_or_else(|error| panic!("{error}"))
        .expect("parses");
    assert!(gated.is_empty(), "no paragon export, no message");
}

#[test]
fn mobalytics_profile_state_parses_sorts_and_composes_variant_urls() {
    let url = "https://mobalytics.gg/diablo-4/profile/test-profile?class=campaign";
    let profile = parse_mobalytics_profile_state(MOBALYTICS_PROFILE_STATE_FIXTURE, url)
        .unwrap_or_else(|error| panic!("{error}"))
        .expect("the fixture carries both queries");

    assert_eq!(profile.id, "profile-123");
    assert_eq!(profile.profile_name, "test-profile");
    // The display name carries the upstream ` - {value}` filter suffix.
    assert_eq!(profile.name, "Test Profile - campaign");
    assert_eq!(profile.url, url);
    assert_eq!(profile.variants.len(), 2);
    // Date descending sort (upstream DateTime.Parse comparison).
    assert_eq!(profile.variants[0].name, "Spiritborn Endgame");
    assert_eq!(
        profile.variants[0].url,
        "https://mobalytics.gg/diablo-4/profile/test-profile/builds/spiritborn-endgame"
    );
    // An empty slugifiedName falls back to the build id.
    assert_eq!(profile.variants[1].name, "Warlock Starter");
    assert_eq!(
        profile.variants[1].url,
        "https://mobalytics.gg/diablo-4/profile/test-profile/builds/moba-build-002"
    );

    // Missing queries mirror the upstream "Failed. Invalid json." branch.
    let missing = parse_mobalytics_profile_state(r#"{"apollo":{"graphqlV2":{"queries":[]}}}"#, url)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(missing.is_none());
    // Malformed JSON mirrors the upstream deserialization exception.
    assert!(parse_mobalytics_profile_state("{not json", url).is_err());
}

#[test]
fn mobalytics_profile_cache_export_load_remove_round_trips() {
    let profile = MobalyticsProfileFile {
        id: "profile-123".to_string(),
        // Display names may contain spaces — upstream names the file
        // after the display name.
        name: "Test Profile - campaign".to_string(),
        profile_name: "test-profile".to_string(),
        url: "https://mobalytics.gg/diablo-4/profile/test-profile".to_string(),
        variants: vec![MobalyticsProfileVariant {
            date: "2026-05-31T12:00:00Z".to_string(),
            id: "moba-build-001".to_string(),
            name: "Spiritborn Endgame".to_string(),
            url: "https://mobalytics.gg/diablo-4/profile/test-profile/builds/spiritborn-endgame"
                .to_string(),
        }],
    };
    let dir = std::env::temp_dir().join("d4lc-rules-mobalytics-profiles");
    let _ = std::fs::remove_dir_all(&dir);

    let path = export_mobalytics_profile_cache(&profile, &dir).expect("export");
    assert_eq!(path, dir.join("Test Profile - campaign.json"));
    assert!(path.is_file());

    let (loaded, skipped) = load_available_mobalytics_profiles(&dir).expect("load");
    assert_eq!(loaded, vec![profile.clone()]);
    assert!(skipped.is_empty(), "{skipped:?}");

    // A corrupt file is reported and skipped; a blank one is skipped
    // silently like upstream; the valid profile still loads.
    std::fs::write(dir.join("Broken.json"), "{\"Name\": ").unwrap();
    std::fs::write(dir.join("Blank.json"), "  \n").unwrap();
    let (loaded, skipped) = load_available_mobalytics_profiles(&dir).expect("load");
    assert_eq!(loaded, vec![profile.clone()]);
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].starts_with("failed to parse ") && skipped[0].contains("Broken.json"));
    std::fs::remove_file(dir.join("Broken.json")).unwrap();
    std::fs::remove_file(dir.join("Blank.json")).unwrap();

    // Remove by display NAME (upstream RemoveMobalyticsProfile).
    assert!(remove_mobalytics_profile(&dir, &profile.name).expect("remove"));
    assert!(!path.exists());
    assert!(load_available_mobalytics_profiles(&dir)
        .expect("load")
        .0
        .is_empty());
    // A missing file is not an error; a hostile name fails the
    // port-side file-name gate before any filesystem access.
    assert!(!remove_mobalytics_profile(&dir, &profile.name).expect("remove missing"));
    assert!(remove_mobalytics_profile(&dir, "../escape").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_available_mobalytics_variants_parses_graphql_v2_profile_queries() {
    let variants = load_available_mobalytics_variants(&mobalytics_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-build-001"
            && variant.source_build_name == "Spiritborn Endgame"
            && variant.variant_name.as_deref() == Some("Spiritborn Endgame")
    }));
    assert!(variants.iter().any(|variant| {
        variant.source_build_id == "moba-build-002"
            && variant.source_build_name == "Warlock Starter"
            && variant.variant_name.as_deref() == Some("Warlock Starter")
    }));
}

#[test]
fn mobalytics_browser_is_hidden_by_default() {
    let visible = std::hint::black_box(MOBALYTICS_SHOW_BROWSER_DEFAULT);
    assert!(!visible);
}

/// The recorded `diablo4State` profile fixture — the value upstream
/// serializes out of `window.__PRELOADED_STATE__` and passes to
/// `ParseJsonProfile` (the app extracts the same entry).
const MOBALYTICS_PROFILE_STATE_FIXTURE: &str =
    include_str!("../../../tests/fixtures/imports/mobalytics/source/profile_graphql_v2.json");

fn mobalytics_expected_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/mobalytics/expected")
}

fn assert_matches_mobalytics_expected_import_fixture(
    result: &ImportWorkflowResult,
    file_name: &str,
) {
    let mut actual = serde_json::to_value(result).unwrap_or_else(|error| panic!("{error}"));
    actual["source_url_or_file"] = serde_json::Value::String(format!(
        "tests/fixtures/imports/mobalytics/source/{file_name}"
    ));
    if std::env::var_os("D4LC_WRITE_IMPORT_FIXTURES").is_some() {
        std::fs::create_dir_all(mobalytics_expected_fixture_dir())
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            mobalytics_expected_fixture_dir().join(file_name),
            serde_json::to_string_pretty(&actual).unwrap_or_else(|error| panic!("{error}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(mobalytics_expected_fixture_dir().join(file_name))
            .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(actual, expected);
}

// -----------------------------------------------------------------------
// Live site shapes (October 2026): reduced real captures, see
// tests/fixtures/imports/mobalytics/live/README.md.
// -----------------------------------------------------------------------

fn live_fixture(name: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/mobalytics/live")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn live_graphql_body_with_null_lists_and_null_entries_parses() {
    // `implicitStats: null` and `null` gear-stat entries made the whole
    // body unreadable: the import reported an empty document.
    let url = "https://mobalytics.gg/diablo-4/profile/littlegrief-ic8lxh/builds/a682e939-08d1-41b3-a90d-bb2f51bf8e0d";
    let build =
        parse_mobalytics_graphql_build(&live_fixture("build-by-id.graphql.json"), url, true)
            .expect("readable")
            .expect("a document");
    assert_eq!(build.id, "a682e939-08d1-41b3-a90d-bb2f51bf8e0d");
    assert_eq!(
        build.name,
        "*FINAL FORM* S15 NO BUTTON FROST GOD: No-Skill Sorc [PIT 120 CLEAR]"
    );
    let names: Vec<&str> = build.variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["FINAL FORM", "2.0", "Original"]);
    for variant in &build.variants {
        let affixes = variant.helm.len()
            + variant.chest.len()
            + variant.gloves.len()
            + variant.pants.len()
            + variant.boots.len()
            + variant.amulet.len()
            + variant.ring.len()
            + variant.weapon.len()
            + variant.offhand.len();
        assert!(affixes > 10, "{}: {affixes} gear affixes", variant.name);
        assert!(!variant.paragon_boards.is_empty(), "{}", variant.name);
    }
}

#[test]
fn live_slug_state_with_foreign_query_shapes_parses_the_listed_variants() {
    // The comments query (object `data`) and a `null` entry made the whole
    // state unreadable; the empty `no-variant-id` builder is no variant the
    // page lists.
    let url = "https://mobalytics.gg/diablo-4/builds/warlock-blazing-scream";
    let build =
        parse_mobalytics_graphql_build(&live_fixture("build-by-slug.diablo4state.json"), url, true)
            .expect("readable")
            .expect("a document");
    assert_eq!(build.name, "Blazing Scream Warlock Endgame Build Guide");
    let names: Vec<&str> = build.variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        names,
        ["Speed Farm - Tank", "Mid Game", "Starter", "Tower Push"]
    );
    // The page's "Speed Farm - Tank" equipment.
    let speed_farm = &build.variants[0];
    for unique in ["leorics crown", "enigma", "hand of apotheosis", "temerity"] {
        assert!(
            speed_farm.uniques.iter().any(|u| u == unique),
            "{unique} in {:?}",
            speed_farm.uniques
        );
    }
    assert_eq!(speed_farm.aspect, ["undying"]);
}

#[test]
fn live_profile_builds_state_lists_every_published_build() {
    let url = "https://mobalytics.gg/diablo-4/profile/mrronit";
    let profile =
        parse_mobalytics_profile_state(&live_fixture("profile-builds.diablo4state.json"), url)
            .expect("readable")
            .expect("profile queries present");
    assert_eq!(profile.name, "MrRonit");
    assert_eq!(profile.profile_name, "mrronit");
    assert_eq!(profile.variants.len(), 10);
    assert!(profile.variants.iter().all(|v| v
        .url
        .starts_with("https://mobalytics.gg/diablo-4/profile/mrronit/builds/")));
}

#[test]
fn an_unreadable_by_id_document_is_an_error_not_a_missing_document() {
    let body = r#"{"data":{"game":{"documents":{"userGeneratedDocumentById":{"error":null,"data":{"id":7}}}}}}"#;
    let error = parse_mobalytics_graphql_build(body, "https://mobalytics.gg/x", true)
        .expect_err("a broken document must be reported");
    assert!(error.contains("could not be read"), "{error}");
}
