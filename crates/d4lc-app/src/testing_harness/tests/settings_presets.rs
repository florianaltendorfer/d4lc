use super::*;

// ── presets: Settings system-preset completeness scenario ─────

/// Evidence directory for the preset-completeness scenario.
const PRESET_EVIDENCE_DIR: &str = "pilots/presets";

/// Real-click the Settings preset controls and assert each projected
/// list equals the shipped preset set enumerated straight from the
/// vendored `assets/original/systempresets` tree. Fails loudly when any
/// shipped preset is missing from the projected UI list (the reported
/// defect: both ComboBoxes showed only a config-derived fragment).
///
/// Upstream parity (pinned v5.4.0.0, d508a473): the sidebar
/// "System presets" list mirrors the extracted-preset enumeration
/// (`SettingsViewModel.InitSystemPresets`, `Images\*p_*`), the
/// "Available system presets" list mirrors the `systempresets.json`
/// catalog with no status filter (`SystemPresetManager`). The Linux
/// port ships both offline: extracted preset directories and the
/// `zips/*.zip` catalog mirror.
pub(super) fn scenario_settings_presets_complete(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
) {
    eprintln!("[gui-presets] starting Settings preset completeness scenario");

    // Shipped sets enumerated independently from the vendored assets.
    fn shipped_preset_dirs() -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(workspace_root().join("assets/original/systempresets"))
                .expect("vendored assets/original/systempresets must exist")
                .flatten()
                .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|name| name.contains("p_"))
                .collect();
        names.sort();
        names
    }

    fn shipped_catalog_zips() -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(workspace_root().join("assets/original/systempresets/zips"))
                .expect("vendored assets/original/systempresets/zips must exist")
                .flatten()
                .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|name| name.to_ascii_lowercase().ends_with(".zip"))
                .collect();
        names.sort();
        names
    }

    // Real click through the nav to the Settings screen.
    click_on_ui("Settings");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        4,
        "presets scenario: nav click must switch to Settings"
    );

    // Real click on the "System presets" ComboBox control, then assert
    // the projected list equals the shipped set.
    click_on_ui("System preset");
    let ui_system_presets = read_ui(|app| {
        let model = app.get_system_presets();
        (0..model.row_count())
            .map(|row| model.row_data(row).expect("system preset row").to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        ui_system_presets,
        shipped_preset_dirs(),
        "presets scenario: the projected System presets list must equal the shipped set"
    );
    assert!(
        ui_system_presets.contains(&read_ui(|app| app.get_selected_system_preset().to_string())),
        "presets scenario: the selected system preset must be visible in the projected list"
    );

    // Real click on the "Available system presets" ComboBox control,
    // then assert the projected list equals the shipped catalog.
    click_on_ui("Community preset");
    let ui_community_presets = read_ui(|app| {
        let model = app.get_community_presets();
        (0..model.row_count())
            .map(|row| {
                model
                    .row_data(row)
                    .expect("community preset row")
                    .to_string()
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        ui_community_presets,
        shipped_catalog_zips(),
        "presets scenario: the projected Available system presets list must equal the \
         shipped catalog"
    );

    // Cross-lock: every shipped zip is listed by the vendored upstream
    // `systempresets.json` (the catalog upstream's list is built from).
    let catalog_text = std::fs::read_to_string(
        workspace_root().join("assets/original/systempresets/systempresets.json"),
    )
    .expect("vendored systempresets.json must exist");
    for zip in shipped_catalog_zips() {
        assert!(
            catalog_text.contains(&zip),
            "presets scenario: shipped zip {zip} must be listed in systempresets.json"
        );
    }

    // ── Display-backend selector: renders, persists
    //    the choice, logs the Wayland unavailability, and
    //    re-seeds from disk on the restart projection. Real clicks
    //    through the same ComboBox pattern as the presets above.
    scenario_settings_display_backend(client, root);

    // ── Game-data language combo: exposes the 14
    //    upstream ids, the selection persists through the
    //    set_language chain, and the startup projection re-seeds
    //    from disk.
    scenario_language_selection(client, root);

    // ── UI language combo: a real pick relabels the UI at runtime,
    //    persists the culture and re-seeds from disk on the restart
    //    projection.
    scenario_app_language_selection(client, root);

    // Evidence screenshot of the Settings screen.
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic("presets screenshot", &error));
    assert_png_and_dimensions(&png);
    let dir = workspace_root()
        .join("debug-output")
        .join(PRESET_EVIDENCE_DIR);
    std::fs::create_dir_all(&dir).expect("create the evidence directory");
    std::fs::write(dir.join("settings-presets.png"), &png).expect("write presets screenshot");
    eprintln!(
        "[gui-presets] screenshot written: {}",
        dir.join("settings-presets.png").display()
    );
    eprintln!(
        "[gui-presets] projected lists complete: {} system presets, {} available presets",
        ui_system_presets.len(),
        ui_community_presets.len()
    );
}

/// Open the Settings display-backend ComboBox with a real click and
/// return the popup's option labels in model order. When `select` is
/// `Some(option)`, click that popup `ListItem` through the real
/// pointer pipeline and assert the selection landed (the affix-surface
/// preset-ComboBox pattern).
///
/// The presets scenario above leaves an earlier ComboBox popup
/// open; the first outside click is consumed closing it (Slint
/// popup semantics), so the query repeats after one extra real
/// click until the display-backend options are actually visible.
fn open_display_backend_popup(
    client: &McpClient,
    root: McpHandle,
    select: Option<&str>,
) -> Vec<String> {
    let query_options = |client: &McpClient,
                         root: McpHandle|
     -> Option<(Vec<String>, Option<McpHandle>)> {
        let items = client
            .query_element_descendants(root, descendants_with_role("ListItem"), true)
            .unwrap_or_else(|error| harness_panic("query display-backend popup items", &error));
        let mut labels = Vec::new();
        let mut target: Option<McpHandle> = None;
        for handle in items {
            let properties = client
                .get_element_properties(handle)
                .unwrap_or_else(|error| harness_panic("read display-backend popup item", &error));
            if let Some(label) = str_property(&properties, "accessibleLabel").map(str::to_string) {
                if select == Some(label.as_str()) {
                    target = Some(handle);
                }
                labels.push(label);
            }
        }
        let complete = ["Auto", "Wayland", "X11"]
            .iter()
            .all(|option| labels.contains(&option.to_string()));
        complete.then_some((labels, target))
    };

    let mut opened: Option<(Vec<String>, Option<McpHandle>)> = None;
    for _ in 0..2 {
        click_on_ui("Display backend");
        if let Some(found) = query_options(client, root) {
            opened = Some(found);
            break;
        }
    }
    let (labels, target) = opened.unwrap_or_else(|| {
        panic!(
            "display backend: the ComboBox popup did not open or its options are not \
                 exposed after real clicks"
        )
    });

    if let Some(option) = select {
        let item = target.unwrap_or_else(|| {
            panic!("display backend: option {option:?} not reachable in the open ComboBox popup")
        });
        client
            .click_element(item, None)
            .unwrap_or_else(|error| harness_panic("click the display-backend option", &error));
        assert_eq!(
            read_ui(|app| app.get_settings_display_backend().to_string()),
            option,
            "display backend: selecting {option:?} must update the selector"
        );
    }
    labels
}

/// Display-backend selector scenario, run inside the
/// settings presets scenario while the Settings screen is open.
///
/// Decision points covered: the selector renders with exactly the
/// three shipped options and the persisted seed; a Wayland click
/// persists the token to `Config/Settings.json`, logs the structured
/// status (unavailability naming the missing build piece without the
/// capture feature, the portal-path note with it — never a silent
/// X11 claim), and the startup projection re-seeds the selection
/// from disk (restart).
fn scenario_settings_display_backend(client: &McpClient, root: McpHandle) {
    let config_root = crate::actions::settings::resolve_config_root();

    // A fresh harness config root seeds the `auto` default.
    assert_eq!(
        read_ui(|app| app.get_settings_display_backend().to_string()),
        "Auto",
        "display backend: a fresh config must seed the Auto default"
    );

    // Render + select: the real click opens the popup offering
    // exactly the shipped option set, and the Wayland click lands.
    // The testing backend exposes every popup ListItem through
    // several arena nodes, so the labels dedupe before the
    // exact-set comparison.
    let mut options = open_display_backend_popup(client, root, Some("Wayland"));
    options.sort();
    options.dedup();
    assert_eq!(
        options,
        vec!["Auto", "Wayland", "X11"],
        "display backend: the selector must offer exactly Auto / X11 / Wayland"
    );

    // Persisted: the token (not the label) survives in the config.
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("display backend: reload settings: {error}"));
    assert_eq!(
        persisted.display_backend, "wayland",
        "display backend: the Wayland selection must persist as the 'wayland' token"
    );

    // Surfacing: the structured Wayland status is logged,
    // never a silent fallback claim — the unavailability naming the
    // missing build piece in builds without the capture feature, or
    // the portal-path note in builds with it.
    let wayland_log_reason = {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        state
            .logging
            .messages
            .iter()
            .find(|entry| {
                entry.message.contains("Wayland")
                    && (entry.message.contains("not built into this binary")
                        || entry.message.contains("ScreenCast portal"))
            })
            .map(|entry| entry.message.clone())
    };
    let wayland_log_reason = wayland_log_reason.unwrap_or_else(|| {
        panic!(
            "display backend: selecting Wayland must log the structured unavailability; \
             no such log entry found"
        )
    });
    assert!(
        wayland_log_reason.contains("next runtime start"),
        "the unavailability log must state when the setting applies: {wayland_log_reason}"
    );

    // Restart re-seed: reset the property, then run the exact
    // startup projection — only the persisted token can explain the
    // re-seeded selection.
    read_ui(|app| app.set_settings_display_backend("Auto".into()));
    {
        let config_root = config_root.clone();
        on_ui(move |app| {
            crate::actions::settings::apply_settings_display_backend_projection(app, &config_root);
        });
    }
    assert_eq!(
        read_ui(|app| app.get_settings_display_backend().to_string()),
        "Wayland",
        "display backend: the startup projection must re-seed Wayland from disk"
    );

    // Leave the throwaway config root on the default path.
    open_display_backend_popup(client, root, Some("Auto"));
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("display backend: reload settings: {error}"));
    assert_eq!(
        persisted.display_backend, "auto",
        "display backend: selecting Auto must persist the 'auto' token"
    );
    eprintln!(
        "[gui-presets] display-backend selector scenario complete (render, persist, \
         Wayland unavailability, restart re-seed)"
    );
}

/// Open a language ComboBox by its accessible label with a real click
/// and return the deduped popup option labels the popup currently
/// instantiates. `expected` is the combo's full option set and `selection`
/// reads the root property the combo is bound to. When `select` is
/// `Some(option)`, click that popup `ListItem` through the real pointer
/// pipeline and assert the selection landed on that property.
///
/// The ComboBox popup is a virtualized ListView: only the viewport
/// rows (~7 of the 14) are instantiated at a time, so the full option
/// set is asserted on the projected model (the presets-scenario
/// pattern), never on the popup nodes. The popup labels must be a
/// non-empty subset of the model, which also proves the popup opened.
fn open_language_popup(
    client: &McpClient,
    root: McpHandle,
    combo_label: &str,
    expected: &[String],
    selection: fn(&AppWindow) -> String,
    select: Option<&str>,
) -> Vec<String> {
    let query_labels = |client: &McpClient| -> Vec<(String, Option<McpHandle>)> {
        let items = client
            .query_element_descendants(root, descendants_with_role("ListItem"), true)
            .unwrap_or_else(|error| harness_panic("query language popup items", &error));
        items
            .into_iter()
            .filter_map(|handle| {
                let properties = client
                    .get_element_properties(handle)
                    .unwrap_or_else(|error| harness_panic("read language popup item", &error));
                str_property(&properties, "accessibleLabel")
                    .map(str::to_string)
                    .map(|label| (label, Some(handle)))
            })
            // The affix screen's preset entry rows are ListItems too;
            // keep only the popup's language options.
            .filter(|(label, _)| expected.contains(label))
            .collect()
    };

    let static_label = combo_label.to_string();
    click_on_label(&static_label);
    let mut labels = query_labels(client);
    if labels.is_empty() {
        // Race fallback: the first click may have landed before the
        // popup registered its items (or toggled a stale popup); one
        // more real click leaves the combo's popup open either way.
        click_on_label(&static_label);
        labels = query_labels(client);
    }
    let mut exposed: Vec<String> = labels.iter().map(|(label, _)| label.clone()).collect();
    assert!(
        !exposed.is_empty(),
        "language: the {combo_label:?} ComboBox popup did not expose any language option \
         after real clicks"
    );
    exposed.sort();
    exposed.dedup();

    if let Some(option) = select {
        // The popup viewport instantiates ~7 of the 14 rows (ListView
        // virtualization); the clicked option must be reachable
        // without scrolling the popup.
        let item = labels
            .iter()
            .find_map(|(label, handle)| (label == option).then_some(*handle))
            .flatten()
            .unwrap_or_else(|| {
                panic!(
                    "language: option {option:?} not instantiated in the {combo_label:?} \
                     popup viewport (exposed: {exposed:?})"
                )
            });
        client
            .click_element(item, None)
            .unwrap_or_else(|error| harness_panic("click the language option", &error));
        assert_eq!(
            read_ui(selection),
            option,
            "language: selecting {option:?} must update the combo's selection"
        );
    }
    exposed
}

/// Language-selection scenario, run inside the settings
/// presets scenario right after the display-backend leg (Settings
/// screen open, throwaway config root, persisted language `enUS`).
///
/// Decision points covered: the affix-screen combo exposes exactly the 14
/// upstream ids; a selection persists the token and runs the in-memory
/// `set_language` chain without touching the UI language; every real
/// selection is preparation-relevant (the settings hot-reload monitor
/// requests the OCR/fuzzy rebuild — the live-prep signature check);
/// the startup projection re-seeds from disk (restart); a hand-edited
/// `Settings.json` seeds the same end state as a GUI selection; an
/// unknown stored token selects nothing and is never rewritten.
fn scenario_language_selection(client: &McpClient, root: McpHandle) {
    let config_root = crate::actions::settings::resolve_config_root();

    // A fresh harness config root seeds the upstream default id.
    assert_eq!(
        read_ui(|app| app.get_selected_language().to_string()),
        "enUS",
        "language: a fresh config must seed the enUS default"
    );

    // Live-prep signature baseline taken BEFORE the first selection:
    // every persisted language change below must turn into a reload
    // request (selected_affix_language is part of the runtime
    // preparation signature).
    let mut monitor =
        crate::runtime_bridge::settings_reload::SettingsReloadMonitor::new(&config_root);

    // First leg, on the affix screen: the projected model offers exactly the
    // 14 upstream ids in upstream order and the frFR click persists the
    // token. (The popup itself virtualizes — the model is the option-set
    // oracle, the popup subset check lives in `open_language_popup`.)
    click_on_ui("Current Item");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        0,
        "language scenario: nav click must switch to the affix screen"
    );
    let expected: Vec<String> = d4lc_gui::settings::AFFIX_LANGUAGE_IDS
        .iter()
        .map(|id| id.to_string())
        .collect();
    let data_language: fn(&AppWindow) -> String = |app| app.get_selected_language().to_string();
    let options = open_language_popup(
        client,
        root,
        "Affix language",
        &expected,
        data_language,
        Some("frFR"),
    );
    let model = read_ui(|app| {
        let model = app.get_language_options();
        (0..model.row_count())
            .map(|row| {
                model
                    .row_data(row)
                    .expect("language option row")
                    .to_string()
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        model, expected,
        "language: the projected combo model must be exactly the 14 upstream ids in \
         upstream order"
    );
    assert!(
        options.iter().all(|option| expected.contains(option)),
        "language: the popup viewport must only expose model options"
    );
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("language: reload settings: {error}"));
    assert_eq!(
        persisted.selected_affix_language, "frFR",
        "language: the frFR selection must persist the upstream token"
    );
    assert_eq!(
        persisted.selected_app_language, "en-US",
        "language: the game-data language must not touch the UI language"
    );
    assert_eq!(
        HARNESS_APP_STATE
            .get()
            .expect("harness app state")
            .lock()
            .expect("app state lock")
            .shell
            .settings
            .language,
        "frFR",
        "language: the selection must run the in-memory set_language chain"
    );
    assert!(
        matches!(
            monitor.poll(),
            crate::runtime_bridge::settings_reload::SettingsReloadDecision::Changed {
                ref reprepare,
                ..
            } if reprepare.iter().any(|key| key == "selected_affix_language")
        ),
        "language: the persisted frFR change must request the OCR/fuzzy rebuild"
    );
    assert_eq!(
        read_ui(|app| app.get_selected_app_language().to_string()),
        "English",
        "language: the settings screen's UI-language combo must be unaffected"
    );

    // Second leg: another data language goes through the same chain.
    open_language_popup(
        client,
        root,
        "Affix language",
        &expected,
        data_language,
        Some("deDE"),
    );
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("language: reload settings: {error}"));
    assert_eq!(
        persisted.selected_affix_language, "deDE",
        "language: the deDE selection must persist"
    );
    assert!(
        matches!(
            monitor.poll(),
            crate::runtime_bridge::settings_reload::SettingsReloadDecision::Changed {
                ref reprepare,
                ..
            } if reprepare.iter().any(|key| key == "selected_affix_language")
        ),
        "language: the deDE change must request the rebuild just like the first leg"
    );

    // Restart re-seed: reset the property, then run the exact startup
    // projection — only the persisted token can explain the re-seeded
    // selection.
    read_ui(|app| app.set_selected_language("".into()));
    {
        let config_root = config_root.clone();
        on_ui(move |app| {
            crate::actions::settings::apply_settings_language_projection(app, &config_root);
        });
    }
    assert_eq!(
        read_ui(|app| app.get_selected_language().to_string()),
        "deDE",
        "language: the startup projection must re-seed deDE from disk"
    );

    // Hand-edited Settings.json parity: a hand-written known token
    // seeds the same end state a GUI selection produces. The UI event
    // loop may still carry a queued persist from an earlier real
    // click, so each attempt first drains the queue (a read_ui round
    // trip is a FIFO barrier), then re-writes the hand edit, then
    // projects — a GUI persist that lands between the write and the
    // projection would otherwise race the hand edit (last writer
    // wins) and flap this leg under parallel test load.
    let mut hand_edit_seeded = false;
    for _attempt in 0..10 {
        read_ui(|_app| ());
        {
            let mut settings = d4lc_config::load_app_settings(&config_root)
                .unwrap_or_else(|error| panic!("language: reload settings: {error}"));
            settings.selected_affix_language = "esES".to_string();
            d4lc_config::save_app_settings(&config_root, &settings)
                .unwrap_or_else(|error| panic!("language: hand-edit save: {error}"));
        }
        read_ui(|app| app.set_selected_language("".into()));
        {
            let config_root = config_root.clone();
            on_ui(move |app| {
                crate::actions::settings::apply_settings_language_projection(app, &config_root);
            });
        }
        if read_ui(|app| app.get_selected_language().to_string()) == "esES" {
            hand_edit_seeded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        hand_edit_seeded,
        "language: a hand-edited Settings.json must seed the same end state"
    );

    // Unknown stored value falls back: the combo selects
    // nothing and the stored token is not rewritten. Same drain /
    // re-write / project recipe as the hand-edit leg above — a queued
    // GUI persist landing between the write and the projection would
    // otherwise race the unknown token (last writer wins) and flap
    // this leg.
    let mut unknown_token_seeded = false;
    for _attempt in 0..10 {
        read_ui(|_app| ());
        {
            let mut settings = d4lc_config::load_app_settings(&config_root)
                .unwrap_or_else(|error| panic!("language: reload settings: {error}"));
            settings.selected_affix_language = "klingon".to_string();
            d4lc_config::save_app_settings(&config_root, &settings)
                .unwrap_or_else(|error| panic!("language: unknown-token save: {error}"));
        }
        read_ui(|app| app.set_selected_language("enUS".into()));
        {
            let config_root = config_root.clone();
            on_ui(move |app| {
                crate::actions::settings::apply_settings_language_projection(app, &config_root);
            });
        }
        if read_ui(|app| app.get_selected_language().to_string()).is_empty() {
            unknown_token_seeded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        unknown_token_seeded,
        "language: an unknown stored token must select nothing, not a silent default"
    );
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("language: reload settings: {error}"));
    assert_eq!(
        persisted.selected_affix_language, "klingon",
        "language: the fallback must not rewrite the stored token"
    );

    // Restore through the real GUI path: selecting enUS rewrites the
    // stored token and leaves panel + disk on the upstream default for
    // the scenarios that follow.
    open_language_popup(
        client,
        root,
        "Affix language",
        &expected,
        data_language,
        Some("enUS"),
    );
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("language: reload settings: {error}"));
    assert_eq!(
        persisted.selected_affix_language, "enUS",
        "language: the GUI selection must overwrite the stale stored token"
    );

    // Leave the tab state as the presets scenario delivered it.
    click_on_ui("Settings");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        4,
        "language scenario: must end on the Settings screen"
    );
    eprintln!(
        "[gui-presets] language-selection scenario complete (14-id model, persist, \
         rebuild requests, restart re-seed, hand-edit parity, fallback)"
    );
}

/// UI-language scenario, run inside the settings presets scenario right
/// after the game-data language scenario (Settings screen open, throwaway
/// config root, persisted UI language `en-US`).
///
/// Decision points covered: the combo offers the five upstream cultures; a
/// real pick of Spanish (EU) persists `es-ES`, relabels the `@tr` text (a
/// toggle caption) and the Rust-projected tab captions in place, and leaves
/// the game-data language alone; picking English restores both; the startup
/// projection applies a hand-edited stored culture.
fn scenario_app_language_selection(client: &McpClient, root: McpHandle) {
    use slint::Model as _;

    let config_root = crate::actions::settings::resolve_config_root();
    let captions: Vec<String> = d4lc_gui::settings::APP_LANGUAGES
        .iter()
        .map(|language| language.name.to_string())
        .collect();
    let selection: fn(&AppWindow) -> String = |app| app.get_selected_app_language().to_string();
    let settings_tab_caption = || {
        read_ui(|app| {
            app.get_tab_labels()
                .row_data(4)
                .map(|label| label.to_string())
                .unwrap_or_default()
        })
    };

    // A fresh harness config root seeds the upstream default culture, and the
    // window starts in English whatever the desktop locale is.
    assert_eq!(
        read_ui(selection),
        "English",
        "app language: a fresh config must seed en-US"
    );
    assert_label_findable("Controller Mode");
    assert_eq!(settings_tab_caption(), "Settings");

    open_language_popup(
        client,
        root,
        "App language",
        &captions,
        selection,
        Some("Spanish (EU)"),
    );
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("app language: reload settings: {error}"));
    assert_eq!(
        persisted.selected_app_language, "es-ES",
        "app language: the pick must persist the culture id"
    );
    assert_eq!(
        persisted.selected_affix_language, "enUS",
        "app language: the game-data language must not change"
    );
    assert_eq!(
        label_count("Controller Mode"),
        0,
        "app language: the English caption must be gone"
    );
    assert_label_findable("Modo Mando");
    assert_eq!(
        settings_tab_caption(),
        "Parámetros",
        "app language: the Rust-projected tab caption must follow the language"
    );

    // The model is the option-set oracle (the popup virtualizes).
    let model = read_ui(|app| {
        let model = app.get_app_language_options();
        (0..model.row_count())
            .filter_map(|row| model.row_data(row))
            .map(|caption| caption.to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(model, captions, "app language: the five upstream cultures");

    // Back to English through the same combo.
    open_language_popup(
        client,
        root,
        "App language",
        &captions,
        selection,
        Some("English"),
    );
    assert_label_findable("Controller Mode");
    assert_eq!(label_count("Modo Mando"), 0);
    assert_eq!(settings_tab_caption(), "Settings");
    let persisted = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("app language: reload settings: {error}"));
    assert_eq!(persisted.selected_app_language, "en-US");

    // Restart projection: a hand-edited culture is applied from disk.
    let mut settings = d4lc_config::load_app_settings(&config_root)
        .unwrap_or_else(|error| panic!("app language: reload settings: {error}"));
    settings.selected_app_language = "es-ES".to_string();
    d4lc_config::save_app_settings(&config_root, &settings)
        .unwrap_or_else(|error| panic!("app language: hand-edit save: {error}"));
    read_ui(|app| app.set_selected_app_language("".into()));
    {
        let config_root = config_root.clone();
        on_ui(move |app| {
            crate::actions::settings::apply_settings_language_projection(app, &config_root);
        });
    }
    assert_eq!(read_ui(selection), "Spanish (EU)");
    assert_label_findable("Modo Mando");
    // A restart also hydrates the panel state from the same file.
    {
        let mut state = HARNESS_APP_STATE
            .get()
            .expect("harness app state")
            .lock()
            .expect("app state lock");
        state.shell.settings.app_language = "es-ES".to_string();
        state.bump_rules_input_revision();
    }

    // Restore through the real path so the scenarios that follow run in
    // English on a default config.
    open_language_popup(
        client,
        root,
        "App language",
        &captions,
        selection,
        Some("English"),
    );
    assert_label_findable("Controller Mode");
    eprintln!(
        "[gui-presets] app-language scenario complete (pick, relabel, persist, restart seed)"
    );
}
