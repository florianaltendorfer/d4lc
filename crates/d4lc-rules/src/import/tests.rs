use super::*;
use crate::import::d2core::import_d2core_build_variant;
use crate::import::d4builds::import_d4builds_build_variant;
use crate::import::infinity_builds::import_infinity_builds_share_html;
use crate::import::maxroll::import_maxroll_build_profile;
use crate::import::mobalytics::{
    export_mobalytics_build_cache, import_mobalytics_build_variant, MobalyticsAffixEntry,
    MobalyticsBuildFile, MobalyticsVariantFile,
};
use crate::test_support::{
    canonical_affix_id, d2core_source_fixture_dir, d4builds_fixture_dir,
    infinity_builds_share_fixture_dir, maxroll_source_fixture_dir, mobalytics_source_fixture_dir,
    workspace_root, write_overlay_debug_json,
};

#[test]
fn available_builds_unix_ms_stamp_renders_utc_or_empty() {
    assert_eq!(unix_ms_to_utc_timestamp(1717203600000), "2024-06-01 01:00");
    assert_eq!(unix_ms_to_utc_timestamp(0), "");
    assert_eq!(unix_ms_to_utc_timestamp(-1), "");
}

#[test]
fn import_providers_attach_tuning_prisms_from_imported_affix_data() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();

    let d2core = import_d2core_build_variant(
        &repo,
        &d2core_source_fixture_dir().join("whirlwind_arsenal.json"),
        Some("Whirlwind Arsenal"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let d4builds = import_d4builds_build_variant(
        &repo,
        &d4builds_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let maxroll = import_maxroll_build_profile(
        &repo,
        &maxroll_source_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mobalytics = import_mobalytics_build_variant(
        &repo,
        &mobalytics_source_fixture_dir().join("build_slugified_rotation.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    // Upstream attaches the imported affix row `TuningPrisms` to every
    // converted ItemAffix (D2Core/D4Builds/Mobalytics/Maxroll).
    for result in [&d2core, &d4builds, &maxroll, &mobalytics] {
        let attack_speed = result
            .affix_preset
            .affixes
            .iter()
            .find(|affix| affix.affix_id == canonical_affix_id("AttackSpeed"))
            .unwrap_or_else(|| panic!("{} should import AttackSpeed", result.provider));
        assert_eq!(
            attack_speed.tuning_prisms,
            vec!["TuningStone_1".to_string()]
        );
    }

    // Movement Speed prisms come from the affix row as well (D4Builds).
    let movement_speed = d4builds
        .affix_preset
        .affixes
        .iter()
        .find(|affix| affix.affix_id == canonical_affix_id("Movement_Speed"))
        .unwrap_or_else(|| panic!("D4Builds should import Movement_Speed"));
    assert_eq!(
        movement_speed.tuning_prisms,
        vec!["TuningStone_4".to_string()]
    );
}

#[test]
fn import_lookups_resolve_one_affix_to_one_canonical_id() {
    // The same affix reached through a Maxroll SNO, a
    // D2Core `IdNameList` name and a D4Builds/Mobalytics fuzzy label
    // resolves to the identical canonical identity of the pinned catalog
    // state, so the evaluation's exact id comparison can match it.
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let canonical = canonical_affix_id("AttackSpeed");
    assert_ne!(
        canonical, "AttackSpeed",
        "the composite IdName is the identity, not its first alias"
    );

    let maxroll =
        MaxrollImportLookupTables::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
    let by_sno = maxroll
        .affix
        .match_candidate_by_sno("1341731")
        .unwrap_or_else(|| panic!("Maxroll SNO 1341731 must resolve"));
    assert_eq!(by_sno.id, canonical);

    let d2core = load_id_name_lookup_index(&repo, "enUS", "Affixes", "DescriptionClean")
        .unwrap_or_else(|error| panic!("{error}"));
    let by_alias = d2core
        .match_id_name("AttackSpeed")
        .unwrap_or_else(|| panic!("D2Core alias AttackSpeed must resolve"));
    assert_eq!(by_alias.id, canonical);

    let labels = ImportLookupTables::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
    let by_label = labels
        .affix
        .match_candidate("Attack Speed")
        .unwrap_or_else(|| panic!("D4Builds label Attack Speed must resolve"));
    assert_eq!(by_label.id, canonical);
}

/// The seven `DefaultColor*` settings with every class distinct, so a
/// conversion that ignores the settings cannot pass by accident.
fn classified_color_settings() -> AppSettings {
    AppSettings {
        default_colors: d4lc_config::DefaultColors {
            greater: "#111111".to_string(),
            implicit: "#222222".to_string(),
            normal: "#333333".to_string(),
            tempered: "#444444".to_string(),
            aspects: "#555555".to_string(),
            uniques: "#666666".to_string(),
            runes: "#777777".to_string(),
        },
        ..AppSettings::default()
    }
}

#[test]
fn import_conversions_use_the_configured_default_colors() {
    // Every provider conversion classifies its entries through the
    // configured `DefaultColor*` settings like its upstream counterpart
    // (`BuildsManagerD4Builds.cs:548-558`, `BuildsManagerD2Core.cs:403-412`,
    // `BuildsManagerMaxroll.cs:330,360`, `BuildsManagerInfinityBuilds.cs:
    // 647-650`, `BuildsManagerMobalytics.cs:680-683`); fixed import colors
    // would store conditions the user never configured.
    let repo = AssetRepository::new(workspace_root());
    let settings = classified_color_settings();
    let colors = ImportColors::from(&settings);
    let upstream_default = UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR;
    assert_ne!(
        colors.normal, upstream_default,
        "the palette must differ from the upstream ItemAffix default, \
             otherwise 'keeps the default' assertions prove nothing"
    );
    assert_ne!(colors.normal, colors.greater);

    // D2Core: greater ? greater : normal for affixes
    // (`BuildsManagerD2Core.cs:409`), uniques from the uniques color; the
    // runes and aspects upstream leaves uncolored keep the default.
    let d2core = import_d2core_build_variant(
        &repo,
        &d2core_source_fixture_dir().join("whirlwind_arsenal.json"),
        None,
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(d2core
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.is_greater));
    assert!(d2core
        .affix_preset
        .affixes
        .iter()
        .any(|affix| !affix.is_greater));
    for affix in &d2core.affix_preset.affixes {
        let expected = if affix.is_greater {
            &colors.greater
        } else {
            &colors.normal
        };
        assert_eq!(&affix.color, expected, "D2Core affix {}", affix.affix_id);
    }
    assert!(
        d2core
            .uniques
            .iter()
            .all(|unique| unique.color == colors.uniques),
        "D2Core uniques take the configured uniques color"
    );
    assert!(d2core
        .runes
        .iter()
        .all(|rune| rune.color == upstream_default));
    assert!(d2core
        .aspects
        .iter()
        .all(|aspect| aspect.color == upstream_default));

    // D4Builds: tempered, then implicit, else normal for affixes
    // (`BuildsManagerD4Builds.cs:548-550`), plus the aspects / runes /
    // uniques colors.
    let d4builds = import_d4builds_build_variant(
        &repo,
        &d4builds_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    for affix in &d4builds.affix_preset.affixes {
        let expected = if affix.is_tempered {
            &colors.tempered
        } else if affix.is_implicit {
            &colors.implicit
        } else {
            &colors.normal
        };
        assert_eq!(&affix.color, expected, "D4Builds affix {}", affix.affix_id);
    }
    assert!(d4builds
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.is_tempered));
    assert!(d4builds
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.is_implicit));
    assert!(d4builds
        .aspects
        .iter()
        .all(|aspect| aspect.color == colors.aspects));
    assert!(d4builds.runes.iter().all(|rune| rune.color == colors.runes));
    assert!(d4builds
        .uniques
        .iter()
        .all(|unique| unique.color == colors.uniques));

    // Maxroll: explicits greater/normal (:330), uniques' implicits
    // implicit (:214,262,287), tempered (:360), aspects (:393-402);
    // runes keep the upstream default file's unassigned color (:159).
    let maxroll = import_maxroll_build_profile(
        &repo,
        &maxroll_source_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    for affix in &maxroll.affix_preset.affixes {
        let expected = if affix.is_tempered {
            &colors.tempered
        } else if affix.is_implicit {
            &colors.implicit
        } else if affix.is_greater {
            &colors.greater
        } else {
            &colors.normal
        };
        assert_eq!(&affix.color, expected, "Maxroll affix {}", affix.affix_id);
    }
    assert!(maxroll
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.is_tempered && affix.color == colors.tempered));
    assert!(maxroll
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.is_implicit && affix.color == colors.implicit));
    assert!(maxroll
        .affix_preset
        .affixes
        .iter()
        .any(|affix| affix.is_greater && affix.color == colors.greater));
    assert!(maxroll
        .aspects
        .iter()
        .all(|aspect| aspect.color == colors.aspects));
    assert!(maxroll
        .uniques
        .iter()
        .all(|unique| unique.color == colors.uniques));
    assert!(maxroll
        .runes
        .iter()
        .all(|rune| rune.color == upstream_default));

    // InfinityBuilds: implicit, then greater, then tempered, else normal
    // (`BuildsManagerInfinityBuilds.cs:647-650`) plus the three class
    // colors; the conversion runs at share-import time and the cache
    // carries the result.
    let html =
        std::fs::read_to_string(infinity_builds_share_fixture_dir().join("whirlwind_endgame.html"))
            .unwrap_or_else(|error| panic!("{error}"));
    let infinity = import_infinity_builds_share_html(
        &repo,
        &html,
        "https://infinitybuilds.gg/build/x",
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let variant = &infinity.build.variants[0];
    for affix in &variant.affix_preset.affixes {
        let expected = colors.by_implicit_greater_tempered(
            affix.is_implicit,
            affix.is_greater,
            affix.is_tempered,
        );
        assert_eq!(
            affix.color, expected,
            "InfinityBuilds affix {}",
            affix.affix_id
        );
    }
    assert_eq!(variant.affix_preset.affixes.len(), 6);
    assert!(variant
        .item_aspects
        .iter()
        .all(|aspect| aspect.color == colors.aspects));
    assert!(variant
        .item_runes
        .iter()
        .all(|rune| rune.color == colors.runes));
    assert!(variant
        .item_uniques
        .iter()
        .all(|unique| unique.color == colors.uniques));

    // Mobalytics: the same class order (`BuildsManagerMobalytics.cs:
    // 680-683`) on the stored entity import path.
    let entity = MobalyticsBuildFile {
        date: "2026-09-01T10:00:00Z".to_string(),
        id: "moba-import-colors".to_string(),
        name: "Import Colors".to_string(),
        url: "https://mobalytics.gg/diablo-4/builds/import-colors".to_string(),
        variants: vec![MobalyticsVariantFile {
            name: "Endgame".to_string(),
            gloves: vec![
                MobalyticsAffixEntry {
                    affix_text: "greater-attack-speed".to_string(),
                    affix_text_list: vec!["greater".to_string()],
                    is_greater: true,
                    ..MobalyticsAffixEntry::default()
                },
                MobalyticsAffixEntry {
                    affix_text: "implicit-attack-speed".to_string(),
                    affix_text_list: vec!["implicit".to_string()],
                    is_implicit: true,
                    ..MobalyticsAffixEntry::default()
                },
                MobalyticsAffixEntry {
                    affix_text: "tempered-attack-speed".to_string(),
                    affix_text_list: vec!["tempered".to_string()],
                    is_tempered: true,
                    ..MobalyticsAffixEntry::default()
                },
            ],
            aspect: vec!["Bold Chieftain's Aspect".to_string()],
            uniques: vec!["harlequin crest".to_string()],
            runes: vec!["Yul".to_string()],
            ..MobalyticsVariantFile::default()
        }],
    };
    let dir = std::env::temp_dir().join("d4lc-rules-mobalytics-import-colors");
    let _ = std::fs::remove_dir_all(&dir);
    let path =
        export_mobalytics_build_cache(&entity, &dir).unwrap_or_else(|error| panic!("{error}"));
    let mobalytics = import_mobalytics_build_variant(&repo, &path, Some("Endgame"), &settings)
        .unwrap_or_else(|error| panic!("{error}"));
    let classes: std::collections::BTreeMap<&str, String> = mobalytics
        .affix_preset
        .affixes
        .iter()
        .map(|affix| {
            let class = if affix.is_tempered {
                "tempered"
            } else if affix.is_implicit {
                "implicit"
            } else if affix.is_greater {
                "greater"
            } else {
                "normal"
            };
            (class, affix.color.clone())
        })
        .collect();
    assert_eq!(
        classes,
        std::collections::BTreeMap::from([
            ("greater", colors.greater.clone()),
            ("implicit", colors.implicit.clone()),
            ("tempered", colors.tempered.clone()),
        ]),
        "every Mobalytics class takes its configured color"
    );
    assert!(mobalytics
        .aspects
        .iter()
        .all(|aspect| aspect.color == colors.aspects));
    assert!(mobalytics
        .uniques
        .iter()
        .all(|unique| unique.color == colors.uniques));
    assert!(mobalytics
        .runes
        .iter()
        .all(|rune| rune.color == colors.runes));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_providers_preserve_paragon_routes_for_overlay_state() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let settings = AppSettings::default();
    let d2core = import_d2core_build_variant(
        &repo,
        &d2core_source_fixture_dir().join("whirlwind_arsenal.json"),
        Some("Whirlwind Arsenal"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let d4builds = import_d4builds_build_variant(
        &repo,
        &d4builds_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let maxroll = import_maxroll_build_profile(
        &repo,
        &maxroll_source_fixture_dir().join("whirlwind_endgame.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mobalytics = import_mobalytics_build_variant(
        &repo,
        &mobalytics_source_fixture_dir().join("build_slugified_rotation.json"),
        Some("Endgame"),
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    for result in [&d2core, &d4builds, &maxroll, &mobalytics] {
        assert!(!result.paragon_routes.is_empty());
        assert_eq!(
            flatten_paragon_routes(&result.paragon_routes),
            result.paragon_boards
        );
    }
    assert!(d2core
        .paragon_routes
        .iter()
        .all(|route| route.step_name.is_none()));
    assert!(d4builds
        .paragon_routes
        .iter()
        .all(|route| route.step_name.is_none()));
    assert!(mobalytics
        .paragon_routes
        .iter()
        .all(|route| route.step_name.is_none()));
    assert!(maxroll
        .paragon_routes
        .iter()
        .any(|route| route.step_name.is_some()));

    write_paragon_routes_debug_artifact(
        "paragon-provider-routes.json",
        &[d2core, d4builds, maxroll, mobalytics],
    );
}

#[test]
fn localized_data_state_reports_missing_language_structurally() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let error = match ImportLocalizedData::load(&repo, "xxYY") {
        Ok(_) => panic!("validation should fail"),
        Err(error) => error,
    };

    assert_eq!(error.language, "xxYY");
    assert_eq!(error.component, LocalizedDataComponent::Affixes);
    assert!(error
        .path
        .ends_with("assets/original/data/Affixes.xxYY.json"));
}

#[test]
fn localized_data_state_switches_language_and_reloads_item_types_and_sigils() {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    let mut localized =
        ImportLocalizedData::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

    localized
        .switch_language(&repo, "deDE")
        .unwrap_or_else(|error| panic!("{error}"));

    let state = localized.state();

    assert_eq!(localized.language(), "deDE");
    assert_eq!(state.language, "deDE");
    assert!(state.item_type_count > 0);
    assert!(state.affix_count > 0);
    assert!(state.aspect_count > 0);
    assert!(state.unique_count > 0);
    assert!(state.rune_count > 0);
    assert!(state.sigil_count > 0);
}

fn write_paragon_routes_debug_artifact(file_name: &str, results: &[ImportWorkflowResult]) {
    let json = serde_json::Value::Array(
        results
            .iter()
            .map(|result| {
                serde_json::json!({
                    "provider": result.provider,
                    "build": result.source_build_name,
                    "paragon_board_count": result.paragon_boards.len(),
                    "paragon_route_count": result.paragon_routes.len(),
                    "paragon_routes": result.paragon_routes.iter().map(|route| {
                        serde_json::json!({
                            "step_name": route.step_name,
                            "board_names": route.boards.iter().map(|board| board.name.clone()).collect::<Vec<_>>(),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>(),
    );
    write_overlay_debug_json(file_name, &json);
}

#[test]
fn cache_filename_gate_accepts_provider_ids_and_refuses_path_tricks() {
    for id in [
        "1N1m",
        "ancients-barbarian-endgame",
        "23ae9cbb-933e-4a88-999c-2241654cc8e2",
        "druid_pulverize.v2~x",
    ] {
        assert_eq!(ensure_cache_filename_safe(id), Ok(()), "{id}");
    }
    for id in ["", ".", "..", "../x", "a/b", "a\\b", "a b", "x\0y", "é"] {
        let error = ensure_cache_filename_safe(id).expect_err(id);
        assert!(
            error.contains("cannot be used as a cache file name"),
            "{id}: {error}"
        );
    }
}
