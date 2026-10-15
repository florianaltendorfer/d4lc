use super::*;

// ── affix-surface: affix/filter deep-interaction suite ──────────────

/// Build a preset entry for the affix-surface seeds.
pub(super) fn preset_entry(
    kind: d4lc_gui::PresetEntryKind,
    id: &str,
    label: Option<&str>,
    item_type: Option<&str>,
    color: &str,
) -> d4lc_gui::PresetEntry {
    d4lc_gui::PresetEntry {
        kind,
        id: id.to_string(),
        label: label.map(str::to_string),
        item_type: item_type.map(str::to_string),
        color: color.to_string(),
        is_any_type: item_type.is_none(),
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    }
}

/// Replace the harness editor state with the deterministic
/// three-preset seed ("Alpha" mixed-kind and selected, "Beta"
/// empty, "Gamma" one affix) and re-project the root models.
///
/// Non-Affix entries have no UI create path (the SetAffix dialog
/// persists Affix-kind entries; the import path is network-gated),
/// so mixed-kind coverage seeds `AppState` directly through
/// `HARNESS_APP_STATE` — the same justification as the harness
/// seed in `wired_test_app`. Every interaction under test is
/// still driven by real clicks and keys.
pub(super) fn seed_affix_presets() {
    let presets = vec![
        d4lc_gui::AffixPresetEditorPreset {
            name: "Alpha".into(),
            entries: vec![
                d4lc_gui::PresetEntry {
                    is_tempered: true,
                    ..preset_entry(
                        d4lc_gui::PresetEntryKind::Affix,
                        "AttackSpeed",
                        Some("Attack Speed"),
                        Some("Amulet"),
                        "#22C55E",
                    )
                },
                preset_entry(
                    d4lc_gui::PresetEntryKind::Affix,
                    "Life",
                    None,
                    Some("ring"),
                    "#EF4444",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "BloodForBlood",
                    Some("Blood For Blood"),
                    None,
                    "#3B82F6",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Sigil,
                    "Torture",
                    None,
                    None,
                    "#F59E0B",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Unique,
                    "RingOfVayar",
                    None,
                    None,
                    "#111111",
                ),
                preset_entry(
                    d4lc_gui::PresetEntryKind::Rune,
                    "ElixirOfSorrow",
                    None,
                    None,
                    "#8B5CF6",
                ),
            ],
        },
        d4lc_gui::AffixPresetEditorPreset {
            name: "Beta".into(),
            entries: Vec::new(),
        },
        d4lc_gui::AffixPresetEditorPreset {
            name: "Gamma".into(),
            entries: vec![preset_entry(
                d4lc_gui::PresetEntryKind::Affix,
                "MaxLife",
                Some("Max Life"),
                Some("Helm"),
                "#0EA5E9",
            )],
        },
    ];
    on_ui(move |_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(presets);
        state.affix_preset_editor.selected_preset_name = Some("Alpha".into());
        state.shell.affix_presets.preset_count = state.affix_preset_editor.presets.len();
        state.shell.affix_presets.selected_preset =
            state.affix_preset_editor.selected_preset_name.clone();
    });
    reproject_affixes();
}

/// Re-project the current editor state onto the root — the same
/// update the wired handlers perform after preset-entry mutations.
/// This is the setup half of a seed, never a substitute for an
/// interaction under test.
pub(super) fn reproject_affixes() {
    on_ui(|app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        crate::actions::affix::project_affix_entries(app, &mut state);
        let names: Vec<slint::SharedString> = state
            .affix_preset_editor
            .presets
            .iter()
            .map(|preset| slint::SharedString::from(preset.name.as_str()))
            .collect();
        app.set_preset_names(Rc::new(slint::VecModel::from(names)).into());
        app.set_selected_preset(slint::SharedString::from(
            state
                .affix_preset_editor
                .selected_preset_name
                .as_deref()
                .unwrap_or(""),
        ));
    });
}

/// Number of projected affix rows on the root: the whole localized
/// catalog for the data language plus the selected preset's entries
/// whose ids the catalog does not know.
pub(super) fn affix_row_count() -> usize {
    read_ui(|app| app.get_affix_entries().row_count())
}

/// Number of wanted-entry rail chips (the rail is preset-driven, so this
/// is one chip per selected-preset entry, independent of the grid filter).
fn rail_chip_count() -> usize {
    read_ui(|app| {
        app.get_affix_rail_rows()
            .iter()
            .filter(|row| !row.is_header)
            .count()
    })
}

/// Number of *wanted* affix rows in the projection (the catalog rows carry
/// the preset's wanted state).
pub(super) fn wanted_affix_count() -> usize {
    read_ui(|app| {
        app.get_affix_entries()
            .iter()
            .filter(|entry| entry.is_wanted)
            .count()
    })
}

/// Number of *wanted* rows of one family tab, addressed by the root
/// model the tab renders.
fn wanted_family_count(kind: &str) -> usize {
    let kind = kind.to_string();
    read_ui(move |app| match kind.as_str() {
        "aspect" => app
            .get_aspect_entries()
            .iter()
            .filter(|entry| entry.is_wanted)
            .count(),
        "sigil" => app
            .get_sigil_entries()
            .iter()
            .filter(|entry| entry.is_wanted)
            .count(),
        "unique" => app
            .get_unique_entries()
            .iter()
            .filter(|entry| entry.is_wanted)
            .count(),
        "rune" => app
            .get_rune_entries()
            .iter()
            .filter(|entry| entry.is_wanted)
            .count(),
        other => panic!("unknown catalog kind {other:?}"),
    })
}

/// Projected row count of one catalog tab.
fn catalog_count(kind: &str) -> usize {
    let kind = kind.to_string();
    read_ui(move |app| match kind.as_str() {
        "affix" => app.get_affix_entries().row_count(),
        "aspect" => app.get_aspect_entries().row_count(),
        "sigil" => app.get_sigil_entries().row_count(),
        "unique" => app.get_unique_entries().row_count(),
        "rune" => app.get_rune_entries().row_count(),
        other => panic!("unknown catalog kind {other:?}"),
    })
}

/// Select `option` in the AffixScreen preset ComboBox through real
/// input: a real click opens the popup, a real click on the popup
/// option (a `ListItem` addressed by its accessible label)
/// selects it. Asserts the selection landed.
pub(super) fn select_preset(client: &McpClient, root: McpHandle, option: &str) {
    click_on_ui("Preset");
    // The ComboBox popup renders as an in-window layer on the
    // testing backend; its option ListItems carry the option text
    // as accessible label.
    let option_owned = option.to_string();
    let clicked = on_ui(move |app| {
        let found = ElementHandle::find_by_accessible_label(app, &option_owned).find(|element| {
            matches!(
                element.accessible_role(),
                Some(i_slint_backend_testing::AccessibleRole::ListItem)
            )
        });
        found
            .map(|item| item.mock_single_click(slint::platform::PointerEventButton::Left))
            .is_some()
    });
    if !clicked {
        // Fallback: the popup item tree may only be reachable
        // through the backend's arena — scan it via MCP and click
        // the option through the real pointer pipeline there.
        let items = client
            .query_element_descendants(root, descendants_with_role("ListItem"), true)
            .unwrap_or_else(|error| harness_panic("query popup list items", &error));
        let item = items
            .into_iter()
            .find(|handle| {
                client
                    .get_element_properties(*handle)
                    .ok()
                    .is_some_and(|properties| {
                        str_property(&properties, "accessibleLabel") == Some(option)
                    })
            })
            .unwrap_or_else(|| {
                panic!(
                    "affix-surface: preset ComboBox option {option:?} not reachable after a \
                     real click on the ComboBox (popup did not open or options are not \
                     exposed)"
                )
            });
        client
            .click_element(item, None)
            .unwrap_or_else(|error| harness_panic("click the popup option", &error));
    }
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        option,
        "affix-surface: selecting {option:?} in the preset ComboBox must update the selection"
    );
}

/// The affix-surface affix/filter surface family: the search/filter
/// input, every entry-card family (including the fixed edit (+)
/// overlap and the kind-tagged-key addressing), config dialog
/// cycles, numeric spin boxes, toggle switches, preset CRUD
/// edges, and the multi-build/trade surfaces that reference
/// presets. Real input only; scenario-local `AppState` seeds
/// cover state with no UI create path.
pub(super) fn affix_surface_scenarios(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[affix-surface] starting affix/filter surface scenarios");
    filter_input_scenario(client, window, root);
    entry_card_families_scenario(client, window);
    config_dialog_cycles_scenario(client, window);
    numeric_spin_box_scenario(client, window);
    toggle_switch_scenario(client, window);
    preset_crud_edges_scenario(client, window, root);
    multi_build_trade_scenario(client, window);
    eprintln!("[affix-surface] affix/filter surface scenarios complete");
}

/// Catalog browsing per tab: every tab lists the full
/// localized catalog for the data language (counts cross-checked
/// against the vendored index files), the Config cell stays first, the
/// per-entry attribute badges carry the data markers, the card "+"
/// adds through the kind-aware flow and the added id becomes a wanted
/// row/card.
pub(super) fn catalog_scenarios(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[catalog] starting catalog scenarios");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    // Every tab lists the render window of the catalog plus the seeded
    // wanted rows: an entry whose id the data does not carry keeps its own
    // row (one per family kind), and the two affix ids resolve to index
    // rows beyond the window, which lead the grid. The projection windows
    // the catalog to 60 rows per tab (upstream's VirtualizingWrapPanel
    // materializes the visible viewport only).
    for (kind, tab, windowed_rows, wanted_rows) in [
        ("affix", "Affixes", 60 + 2, 2),
        ("aspect", "Aspects", 60 + 1, 1),
        ("sigil", "Sigils", 60 + 1, 1),
        ("unique", "Uniques", 60 + 1, 1),
        ("rune", "Runes", 52 + 1, 1),
    ] {
        click_control_by_label(tab);
        assert_eq!(
            catalog_count(kind),
            windowed_rows,
            "catalog: the {tab} tab must list the render window plus the \
             preset's unknown ids"
        );
        if kind == "affix" {
            assert_eq!(wanted_affix_count(), wanted_rows);
        } else {
            assert_eq!(wanted_family_count(kind), wanted_rows);
        }
        // The Config cell stays the first grid cell of the tab.
        assert_label_findable(&format!(
            "{} config",
            match kind {
                "affix" => "Affix",
                "aspect" => "Aspect",
                "sigil" => "Sigil",
                "unique" => "Unique",
                _ => "Rune",
            }
        ));
    }

    // The data side of the same cross-check: the loaded catalog carries
    // every localized enUS row (the render window bounds only what the
    // grid instantiates, not what the projection can reach).
    {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        let catalog = state.affix_catalog.as_ref().expect("loaded catalog");
        for (kind, expected) in [
            (d4lc_rules::CatalogKind::Affix, 894),
            (d4lc_rules::CatalogKind::Aspect, 523),
            (d4lc_rules::CatalogKind::Sigil, 333),
            (d4lc_rules::CatalogKind::Unique, 306),
            (d4lc_rules::CatalogKind::Rune, 52),
        ] {
            assert_eq!(
                catalog.entries(kind).len(),
                expected,
                "catalog: the enUS catalog must carry every {kind:?} row"
            );
        }
    }

    // The render window grows one page when the grid is scrolled to its
    // bottom edge (the port's replacement for upstream's
    // VirtualizingWrapPanel materializing the next viewport) and resets
    // per tab (the screen re-instantiates with the tab).
    click_control_by_label("Affixes");
    assert_eq!(
        catalog_count("affix"),
        60 + 2,
        "catalog: the affix window plus the seeded preset's two wanted rows"
    );
    let rail_rows_before = read_ui(|app| app.get_affix_rail_rows().row_count());
    let projections_before = catalog_projections();
    scroll_grid_to_bottom(client, window, "AffixScreen::affix-grid-scroll");
    assert_eq!(
        catalog_count("affix"),
        crate::app_state::DEFAULT_CATALOG_WINDOW + CATALOG_WINDOW_PAGE + 2,
        "catalog: scrolling the affix grid to its bottom edge must grow the render \
         window by one page, plus the wanted rows the window still does not carry"
    );
    // The gesture anchor: the render window grew once for the whole
    // gesture, not once per scroll event (re-projecting every model on each
    // event at the bottom edge makes scrolling stutter), and the wanted-entry rail
    // — a function of the preset alone — was not rebuilt at all.
    assert!(
        catalog_projections().saturating_sub(projections_before) <= 2,
        "catalog: one bottom-edge scroll gesture may re-project the window at most twice"
    );
    assert_eq!(
        read_ui(|app| app.get_affix_rail_rows().row_count()),
        rail_rows_before,
        "catalog: growing the render window must not rebuild the wanted-entry rail"
    );
    click_control_by_label("Aspects");
    click_control_by_label("Affixes");
    assert_eq!(
        catalog_count("affix"),
        60 + 2,
        "catalog: a tab switch must reset the render window to one page"
    );

    // The keyword filter and the class toggles re-project the
    // grids live (upstream `RefreshAffixViewFilter` from every toggle
    // setter, AffixViewModel.cs:434-456). '+#% Weapon Mastery Damage' is
    // the only row carrying that phrase and is barbarian-restricted and
    // tempering-available in the vendored data, so one filtered row
    // proves both marker kinds and the class arm.
    set_filter(client, root, "weapon mastery damage");
    assert_eq!(
        catalog_count("affix"),
        1,
        "catalog: the keyword filter must narrow the affix grid to the single \
         '+#% Weapon Mastery Damage' row (the wanted state rides on the index \
         rows, so the seeded entries leave the grid with the filter)"
    );
    assert_label_findable("Tempering available.");
    assert_label_findable("Barbarian");
    click_control_by_label("Barbarian");
    assert_eq!(
        catalog_count("affix"),
        0,
        "catalog: turning the Barbarian arm off must drop the barbarian-only row, \
         and the filter leaves nothing else in the wanted-merged grid"
    );
    click_control_by_label("Barbarian");
    assert_eq!(
        catalog_count("affix"),
        1,
        "catalog: restoring the Barbarian arm restores the row"
    );
    set_filter(client, root, "");

    // Attribute badges follow the data markers (spot-check per
    // category, upstream AffixView.xaml:1538-1576,1602-1662,1785-1788):
    // the filter brings each spot-checked card into the window
    // deterministically.
    click_control_by_label("Aspects");
    set_filter(client, root, "Ballistic");
    assert_label_findable("Codex (Whispering Pines)");
    set_filter(client, root, "Accelerating");
    assert_label_findable("Drop only");
    set_filter(client, root, "");
    click_control_by_label("Sigils");
    set_filter(client, root, "Akkhan");
    assert_label_findable("Seasonal");
    set_filter(client, root, "");
    click_control_by_label("Uniques");
    set_filter(client, root, "Accord of the Wilds");
    assert_label_findable("Druid");
    set_filter(client, root, "");
    click_control_by_label("Runes");
    assert_label_findable("Rune condition");
    assert_label_findable("Rune effect");

    // The dungeon tier badge follows the configured tier for the sigil
    // id while the tier display is on (upstream
    // `SigilInfoWanted.IsTierInfoEnabled`), and the badge is upstream's
    // tier button: a real click advances the configured tier through
    // the `S..F` list and persists `Config/DungeonTiers.json`
    // (AffixView.xaml:1769-1784, SigilInfoVM.cs:139-149).
    {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state.bump_rules_input_revision();
        state.shell.settings.dungeon_tiers_enabled = true;
        state.dungeon_tiers.insert(
            "World_DGN_Kehj_AbandonedMineworks".to_string(),
            "S".to_string(),
        );
    }
    reproject_affixes();
    click_control_by_label("Sigils");
    assert!(
        read_ui(|app| app.get_sigil_entries().iter().any(
            |entry| entry.tier_label == "S" && entry.id == "World_DGN_Kehj_AbandonedMineworks"
        )),
        "catalog: the configured dungeon tier must reach the sigil card"
    );
    assert_eq!(
        label_count("Cycle tier for Abandoned Mineworks in Kehjistan"),
        1,
        "catalog: the dungeon sigil card must carry exactly one tier button"
    );
    click_on_label("Cycle tier for Abandoned Mineworks in Kehjistan");
    assert!(
        read_ui(|app| app.get_sigil_entries().iter().any(
            |entry| entry.tier_label == "A" && entry.id == "World_DGN_Kehj_AbandonedMineworks"
        )),
        "catalog: the card tier button must advance the configured tier to the next entry \
         of the upstream S..F list"
    );
    let tier_config_root = crate::actions::settings::resolve_config_root();
    let tiers = d4lc_config::load_dungeon_tiers(&tier_config_root).unwrap_or_else(|error| {
        harness_panic(
            "catalog: dungeon-tiers readback after the card cycle",
            &error,
        )
    });
    assert_eq!(
        tiers
            .get("World_DGN_Kehj_AbandonedMineworks")
            .map(String::as_str),
        Some("A"),
        "catalog: the card tier button must persist Config/DungeonTiers.json, saw {tiers:?}"
    );

    // The catalog card "+" opens the kind-aware add flow seeded with
    // the catalog id (upstream SetAffixCommand, AffixViewModel.cs:821-839),
    // and confirming creates the preset entry. The filter brings the
    // card into the render window.
    click_control_by_label("Affixes");
    set_filter(client, root, "pickup");
    click_on_ui("Set affix: +# Pickup Radius");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "set_affix",
        "catalog: the catalog card + must open set_affix (saw visible={visible}, \
         id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_entry_id().to_string()),
        "2HStaff_Unique_AF_001_PickupRadius",
        "catalog: the catalog card + must seed the dialog with the catalog id"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_edit_index()),
        -1,
        "catalog: a catalog-only affix opens the create path"
    );
    // The upstream creation path is the dialog's per-slot Add button
    // (`AddAffixExecute`, SetAffixView.xaml:31-98): it writes the entry
    // with its slot token, which is what the rail's slot sections
    // group by.
    click_control_by_label("Amulet");
    // The filter keeps the grid on the matching rows (the new entry's row
    // is one of them and now carries the wanted state); the rail gains the
    // entry's own chip (the seeded preset has six).
    assert_eq!(
        wanted_affix_count(),
        1,
        "catalog: the per-slot add must mark the matching grid row as wanted"
    );
    assert_eq!(
        rail_chip_count(),
        7,
        "catalog: the per-slot add must add one wanted rail chip"
    );
    assert!(
        read_ui(|app| app
            .get_affix_entries()
            .iter()
            .any(|entry| entry.id == "2HStaff_Unique_AF_001_PickupRadius"
                && entry.is_wanted
                && entry.item_type == "amulet")),
        "catalog: the catalog row must carry the new wanted state and its slot"
    );
    assert_label_findable("Set color: +# Pickup Radius");
    assert_label_findable("Remove +# Pickup Radius");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close set_affix");
    set_filter(client, root, "");

    // The family tabs add through the same kind-aware flow; their
    // spot-checked cards sort first, so the default window shows them.
    for (tab, card, kind) in [
        ("Aspects", "Add entry: Accelerating", "aspect"),
        (
            "Sigils",
            "Add entry: Abandoned Mineworks in Kehjistan",
            "sigil",
        ),
        ("Uniques", "Add entry: 100,000 Steps", "unique"),
        ("Runes", "Add entry: Ahu", "rune"),
    ] {
        click_control_by_label(tab);
        click_on_ui(card);
        assert_eq!(
            wanted_family_count(kind),
            2,
            "catalog: adding from the {tab} catalog must append one wanted entry"
        );
        let label = card_label(card).to_string();
        let wanted_label = label.clone();
        let has_wanted_row = read_ui(move |app| match kind {
            "aspect" => app
                .get_aspect_entries()
                .iter()
                .any(|entry| entry.is_wanted && entry.label == wanted_label),
            "sigil" => app
                .get_sigil_entries()
                .iter()
                .any(|entry| entry.is_wanted && entry.label == wanted_label),
            "unique" => app
                .get_unique_entries()
                .iter()
                .any(|entry| entry.is_wanted && entry.label == wanted_label),
            _ => app
                .get_rune_entries()
                .iter()
                .any(|entry| entry.is_wanted && entry.label == wanted_label),
        });
        assert!(
            has_wanted_row,
            "catalog: the added {tab} row must carry the wanted state"
        );
    }

    take_evidence_screenshot(client, window, "catalog", "catalog-tabs.png");

    // Restore the tier state the tier-badge step configured, so the
    // following families start from the harness default. The card
    // cycle persisted the tier, so the restore writes the file back
    // through the same seam.
    on_ui(move |_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        state.bump_rules_input_revision();
        state.shell.settings.dungeon_tiers_enabled = false;
        state
            .dungeon_tiers
            .remove("World_DGN_Kehj_AbandonedMineworks");
        if let Err(error) = d4lc_config::save_dungeon_tiers(&tier_config_root, &state.dungeon_tiers)
        {
            harness_panic(
                "catalog: restore the dungeon tiers after the card cycle",
                &error,
            );
        }
    });
    seed_affix_presets();
    click_control_by_label("Affixes");
    eprintln!("[catalog] catalog scenarios complete");
}

/// The gear-slot rail is a bounded scroll area at every
/// window height.
///
/// At the live window height the rail's slot sections ran
/// into the window's bottom edge with the last chip sliced mid-row and no
/// visible scroll affordance, so the list read as one that runs out of the
/// window. Measured through `ElementHandle` geometry (not by rendering):
///
/// 1. short window (1150x440) — the rail's viewport ends inside the
///    window, its content is taller than that viewport (so the wheel
///    scrolls it), the wheel moves the content offset, the presets block
///    stays above the rail, and the rail's bottom-edge fade marks the
///    remaining rows and disappears at the end of the list;
/// 2. window below the column's fixed-row height (1150x340) — nothing of
///    the left column is laid out past the window bottom (the std
///    ScrollView's 50px min-height would otherwise be the column's floor).
pub(super) fn affix_rail_scroll_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[affix-rail-scroll] starting affix rail scroll scenario");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        0,
        "affix-rail-scroll: the affix screen must be the active tab"
    );

    // The scenario owns the window size and restores what it found (the
    // coverage matrix runs the suite on a tall window).
    let (width, height) = read_ui(|app| {
        let size = app.window().size();
        (size.width as f32, size.height as f32)
    });

    // ── Short window: the rail clamps and scrolls ─────────────────────
    read_ui(|app| {
        app.window()
            .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
                1150.0, 440.0,
            )));
    });
    std::thread::sleep(Duration::from_millis(250));

    let measure = || {
        read_ui(|app| {
            let window_height = app.window().size().height as f32;
            let rail = find_by_element_id(app, "AffixScreen::affix-rail-scroll");
            let content = find_by_element_id(app, "AffixScreen::affix-rail-content");
            let presets = find_by_element_id(app, "AffixScreen::affix-presets-panel");
            let fade = ElementHandle::find_by_element_id(app, "AffixScreen::affix-rail-fade")
                .next()
                .map(|fade| fade.absolute_position().y + fade.size().height);
            let rail_position = rail.absolute_position();
            let rail_size = rail.size();
            let content_position = content.absolute_position();
            (
                window_height,
                rail_position.y,
                rail_size.height,
                content_position.y,
                content.size().height,
                presets.absolute_position().y + presets.size().height,
                fade,
            )
        })
    };

    let (
        window_height,
        rail_top,
        rail_height,
        content_top,
        content_height,
        presets_bottom,
        fade_bottom,
    ) = measure();
    let rail_bottom = rail_top + rail_height;
    assert!(
        rail_bottom <= window_height,
        "affix-rail-scroll: the rail viewport must end inside the window (bottom {rail_bottom} vs \
         window {window_height})"
    );
    assert!(
        content_height > rail_height,
        "affix-rail-scroll: the rail content ({content_height}) must exceed the viewport \
         ({rail_height}) so the rail scrolls instead of overflowing"
    );
    assert!(
        presets_bottom <= rail_top,
        "affix-rail-scroll: the presets block (bottom {presets_bottom}) must stay above the rail \
         (top {rail_top})"
    );
    assert_eq!(
        fade_bottom,
        Some(rail_bottom),
        "affix-rail-scroll: the rail's bottom-edge fade must mark the rows below the fold, on the \
         viewport's own bottom edge"
    );

    // The wheel scrolls the rail's own viewport: the content offset moves
    // and the presets block stays put.
    let rail = client
        .find_elements_by_id(window, "AffixScreen::affix-rail-scroll")
        .unwrap_or_else(|error| {
            harness_panic("affix-rail-scroll: find the rail scroll view", &error)
        })
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("affix-rail-scroll: rail scroll view not found"));
    client
        .scroll_element(rail, 0.0, -120.0)
        .unwrap_or_else(|error| harness_panic("affix-rail-scroll: scroll the rail", &error));
    std::thread::sleep(Duration::from_millis(150));
    let (_, rail_top_after, rail_height_after, content_top_after, _, presets_bottom_after, _) =
        measure();
    assert!(
        content_top_after < content_top,
        "affix-rail-scroll: a wheel gesture over the rail must move its content offset \
         ({content_top} -> {content_top_after})"
    );
    assert_eq!(
        (rail_top_after, rail_height_after, presets_bottom_after),
        (rail_top, rail_height, presets_bottom),
        "affix-rail-scroll: scrolling the rail must not move the viewport or the presets block"
    );

    // Scrolled to the end of the list the affordance is gone: the fade
    // marks remaining rows only.
    client
        .scroll_element(rail, 0.0, -4000.0)
        .unwrap_or_else(|error| {
            harness_panic("affix-rail-scroll: scroll the rail to its end", &error)
        });
    std::thread::sleep(Duration::from_millis(150));
    let (_, _, rail_height_end, content_top_end, content_height_end, _, fade_end) = measure();
    assert_eq!(
        fade_end, None,
        "affix-rail-scroll: the fade must disappear at the end of the rail's list"
    );
    assert!(
        content_top_end + content_height_end <= rail_top + rail_height_end + 1.0,
        "affix-rail-scroll: at the end of the list the content bottom ({}) must reach the viewport \
         bottom ({})",
        content_top_end + content_height_end,
        rail_top + rail_height_end
    );

    // ── Window below the column's fixed rows: nothing overflows ──────
    read_ui(|app| {
        app.window()
            .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
                1150.0, 340.0,
            )));
    });
    std::thread::sleep(Duration::from_millis(250));
    let (window_height, column_bottom) = read_ui(|app| {
        let column = find_by_element_id(app, "AffixScreen::affix-left-column");
        (
            app.window().size().height as f32,
            column.absolute_position().y + column.size().height,
        )
    });
    assert!(
        column_bottom <= window_height,
        "affix-rail-scroll: at 1150x340 the left column (bottom {column_bottom}) must stay inside the \
         window ({window_height}) — the rail yields its height instead of overflowing"
    );

    // Restore the size the scenario found and reset the rail's scroll
    // position (the screen is re-instantiated with the tab).
    read_ui(move |app| {
        app.window()
            .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
                width, height,
            )));
    });
    click_control_by_label("Trade Imports");
    click_control_by_label("Current Item");
    std::thread::sleep(Duration::from_millis(250));
    eprintln!("[affix-rail-scroll] affix rail scroll scenario complete");
}

/// The catalog label a card's "Add entry: <label>" button addresses.
fn card_label(card: &str) -> &str {
    card.trim_start_matches("Add entry: ")
}

/// Select a build-import website through the combo's real popup: click
/// the combo, click the provider row, assert the shared selection.
///
/// Upstream remembers the website (`AffixViewModel.InitBuildImportWebsites`)
/// and the Import button then opens the tabbed dialog on that provider's
/// tab (`ImportAffixPresetCommandExecute`).
pub(super) fn select_website(client: &McpClient, root: McpHandle, website: &str) {
    let expected: Vec<String> = crate::actions::build_import_dialogs::ALL_BUILD_IMPORT_WEBSITES
        .iter()
        .map(|name| name.to_string())
        .collect();
    let query_items = |client: &McpClient| -> Vec<(String, McpHandle)> {
        let items = client
            .query_element_descendants(root, descendants_with_role("ListItem"), true)
            .unwrap_or_else(|error| harness_panic("query website popup items", &error));
        items
            .into_iter()
            .filter_map(|handle| {
                let properties = client
                    .get_element_properties(handle)
                    .unwrap_or_else(|error| harness_panic("read website popup item", &error));
                str_property(&properties, "accessibleLabel")
                    .map(str::to_string)
                    .map(|label| (label, handle))
            })
            .filter(|(label, _)| expected.contains(label))
            .collect()
    };

    click_on_label("Import provider");
    let mut items = query_items(client);
    if items.is_empty() {
        // Race fallback: the first click may have toggled a stale popup.
        click_on_label("Import provider");
        items = query_items(client);
    }
    let handle = items
        .iter()
        .find_map(|(label, handle)| (label == website).then_some(*handle))
        .unwrap_or_else(|| {
            panic!(
                "coverage matrix: website {website:?} not instantiated in the Import provider \
                 popup (exposed: {items:?})"
            )
        });
    client
        .click_element(handle, None)
        .unwrap_or_else(|error| harness_panic("click the website option", &error));
    assert_eq!(
        read_ui(|app| app.get_build_import_provider().to_string()),
        website,
        "coverage matrix: selecting {website:?} must update the shared website selection"
    );
}

/// Type `text` into the affix-screen catalog filter through the real
/// input pipeline (the accessibility `SetValue` path, the same channel
/// the coverage matrix uses for input fields) and assert the field holds
/// it.
fn set_filter(client: &McpClient, root: McpHandle, text: &str) {
    let field = find_text_input(client, root, "Enter filter");
    client
        .set_element_value(field, text)
        .unwrap_or_else(|error| harness_panic("type into the catalog filter", &error));
    assert_eq!(
        input_value(client, root, "Enter filter"),
        text,
        "catalog: the catalog filter field must hold the typed text"
    );
}

/// Catalog projections run since process start (the counter scroll
/// stutter is measured with).
fn catalog_projections() -> usize {
    crate::actions::affix::AFFIX_PROJECTION_COUNT.load(std::sync::atomic::Ordering::SeqCst)
}

/// Wheel a catalog grid to its bottom edge through the real input
/// pipeline (the auto-extension trigger).
fn scroll_grid_to_bottom(client: &McpClient, window: McpHandle, grid_id: &str) {
    let grid = client
        .find_elements_by_id(window, grid_id)
        .unwrap_or_else(|error| harness_panic("find the catalog grid scroll view", &error))
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("catalog: catalog grid {grid_id:?} not found"));
    client
        .scroll_element(grid, 0.0, -4000.0)
        .unwrap_or_else(|error| {
            harness_panic("scroll the catalog grid to its bottom edge", &error)
        });
}

/// (a) Search/filter input: typing, clearing, whitespace-only,
/// unicode, very long strings, rapid re-typing — each through the
/// real input pipeline, asserting the input's value and the
/// projected entry lists after every step.
///
/// Upstream filters the card lists live through `AffixTextFilter`
/// (`ListCollectionView.Filter`, keyword split on ";"). The port's
/// filter text is screen-local and has no consumer yet, so the
/// filtered-result property is "text lands, lists stay
/// consistent, nothing panics" — printed as an expected-mismatch
/// row rather than a fake filtering assertion.
fn filter_input_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[affix-surface] (a) filter input");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    let filter_input = find_text_input(client, root, "Enter filter");
    // Unfiltered projection: one render window of the enUS affix catalog
    // plus the seeded preset's two wanted rows (their ids resolve to index
    // rows beyond the window, so they lead the grid with the localized
    // index text the converter produces).
    let full_rows = catalog_count("affix");
    assert_eq!(
        full_rows,
        60 + 2,
        "affix-surface: the unfiltered affix grid must list one render window"
    );
    let assert_field = |client: &McpClient, root: McpHandle, expected: &str| {
        assert_eq!(
            input_value(client, root, "Enter filter"),
            expected,
            "affix-surface: the filter input must hold the typed text"
        );
        // The rail carries one chip per selected-preset entry and
        // upstream filters the catalog views only
        // (`RefreshAffixViewFilter`, AffixViewModel.cs:434-456), so the
        // rail stays complete under every filter input. The grid shows the
        // wanted state of the rows the filter keeps
        // (`CatalogFilter::matches` over the localized rows).
        assert_eq!(
            rail_chip_count(),
            6,
            "affix-surface: the wanted rail chips (two affixes plus the four family \
             entries of the seeded preset) must stay consistent under filter input"
        );
        assert!(
            wanted_affix_count() <= 2,
            "affix-surface: the filtered grid can only carry the wanted state of the rows \
             the filter kept"
        );
    };
    // Every catalog row (the wanted rail's rows are preset entries, not
    // catalog rows) must carry every `;`-separated term: the projection
    // narrowed live.
    let assert_rows_match = |terms: &str| {
        let terms: Vec<String> = terms
            .split(';')
            .map(|term| term.trim().to_lowercase())
            .filter(|term| !term.is_empty())
            .collect();
        let expected = terms.clone();
        assert!(
            read_ui(move |app| app
                .get_affix_entries()
                .iter()
                .filter(|entry| !entry.is_wanted)
                .all(|entry| {
                    let label = entry.label.to_lowercase();
                    terms.iter().all(|term| label.contains(term.as_str()))
                })),
            "affix-surface: every catalog row must match every filter term of {expected:?}"
        );
    };

    // Typed characters through the real key pipeline: the grid narrows
    // as the text lands (upstream `AffixTextFilter` setter re-filters
    // the active catalog view on every keystroke).
    client
        .click_element(filter_input, None)
        .expect("click into the filter input");
    for character in ['a', 't', 't'] {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| harness_panic("type into filter input", &error));
    }
    assert_field(client, root, "att");
    assert_rows_match("att");
    assert!(
        catalog_count("affix") < full_rows,
        "affix-surface: typing into the filter must narrow the affix grid"
    );
    take_evidence_screenshot(client, window, "affix-surface", "filter-typed.png");

    // Clearing restores the full catalog.
    client
        .set_element_value(filter_input, "")
        .expect("clear the filter input");
    assert_field(client, root, "");
    assert_eq!(
        catalog_count("affix"),
        full_rows,
        "affix-surface: clearing the filter must restore the full catalog"
    );

    // `;`-separated terms AND-combine: 'attack' alone matches the 31
    // enUS affix descriptions carrying it, 'attack;speed' only the 24
    // carrying both (plus the two wanted rows in both cases).
    client
        .set_element_value(filter_input, "attack")
        .expect("type one keyword term");
    assert_field(client, root, "attack");
    assert_rows_match("attack");
    assert_eq!(
        catalog_count("affix"),
        31,
        "affix-surface: 'attack' must match the 31 enUS affix descriptions"
    );
    client
        .set_element_value(filter_input, "attack;speed")
        .expect("type a two-term AND filter");
    assert_field(client, root, "attack;speed");
    assert_rows_match("attack;speed");
    assert_eq!(
        catalog_count("affix"),
        24,
        "affix-surface: 'attack;speed' must AND-combine to the 24 rows carrying both"
    );

    // Whitespace-only terms are ignored (the full catalog stays), and a
    // filter matching no catalog row empties the grid: the wanted state
    // rides on the catalog rows (`AffixInfoWanted` merged into the row), so
    // it is filtered with them and the screen's "No entries match the
    // filter." state is reached — upstream's empty `CollectionView`.
    client
        .set_element_value(filter_input, "   ")
        .expect("type whitespace-only filter");
    assert_field(client, root, "   ");
    assert_eq!(
        catalog_count("affix"),
        full_rows,
        "affix-surface: blank filter terms must not narrow the grid"
    );
    client
        .set_element_value(filter_input, "life;amulet")
        .expect("type a filter matching no catalog row");
    assert_field(client, root, "life;amulet");
    assert_eq!(
        catalog_count("affix"),
        0,
        "affix-surface: a filter matching no catalog row leaves an empty grid"
    );
    // The rail is preset-driven and stays complete (six chips).
    assert_eq!(rail_chip_count(), 6);

    // Unicode and a very long string must not break the projection.
    client
        .set_element_value(filter_input, "Zerstörung ♞ 毀す")
        .expect("type a unicode filter");
    assert_field(client, root, "Zerstörung ♞ 毀す");
    assert_eq!(
        catalog_count("affix"),
        0,
        "affix-surface: a unicode filter matching no row leaves the grid empty"
    );
    let long = "x".repeat(512);
    client
        .set_element_value(filter_input, &long)
        .expect("type a very long filter");
    assert_field(client, root, &long);

    // Rapid re-typing: five immediate value changes; the last one
    // must win deterministically.
    for step in 0..5 {
        client
            .set_element_value(filter_input, &format!("rapid{step}"))
            .expect("rapid re-type into the filter input");
    }
    assert_field(client, root, "rapid4");
    assert_eq!(
        catalog_count("affix"),
        0,
        "affix-surface: the last rapid re-type must be the active filter"
    );
    take_evidence_screenshot(client, window, "affix-surface", "filter-rapid.png");

    client
        .set_element_value(filter_input, "")
        .expect("clear the filter input before the next scenario");
    assert_eq!(
        catalog_count("affix"),
        full_rows,
        "affix-surface: clearing the filter must restore the catalog"
    );
}

/// (b) Entry-card families Affix/Aspect/Sigil/Unique/Rune plus the
/// F1 and F2 live proofs: the card edit (+) click opens SetAffix
/// seeded from its entry (the overlap fix), and a mixed-kind color
/// apply re-seeds the dialog from the addressed entry (the
/// kind-tagged-key contract).
fn entry_card_families_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[affix-surface] (b) entry-card families + F1/F2 live proofs");
    let mut mismatches: Vec<String> = Vec::new();

    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    // The AffixEntryCard edit (+) button must open SetAffix: the real
    // click must not reach the remove X and delete the entry.
    click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "set_affix",
        "affix-surface/F1: the card edit (+) click must open set_affix \
         (saw visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_entry_id().to_string()),
        "AttackSpeed",
        "affix-surface/F1: SetAffix must be seeded from the clicked card's entry"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_item_type().to_string()),
        "Amulet",
        "affix-surface/F1: SetAffix must seed the entry's item type"
    );
    take_evidence_screenshot(client, window, "affix-surface", "cards-setaffix-open.png");

    // Edit through the dialog: toggle a flag and confirm; the
    // upsert must update exactly this entry (same kind, id and
    // item type), proven by the dialog re-seed.
    click_control_by_label("Greater affix");
    click_on_ui("Apply");
    assert!(!dialog_state().0, "Apply must close set_affix");
    click_on_ui(&format!("Set affix: {SEED_ATTACK_SPEED_TEXT}"));
    assert!(
        dialog_state().0 && dialog_state().1 == "set_affix",
        "affix-surface: the SetAffix dialog must reopen for the persistence read-back"
    );
    assert!(
        read_ui(|app| app.get_set_affix_is_greater()),
        "affix-surface: the confirmed greater flag must persist into the entry and re-seed \
         the dialog form"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close set_affix");

    // F2 (mixed kind): applying Blue to the ASPECT entry must
    // target the aspect — under the old per-kind-index contract
    // this re-seeded from the wrong (affix) entry.
    click_control_by_label("Aspects");
    assert_label_findable("Set color: Blood For Blood");
    click_on_ui("Set color: Blood For Blood");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "set_affix_color",
        "affix-surface/F2: the aspect color swatch must open set_affix_color"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_color_value().to_string()),
        "#3B82F6",
        "affix-surface/F2: the color dialog must seed from the aspect entry"
    );
    click_on_ui("Blue");
    click_on_ui("Apply");
    assert!(!dialog_state().0, "Apply must close set_affix_color");
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "cards-mixed-kind-color.png",
    );

    // Persistence proof: the ASPECT entry carries the applied
    // color ...
    click_on_ui("Set color: Blood For Blood");
    assert!(
        dialog_state().0 && dialog_state().1 == "set_affix_color",
        "affix-surface/F2: the color dialog must reopen for the persistence read-back"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_color_value().to_string()),
        "#0088FF",
        "affix-surface/F2: the applied Blue must persist into the aspect entry"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close set_affix_color");

    // ... and the affix sibling is untouched (the old contract
    // addressed exactly this wrong entry).
    click_control_by_label("Affixes");
    click_on_ui(&format!("Set color: {SEED_LIFE_TEXT}"));
    assert!(
        dialog_state().0 && dialog_state().1 == "set_affix_color",
        "affix-surface/F2: the Life color dialog must reopen"
    );
    assert_eq!(
        read_ui(|app| app.get_set_affix_color_value().to_string()),
        "#EF4444",
        "affix-surface/F2: the Life (affix) entry must be untouched by the aspect color apply"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close set_affix_color");

    // Aspect remove: unprojects exactly the aspect entry.
    click_control_by_label("Aspects");
    click_on_ui("Remove Blood For Blood");
    assert_eq!(
        label_count("Set color: Blood For Blood"),
        0,
        "affix-surface: removing the aspect must unproject its cells"
    );

    // Sigil / Unique / Rune families: color dialog seeds per
    // entry, cancel discards, remove unprojects. The per-family
    // add (+) buttons are wired to the kind-aware preset-entry
    // add handler since dead-controls (full coverage in the
    // dead-controls family); this loop still asserts the invariants
    // that hold on already-wanted cards: no dialog opens and the
    // entry stays.
    for (category, name, seeded_hex) in [
        ("Sigils", "Torture", "#F59E0B"),
        ("Uniques", "RingOfVayar", "#111111"),
        ("Runes", "ElixirOfSorrow", "#8B5CF6"),
    ] {
        click_control_by_label(category);
        assert_label_findable(&format!("Set color: {name}"));

        // The add (+) button must not corrupt state (no dialog,
        // entry stays).
        click_on_label(&format!("Add entry: {name}"));
        let (visible, dialog_id) = dialog_state();
        assert!(
            !visible,
            "affix-surface: the per-family add (+) click must not open a dialog \
             (saw id={dialog_id:?})"
        );
        assert_label_findable(&format!("Set color: {name}"));

        // Color dialog seeds the entry's color; Cancel discards.
        click_on_label(&format!("Set color: {name}"));
        assert!(
            dialog_state().0 && dialog_state().1 == "set_affix_color",
            "affix-surface: the {category} color swatch must open set_affix_color"
        );
        assert_eq!(
            read_ui(|app| app.get_set_affix_color_value().to_string()),
            seeded_hex,
            "affix-surface: the {category} color dialog must seed from its entry"
        );
        click_on_ui("Purple");
        click_on_ui("Cancel");
        assert!(!dialog_state().0, "Cancel must close set_affix_color");
        click_on_label(&format!("Set color: {name}"));
        assert_eq!(
            read_ui(|app| app.get_set_affix_color_value().to_string()),
            seeded_hex,
            "affix-surface: the cancelled color draft must not survive"
        );
        click_on_ui("Cancel");
        assert!(!dialog_state().0);

        // Remove unprojects the entry.
        click_on_label(&format!("Remove {name}"));
        assert_eq!(
            label_count(&format!("Set color: {name}")),
            0,
            "affix-surface: removing the {category} entry must unproject its cells"
        );
    }
    mismatches.push(
        "aspect/sigil/unique/rune add (+) buttons — wired since dead-controls to the \
         kind-aware preset-entry add handler (no dialog opens on an already-wanted \
         card, asserted); catalog-sourced adds arrive with catalog"
            .to_string(),
    );

    eprintln!(
        "[affix-surface] (b) coverage: F1 edit path, F2 mixed-kind color apply + persistence, \
         cancel-discard and removal for every card family ({} expected-mismatch rows)",
        mismatches.len()
    );
    for row in &mismatches {
        eprintln!("[affix-surface] expected-mismatch: {row}");
    }
}

/// (c) Config dialog cycles: every per-category config dialog and
/// the paragon config dialog, each opened by a real click with
/// confirm and cancel paths asserted.
fn config_dialog_cycles_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[affix-surface] (c) config dialog cycles");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    // Per-category config dialogs: confirm (Close) and cancel
    // paths. The five config dialogs edit real settings now
    // their switches / selector write through on
    // interaction and persist immediately, so Done/Close only
    // closes and Cancel leaves the already-persisted edits in
    // place (upstream `*ConfigDoneExecute` parity — the upstream
    // setters save, there is no draft). The write-through flows
    // are asserted by the toggle-sweep affix-family / sigil config
    // scenarios.
    for (switch, cell, dialog_id) in [
        ("Affixes", "Affix config", "affix_config"),
        ("Aspects", "Aspect config", "aspect_config"),
        ("Sigils", "Sigil config", "sigil_config"),
        ("Uniques", "Unique config", "unique_config"),
        ("Runes", "Rune config", "rune_config"),
    ] {
        click_control_by_label(switch);
        click_on_ui(cell);
        let (visible, dialog_id_seen) = dialog_state();
        assert!(
            visible && dialog_id_seen == dialog_id,
            "affix-surface: clicking {cell:?} must open {dialog_id:?}"
        );
        take_evidence_screenshot(
            client,
            window,
            "affix-surface",
            &format!("cfg-{dialog_id}-open.png"),
        );
        click_on_ui("Close");
        let (visible, _) = dialog_state();
        assert!(
            !visible,
            "affix-surface: Close must confirm-close {dialog_id:?}"
        );
        click_on_ui(cell);
        assert!(dialog_state().0, "affix-surface: {cell:?} must reopen");
        click_on_ui("Cancel");
        let (visible, _) = dialog_state();
        assert!(!visible, "affix-surface: Cancel must close {dialog_id:?}");
    }

    // Paragon config (Settings family): open, toggle, confirm;
    // then the cancel path.
    click_on_ui("Settings");
    assert_eq!(read_ui(|app| app.get_active_tab()), 4);
    click_on_ui("Open paragon config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "paragon_config",
        "affix-surface: Open paragon config must open paragon_config"
    );
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "cfg-paragon_config-open.png",
    );
    let paragon_before = read_ui(|app| app.get_settings_paragon_mode_active());
    click_control_by_label("Enable paragon mode");
    assert_ne!(
        read_ui(|app| app.get_settings_paragon_mode_active()),
        paragon_before,
        "affix-surface: the paragon toggle must flip the root form value"
    );
    click_on_ui("Apply");
    assert!(!dialog_state().0, "Apply must close paragon_config");
    click_on_ui("Open paragon config");
    assert!(dialog_state().0);
    let paragon_now = read_ui(|app| app.get_settings_paragon_mode_active());
    assert_ne!(
        paragon_now, paragon_before,
        "affix-surface: the confirmed paragon toggle must have been persisted to the root seed"
    );
    click_control_by_label("Enable paragon mode");
    click_on_ui("Apply");
    assert!(!dialog_state().0, "cleanup: paragon toggle restored");
}

/// (d) Numeric spin boxes: stepper clicks, clamping at both ends,
/// rapid clicks and the accessible increment/decrement actions —
/// proven on the Debug screen's root-observable Type Area Offset
/// Left spin box (NumericSpinBox component).
///
/// Typed garbage ("abc", "1.5", "-3", empty, overflow-width)
/// cannot occur through the UI: the port's NumericSpinBox renders
/// a read-only value box with no TextInput (upstream MahApps
/// NumericUpDown accepts typed text) — printed as an
/// expected-mismatch row.
fn numeric_spin_box_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[affix-surface] (d) numeric spin box");
    click_on_ui("Debug");
    assert_eq!(read_ui(|app| app.get_active_tab()), 3);

    let value = || read_ui(|app| app.get_type_area_offset_left());
    let before = value();

    // Single stepper click.
    click_on_ui("Type Area Offset (Left): increment");
    assert_eq!(
        value(),
        (before + 1.0).min(25.0),
        "affix-surface: the increment stepper must raise the root value"
    );

    // Rapid clicks: deterministic cumulative steps with clamping
    // above the maximum (drives through the clamp without panic).
    for _ in 0..60 {
        click_on_ui("Type Area Offset (Left): increment");
    }
    assert_eq!(
        value(),
        25.0,
        "affix-surface: repeated increments must clamp at the maximum without panicking"
    );
    take_evidence_screenshot(client, window, "affix-surface", "spinbox-clamped-max.png");
    for _ in 0..60 {
        click_on_ui("Type Area Offset (Left): decrement");
    }
    assert_eq!(
        value(),
        0.0,
        "affix-surface: repeated decrements must clamp at the minimum without panicking"
    );

    // Accessible increment/decrement actions (screen-reader path).
    on_ui(|app| {
        let spinbox = ElementHandle::find_by_accessible_label(app, "Type Area Offset (Left):")
            .find(|element| {
                matches!(
                    element.accessible_role(),
                    Some(i_slint_backend_testing::AccessibleRole::Spinbox)
                )
            })
            .unwrap_or_else(|| {
                panic!(
                    "affix-surface: the Type Area Offset Left spinbox must expose the \
                        spinbox accessible role"
                )
            });
        spinbox.invoke_accessible_increment_action();
    });
    assert_eq!(
        value(),
        1.0,
        "affix-surface: the accessible increment action must step the value"
    );
    on_ui(|app| {
        let spinbox = ElementHandle::find_by_accessible_label(app, "Type Area Offset (Left):")
            .find(|element| {
                matches!(
                    element.accessible_role(),
                    Some(i_slint_backend_testing::AccessibleRole::Spinbox)
                )
            })
            .unwrap_or_else(|| panic!("affix-surface: spinbox element vanished"));
        spinbox.invoke_accessible_decrement_action();
    });
    assert_eq!(
        value(),
        0.0,
        "affix-surface: the accessible decrement action must step the value"
    );

    eprintln!(
        "[affix-surface] expected-mismatch: numeric spin box typed input — the port's \
         NumericSpinBox value box is read-only (no TextInput), so typed garbage / \
         empty confirm / overflow-width text cannot occur through the UI; upstream \
         MahApps NumericUpDown accepts typed text (parity gap)"
    );

    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
}

/// (e) Toggle switches: click, rapid double-toggle, and the
/// cancel-reopen cycle (no half-applied state) on the overlay
/// config dialog's root-observable switches.
fn toggle_switch_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[affix-surface] (e) toggle switch");
    click_on_ui("Settings");
    assert_eq!(read_ui(|app| app.get_active_tab()), 4);

    click_on_ui("Open overlay config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "overlay_config",
        "affix-surface: Open overlay config must open overlay_config"
    );
    let enabled = || read_ui(|app| app.get_settings_overlay_enabled());

    // Rapid double-toggle: two real clicks, both processed — two
    // flips return to the starting value.
    let before = enabled();
    click_control_by_label("Enable overlay");
    click_control_by_label("Enable overlay");
    assert_eq!(
        enabled(),
        before,
        "affix-surface: two rapid clicks must toggle exactly twice (back to the start)"
    );

    // A single click flips the two-way bound form value.
    click_control_by_label("Enable overlay");
    assert_ne!(
        enabled(),
        before,
        "affix-surface: one click must flip the overlay switch"
    );
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "toggles-overlay-config.png",
    );

    // Cancel after toggling: no half-applied state — the reopen
    // must re-seed from AppState (the draft was never confirmed).
    // `enabled` currently holds the unconfirmed draft (flipped);
    // add a second drafted switch, then cancel both away.
    click_control_by_label("Lock overlay menu");
    let drafted = enabled();
    assert_ne!(drafted, before, "affix-surface: the draft must be flipped");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close overlay_config");
    click_on_ui("Open overlay config");
    assert!(dialog_state().0);
    assert_eq!(
        enabled(),
        before,
        "affix-surface: the cancelled toggle drafts must not survive the reopen"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "cleanup: overlay config closed");
}

/// (f) Preset CRUD edges: rename to existing/empty/very-long/
/// path-separator names, duplicate create, delete selected and
/// delete last preset, and preset switches (ComboBox popup) with
/// unsaved edits — the F3 re-projection live proof.
fn preset_crud_edges_scenario(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[affix-surface] (f) preset CRUD edges + F3 live proof");
    seed_affix_presets();
    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);

    // Rename to an existing name: the dialog must surface the
    // error and stay open, the model must not change.
    click_on_ui("Rename preset");
    assert!(
        dialog_state().0 && dialog_state().1 == "rename_preset",
        "affix-surface: Rename preset must open rename_preset"
    );
    let rename_input = find_text_input(client, root, "e.g. Speed, Boss, AoE");
    client
        .set_element_value(rename_input, "Beta")
        .expect("type the colliding rename");
    click_on_ui("Apply");
    assert!(
        dialog_state().0 && read_ui(|app| app.get_dialog_error_visible()),
        "affix-surface: renaming to an existing name must surface the dialog error"
    );
    assert!(
        preset_names().contains(&"Beta".to_string())
            && preset_names().iter().filter(|n| *n == "Beta").count() == 1,
        "affix-surface: the colliding rename must not create a duplicate name"
    );
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "rename-collision-error.png",
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close rename_preset");

    // Rename to empty: rejected.
    click_on_ui("Rename preset");
    let rename_input = find_text_input(client, root, "e.g. Speed, Boss, AoE");
    client
        .set_element_value(rename_input, "")
        .expect("clear the rename input");
    click_on_ui("Apply");
    assert!(
        dialog_state().0 && read_ui(|app| app.get_dialog_error_visible()),
        "affix-surface: an empty rename must surface the dialog error"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0);

    // Rename with a path separator and to a very long name: the
    // validator only normalizes whitespace, so both persist.
    let long_name = "L".repeat(120);
    for name in ["A/B", long_name.as_str()] {
        click_on_ui("Rename preset");
        let rename_input = find_text_input(client, root, "e.g. Speed, Boss, AoE");
        client
            .set_element_value(rename_input, name)
            .unwrap_or_else(|error| harness_panic("type the rename", &error));
        click_on_ui("Apply");
        assert!(
            !dialog_state().0,
            "affix-surface: renaming to {name:?} must confirm and close"
        );
        assert!(
            preset_names().iter().any(|n| n == name),
            "affix-surface: the renamed preset {name:?} must appear in the model"
        );
        // Rename back for the next step.
        click_on_ui("Rename preset");
        let rename_input = find_text_input(client, root, "e.g. Speed, Boss, AoE");
        client
            .set_element_value(rename_input, "Alpha")
            .unwrap_or_else(|error| harness_panic("rename back", &error));
        click_on_ui("Apply");
        assert!(!dialog_state().0);
    }

    // Duplicate rename through the modal path: error, model
    // unchanged.
    let count_before = preset_names().len();
    click_on_ui("Rename preset");
    assert!(
        dialog_state().0 && dialog_state().1 == "rename_preset",
        "affix-surface: Rename preset must open rename_preset"
    );
    let preset_input = find_text_input(client, root, "e.g. Speed, Boss, AoE");
    client
        .set_element_value(preset_input, "Beta")
        .expect("type the duplicate preset name");
    click_on_ui("Apply");
    assert!(
        dialog_state().0 && read_ui(|app| app.get_dialog_error_visible()),
        "affix-surface: a duplicate rename must surface the dialog error"
    );
    assert_eq!(
        preset_names().len(),
        count_before,
        "affix-surface: the duplicate rename must not change the model"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0);

    // Inline create with a whitespace-only name is a no-op.
    let inline_input = find_text_input(client, root, "Enter name");
    client
        .set_element_value(inline_input, "   ")
        .expect("type whitespace into the inline create field");
    click_on_ui("Create preset");
    assert_eq!(
        preset_names().len(),
        count_before,
        "affix-surface: a whitespace-only inline create must be a no-op"
    );

    // Switching presets through the ComboBox must re-project
    // the entries — Alpha has 2 affix entries, Beta none.
    assert_eq!(wanted_affix_count(), 2, "Alpha is selected");
    select_preset(client, root, "Beta");
    assert_eq!(
        wanted_affix_count(),
        0,
        "affix-surface/F3: switching to the empty Beta preset must re-project (no wanted rows)"
    );
    assert!(
        label_count("Set color: Life") == 0 && label_count("Set color: Attack Speed") == 0,
        "affix-surface/F3: the previous preset's entries must be unprojected"
    );
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "preset-switch-reprojected.png",
    );
    select_preset(client, root, "Alpha");
    assert_eq!(
        wanted_affix_count(),
        2,
        "affix-surface/F3: switching back must re-project Alpha's entries"
    );

    // Switch with unsaved edits: a cancelled color draft must not
    // follow the switch, and each preset's projection is its own.
    click_on_ui(&format!("Set color: {SEED_LIFE_TEXT}"));
    assert!(dialog_state().0);
    click_on_ui("Blue");
    click_on_ui("Cancel");
    assert!(!dialog_state().0);
    select_preset(client, root, "Gamma");
    assert_eq!(
        wanted_affix_count(),
        1,
        "affix-surface: Gamma projects exactly its one wanted affix row"
    );
    assert_label_findable("Set color: Max Life");
    select_preset(client, root, "Alpha");
    click_on_ui(&format!("Set color: {SEED_LIFE_TEXT}"));
    assert_eq!(
        read_ui(|app| app.get_set_affix_color_value().to_string()),
        "#EF4444",
        "affix-surface: the cancelled draft must not have followed the preset switch"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0);

    // Delete the selected preset: the upstream window-modal prompt
    // (`AffixViewModel.cs:1764` — "Delete" /
    // `Are you sure you want to delete preset "Beta"`) asks first, No
    // keeps the preset, and only Yes deletes and falls back to the
    // first preset.
    select_preset(client, root, "Beta");
    click_on_ui("Remove preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "affix-surface: Remove preset must ask through the confirm prompt (observed: \
         visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_dialog_title().to_string()),
        "Delete",
        "affix-surface: the delete prompt must carry the upstream title"
    );
    assert_eq!(
        read_ui(|app| app.get_dialog_message().to_string()),
        "Are you sure you want to delete preset \"Beta\"",
        "affix-surface: the delete prompt must carry the upstream copy"
    );
    take_evidence_screenshot(client, window, "affix-surface", "delete-preset-prompt.png");

    // No cancels: the prompt closes and the preset survives with its
    // selection.
    click_on_ui("No");
    assert!(
        !dialog_state().0,
        "affix-surface: No must close the delete prompt"
    );
    assert!(
        preset_names().contains(&"Beta".to_string()),
        "affix-surface: No must keep the preset: {:?}",
        preset_names()
    );
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        "Beta",
        "affix-surface: No must keep the selection"
    );

    // Yes deletes and re-projects the fallback preset.
    click_on_ui("Remove preset");
    click_on_ui("Yes");
    assert!(
        !dialog_state().0,
        "affix-surface: Yes must close the delete prompt"
    );
    assert!(
        !preset_names().contains(&"Beta".to_string()),
        "affix-surface: the removed preset must leave the model"
    );
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        "Alpha",
        "affix-surface: removing the selected preset must fall back to the first preset"
    );

    // Delete down to the last preset and past it: the wanted projection
    // empties without panicking. The catalog grids keep browsing the
    // game data (upstream's catalog iteration is independent of the
    // preset), so the models hold the render window and no wanted row.
    // Every delete is answered Yes on the shared prompt.
    click_on_ui("Remove preset");
    click_on_ui("Yes");
    click_on_ui("Remove preset");
    click_on_ui("Yes");
    assert!(
        preset_names().is_empty(),
        "affix-surface: deleting the last preset must empty the model: {:?}",
        preset_names()
    );
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        "",
        "affix-surface: no preset left, no selection"
    );
    assert_eq!(
        affix_row_count(),
        60,
        "affix-surface: without a preset the affix grid still lists the catalog window"
    );
    assert_eq!(
        wanted_affix_count(),
        0,
        "affix-surface: without a preset the wanted projection must be empty"
    );
    take_evidence_screenshot(client, window, "affix-surface", "preset-list-empty.png");

    // Disabled-state pilot (upstream parity): Rename/Remove preset are
    // RelayCommand-gated on an active selection (upstream
    // CanRenameAffixPresetCommandExecute / CanRemoveAffixPresetNameExecute).
    // With the model emptied above, both buttons are disabled — a real
    // click is a no-op: no dialog opens (the rename handler would open
    // rename_preset even without a selection, so this discriminates the
    // gate from handler behavior), the selection and model stay empty.
    // The a11y tree carries the gate too: the MCP element-properties
    // payload only serializes accessibleEnabled when it is true, so an
    // absent flag means disabled.
    let rename_props = client
        .get_element_properties(find_button_by_label(client, root, "Rename preset"))
        .unwrap_or_else(|error| harness_panic("read Rename preset properties", &error));
    assert!(
        rename_props.get("accessibleEnabled").is_none(),
        "affix-surface: without a selection Rename preset must be disabled (no \
         accessibleEnabled flag): {rename_props:.400}"
    );
    click_on_ui("Rename preset");
    assert!(
        !dialog_state().0,
        "affix-surface: a real click on the disabled Rename preset must be a no-op (no dialog)"
    );
    assert_eq!(
        read_ui(|app| app.get_selected_preset().to_string()),
        "",
        "affix-surface: the disabled Rename click must leave the selection empty"
    );
    click_on_ui("Remove preset");
    assert!(
        preset_names().is_empty(),
        "affix-surface: a real click on the disabled Remove preset must not change the model: {:?}",
        preset_names()
    );
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "preset-controls-disabled.png",
    );

    // Restore the standard seed for the remaining scenarios.
    seed_affix_presets();

    // With a selection the same controls are enabled again: the a11y
    // tree reports the gate, and the scenario's opening Rename click
    // (which opened rename_preset) already proved the enabled path
    // behaviorally.
    for label in ["Rename preset", "Remove preset"] {
        let props = client
            .get_element_properties(find_button_by_label(client, root, label))
            .unwrap_or_else(|error| harness_panic("read preset button properties", &error));
        assert!(
            accessible_enabled(&props),
            "affix-surface: with a selection {label} must report accessibleEnabled true: \
             {props:.400}"
        );
    }

    // A forced remove failure must reach the Logging screen and the
    // prompt's error row instead of stderr only (area-08
    // remove-failure row). The failure is produced the only way the
    // port can still fail here: the editor no longer holds the preset
    // the screen shows (upstream's `CanExecute` pre-gate only checks
    // that a selection exists, so a preset removed behind the screen
    // reaches the remove body). The Logging screen renders the very
    // `log-messages` model asserted here (`main_window.slint`,
    // `active-tab == 2` arm).
    on_ui(|_app| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = arc.lock().expect("app state lock");
        // Seeding replaces rule inputs: bump like every production writer.
        state.bump_rules_input_revision();
        state
            .affix_preset_editor
            .presets
            .retain(|preset| preset.name != "Alpha");
    });
    click_on_ui("Remove preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "affix-surface: the remove click must still ask through the prompt (observed: \
         visible={visible}, id={dialog_id:?})"
    );
    click_on_ui("Yes");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "confirm",
        "affix-surface: a failed delete must keep the prompt open (observed: \
         visible={visible}, id={dialog_id:?})"
    );
    assert!(
        read_ui(|app| app.get_dialog_error_visible()
            && app
                .get_dialog_error()
                .to_string()
                .contains("remove preset failed")),
        "affix-surface: the forced remove failure must surface in the prompt error row, saw {:?}",
        read_ui(|app| app.get_dialog_error().to_string())
    );
    let last_log = log_messages().pop();
    assert!(
        last_log
            .as_deref()
            .is_some_and(|message| message.contains("remove preset failed")),
        "affix-surface: the forced remove failure must reach the Logging screen, saw {last_log:?}"
    );
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "delete-preset-failure-prompt.png",
    );
    click_on_ui("No");
    assert!(
        !dialog_state().0,
        "affix-surface: cleanup: the failed prompt must cancel cleanly"
    );

    // Restore the standard seed (in memory and on disk) for the
    // scenarios after this one; the failure probe above left the editor
    // one preset short of what the screen shows.
    restore_standard_presets();
}

/// (g) Multi-build / trade rows referencing presets, where
/// observable in the harness (both row models are empty here, so
/// the dialogs open and the absence of rows is asserted).
fn multi_build_trade_scenario(client: &McpClient, window: McpHandle) {
    eprintln!("[affix-surface] (g) multi-build / trade interplay");
    seed_affix_presets();

    click_on_ui("Current Item");
    assert_eq!(read_ui(|app| app.get_active_tab()), 0);
    click_on_ui("Affix config");
    click_on_ui("Multi-build config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "multi_build_config",
        "affix-surface: the AffixConfig dialog's Multi-build config must open multi_build_config"
    );
    take_evidence_screenshot(
        client,
        window,
        "affix-surface",
        "multi-build-config-open.png",
    );
    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close multi_build_config");
    eprintln!(
        "[affix-surface] expected-mismatch: multi-build rows referencing presets — the \
         multi-build-names model is empty in the harness, so no preset-referencing \
         rows exist to click (attributes shipped)"
    );

    click_on_ui("Affix Presets");
    assert_eq!(read_ui(|app| app.get_active_tab()), 1);
    click_on_ui("Trade config");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "trade_config",
        "affix-surface: Trade config must open trade_config"
    );
    take_evidence_screenshot(client, window, "affix-surface", "trade-config-open.png");
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close trade_config");
    click_on_ui("Add trade item");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "add_trade_item",
        "affix-surface: Add trade item must open add_trade_item"
    );
    click_on_ui("Cancel");
    assert!(!dialog_state().0, "Cancel must close add_trade_item");
    eprintln!(
        "[affix-surface] expected-mismatch: wanted trade rows referencing presets — the \
         trade-items model is empty in the harness, so no rows exist to click \
         (attributes shipped)"
    );
}
