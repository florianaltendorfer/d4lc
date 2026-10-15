use super::*;
use crate::test_support::{canonical_affix_id, maxroll_source_fixture_dir, workspace_root};

#[test]
fn available_builds_maxroll_lists_stored_builds_with_envelope_date() {
    let load = load_available_maxroll_builds(&maxroll_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));
    let builds = &load.builds;

    assert!(builds.len() >= 2);
    let charm = builds
        .iter()
        .find(|build| build.id == "maxroll-charm-seal")
        .expect("charm fixture");
    assert_eq!(charm.name, "Charm Seal Source IDs");
    assert_eq!(charm.date, "2026-06-30T15:00:00Z");
    assert!(
        charm.variant_names.iter().any(|name| name == "Charm Seal"),
        "profile names are the variant rows"
    );
}

#[test]
fn load_available_maxroll_builds_skips_malformed_file_with_structured_warning() {
    // One malformed cached download must not lose the whole stored
    // builds list. Upstream keeps the app usable the same way:
    // `LoadAvailableMaxrollBuilds` wraps its enumeration in a
    // try/catch that logs the bad file.
    let dir = std::env::temp_dir().join("d4lc-maxroll-list-tolerance");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        maxroll_source_fixture_dir().join("whirlwind_speedfarm.json"),
        dir.join("whirlwind_speedfarm.json"),
    )
    .unwrap();
    // The nested planner payload is not JSON.
    std::fs::write(
        dir.join("hidaul0h.json"),
        r#"{"data":"not json","date":"2026-09-01T00:00:00Z","id":"hidaul0h","name":"Broken Cached Build"}"#,
    )
    .unwrap();

    let load = load_available_maxroll_builds(&dir).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(load.builds.len(), 1, "only the valid build loads");
    assert_eq!(load.builds[0].id, "barb-whirlwind-speedfarm");
    assert_eq!(load.warnings.len(), 1, "the malformed file warns once");
    let warning = &load.warnings[0];
    assert_eq!(warning.provider, MAXROLL_PROVIDER);
    assert_eq!(warning.source_field, "hidaul0h.json");
    assert!(
        warning.message.contains("hidaul0h.json") && warning.message.contains("data payload"),
        "warning carries the file and the parse error, got: {}",
        warning.message
    );
    assert_eq!(
        warning.upstream_method, MAXROLL_LOAD_AVAILABLE_METHOD,
        "the skip surfaces under the upstream list-load method"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_available_maxroll_builds_imports_paragon_step_data_without_glyph() {
    // Live planner envelope `7vu83u0t` (Apocalypse Endgame Warlock) omits
    // `glyph` on some paragon step-data entries. Upstream deserializes
    // `MaxrollBuildDataParagonStepData.Glyph` with a `string.Empty`
    // initializer, so the build imports with an empty glyph; the Rust
    // mirror must not drop the whole envelope.
    let dir = std::env::temp_dir().join("d4lc-maxroll-missing-glyph");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source_path = dir.join("7vu83u0t.json");
    std::fs::write(
        &source_path,
        r#"{"data":"{\"profiles\":[{\"name\":\"Endgame\",\"items\":{},\"paragon\":{\"steps\":[{\"name\":\"Starter\",\"data\":[{\"id\":\"Paragon_Warlock_00\",\"nodes\":{\"105\":1},\"rotation\":0},{\"id\":\"Paragon_Warlock_04\",\"nodes\":{},\"rotation\":1}]}]}}]}","date":"2026-09-28T00:16:59Z","id":"7vu83u0t","name":"Apocalypse Endgame Warlock"}"#,
    )
    .unwrap();

    let load = load_available_maxroll_builds(&dir).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        load.builds.len(),
        1,
        "a served envelope without paragon `glyph` must still import"
    );
    assert!(load.warnings.is_empty(), "no warning for a served envelope");
    assert_eq!(load.builds[0].id, "7vu83u0t");
    assert_eq!(load.builds[0].name, "Apocalypse Endgame Warlock");
    assert_eq!(load.builds[0].variant_names, vec!["Endgame".to_string()]);

    // The single-profile import itself resolves the glyphless board with
    // the upstream empty-string fallback instead of failing.
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let result = import_maxroll_build_profile(
        &repo,
        &source_path,
        Some("Endgame"),
        &AppSettings::default(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.paragon_boards.len(), 2);
    assert_eq!(result.paragon_boards[1].rotation, "90°");
    assert!(
        result.paragon_boards[1].glyph.is_empty(),
        "a missing paragon glyph imports as the upstream empty string, got: {}",
        result.paragon_boards[1].glyph
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_available_maxroll_variants_reads_saved_build_json_without_network() {
    let variants = load_available_maxroll_variants(&maxroll_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(variants.len(), 5);
    assert_eq!(variants[0].source_build_id, "barb-hota-endgame");
    assert_eq!(variants[0].source_build_name, "Hammer Endgame");
    assert_eq!(variants[0].variant_name.as_deref(), Some("Endgame"));
    assert_eq!(variants[1].source_build_id, "barb-hota-endgame");
    assert_eq!(variants[1].variant_name.as_deref(), Some("Speedfarm"));
    assert_eq!(variants[2].source_build_id, "barb-whirlwind-speedfarm");
    assert_eq!(variants[2].source_build_name, "Whirlwind Speedfarm");
    assert_eq!(variants[2].variant_name.as_deref(), Some("Speedfarm"));
    assert_eq!(variants[3].source_build_id, "maxroll-charm-seal");
    assert_eq!(variants[3].source_build_name, "Charm Seal Source IDs");
    assert_eq!(variants[3].variant_name.as_deref(), Some("Charm Seal"));
    assert_eq!(variants[4].source_build_id, "sorc-arc-lash");
    assert_eq!(variants[4].variant_name.as_deref(), Some("Starter"));
}

#[test]
fn import_maxroll_variant_matches_expected_preset_and_paragon_result() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let result = import_maxroll_build_profile(
        &repo,
        &maxroll_source_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_matches_expected_import_fixture(&result, "whirlwind_endgame.json");
}

#[test]
fn import_maxroll_variant_reports_unknown_implicit_affix_snos_as_structured_warnings() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let result = import_maxroll_build_profile(
        &repo,
        &maxroll_source_fixture_dir().join("whirlwind_unknown_implicit.json"),
        Some("Starter"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_matches_expected_import_fixture(&result, "whirlwind_unknown_implicit.json");
}

#[test]
fn import_maxroll_variant_treats_rune_socket_prefix_case_insensitively() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let fixture = workspace_root().join("target/tmp/maxroll-case-insensitive-rune.json");
    let fixture_body = r#"{
  "data": "{\"profiles\":[{\"name\":\"Endgame\",\"items\":{\"4\":100},\"paragon\":{\"steps\":[]}}],\"items\":{\"100\":{\"id\":\"HLM_base05\",\"explicits\":[],\"implicits\":[],\"tempered\":[],\"aspects\":[],\"sockets\":[\"rUnE_Condition_HitHealthierEnemy\"]}}}",
  "date": "2026-06-01",
  "id": "case-rune",
  "name": "Case Rune"
}"#;
    std::fs::create_dir_all(fixture.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(&fixture, fixture_body).unwrap_or_else(|error| panic!("{error}"));

    let result = import_maxroll_build_profile(&repo, &fixture, Some("Endgame"), &settings)
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(result.runes.len(), 1);
    assert_eq!(result.runes[0].id, "Item_rUnE_Condition_HitHealthierEnemy");
}

#[test]
fn import_maxroll_variant_maps_charm_and_seal_slots_and_skips_them_silently() {
    assert_eq!(maxroll_item_type_from_slot(20), Some("horadricseal"));
    for slot in 21..=26 {
        assert_eq!(maxroll_item_type_from_slot(slot), Some("charm"));
    }

    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let result = import_maxroll_build_profile(
        &repo,
        &maxroll_source_fixture_dir().join("whirlwind_charm_seal.json"),
        Some("Charm Seal"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_matches_expected_import_fixture(&result, "whirlwind_charm_seal.json");

    // Charm (21, 26) and HoradricSeal (20) slots are skipped silently:
    // no import warning, no affixes imported from their items.
    assert!(result.warnings.is_empty());
    assert!(!result.affix_preset.affixes.is_empty());
    assert!(result
        .affix_preset
        .affixes
        .iter()
        .all(|affix| affix.item_type == "helm"));
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].item_type, "helm");
    // The regular helm slot still imports its affixes with prisms.
    assert!(result.affix_preset.affixes.iter().any(|affix| {
        affix.affix_id == canonical_affix_id("AttackSpeed")
            && affix.tuning_prisms == vec!["TuningStone_1".to_string()]
    }));
}

#[test]
fn import_maxroll_uses_selected_language_for_paragon_labels() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings {
        selected_affix_language: "deDE".to_string(),
        ..AppSettings::default()
    };
    let result = import_maxroll_build_profile(
        &repo,
        &maxroll_source_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(result.paragon_boards[0].glyph, "Verzauberer");
}

fn assert_matches_expected_import_fixture(result: &ImportWorkflowResult, file_name: &str) {
    let actual = normalized_maxroll_import_fixture_value(result, file_name);
    if std::env::var_os("D4LC_WRITE_IMPORT_FIXTURES").is_some() {
        std::fs::create_dir_all(maxroll_expected_fixture_dir())
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            maxroll_expected_fixture_dir().join(file_name),
            serde_json::to_string_pretty(&actual).unwrap_or_else(|error| panic!("{error}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(maxroll_expected_fixture_dir().join(file_name))
            .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(actual, expected);
}

fn normalized_maxroll_import_fixture_value(
    result: &ImportWorkflowResult,
    file_name: &str,
) -> serde_json::Value {
    let mut actual = serde_json::to_value(result).unwrap_or_else(|error| panic!("{error}"));
    actual["source_url_or_file"] =
        serde_json::Value::String(format!("tests/fixtures/imports/maxroll/source/{file_name}"));
    actual
}

fn maxroll_expected_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/maxroll/expected")
}
