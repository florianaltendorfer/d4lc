use super::*;
use crate::test_support::{d2core_source_fixture_dir, workspace_root};

#[test]
fn load_available_d2core_variants_reads_saved_build_json_without_network() {
    let variants = load_available_d2core_variants(&d2core_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0].source_build_id, "barb-d2-warning");
    assert_eq!(variants[0].source_build_name, "Warning Cases");
    assert_eq!(variants[0].variant_name.as_deref(), Some("Starter"));
    assert_eq!(variants[1].source_build_id, "barb-d2-whirlwind");
    assert_eq!(
        variants[1].variant_name.as_deref(),
        Some("Whirlwind Arsenal")
    );
}

#[test]
fn available_builds_d2core_lists_stored_builds_with_utc_date_and_variants() {
    let builds = load_available_d2core_builds(&d2core_source_fixture_dir())
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(builds.len(), 2);
    let whirlwind = builds
        .iter()
        .find(|build| build.id == "barb-d2-whirlwind")
        .expect("whirlwind fixture");
    assert_eq!(whirlwind.name, "Whirlwind Arsenal");
    // `_updateTime` 1717203600000 renders as the UTC stamp.
    assert_eq!(whirlwind.date, "2024-06-01 01:00");
    assert_eq!(whirlwind.variant_names, vec!["Whirlwind Arsenal"]);
}

#[test]
fn import_d2core_variant_matches_expected_preset_and_paragon_result() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let result = import_d2core_build_variant(
        &repo,
        &d2core_source_fixture_dir().join("whirlwind_arsenal.json"),
        Some("Whirlwind Arsenal"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_matches_d2core_expected_import_fixture(&result, "whirlwind_arsenal.json");
}

#[test]
fn import_d2core_variant_can_disable_paragon_import() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings {
        is_import_paragon_d2core_enabled: false,
        ..AppSettings::default()
    };
    let result = import_d2core_build_variant(
        &repo,
        &d2core_source_fixture_dir().join("whirlwind_arsenal.json"),
        Some("Whirlwind Arsenal"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(result.paragon_boards, Vec::<ImportedParagonBoard>::new());
    assert_eq!(result.paragon_routes, Vec::<ImportedParagonRoute>::new());
    assert!(!result
        .source_trace
        .iter()
        .any(|entry| entry == D2CORE_PARAGON_BOARD_METHOD || entry == D2CORE_PARAGON_GLYPH_METHOD));
}

#[test]
fn import_d2core_uses_selected_language_for_paragon_labels() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings {
        selected_affix_language: "deDE".to_string(),
        ..AppSettings::default()
    };
    let result = import_d2core_build_variant(
        &repo,
        &d2core_source_fixture_dir().join("whirlwind_arsenal.json"),
        Some("Whirlwind Arsenal"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(result.paragon_boards[0].name, "Start");
    assert_eq!(result.paragon_boards[0].glyph, "Marschall");
}

#[test]
fn import_d2core_variant_reports_unknown_source_ids_as_structured_warnings() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let result = import_d2core_build_variant(
        &repo,
        &d2core_source_fixture_dir().join("warning_cases.json"),
        Some("Starter"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_matches_d2core_expected_import_fixture(&result, "warning_cases.json");
    assert_eq!(result.warnings.len(), 2);
    assert!(result.warnings.iter().any(|warning| {
        warning.source_field == "gear.gloves.mods[0].name"
            && warning.source_value == "Imaginary_Affix"
            && warning.upstream_method == "AffixManager.GetAffixInfoByIdName"
    }));
    assert!(result.warnings.iter().any(|warning| {
        warning.source_field == "gear.totem.itemType"
            && warning.source_value == "Totem"
            && warning.upstream_method == "BuildsManagerD2Core.CreatePresetFromD2CoreBuild"
    }));
}

#[test]
fn import_d2core_variant_keeps_case_insensitive_aspect_prefix_and_silently_skips_unknown_unique() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let fixture = workspace_root().join("target/tmp/d2core-case-and-unique-order.json");
    let fixture_body = r#"{
  "data": {
    "_id": "barb-d2-order",
    "title": "Order Cases",
    "_createTime": 1717200000000,
    "_updateTime": 1717203600000,
    "variants": [
      {
        "name": "Starter",
        "gear": {
          "amulet": {
            "itemType": "Amulet",
            "key": "Helm_Unique_Generic_002",
            "mods": [],
            "sockets": [],
            "type": "uniqueItem"
          },
          "helm": {
            "itemType": "Helm",
            "key": "Helm_Unique_Generic_002",
            "mods": [],
            "sockets": [],
            "type": "uniqueItem"
          },
          "boots": {
            "itemType": "Boots",
            "key": "Unknown_Unique_Item",
            "mods": [],
            "sockets": [],
            "type": "uniqueItem"
          },
          "ring1": {
            "itemType": "Ring",
            "key": "legendary_aFfIx_barb_057",
            "mods": [],
            "sockets": [],
            "type": "legendary"
          },
          "ring2": {
            "itemType": "Ring",
            "key": "AFFIX_legendary_barb_057",
            "mods": [],
            "sockets": [],
            "type": "legendary"
          },
          "ring3": {
            "itemType": "Ring",
            "key": "Affix_legendary_barb_057",
            "mods": [],
            "sockets": [],
            "type": "legendary"
          }
        },
        "paragon": {}
      }
    ]
  }
}"#;
    std::fs::create_dir_all(fixture.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(&fixture, fixture_body).unwrap_or_else(|error| panic!("{error}"));

    let result = import_d2core_build_variant(&repo, &fixture, Some("Starter"), &settings)
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(result.aspects.len(), EQUIPMENT_ITEM_TYPES.len());
    assert!(result
        .aspects
        .iter()
        .all(|aspect| aspect.id == "legendary_barb_057"));
    assert_eq!(result.uniques.len(), 2);
    // Upstream v3.2.0 consolidated the v3.0.4 unique ids into semicolon
    // joined IdName lists, so the resolved unique id echoes the joined name.
    assert_eq!(
        result.uniques[0].id,
        "Helm_Unique_Generic_002;S14_Helm_Unique_Generic_002"
    );
    assert_eq!(
        result.uniques[1].id,
        "Helm_Unique_Generic_002;S14_Helm_Unique_Generic_002"
    );
    assert!(result.warnings.is_empty());
}

fn d2core_expected_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/d2core/expected")
}

fn assert_matches_d2core_expected_import_fixture(result: &ImportWorkflowResult, file_name: &str) {
    let mut actual = serde_json::to_value(result).unwrap_or_else(|error| panic!("{error}"));
    actual["source_url_or_file"] =
        serde_json::Value::String(format!("tests/fixtures/imports/d2core/source/{file_name}"));
    if std::env::var_os("D4LC_WRITE_IMPORT_FIXTURES").is_some() {
        std::fs::create_dir_all(d2core_expected_fixture_dir())
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            d2core_expected_fixture_dir().join(file_name),
            serde_json::to_string_pretty(&actual).unwrap_or_else(|error| panic!("{error}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(d2core_expected_fixture_dir().join(file_name))
            .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(actual, expected);
}
