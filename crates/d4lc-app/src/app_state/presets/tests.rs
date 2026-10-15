use super::*;
use crate::app_state::test_support::sample_paragon_routes;
use d4lc_core::Rect;

// -----------------------------------------------------------------------
// Affix-preset persistence tests
// -----------------------------------------------------------------------

fn preset_persistence_root(test_name: &str) -> crate::test_root::TestRoot {
    crate::test_root::TestRoot::new(&format!("app-{test_name}"))
}

/// An upstream-shaped `Config/AffixPresets-v2.json` fixture with
/// mixed-kind entries plus a Settings.json selecting "Boss".
fn write_preset_fixture(root: &std::path::Path) {
    let config_dir = root.join("Config");
    std::fs::create_dir_all(&config_dir).expect("create Config dir");
    std::fs::write(
        config_dir.join("AffixPresets-v2.json"),
        r##"[
  {
"Name": "Boss",
"ItemAffixes": [
  { "Id": "AttackSpeed", "Type": "Amulet", "Color": { "A": 255, "R": 34, "G": 197, "B": 94 }, "IsGreater": true, "IsImplicit": false, "IsTempered": true }
],
"ItemAspects": [
  { "Id": "RapidAspect", "Type": "Gloves", "Color": { "A": 255, "R": 0, "G": 255, "B": 255 }, "IsGreater": false, "IsImplicit": false, "IsTempered": false }
],
"ItemSigils": [],
"ItemUniques": [],
"ItemRunes": []
  }
]"##,
    )
    .expect("write preset fixture");
    let settings = d4lc_config::AppSettings {
        selected_affix_preset: "Boss".to_string(),
        ..d4lc_config::AppSettings::default()
    };
    d4lc_config::save_app_settings(root, &settings).expect("write settings fixture");
}

#[test]
fn hydrate_affix_presets_restores_presets_and_selection() {
    let root = preset_persistence_root("hydrate_affix_presets_restores");
    write_preset_fixture(&root);

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);

    assert_eq!(state.affix_preset_editor.presets.len(), 1);
    let preset = &state.affix_preset_editor.presets[0];
    assert_eq!(preset.name, "Boss");
    // The mixed-kind editor list restores both the affix and the
    // aspect entry with their kinds and flags intact.
    assert_eq!(preset.entries.len(), 2);
    let affix = &preset.entries[0];
    assert_eq!(affix.kind, PresetEntryKind::Affix);
    // The fixture's shortened `AttackSpeed` is the first `IdNameList`
    // entry of a composite catalog row: hydrate migrates it onto the
    // canonical `IdName` the evaluation compares, and the editor adopts the
    // migrated id.
    assert_eq!(affix.id, canonical_affix_id("AttackSpeed"));
    assert_ne!(affix.id, "AttackSpeed");
    assert_eq!(affix.item_type.as_deref(), Some("Amulet"));
    assert!(affix.is_greater && affix.is_tempered && !affix.is_implicit);
    assert_eq!(affix.color, "#22C55E");
    let aspect = &preset.entries[1];
    assert_eq!(aspect.kind, PresetEntryKind::Aspect);
    // `RapidAspect` is not a key of the pinned Aspects catalog: an unknown
    // id stays exactly as stored, never guessed.
    assert_eq!(aspect.id, "RapidAspect");

    // The migration is visible: one finding names the rewritten id and its
    // preset, one names the unknown id and the catalog it is missing from.
    let messages: Vec<&str> = state
        .logging
        .messages
        .iter()
        .map(|message| message.message.as_str())
        .collect();
    assert!(
        messages.iter().any(|message| {
            message.contains("preset id migrated: 'AttackSpeed' ->")
                && message.contains("in preset 'Boss'")
        }),
        "the rewritten id needs a visible finding: {messages:?}"
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("preset id not migrated: 'RapidAspect'")
                && message.contains("pinned Aspects catalog for 'enUS'")
        }),
        "the unknown id needs a visible finding naming the id and reason: {messages:?}"
    );

    // The persisted selection is restored over the first-preset
    // default and the shell panel projection follows the editor.
    assert_eq!(
        state.affix_preset_editor.selected_preset_name.as_deref(),
        Some("Boss")
    );
    assert_eq!(state.shell.affix_presets.preset_count, 1);
    assert_eq!(
        state.shell.affix_presets.selected_preset.as_deref(),
        Some("Boss")
    );

    // The migrated file is persisted through the transactional write: the
    // stored id is the canonical one and the revision was applied.
    let reloaded = d4lc_config::load_affix_presets(&root).expect("reload migrated presets");
    assert_eq!(
        reloaded[0].item_affixes[0].id,
        canonical_affix_id("AttackSpeed")
    );
    assert_eq!(reloaded[0].item_aspects[0].id, "RapidAspect");
    // The pre-migration bytes are kept once so the rewrite stays reversible.
    let backup = root.join("Config/AffixPresets-v2.pre-id-migration.bak.json");
    let backup_text = std::fs::read_to_string(&backup).expect("pre-migration backup kept");
    assert!(backup_text.contains("\"AttackSpeed\""));
    assert!(
        state.applied_config_revision.is_some(),
        "a persisted migration advances the applied revision"
    );
    assert_eq!(
        state
            .applied_config_revision
            .as_ref()
            .map(|revision| &revision.files),
        Some(&vec![d4lc_config::AFFIX_PRESETS_PATH.to_string()])
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_affix_presets_without_file_keeps_empty_state() {
    let root = preset_persistence_root("hydrate_affix_presets_without_file");
    let mut state = AppState::default_startup();
    state
        .affix_preset_editor
        .create_preset("ShouldVanish")
        .unwrap();

    state.hydrate_affix_presets_from_config(&root);

    assert!(
        state.affix_preset_editor.presets.is_empty(),
        "a fresh install starts with the empty state"
    );
    assert_eq!(state.affix_preset_editor.selected_preset_name, None);

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_affix_presets_with_unreadable_file_logs_and_keeps_state() {
    let root = preset_persistence_root("hydrate_affix_presets_unreadable");
    std::fs::create_dir_all(root.join("Config")).expect("create Config dir");
    std::fs::write(root.join("Config").join("AffixPresets-v2.json"), "{ broken")
        .expect("write broken fixture");

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);

    // The editor stays empty (nothing was loaded) and the failure
    // is surfaced on the Logging screen instead of being swallowed.
    assert!(state.affix_preset_editor.presets.is_empty());
    let logged = state
        .logging
        .messages
        .iter()
        .any(|message| message.message.contains("affix presets load failed"));
    assert!(logged, "an unreadable presets file must be logged");

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_rewrites_the_preset_file_sorted_like_upstream() {
    // AffixManager.cs:920-926: load sorts the list by name and saves.
    let root = preset_persistence_root("hydrate_rewrites_sorted");
    std::fs::create_dir_all(root.join("Config")).expect("create Config dir");
    std::fs::write(
        root.join("Config").join("AffixPresets-v2.json"),
        r##"[
  { "Name": "Zulu", "ItemAffixes": [], "ItemAspects": [], "ItemSigils": [], "ItemUniques": [], "ItemRunes": [] },
  { "Name": "Alpha", "ItemAffixes": [], "ItemAspects": [], "ItemSigils": [], "ItemUniques": [], "ItemRunes": [] }
]"##,
    )
    .expect("write preset fixture");

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);

    let names: Vec<&str> = state
        .affix_preset_editor
        .presets
        .iter()
        .map(|preset| preset.name.as_str())
        .collect();
    assert_eq!(names, ["Alpha", "Zulu"]);
    let stored: Vec<String> = d4lc_config::load_affix_presets(&root)
        .expect("rewritten presets load")
        .into_iter()
        .map(|preset| preset.name)
        .collect();
    assert_eq!(stored, ["Alpha", "Zulu"]);
    let text = std::fs::read_to_string(root.join("Config").join("AffixPresets-v2.json"))
        .expect("read rewritten presets");
    assert!(
        text.find("\"Alpha\"") < text.find("\"Zulu\""),
        "the file itself is rewritten in name order: {text}"
    );
    assert!(state.applied_config_revision.is_some());

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_keeps_paragon_board_indices_and_leaves_an_up_to_date_file_alone() {
    // A D2Core preset as upstream persists it: boards carry their `Index`
    // (BuildsManagerD2Core.cs:462), sorted by it.
    let root = preset_persistence_root("hydrate_keeps_board_indices");
    let board = |name: &str, index: i32| d4lc_config::PresetParagonBoard {
        name: name.to_string(),
        glyph: "Marshal".to_string(),
        rotation: "0\u{b0}".to_string(),
        active_nodes: vec![0, 21],
        index,
    };
    let preset = d4lc_config::AffixPreset {
        name: "D2Core".to_string(),
        item_affixes: Vec::new(),
        item_aspects: Vec::new(),
        item_sigils: Vec::new(),
        item_uniques: Vec::new(),
        item_runes: Vec::new(),
        paragon_boards_list: vec![vec![board("Start", 3), board("Blood Rage", 7)]],
    };
    d4lc_config::save_affix_presets(&root, &[preset]).expect("write the upstream-shaped file");
    let file = root.join("Config").join("AffixPresets-v2.json");
    let before = std::fs::read(&file).expect("read presets");
    let modified = std::fs::metadata(&file)
        .and_then(|meta| meta.modified())
        .expect("mtime");

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);

    assert_eq!(std::fs::read(&file).expect("read presets"), before);
    assert_eq!(
        std::fs::metadata(&file)
            .and_then(|meta| meta.modified())
            .expect("mtime"),
        modified,
        "an up-to-date file is not rewritten"
    );

    // A later save from the hydrated state keeps the indices.
    state
        .persist_affix_presets_to_config(&root)
        .expect("save after hydrate");
    let reloaded = d4lc_config::load_affix_presets(&root).expect("reload");
    let indices: Vec<i32> = reloaded[0].paragon_boards_list[0]
        .iter()
        .map(|board| board.index)
        .collect();
    assert_eq!(indices, [3, 7]);

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_creates_the_empty_preset_file_on_a_fresh_install() {
    // `SaveAffixPresets` after a load with no file (AffixManager.cs:913-926).
    let root = preset_persistence_root("hydrate_creates_empty_file");

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);

    let text = std::fs::read_to_string(root.join("Config").join("AffixPresets-v2.json"))
        .expect("a fresh install writes the preset file");
    assert_eq!(text.trim(), "[]");

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_leaves_an_unreadable_preset_file_untouched() {
    let root = preset_persistence_root("hydrate_unreadable_untouched");
    std::fs::create_dir_all(root.join("Config")).expect("create Config dir");
    let file = root.join("Config").join("AffixPresets-v2.json");
    std::fs::write(&file, "{ broken").expect("write broken fixture");

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);

    assert_eq!(
        std::fs::read_to_string(&file).expect("read broken fixture"),
        "{ broken"
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_ignores_selection_for_unknown_preset() {
    let root = preset_persistence_root("hydrate_ignores_selection_for_unknown");
    write_preset_fixture(&root);
    let settings = d4lc_config::AppSettings {
        selected_affix_preset: "Ghost".to_string(),
        ..d4lc_config::AppSettings::default()
    };
    d4lc_config::save_app_settings(&root, &settings).expect("write settings fixture");

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);

    // Upstream restores the settings selection only when a preset
    // with that name exists; otherwise the editor default applies.
    assert_eq!(
        state.affix_preset_editor.selected_preset_name.as_deref(),
        Some("Boss")
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn persist_affix_presets_round_trips_through_reload() {
    let root = preset_persistence_root("persist_affix_presets_round_trips");
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Speed").unwrap();
    state
        .affix_preset_editor
        .edit_entry(
            "Speed",
            d4lc_gui::PresetEntry {
                kind: PresetEntryKind::Affix,
                id: "AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                item_type: Some("Amulet".to_string()),
                color: "#22C55E".to_string(),
                is_any_type: false,
                is_greater: true,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms: Vec::new(),
            },
        )
        .unwrap();

    state
        .persist_affix_presets_to_config(&root)
        .expect("persist affix presets");
    state.persist_selected_affix_preset_to_config(&root);

    // Simulated restart: a fresh state hydrates the persisted shape.
    let mut reloaded = AppState::default_startup();
    reloaded.hydrate_affix_presets_from_config(&root);

    assert_eq!(reloaded.affix_preset_editor.presets.len(), 1);
    let preset = &reloaded.affix_preset_editor.presets[0];
    assert_eq!(preset.name, "Speed");
    assert_eq!(preset.entries.len(), 1);
    let entry = &preset.entries[0];
    assert_eq!(entry.kind, PresetEntryKind::Affix);
    // The in-session editor kept the id it was given; hydrate migrates the
    // stored shortened id onto the canonical catalog identity, so the
    // restarted editor (and the evaluation that follows) works on the
    // canonical id.
    assert_eq!(entry.id, canonical_affix_id("AttackSpeed"));
    assert_eq!(entry.item_type.as_deref(), Some("Amulet"));
    assert!(entry.is_greater && entry.is_tempered && !entry.is_implicit);
    assert_eq!(
        reloaded.affix_preset_editor.selected_preset_name.as_deref(),
        Some("Speed")
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn persist_affix_presets_refuses_to_replace_an_unreadable_file() {
    let root = preset_persistence_root("persist_affix_presets_skips_unreadable");
    std::fs::create_dir_all(root.join("Config")).expect("create Config dir");
    let file = root.join("Config").join("AffixPresets-v2.json");
    std::fs::write(&file, "{ broken").expect("write broken fixture");

    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Speed").unwrap();
    let error = state
        .persist_affix_presets_to_config(&root)
        .expect_err("an unreadable file must refuse the save");
    assert!(
        matches!(error, d4lc_config::ConfigError::Malformed { .. }),
        "unexpected error: {error}"
    );
    assert!(
        state.applied_config_revision.is_none(),
        "a refused write must not advance the applied revision"
    );

    // User state we could not read is never silently overwritten.
    let contents = std::fs::read_to_string(&file).expect("read presets file");
    assert_eq!(contents, "{ broken");
    let logged = state
        .logging
        .messages
        .iter()
        .any(|message| message.message.contains("affix presets save failed"));
    assert!(logged, "the refused save must be logged");

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn persist_presets_and_selection_failure_before_replacement_keeps_the_previous_revision() {
    // Phase 1: the change fails at its first replacement, so
    // nothing is partial, the previous applied revision is unchanged
    // and no interrupted record is left behind.
    let root = preset_persistence_root("persist_transaction_before_replacement");
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Alpha").unwrap();
    state
        .persist_affix_presets_to_config(&root)
        .expect("seed preset file");
    state.persist_selected_affix_preset_to_config(&root);
    let previous_revision = state
        .applied_config_revision
        .clone()
        .expect("the seed writes apply a revision");
    let settings_before =
        std::fs::read_to_string(root.join(d4lc_config::SETTINGS_PATH)).expect("read settings");

    // The preset file becomes a directory: the commit fails before it
    // replaces anything.
    let presets_target = root.join(d4lc_config::AFFIX_PRESETS_PATH);
    std::fs::remove_file(&presets_target).expect("remove presets file");
    std::fs::create_dir_all(&presets_target).expect("directory in place of the presets file");

    state.affix_preset_editor.create_preset("Beta").unwrap();
    let error = state
        .persist_affix_presets_and_selection_to_config(&root)
        .expect_err("the obstructed write must fail");
    assert!(
        matches!(error, d4lc_config::ConfigError::Io { .. }),
        "unexpected: {error}"
    );
    assert_eq!(
        state.applied_config_revision.as_ref(),
        Some(&previous_revision),
        "a failed write reports no new applied revision"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(d4lc_config::SETTINGS_PATH)).expect("read settings"),
        settings_before,
        "the settings file is untouched"
    );
    assert!(
        d4lc_config::interrupted_transactions(&root)
            .expect("detect")
            .is_empty(),
        "a change that replaced nothing leaves no interrupted record"
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn persist_presets_and_selection_failure_between_replacements_is_detected_and_recoverable() {
    // Phase 2: the preset file is replaced and the settings
    // replacement fails. The caller sees the error, the previous
    // applied revision stays, the partial change is reported instead
    // of being adopted, and the explicit rollback restores the
    // previous preset file.
    let root = preset_persistence_root("persist_transaction_between_replacements");
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Alpha").unwrap();
    state
        .persist_affix_presets_to_config(&root)
        .expect("seed preset file");
    state.persist_selected_affix_preset_to_config(&root);
    let previous_revision = state
        .applied_config_revision
        .clone()
        .expect("the seed writes apply a revision");

    let settings_target = root.join(d4lc_config::SETTINGS_PATH);
    std::fs::remove_file(&settings_target).expect("remove settings file");
    std::fs::create_dir_all(&settings_target).expect("directory in place of settings");

    state.affix_preset_editor.create_preset("Beta").unwrap();
    let error = state
        .persist_affix_presets_and_selection_to_config(&root)
        .expect_err("the interrupted write must fail");
    assert!(
        matches!(error, d4lc_config::ConfigError::CommitInterrupted { .. }),
        "unexpected: {error}"
    );
    assert_eq!(
        state.applied_config_revision.as_ref(),
        Some(&previous_revision),
        "the previous applied revision is unchanged on failure"
    );

    let records = d4lc_config::interrupted_transactions(&root).expect("detect");
    assert_eq!(records.len(), 1, "the partial change stays visible");
    assert_eq!(
        records[0].files(),
        vec![d4lc_config::AFFIX_PRESETS_PATH, d4lc_config::SETTINGS_PATH],
        "the record names exactly the changed files"
    );
    // A follow-up write to an affected file is refused until the
    // change is resolved: no silent takeover of the partial state.
    let refused = state
        .persist_affix_presets_to_config(&root)
        .expect_err("an unresolved change blocks a rewrite");
    assert!(
        matches!(refused, d4lc_config::ConfigError::PendingRecovery { .. }),
        "unexpected: {refused}"
    );

    state
        .recover_interrupted_config_transaction(&root, d4lc_config::Recovery::Rollback)
        .expect("rollback should succeed");
    assert!(
        d4lc_config::interrupted_transactions(&root)
            .expect("list")
            .is_empty(),
        "recovery clears the record"
    );
    let restored = d4lc_config::load_affix_presets(&root).expect("load restored presets");
    assert_eq!(
        restored
            .iter()
            .map(|preset| preset.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Alpha"],
        "rollback restores the previously persisted preset file"
    );
    assert!(
        state
            .logging
            .messages
            .iter()
            .any(|message| message.message.contains("config change recovered")),
        "the explicit recovery is logged"
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_reports_an_interrupted_change_instead_of_adopting_it() {
    // The startup seam detects the interrupted record, logs the
    // explicit recovery options, and never reports an applied
    // revision of its own.
    let root = preset_persistence_root("hydrate_interrupted_change");
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Alpha").unwrap();
    state
        .persist_affix_presets_to_config(&root)
        .expect("seed preset file");
    state.persist_selected_affix_preset_to_config(&root);
    let settings_target = root.join(d4lc_config::SETTINGS_PATH);
    std::fs::remove_file(&settings_target).expect("remove settings file");
    std::fs::create_dir_all(&settings_target).expect("directory in place of settings");
    state.affix_preset_editor.create_preset("Beta").unwrap();
    state
        .persist_affix_presets_and_selection_to_config(&root)
        .expect_err("the interrupted write must fail");

    let mut fresh = AppState::default_startup();
    fresh.hydrate_affix_presets_from_config(&root);
    let reported = fresh
        .logging
        .messages
        .iter()
        .find(|message| message.message.contains("config change interrupted"))
        .expect("the interrupted change is reported at startup");
    // The line names the exact operator action and the config root.
    let id = d4lc_config::interrupted_transactions(&root).expect("detect")[0]
        .id()
        .to_string();
    for expected in [
        format!("d4lc-debug config-recover --complete --record {id}"),
        format!("d4lc-debug config-recover --rollback --record {id}"),
        format!("config root {}", root.display()),
        "--rollback possible".to_string(),
    ] {
        assert!(
            reported.message.contains(&expected),
            "missing {expected:?} in {:?}",
            reported.message
        );
    }
    assert!(
        fresh.applied_config_revision.is_none(),
        "hydration never reports an applied revision"
    );
    // The read-only view is what is on disk: the partial preset file
    // loads, and the record stays visible until it is recovered.
    assert_eq!(
        d4lc_config::interrupted_transactions(&root)
            .expect("detect")
            .len(),
        1
    );
    fresh
        .recover_interrupted_config_transaction(&root, d4lc_config::Recovery::Rollback)
        .expect("rollback should succeed");

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_discards_a_change_that_never_replaced_a_file() {
    // Killed after the record was published and before the first
    // replacement: nothing was applied or reported as saved, so startup
    // discards the staged copies visibly and saving works again.
    let root = preset_persistence_root("hydrate_untouched_change");
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Alpha").unwrap();
    state
        .persist_affix_presets_to_config(&root)
        .expect("seed preset file");
    state.persist_selected_affix_preset_to_config(&root);
    let presets_before =
        std::fs::read_to_string(root.join(d4lc_config::AFFIX_PRESETS_PATH)).expect("presets");
    let settings_before =
        std::fs::read_to_string(root.join(d4lc_config::SETTINGS_PATH)).expect("settings");
    // Dropping a prepared, uncommitted transaction leaves exactly what a
    // kill before the first rename leaves on disk.
    drop(
        d4lc_config::persistence::prepare_transaction(
            &root,
            &[
                d4lc_config::persistence::FileWrite {
                    relative_path: d4lc_config::AFFIX_PRESETS_PATH,
                    contents: "[]",
                },
                d4lc_config::persistence::FileWrite {
                    relative_path: d4lc_config::SETTINGS_PATH,
                    contents: "{}",
                },
            ],
        )
        .expect("prepare"),
    );
    assert!(d4lc_config::interrupted_transactions(&root).expect("detect")[0].is_untouched());

    let mut fresh = AppState::default_startup();
    fresh.hydrate_affix_presets_from_config(&root);
    assert!(
        fresh.logging.messages.iter().any(|message| message
            .message
            .contains("discarded the staged, never-applied change")),
        "the discard is logged: {:?}",
        fresh.logging.messages
    );
    assert!(d4lc_config::interrupted_transactions(&root)
        .expect("list")
        .is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join(d4lc_config::AFFIX_PRESETS_PATH)).expect("presets"),
        presets_before
    );
    assert_eq!(
        std::fs::read_to_string(root.join(d4lc_config::SETTINGS_PATH)).expect("settings"),
        settings_before
    );
    fresh.affix_preset_editor.create_preset("Beta").unwrap();
    fresh
        .persist_affix_presets_and_selection_to_config(&root)
        .expect("saving works again");

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn hydrate_never_acts_on_a_record_whose_staged_path_is_the_target() {
    // A malformed journal entry naming the real settings file as its own
    // "staged copy", with a valid backup, must never make startup delete
    // the settings: the record is unreadable and only reported.
    fn fnv(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |digest, byte| {
            (digest ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }
    let root = preset_persistence_root("hydrate_aliased_record");
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Alpha").unwrap();
    state
        .persist_affix_presets_to_config(&root)
        .expect("seed preset file");
    state.persist_selected_affix_preset_to_config(&root);
    let settings_path = root.join(d4lc_config::SETTINGS_PATH);
    let settings = std::fs::read(&settings_path).expect("settings");
    let journal = root.join(d4lc_config::persistence::JOURNAL_DIR);
    std::fs::create_dir_all(&journal).expect("journal");
    let backup = format!("{}/backup-7-11-0", d4lc_config::persistence::JOURNAL_DIR);
    std::fs::write(root.join(&backup), &settings).expect("valid backup");
    let record = journal.join("txn-7-11.json");
    std::fs::write(
        &record,
        serde_json::json!({"state": "prepared", "files": [{
            "relative": d4lc_config::SETTINGS_PATH, "temp": d4lc_config::SETTINGS_PATH,
            "backup": backup, "existed_before": true,
            "digest": fnv(b"new"), "backup_digest": fnv(&settings),
        }]})
        .to_string(),
    )
    .expect("record");

    let mut fresh = AppState::default_startup();
    fresh.hydrate_affix_presets_from_config(&root);
    assert_eq!(
        std::fs::read(&settings_path).expect("the settings file still exists"),
        settings
    );
    assert!(record.is_file(), "the record is never deleted");
    assert!(
        fresh
            .logging
            .messages
            .iter()
            .any(|message| message.message.contains("config change interrupted")),
        "the record is reported: {:?}",
        fresh.logging.messages
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn persist_presets_and_selection_refuses_a_newer_settings_schema() {
    // A settings file from a future build is never partially
    // overwritten; the preset file and the settings file stay
    // byte-identical, no revision is reported and no record remains.
    let root = preset_persistence_root("persist_transaction_newer_schema");
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Alpha").unwrap();
    state
        .persist_affix_presets_to_config(&root)
        .expect("seed preset file");
    let previous_revision = state.applied_config_revision.clone();
    let future = r##"{ "schema_version": 3, "SelectedSystemPreset": "1080p_SMF" }"##;
    std::fs::write(root.join(d4lc_config::SETTINGS_PATH), future)
        .expect("write future settings fixture");
    let presets_before =
        std::fs::read_to_string(root.join(d4lc_config::AFFIX_PRESETS_PATH)).expect("read presets");

    state.affix_preset_editor.create_preset("Beta").unwrap();
    let error = state
        .persist_affix_presets_and_selection_to_config(&root)
        .expect_err("a newer settings schema must be refused");
    assert!(
        matches!(
            error,
            d4lc_config::ConfigError::NewerSchema { version: 3, .. }
        ),
        "unexpected: {error}"
    );
    assert_eq!(
        state.applied_config_revision, previous_revision,
        "a refused write does not advance the applied revision"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(d4lc_config::SETTINGS_PATH)).expect("read settings"),
        future,
        "the future file survives unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(d4lc_config::AFFIX_PRESETS_PATH)).expect("read presets"),
        presets_before,
        "the preset file is not replaced when the change is refused"
    );
    assert!(d4lc_config::interrupted_transactions(&root)
        .expect("list")
        .is_empty());

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn any_type_and_prisms_survive_hydrate_edit_and_persist() {
    // End to end at the app layer: the file's `IsAnyType` and
    // `TuningPrisms` reach the editor, the rules projection and the
    // next save instead of being derived away and dropped.
    let root = preset_persistence_root("any_type_prisms_roundtrip");
    std::fs::create_dir_all(root.join("Config")).expect("create Config dir");
    std::fs::write(
        root.join(d4lc_config::AFFIX_PRESETS_PATH),
        r#"[
  {
"Name": "Boss",
"ItemAffixes": [
  { "Id": "AttackSpeed", "Type": "Boots", "Color": { "A": 255, "R": 1, "G": 2, "B": 3 }, "IsAnyType": true, "IsGreater": false, "IsImplicit": false, "IsTempered": true, "TuningPrisms": [ "Prism of Control" ] }
]
  }
]"#,
    )
    .expect("write preset fixture");

    let mut state = AppState::default_startup();
    state.hydrate_affix_presets_from_config(&root);
    let entry = &state.affix_preset_editor.presets[0].entries[0];
    assert!(
        entry.is_any_type,
        "the any-type flag reaches the editor model"
    );
    assert_eq!(entry.item_type.as_deref(), Some("Boots"));
    assert_eq!(entry.tuning_prisms, vec!["Prism of Control".to_string()]);

    // The rules projection keeps the flag independent of the type and
    // carries the prism list.
    let requirement = &state.affix_preset_editor.to_rule_presets().affix_presets[0].affixes[0];
    assert!(requirement.is_any_type && requirement.item_type == "Boots");
    assert_eq!(
        requirement.tuning_prisms,
        vec!["Prism of Control".to_string()]
    );

    state
        .persist_affix_presets_to_config(&root)
        .expect("persist the hydrated presets");
    let reloaded = d4lc_config::load_affix_presets(&root).expect("reload persisted presets");
    let persisted = &reloaded[0].item_affixes[0];
    assert!(persisted.is_any_type);
    assert_eq!(persisted.item_type, "Boots");
    assert_eq!(
        persisted.tuning_prisms,
        vec!["Prism of Control".to_string()]
    );

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn editor_config_preset_mapping_preserves_mixed_kinds() {
    let mut editor = d4lc_gui::AffixPresetEditorPreset {
        name: "Mixed".to_string(),
        entries: vec![
            d4lc_gui::PresetEntry {
                kind: PresetEntryKind::Affix,
                id: "AttackSpeed".to_string(),
                label: Some("Attack Speed".to_string()),
                item_type: Some("Amulet".to_string()),
                color: "#22C55E".to_string(),
                is_any_type: false,
                is_greater: true,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms: Vec::new(),
            },
            d4lc_gui::PresetEntry::non_affix(
                PresetEntryKind::Aspect,
                "Rapid",
                Some("Gloves"),
                "Cyan",
            ),
            d4lc_gui::PresetEntry::non_affix(PresetEntryKind::Unique, "Frostburn", None, "Orange"),
            d4lc_gui::PresetEntry::non_affix(PresetEntryKind::Rune, "Xol", None, "Yellow"),
            d4lc_gui::PresetEntry::non_affix(PresetEntryKind::Sigil, "SigilOfStuff", None, "Red"),
        ],
    };
    // A user-facing label on a non-affix entry exercises the same
    // persisted extension field as the affix label above.
    editor.entries[1].label = Some("Rapid Aspect".to_string());

    let config = config_preset_from_editor(&editor);
    assert_eq!(config.name, "Mixed");
    assert_eq!(config.item_affixes.len(), 1);
    assert_eq!(config.item_aspects.len(), 1);
    assert_eq!(config.item_uniques.len(), 1);
    assert_eq!(config.item_runes.len(), 1);
    assert_eq!(config.item_sigils.len(), 1);
    assert_eq!(config.item_affixes[0].id, "AttackSpeed");
    assert_eq!(
        config.item_affixes[0].color,
        d4lc_config::PresetColor::from_rgb(34, 197, 94)
    );

    let back = editor_preset_from_config(&config);
    assert_eq!(back.name, editor.name);
    // Kinds, ids, constraints and the d4lc-local labels round-trip —
    // labels persist through the d4lc-extension `Label` field, which
    // upstream's deserialization skips on load. An empty upstream
    // Type means "any item type" (None). The upstream file is
    // kind-grouped, so a reload restores entries grouped by kind
    // (in list order) rather than the pre-save insertion order —
    // compare as kind+id keyed entries instead of positions.
    assert_eq!(back.entries.len(), editor.entries.len());
    fn key(entry: &d4lc_gui::PresetEntry) -> (u8, String, Option<String>) {
        (
            match entry.kind {
                PresetEntryKind::Affix => 0,
                PresetEntryKind::Aspect => 1,
                PresetEntryKind::Sigil => 2,
                PresetEntryKind::Unique => 3,
                PresetEntryKind::Rune => 4,
            },
            entry.id.clone(),
            entry.item_type.clone(),
        )
    }
    let mut original_entries: Vec<&d4lc_gui::PresetEntry> = editor.entries.iter().collect();
    let mut restored_entries: Vec<&d4lc_gui::PresetEntry> = back.entries.iter().collect();
    original_entries.sort_by_key(|entry| key(entry));
    restored_entries.sort_by_key(|entry| key(entry));
    for (restored, original) in restored_entries.iter().zip(&original_entries) {
        assert_eq!(restored.kind, original.kind);
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.item_type, original.item_type);
        assert_eq!(restored.is_greater, original.is_greater);
        assert_eq!(restored.is_implicit, original.is_implicit);
        assert_eq!(restored.is_tempered, original.is_tempered);
        assert_eq!(restored.label, original.label, "labels are persisted");
    }
    assert_eq!(back.entries[2].item_type, None, "None stays any-type");
}

#[test]
fn renamed_preset_keeps_paragon_across_persist_and_reload() {
    // Upstream stores paragon boards INSIDE the preset
    // (`AffixPreset.ParagonBoardsList`), so a rename must not orphan
    // them: the boards follow the preset to its new name on disk.
    let root = preset_persistence_root("renamed_preset_keeps_paragon");
    let mut state = AppState::default_startup();
    state
        .affix_preset_editor
        .create_preset("Whirlwind")
        .unwrap();
    state.set_imported_paragon_routes("Whirlwind", sample_paragon_routes());

    state
        .affix_preset_editor
        .rename_preset("Whirlwind", "Whirlwind Alt")
        .unwrap();
    state
        .persist_affix_presets_to_config(&root)
        .expect("persist affix presets");

    let mut reloaded = AppState::default_startup();
    reloaded.hydrate_affix_presets_from_config(&root);
    assert!(
        reloaded
            .affix_preset_editor
            .paragon_routes_for("Whirlwind")
            .is_none(),
        "the old name must not keep the boards"
    );
    let routes = reloaded
        .affix_preset_editor
        .paragon_routes_for("Whirlwind Alt")
        .unwrap_or_else(|| panic!("the renamed preset keeps its boards across restarts"));
    assert_eq!(routes[0].boards[0].name, "Start");
    assert_eq!(routes[1].boards[0].name, "Hemorrhage");

    // And removing the preset deletes its persisted boards with it.
    reloaded
        .affix_preset_editor
        .remove_preset("Whirlwind Alt")
        .unwrap();
    reloaded
        .persist_affix_presets_to_config(&root)
        .expect("persist reloaded affix presets");
    let mut reloaded_after_remove = AppState::default_startup();
    reloaded_after_remove.hydrate_affix_presets_from_config(&root);
    assert!(reloaded_after_remove
        .affix_preset_editor
        .paragon_boards_by_preset
        .is_empty());

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn imported_paragon_survives_persist_hydrate_and_projects_real_boards() {
    let root = preset_persistence_root("imported_paragon_survives_persist_hydrate");
    let mut state = AppState::default_startup();
    state.shell.settings.paragon_mode_active = true;
    // The exact seam `confirm_import_preset` uses: editor import
    // (which selects the preset) plus the paragon store update.
    let preset = d4lc_gui::AffixPresetEditorPreset {
        name: "Whirlwind".to_string(),
        entries: Vec::new(),
    };
    state.affix_preset_editor.import_preset(preset, Vec::new());
    state.set_imported_paragon_routes("Whirlwind", sample_paragon_routes());

    state
        .persist_affix_presets_to_config(&root)
        .expect("persist affix presets");
    state.persist_selected_affix_preset_to_config(&root);

    // The preset file carries the boards in the upstream shape.
    let file = std::fs::read_to_string(root.join("Config").join("AffixPresets-v2.json"))
        .expect("read persisted presets");
    assert!(file.contains("\"ParagonBoardsList\""));
    assert!(file.contains("\"Glyph\": \"Enchanter\""));
    assert!(file.contains("\"Rotation\": \"90°\""));

    // Simulated restart: a fresh state hydrates the persisted
    // boards and the projection feeds them to the frozen rules
    // surface with the selected preset's name on the build panel.
    let mut reloaded = AppState::default_startup();
    reloaded.shell.settings.paragon_mode_active = true;
    reloaded.overlay_window_geometry = Some(Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    });
    reloaded.hydrate_affix_presets_from_config(&root);
    reloaded.rebuild_overlay_descriptor_state();

    let paragon = reloaded
        .overlay_descriptor_state
        .as_ref()
        .and_then(|projected| projected.paragon.clone())
        .expect("hydrated paragon must project when mode is active");
    assert_eq!(paragon.state.build_name, "Whirlwind");
    assert_eq!(paragon.state.routes.len(), 2);
    assert_eq!(paragon.state.routes[1].boards[0].name, "Hemorrhage");
    assert_eq!(paragon.state.routes[1].boards[0].glyph, "Enchanter");
    assert_eq!(paragon.state.routes[1].boards[0].rotation, "90°");
    assert_eq!(paragon.state.routes[1].boards[0].active_nodes, vec![36, 57]);
    assert_eq!(paragon.state.current_board_name.as_deref(), Some("Start"));

    std::fs::remove_dir_all(&root).expect("clean persistence test root");
}

#[test]
fn reimporting_a_preset_without_paragon_drops_its_stored_boards() {
    let mut state = AppState::default_startup();
    state.affix_preset_editor.create_preset("Speed").unwrap();
    state.set_imported_paragon_routes("Speed", sample_paragon_routes());
    assert!(state
        .affix_preset_editor
        .paragon_boards_by_preset
        .iter()
        .any(|(name, _)| name == "Speed"));

    // A later import of the same preset without paragon must not
    // keep the earlier boards.
    state.set_imported_paragon_routes("Speed", Vec::new());
    assert!(!state
        .affix_preset_editor
        .paragon_boards_by_preset
        .iter()
        .any(|(name, _)| name == "Speed"));
    // And the import supersedes any download-only boards.
    state.downloaded_paragon_routes = Some(sample_paragon_routes());
    state.set_imported_paragon_routes("Other", Vec::new());
    assert!(state.downloaded_paragon_routes.is_none());
}

/// The migration decision per stored id: an already canonical id stays, an
/// unambiguous shortened key is rewritten, and an unknown key is reported
/// The oracle is a synthetic catalog state here; the pinned one is
/// the oracle of the hydrate tests below.
#[test]
fn preset_id_resolution_migrates_only_unambiguous_keys() {
    let catalog = d4lc_assets::catalog::IdentityCatalog::from_items(
        &[
            serde_json::json!({
                "IdName": "AttackSpeed;Tempered_Generic_AttackSpeed_Tier1",
                "IdNameList": ["AttackSpeed", "Tempered_Generic_AttackSpeed_Tier1"],
                "IdSnoList": ["1341731", "1510423"]
            }),
            serde_json::json!({
                "IdName": "Life;S04_Life",
                "IdNameList": ["Life", "S04_Life"],
                "IdSnoList": ["100", "101"]
            }),
        ],
        "DescriptionClean",
    );
    let canonical = "AttackSpeed;Tempered_Generic_AttackSpeed_Tier1";

    assert_eq!(
        resolve_preset_entry_id(&catalog, canonical),
        PresetEntryIdResolution::Canonical,
        "an id that already is the canonical identity stays untouched"
    );
    assert_eq!(
        resolve_preset_entry_id(&catalog, "Life"),
        PresetEntryIdResolution::Rewrite("Life;S04_Life"),
        "an unambiguous shortened key is migrated to its canonical identity"
    );
    assert_eq!(
        resolve_preset_entry_id(&catalog, "1510423"),
        PresetEntryIdResolution::Rewrite(canonical),
        "an unambiguous SNO key resolves onto its canonical identity"
    );
    assert_eq!(
        resolve_preset_entry_id(&catalog, "NoSuchAffixKey"),
        PresetEntryIdResolution::Unknown
    );
}

/// A key that carries several identities must not be guessed. The pinned
/// catalog state has no ambiguous key (asserted by the catalog tests), so
/// the ambiguity path is proven on a synthetic state with a shared alias.
#[test]
fn preset_id_resolution_reports_an_ambiguous_key_with_its_candidates() {
    let catalog = d4lc_assets::catalog::IdentityCatalog::from_items(
        &[
            serde_json::json!({
                "IdName": "Alpha;Alpha_Shared",
                "IdNameList": ["Alpha_Shared", "Shared_Key"],
                "IdSnoList": ["1"]
            }),
            serde_json::json!({
                "IdName": "Beta;Beta_Shared",
                "IdNameList": ["Beta_Shared", "Shared_Key"],
                "IdSnoList": ["2"]
            }),
        ],
        "DescriptionClean",
    );

    assert_eq!(
        resolve_preset_entry_id(&catalog, "Alpha_Shared"),
        PresetEntryIdResolution::Rewrite("Alpha;Alpha_Shared"),
        "a key only one row carries still resolves"
    );
    assert_eq!(
        resolve_preset_entry_id(&catalog, "Shared_Key"),
        PresetEntryIdResolution::Ambiguous(vec!["Alpha;Alpha_Shared", "Beta;Beta_Shared"]),
        "a shared key lists every candidate and never rewrites"
    );
}

/// The canonical `IdName` of one pinned enUS catalog key — the oracle
/// `hydrate_affix_presets_from_config` resolves the stored preset ids
/// against (`IdentityCatalog::affixes`).
fn canonical_affix_id(key: &str) -> String {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root");
    d4lc_assets::catalog::IdentityCatalog::affixes(&d4lc_assets::AssetRepository::new(root), "enUS")
        .unwrap_or_else(|error| panic!("load the pinned enUS affix catalog: {error}"))
        .resolve(key)
        .resolved()
        .unwrap_or_else(|| panic!("'{key}' must resolve onto one canonical identity"))
        .id_name
        .clone()
}
