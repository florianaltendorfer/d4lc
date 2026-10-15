use super::*;

/// The URL-field placeholders (the accessible labels of the fields).
pub(super) const D2CORE_PLACEHOLDER: &str = "e.g. https://www.d2core.com/d4/planner?bd=23tb";
pub(super) const MOBALYTICS_PLACEHOLDER: &str =
    "e.g. https://mobalytics.gg/diablo-4/builds/warlock-blazing-scream";

// ── Build-import URL input scenarios on the tabbed dialog
//    (build-import-url surface, migrated to the local-merge-tabs tabs) ────────

/// Find a TextInput inside the open dialog by its accessible label
/// (the placeholder doubles as the label, matching the AffixScreen
/// DarkLineEdit convention).
fn find_url_input(client: &McpClient, root: McpHandle, placeholder: &str) -> McpHandle {
    client
        .query_element_descendants(root, descendants_with_role("TextInput"), true)
        .unwrap_or_else(|error| harness_panic("query URL TextInput", &error))
        .into_iter()
        .find(|handle| {
            client
                .get_element_properties(*handle)
                .ok()
                .is_some_and(|properties| {
                    str_property(&properties, "accessibleLabel") == Some(placeholder)
                })
        })
        .unwrap_or_else(|| {
            panic!("build-import-url: no TextInput labelled {placeholder:?} in the open dialog")
        })
}

/// Type `text` into the field identified by `placeholder` through the
/// real key pipeline (click into the field, clear, type). Returns the
/// field handle.
pub(super) fn type_into_field(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
    placeholder: &str,
    text: &str,
) -> McpHandle {
    let input = find_url_input(client, root, placeholder);
    client
        .click_element(input, None)
        .unwrap_or_else(|error| harness_panic("click into the field", &error));
    client
        .set_element_value(input, "")
        .expect("clear the field");
    for character in text.chars() {
        client
            .dispatch_key_event(window, &character.to_string())
            .unwrap_or_else(|error| harness_panic("type into the field", &error));
    }
    input
}

/// The static shape of one provider tab case: chip label, upstream tab
/// index, provider token, URL-field placeholder, a garbage input, a
/// provider-valid input, the expected extracted id for the valid
/// input, and the getters for the per-provider extracted-id and
/// add-enabled props.
struct ImportTabCase {
    // The website combo selection whose Import opens the tab (upstream
    // `ImportAffixPresetCommandExecute`).
    website: &'static str,
    expected_tab: i32,
    provider: &'static str,
    placeholder: &'static str,
    garbage: &'static str,
    valid: &'static str,
    valid_extracted: &'static str,
    extracted: fn(&AppWindow) -> String,
    add_enabled: fn(&AppWindow) -> bool,
}

/// One provider tab, driven through real input:
///
/// 1. the chip opens the tabbed dialog on the provider's tab;
/// 2. a garbage input keeps Add disabled and the extracted-id box
///    empty — upstream parity: invalid input never errors;
/// 3. a provider-valid input fills the extracted id live and enables
///    Add (the typed URL lands in the per-provider URL prop).
fn import_tab_case(client: &McpClient, window: McpHandle, root: McpHandle, case: &ImportTabCase) {
    let ImportTabCase {
        website,
        expected_tab,
        provider,
        placeholder,
        garbage,
        valid,
        valid_extracted,
        extracted,
        add_enabled,
    } = *case;
    select_website(client, root, website);
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "import_preset",
        "local-merge-tabs: clicking {website:?} must open import_preset (saw visible={visible}, \
         id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        expected_tab,
        "local-merge-tabs: the {website:?} chip must land on tab {expected_tab} ({provider})"
    );
    take_evidence_screenshot(
        client,
        window,
        "build-import-url",
        &format!("dialog-import-{provider}.png"),
    );

    // Garbage input: no error anywhere — Add stays disabled and the
    // extracted-id box stays empty (upstream CanAdd*BuildExecute).
    type_into_field(client, window, root, placeholder, garbage);
    assert!(
        !read_ui(add_enabled),
        "local-merge-tabs: the garbage input must keep Add disabled on {provider}"
    );
    assert!(
        read_ui(extracted).is_empty(),
        "local-merge-tabs: the garbage input must leave the extracted id empty on {provider}"
    );
    take_evidence_screenshot(
        client,
        window,
        "build-import-url",
        &format!("dialog-import-{provider}-garbage.png"),
    );

    // Provider-valid input: live extracted id + enabled Add.
    type_into_field(client, window, root, placeholder, valid);
    assert_eq!(
        read_ui(extracted),
        valid_extracted,
        "local-merge-tabs: the valid input must fill the extracted id on {provider}"
    );
    assert!(
        read_ui(add_enabled),
        "local-merge-tabs: the valid input must enable Add on {provider}"
    );
    take_evidence_screenshot(
        client,
        window,
        "build-import-url",
        &format!("dialog-import-{provider}-valid.png"),
    );

    // The tabbed dialog closes via Done (upstream has no Cancel).
    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "local-merge-tabs: Done must close import_preset"
    );
}

/// The tabbed build-import scenario family: every provider chip opens
/// the dialog at its upstream tab, the per-provider URL field reaches
/// the real Rust gates on every keystroke, and invalid input never
/// errors (Add stays disabled). Network-free: Add is never clicked.
pub(super) fn build_import_url_scenarios(client: &McpClient, window: McpHandle, root: McpHandle) {
    eprintln!("[build-import-url] starting build-import URL scenarios (tabbed dialog)");

    let cases = [
        ImportTabCase {
            website: "D2Core.com",
            expected_tab: 2,
            provider: "D2Core",
            placeholder: D2CORE_PLACEHOLDER,
            garbage: "https://evil.example/not-a-build",
            valid: "https://www.d2core.com/d4/planner?bd=ab12",
            valid_extracted: "ab12",
            extracted: |app| app.get_import_d2core_extracted_id().to_string(),
            add_enabled: |app| app.get_import_d2core_add_enabled(),
        },
        ImportTabCase {
            website: "D4Builds.gg",
            expected_tab: 3,
            provider: "D4Builds",
            placeholder: "e.g. 660881f7-cb6a-4162-be62-29f0afeb37bf",
            garbage: "https://d4builds.gg/builds/shortbuildidentifier?var=0",
            valid: "https://d4builds.gg/builds/b7e23ec29af2230b9a3e5c4f1d8a6e2c1234/?var=0",
            valid_extracted: "b7e23ec29af2230b9a3e5c4f1d8a6e2c1234",
            extracted: |app| app.get_import_d4builds_extracted_id().to_string(),
            add_enabled: |app| app.get_import_d4builds_add_enabled(),
        },
        ImportTabCase {
            website: "InfinityBuilds.gg",
            expected_tab: 4,
            provider: "InfinityBuilds",
            placeholder: "e.g. https://infinitybuilds.gg/en/builds/pestilent-swarmskuna-spiritborn-vAB2c3aMt2",
            garbage: "https://evil.example/build/xyz",
            valid: "https://infinitybuilds.gg/en/builds/pestilent",
            valid_extracted: "https://infinitybuilds.gg/en/builds/pestilent",
            extracted: |app| app.get_import_infinity_extracted_id().to_string(),
            add_enabled: |app| app.get_import_infinity_add_enabled(),
        },
        ImportTabCase {
            website: "Maxroll.gg",
            expected_tab: 5,
            provider: "Maxroll",
            placeholder: "e.g. t3jx0916",
            garbage: "whirlwind-speedfarm",
            valid: "t3jx0916",
            valid_extracted: "t3jx0916",
            extracted: |app| app.get_import_maxroll_extracted_id().to_string(),
            add_enabled: |app| app.get_import_maxroll_add_enabled(),
        },
        ImportTabCase {
            website: "Mobalytics.gg",
            expected_tab: 6,
            provider: "Mobalytics",
            placeholder: MOBALYTICS_PLACEHOLDER,
            garbage: "https://mobalytics.gg",
            valid: "https://mobalytics.gg/diablo-4/builds/my-build",
            valid_extracted: "https://mobalytics.gg/diablo-4/builds/my-build",
            extracted: |app| app.get_import_mobalytics_extracted_id().to_string(),
            add_enabled: |app| app.get_import_mobalytics_add_enabled(),
        },
    ];

    for case in &cases {
        import_tab_case(client, window, root, case);
    }

    // Mobalytics: the persistent status line mirrors the upstream
    // three-state copy (error → build detected).
    select_website(client, root, "Mobalytics.gg");
    click_on_ui("Import preset");
    assert!(dialog_state().0 && dialog_state().1 == "import_preset");
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        6,
        "the Mobalytics chip must land on tab 6"
    );
    assert_eq!(
        read_ui(|app| app.get_import_mobalytics_url_status().to_string()),
        crate::actions::build_import_dialogs::MOBALYTICS_STATUS_INVALID,
        "an empty Mobalytics input shows the persistent error line"
    );
    type_into_field(
        client,
        window,
        root,
        MOBALYTICS_PLACEHOLDER,
        "https://mobalytics.gg/diablo-4/builds/my-build",
    );
    assert_eq!(
        read_ui(|app| app.get_import_mobalytics_url_status().to_string()),
        crate::actions::build_import_dialogs::MOBALYTICS_STATUS_BUILD_URL,
        "a build URL shows the detected line"
    );
    click_on_ui("Done");
    assert!(!dialog_state().0, "Done must close import_preset");

    eprintln!("[build-import-url] build-import URL scenarios complete");
}

// ── Browser-download gate scenarios (tabbed dialog surface) ────────

/// Evidence directory under the gitignored `debug-output/` for the
/// browser-driven provider download scenarios.
const BROWSER_DOWNLOAD_EVIDENCE_DIR: &str = "browser-download";

fn write_browser_download_screenshot(name: &str, png: &[u8]) {
    let dir = workspace_root()
        .join("debug-output")
        .join(BROWSER_DOWNLOAD_EVIDENCE_DIR);
    std::fs::create_dir_all(&dir).expect("create debug-output/browser-download");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write browser-download screenshot");
    eprintln!("[browser-download] screenshot written: {}", path.display());
}

fn browser_download_screenshot(client: &McpClient, window: McpHandle, name: &str) {
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic("browser-download screenshot", &error));
    assert_png_and_dimensions(&png);
    write_browser_download_screenshot(name, &png);
}

/// The static shape of one browser-driven provider gate case.
struct BrowserDownloadGateCase {
    // The website combo selection whose Import opens the tab.
    website: &'static str,
    expected_tab: i32,
    provider: &'static str,
    placeholder: &'static str,
    url: &'static str,
    error_needle: &'static str,
}

/// One browser-driven provider gate: the upstream URL / id gate must
/// keep Add disabled for an input that would reach the browser flow in
/// a feature build — deterministic, network- and chrome-free in every
/// build configuration.
fn browser_download_gate_case(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
    case: &BrowserDownloadGateCase,
) {
    let BrowserDownloadGateCase {
        website,
        expected_tab,
        provider,
        placeholder,
        url,
        error_needle,
    } = *case;
    select_website(client, root, website);
    click_on_ui("Import preset");
    let (visible, dialog_id) = dialog_state();
    assert!(
        visible && dialog_id == "import_preset",
        "browser-download: clicking {website:?} must open import_preset \
         (saw visible={visible}, id={dialog_id:?})"
    );
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        expected_tab,
        "browser-download: the {website:?} chip must land on tab {expected_tab}"
    );
    browser_download_screenshot(client, window, &format!("dialog-import-{provider}.png"));
    type_into_field(client, window, root, placeholder, url);
    let disabled = match provider {
        "D2Core" => !read_ui(|app| app.get_import_d2core_add_enabled()),
        "D4Builds" => !read_ui(|app| app.get_import_d4builds_add_enabled()),
        "Mobalytics" => !read_ui(|app| app.get_import_mobalytics_add_enabled()),
        other => panic!("browser-download: unknown provider {other}"),
    };
    assert!(
        disabled,
        "browser-download: the input failing the upstream {error_needle} gate must \
         keep Add disabled on {provider}"
    );
    assert!(
        !read_ui(|app| app.get_dialog_error_visible()),
        "browser-download: a gate failure must stay silent (disabled Add), never \
         an error message"
    );
    browser_download_screenshot(
        client,
        window,
        &format!("dialog-import-{provider}-gate.png"),
    );
    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "browser-download: Done must close import_preset"
    );
}

/// The browser-driven provider download gate scenarios on the tabbed
/// dialog: a URL failing the upstream gate keeps Add disabled BEFORE
/// any browser work would be launched.
pub(super) fn browser_download_gate_scenarios(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
) {
    eprintln!("[browser-download] starting gate scenarios");

    // D2Core: the planner URL passes the host gate; the `bd=` value
    // "abc" fails the upstream 4-character id gate.
    browser_download_gate_case(
        client,
        window,
        root,
        &BrowserDownloadGateCase {
            website: "D2Core.com",
            expected_tab: 2,
            provider: "D2Core",
            placeholder: D2CORE_PLACEHOLDER,
            url: "https://d2core.com/d4/planner?bd=abc",
            error_needle: "4 characters",
        },
    );

    // D4Builds: the URL passes the host gate; the longest `/`
    // segment fails the upstream 36-chars-or-hyphen id gate.
    browser_download_gate_case(
        client,
        window,
        root,
        &BrowserDownloadGateCase {
            website: "D4Builds.gg",
            expected_tab: 3,
            provider: "D4Builds",
            placeholder: "e.g. 660881f7-cb6a-4162-be62-29f0afeb37bf",
            url: "https://d4builds.gg/builds/shortbuildidentifier?var=0",
            error_needle: "36 characters",
        },
    );

    // Mobalytics: the URL passes the host gate; neither `profile` nor
    // `builds` means the upstream gate fails.
    browser_download_gate_case(
        client,
        window,
        root,
        &BrowserDownloadGateCase {
            website: "Mobalytics.gg",
            expected_tab: 6,
            provider: "Mobalytics",
            placeholder: MOBALYTICS_PLACEHOLDER,
            url: "https://mobalytics.gg/diablo-4",
            error_needle: "profile or builds",
        },
    );

    eprintln!("[browser-download] gate scenarios complete");
}

// ── URL row layout + compact progress panel ─────────────────────────
//
// Every provider tab must place Add and Web at the same spot; the progress
// panel must stay compact, block the dialog behind it, offer Cancel from
// the start, and show a classified failure with Close.

/// Evidence directory under the gitignored `debug-output/`.
const IMPORT_OVERLAY_EVIDENCE_DIR: &str = "pilots/import-overlay";

fn write_import_overlay_screenshot(name: &str, png: &[u8]) {
    let dir = workspace_root()
        .join("debug-output")
        .join(IMPORT_OVERLAY_EVIDENCE_DIR);
    std::fs::create_dir_all(&dir).expect("create the evidence directory");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write import overlay screenshot");
    eprintln!("[import-overlay] screenshot written: {}", path.display());
}

fn import_overlay_screenshot(client: &McpClient, window: McpHandle, name: &str) {
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic("import overlay screenshot", &error));
    assert_png_and_dimensions(&png);
    write_import_overlay_screenshot(name, &png);
}

/// Logical geometry `(x, y, width, height)` of an element.
type Geometry = (f32, f32, f32, f32);

/// Logical geometry of the first element with `label`.
fn label_geometry(label: &'static str) -> Geometry {
    on_ui_ctx(&format!("label_geometry({label:?})"), move |app| {
        let element = i_slint_backend_testing::ElementHandle::find_by_accessible_label(app, label)
            .next()
            .unwrap_or_else(|| panic!("import-overlay: no element labelled {label:?}"));
        let position = element.absolute_position();
        let size = element.size();
        (position.x, position.y, size.width, size.height)
    })
}

/// Open the import dialog on `website`'s tab and return the geometry of
/// its Add and Web buttons.
fn url_row_buttons(website: &str) -> (Geometry, Geometry) {
    on_ui(|app| app.invoke_open_import_preset_tab(0));
    let tab = match website {
        "D2Core.com" => 2,
        "D4Builds.gg" => 3,
        "InfinityBuilds.gg" => 4,
        "Maxroll.gg" => 5,
        "Mobalytics.gg" => 6,
        other => panic!("unknown website {other}"),
    };
    on_ui(move |app| app.invoke_open_import_preset_tab(tab));
    assert_eq!(read_ui(|app| app.get_import_tab_index()), tab);
    (
        label_geometry("Add build"),
        label_geometry("Open provider website"),
    )
}

fn assert_same_row_layout(context: &str) {
    let websites = [
        "D2Core.com",
        "D4Builds.gg",
        "InfinityBuilds.gg",
        "Maxroll.gg",
        "Mobalytics.gg",
    ];
    let reference = url_row_buttons(websites[0]);
    let (add, web) = reference;
    assert!(
        add.0 + add.2 <= web.0,
        "{context}: Add sits left of Web: {add:?} {web:?}"
    );
    assert!(
        (add.1 - web.1).abs() < 0.5,
        "{context}: Add and Web share one row"
    );
    for website in &websites[1..] {
        let (other_add, other_web) = url_row_buttons(website);
        assert_eq!(other_add, add, "{context}: Add of {website} moved");
        assert_eq!(other_web, web, "{context}: Web of {website} moved");
    }
    click_on_ui("Done");
    assert!(!dialog_state().0);
}

/// Seed a running import exactly like `start_download` (no worker runs,
/// nothing touches the network) and return its control.
fn seed_running_import(provider: &'static str) -> crate::actions::build_download::ImportControl {
    let control = crate::actions::build_download::ImportControl::new();
    {
        let state = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.build_import.active_download = Some(crate::app_state::ActiveDownload {
            id: 900,
            provider: provider.to_string(),
            subject: "https://www.d2core.com/d4/planner?bd=23tb".to_string(),
            mobalytics_url: None,
            control: control.clone(),
            started: std::time::Instant::now(),
        });
        state.bump_rules_input_revision();
    }
    read_ui(move |app| {
        app.set_import_download_active(true);
        app.set_import_progress_provider(provider.into());
        app.set_import_progress_step("Loading build page".into());
        app.set_import_progress_elapsed("3 s".into());
        app.set_import_progress_status(
            "Downloading https://www.d2core.com/d4/planner?bd=23tb.".into(),
        );
        app.set_import_progress_failed(false);
        app.set_import_progress_visible(true);
    });
    control
}

/// The URL row layout and the compact, cancellable progress panel.
pub(super) fn import_overlay_modality_scenario(
    client: &McpClient,
    window: McpHandle,
    root: McpHandle,
) {
    eprintln!("[import-overlay] starting import layout + progress scenario");

    // 1. Add and Web keep one position across every provider tab, at the
    //    default size, a narrow window, and 1.5x scaling.
    assert_same_row_layout("1150x625");
    read_ui(|app| app.window().set_size(slint::LogicalSize::new(860.0, 600.0)));
    assert_same_row_layout("860x600");
    on_ui(|app| app.invoke_open_import_preset_tab(4));
    import_overlay_screenshot(client, window, "dialog-import-narrow-infinity.png");
    click_on_ui("Done");
    read_ui(|app| {
        app.window()
            .set_size(slint::LogicalSize::new(1150.0, 625.0))
    });
    // 150 % scaling of the default 1150x625 window is a 766x416 logical
    // window: Slint lays out in logical pixels, so the layout is the same.
    // (The testing backend cannot render screenshots at a scale factor
    // other than 1, so the scaled case is driven through its logical size.)
    read_ui(|app| app.window().set_size(slint::LogicalSize::new(766.0, 416.0)));
    assert_same_row_layout("150 % (766x416 logical)");
    on_ui(|app| app.invoke_open_import_preset_tab(6));
    import_overlay_screenshot(client, window, "dialog-import-150pct-mobalytics.png");
    let (x, y, w, h) = label_geometry("Add build");
    let (wx, wy, ww, wh) = label_geometry("Open provider website");
    for (label, (x, y, w, h)) in [("Add", (x, y, w, h)), ("Web", (wx, wy, ww, wh))] {
        assert!(
            x >= 0.0 && y >= 0.0 && x + w <= 766.0 && y + h <= 416.0,
            "{label} stays inside the 150 % window: {x},{y} {w}x{h}"
        );
    }
    click_on_ui("Done");
    read_ui(|app| {
        app.window()
            .set_size(slint::LogicalSize::new(1150.0, 625.0))
    });

    // 2. The D2Core tab with the reported planner URL.
    select_website(client, root, "D2Core.com");
    click_on_ui("Import preset");
    assert_eq!(read_ui(|app| app.get_import_tab_index()), 2);
    type_into_field(
        client,
        window,
        root,
        D2CORE_PLACEHOLDER,
        "https://www.d2core.com/d4/planner?bd=23tb",
    );
    assert!(read_ui(|app| app.get_import_d2core_add_enabled()));
    assert_eq!(
        read_ui(|app| app.get_import_d2core_extracted_id().to_string()),
        "23tb"
    );
    assert_label_findable("Build ID: 23tb");
    assert_label_findable("Open entered build in browser");
    import_overlay_screenshot(client, window, "dialog-import-d2core-url.png");

    // 3. The running panel: compact, centred, modal, with Cancel.
    let control = seed_running_import("D2Core");
    import_overlay_screenshot(client, window, "dialog-import-progress-running.png");
    let (px, py, pw, ph) = label_geometry("Importing build");
    let (window_w, window_h) = read_ui(|app| {
        let size = app.window().size().to_logical(app.window().scale_factor());
        (size.width, size.height)
    });
    assert!(pw <= 400.5, "the panel stays compact: width {pw}");
    assert!(ph <= 160.0, "the panel stays compact: height {ph}");
    assert!(px > 0.0 && px + pw < window_w && py > 0.0 && py + ph < window_h);
    click_on_ui("🌐 Maxroll");
    assert_eq!(
        read_ui(|app| app.get_import_tab_index()),
        2,
        "the modal layer must block clicks on the covered tab strip"
    );
    assert_eq!(
        read_ui(|app| app.get_import_d2core_url().to_string()),
        "https://www.d2core.com/d4/planner?bd=23tb",
        "the URL survives the panel"
    );
    // A real click on Cancel cancels the job and closes the panel at once.
    click_on_ui("Cancel");
    assert!(control.is_cancelled(), "Cancel must signal the running job");
    assert!(!read_ui(|app| app.get_import_progress_visible()));
    assert!(!read_ui(|app| app.get_import_download_active()));
    assert!(
        read_ui(|app| app.get_import_d2core_add_enabled()),
        "Add works again"
    );

    // 4. A failed import: the classified error with a long, wrapping
    //    message stays up until Close.
    let _control = seed_running_import("D2Core");
    {
        let state = HARNESS_APP_STATE.get().expect("harness app state");
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .build_import_events
            .push(crate::app_state::BuildImportEvent::Completed {
                job_id: 900,
                provider: "D2Core".to_string(),
                result: Err(crate::actions::build_download::ImportError::new(
                    crate::actions::build_download::ImportErrorKind::Timeout,
                    "D2Core sent no planner data for build '23tb' before the page went quiet. "
                        .repeat(6),
                )),
                phases: Vec::new(),
                finished_at: std::time::Instant::now(),
            });
    }
    on_ui(|app| {
        let state = HARNESS_APP_STATE.get().expect("harness app state");
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::actions::build_import_dialogs::drain_build_import_events(app, &mut state);
    });
    assert!(read_ui(|app| app.get_import_progress_failed()));
    assert_eq!(
        read_ui(|app| app.get_import_progress_error_title().to_string()),
        "Timed out"
    );
    import_overlay_screenshot(client, window, "dialog-import-progress-failed.png");
    let (_, _, fw, fh) = label_geometry("Timed out");
    assert!(
        fw <= 400.5 && fh <= 260.0,
        "a long error scrolls instead of growing: {fw}x{fh}"
    );
    click_on_ui("Close");
    assert!(!read_ui(|app| app.get_import_progress_visible()));

    click_on_ui("Done");
    assert!(
        !dialog_state().0,
        "Done closes import_preset after the panel"
    );
    eprintln!("[import-overlay] import layout + progress scenario complete");
}
