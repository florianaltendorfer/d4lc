#[cfg(feature = "wayland-portal")]
use super::wayland::portal_stream_reference;
use super::*;
#[cfg(feature = "real-runtime")]
use crate::{app_state::AppState, runtime_bridge::RuntimeEvent};
#[cfg(feature = "real-runtime")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "real-runtime")]
fn make_unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{unique}"));
    std::fs::create_dir_all(&dir).expect("temp dir should be creatable for seam tests");
    dir
}

// ---- resolve_runtime_config tests ----
#[test]
fn resolve_runtime_config_fails_without_valid_config_root() {
    // resolve_runtime_config is the real-runtime boundary; a nonexistent
    // config root must be rejected before load_app_settings() is called.
    let result = resolve_runtime_config(Path::new("/nonexistent/path"), None, None, None);
    assert!(
        result.is_err(),
        "resolve_runtime_config should fail when config root does not exist"
    );
    match result {
        Err(AppRuntimePrepareError::ConfigLoadFailed(_)) => {} // expected
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn resolve_runtime_config_succeeds_with_existing_config_root() {
    // Even with an existing but empty config root, the settings file may
    // be absent; load_app_settings returns defaults and resolution passes.
    // Downstream preparation (build_prepared_detector) will still validate
    // that the resolved preset/templates are usable.
    let dir = std::env::temp_dir().join("d4lc-test-root-succeeds");
    let _ = std::fs::create_dir_all(&dir);
    let result = resolve_runtime_config(&dir, None, None, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        result.is_ok(),
        "resolve_runtime_config should succeed with an existing config root directory"
    );
}

// ---- display-backend selection seam applied to preparation ----
// (The seam's own parse/status unit tests live in
// `crate::display_backend`, which compiles in every build.)

/// Write a settings fixture selecting the Wayland display backend.
fn write_wayland_settings_fixture(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "d4lc-test-wayland-seam-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create config root");
    std::fs::create_dir_all(dir.join("Config")).expect("create Config dir");
    std::fs::write(
        dir.join(d4lc_config::SETTINGS_PATH),
        format!(
            "{{\n  \"schema_version\": {},\n  \"DisplayBackend\": \"wayland\"\n}}\n",
            d4lc_config::SETTINGS_SCHEMA_VERSION
        ),
    )
    .expect("write settings fixture");
    dir
}

#[cfg(not(feature = "wayland-portal"))]
#[test]
fn prepare_app_runtime_rejects_wayland_selection_with_structured_unavailability() {
    // End-to-end over the seam (build without the capture feature): a
    // persisted `DisplayBackend: "wayland"` stops runtime preparation
    // with the structured unavailability (surfaced later on the
    // status surface + log), deterministically and without touching
    // X11 — no crash, no silent X11 run.
    let dir = write_wayland_settings_fixture("refused");

    let result = resolve_runtime_config(&dir, None, None, None);
    match result {
        Ok(config) => {
            // The seam itself resolves successfully — the
            // status travels with the config.
            assert_eq!(config.display_backend, DisplayBackend::Wayland);
            assert!(matches!(
                config.display_backend_status,
                DisplayBackendStatus::WaylandUnavailable { .. }
            ));
        }
        Err(error) => panic!("seam resolution must succeed with a status: {error}"),
    }

    // ...and full preparation refuses with the structured variant.
    let result = prepare_app_runtime(&dir, None, None, None, None, None);
    match result {
        Err(AppRuntimePrepareError::DisplayBackendUnavailable(reason)) => {
            assert!(
                reason.contains("not built into this binary"),
                "unavailability must name the missing build piece: {reason}"
            );
        }
        Err(error) => panic!(
            "Wayland selection must stop preparation with DisplayBackendUnavailable: {error}"
        ),
        Ok(_) => panic!("Wayland selection must not start preparation"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "wayland-portal")]
#[test]
fn prepare_without_portal_refuses_a_wayland_engine_by_name() {
    // A caller beside the live loop (the debug export) must never open
    // a portal session: the Wayland resolution stops with the named
    // refusal before any portal call, instead of waiting on the
    // session slot or taking it from a loop that is still preparing.
    let dir = write_wayland_settings_fixture("no-portal");
    match prepare_app_runtime_without_portal(&dir, "the test caller") {
        Err(AppRuntimePrepareError::PortalSessionNotAllowed(caller)) => {
            assert_eq!(caller, "the test caller");
        }
        Err(error) => panic!("expected PortalSessionNotAllowed, got: {error}"),
        Ok(_) => panic!("a portal-free preparation must not prepare a Wayland engine"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "wayland-portal")]
#[test]
fn resolve_runtime_config_carries_the_wayland_capture_path_and_restore_token() {
    // Build with the capture feature: the seam resolves Wayland to
    // the real path, and a stored restore token travels with the
    // runtime config. Preparation itself would open the compositor
    // consent dialog, so only the resolution is exercised here — the
    // live portal session is covered by the opt-in capture test.
    let dir = std::env::temp_dir().join(format!(
        "d4lc-test-wayland-seam-enabled-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create config root");
    std::fs::create_dir_all(dir.join("Config")).expect("create Config dir");
    std::fs::write(
        dir.join(d4lc_config::SETTINGS_PATH),
        format!(
            "{{\n  \"schema_version\": {},\n  \"DisplayBackend\": \"wayland\",\n  \
             \"WaylandRestoreToken\": \"token-from-last-session\"\n}}\n",
            d4lc_config::SETTINGS_SCHEMA_VERSION
        ),
    )
    .expect("write settings fixture");

    let config = resolve_runtime_config(&dir, None, None, None)
        .unwrap_or_else(|error| panic!("wayland config must resolve: {error}"));
    assert_eq!(config.display_backend, DisplayBackend::Wayland);
    assert!(
        matches!(
            config.display_backend_status,
            DisplayBackendStatus::Wayland { .. }
        ),
        "the explicit wayland status must resolve to the portal engine: {:?}",
        config.display_backend_status
    );
    assert_eq!(
        config.wayland_restore_token.as_deref(),
        Some("token-from-last-session")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn app_runtime_prepare_error_display_backend_unavailable_display() {
    let err = AppRuntimePrepareError::DisplayBackendUnavailable("no wayland yet".into());
    assert_eq!(
        err.to_string(),
        "display backend unavailable: no wayland yet"
    );
}

// ---- resolve_capture_source tests ----

#[test]
fn resolve_capture_source_errors_without_window_and_no_manual_region() {
    let result = resolve_capture_source(None, None);
    assert!(
        result.is_err(),
        "expected error when no X11 window and no manual region"
    );
    match result {
        Err(AppRuntimePrepareError::CaptureSourceFailed(_)) => {} // expected
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn resolve_capture_source_accepts_manual_region() {
    let region = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let result = resolve_capture_source(None, Some(region));
    // Without X11 display, this may fail.  We verify the error is
    // structured (CaptureSourceFailed), not a panic or String::new().
    if let Err(e) = &result {
        assert!(
            matches!(e, AppRuntimePrepareError::CaptureSourceFailed(_)),
            "wrong error variant: {e:?}"
        );
    }
}

// ---- load_template_image tests ----

#[test]
fn load_template_image_fails_for_missing_file() {
    let result = load_template_image(Path::new("/nonexistent/template.png"));
    assert!(result.is_err(), "expected error for missing template file");
}

// ---- error display tests ----

#[test]
fn app_runtime_prepare_error_config_failed_display() {
    let err = AppRuntimePrepareError::ConfigLoadFailed("file not found".into());
    assert_eq!(err.to_string(), "config load failed: file not found");
}

#[test]
fn app_runtime_prepare_error_capture_source_display() {
    let err = AppRuntimePrepareError::CaptureSourceFailed("no display".into());
    assert_eq!(err.to_string(), "capture source failed: no display");
}

#[test]
fn app_runtime_prepare_error_template_load_display() {
    let err = AppRuntimePrepareError::TemplateLoadFailed("missing preset".into());
    assert_eq!(err.to_string(), "template load failed: missing preset");
}

#[test]
fn error_from_string_conversion() {
    let err: AppRuntimePrepareError = "something went wrong".to_string().into();
    assert!(
        matches!(err, AppRuntimePrepareError::ConfigLoadFailed(_)),
        "String should convert to ConfigLoadFailed variant"
    );
}

#[cfg(not(feature = "leptess-backend"))]
#[test]
fn build_ocr_worker_needs_the_ocr_backend() {
    let result = build_ocr_worker(Path::new("/nonexistent"), &dummy_runtime_config());
    assert!(matches!(
        result,
        Err(AppRuntimePrepareError::OcrBackendNotAvailable)
    ));
}

#[test]
fn resolve_game_data_root_carries_the_vendored_localized_data() {
    // The live runtime's OCR session reads `assets/original/data` (the
    // fuzzy index behind the affix ids) from this root. It must be the
    // checkout/bundle root carrying the vendored tree, never the per-user
    // config root — `~/.config/d4lc` holds only `Config/`, `Builds/`, and
    // `Images/`. The config-root argument is therefore irrelevant whenever
    // the vendored tree resolves (the same property the build-import and
    // affix-catalog seams assert).
    let root = resolve_game_data_root(Path::new("/nonexistent-config-root"));
    assert!(
        root.join(d4lc_assets::DATA_DIR).is_dir(),
        "resolved game-data root must carry {}: {}",
        d4lc_assets::DATA_DIR,
        root.display()
    );
}

fn dummy_runtime_config() -> RuntimeConfig {
    RuntimeConfig {
        preset: "1440p_SMF".into(),
        language: "enUS".into(),
        mode: InputMode::Mouse,
        scan_height_percent: 50.0,
        tooltip_width: 500,
        thresholds: ThresholdSettings {
            min: 0,
            max: 255,
            similarity_tooltip: 0.8,
        },
        controller_config: Vec::new(),
        capture_backend: CaptureBackend::Auto,
        display_backend: DisplayBackend::X11,
        display_backend_status: DisplayBackendStatus::X11 {
            reason: "explicit setting".to_string(),
        },
        wayland_restore_token: None,
        ocr_interval: Duration::from_millis(250),
        is_debug_info_enabled: false,
    }
}

// ---- prepare_app_runtime integration tests ----

#[test]
fn prepare_app_runtime_fails_without_config_root() {
    // The full production preparation path must reject a nonexistent
    let result = prepare_app_runtime(Path::new("/nonexistent/path"), None, None, None, None, None);
    assert!(
        result.is_err(),
        "prepare_app_runtime should fail without a valid config root"
    );
    match &result {
        Err(AppRuntimePrepareError::ConfigLoadFailed(msg)) => {
            assert!(
                msg.contains("nonexistent"),
                "error should mention the missing path: {msg}"
            );
        }
        _ => panic!("expected ConfigLoadFailed"),
    }
}

#[test]
fn prepare_app_runtime_fails_with_empty_config_root() {
    // Even with an existing config root, if no runtime prerequisites
    // (templates, capture target, OCR) can be resolved, preparation must
    // not silently succeed.  Acceptable errors are TemplateLoadFailed,
    // CaptureSourceFailed, or OcrBackendNotAvailable depending on feature
    // flags and environment — the key assertion is Err, not Ok.
    let dir = std::env::temp_dir().join("d4lc-test-empty-root");
    let _ = std::fs::create_dir_all(&dir);
    let result = prepare_app_runtime(&dir, None, None, None, None, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        result.is_err(),
        "prepare_app_runtime must not silently succeed with empty config root"
    );
}

#[cfg(feature = "real-runtime")]
#[test]
fn prepare_app_runtime_empty_root_surfaces_structured_prereq_error() {
    let dir = make_unique_temp_dir("d4lc-test-real-runtime-empty-root");
    let result = prepare_app_runtime(&dir, None, None, None, None, None);
    let _ = std::fs::remove_dir_all(&dir);

    match result {
        Err(
            AppRuntimePrepareError::TemplateLoadFailed(_)
            | AppRuntimePrepareError::CaptureSourceFailed(_)
            | AppRuntimePrepareError::DisplayBackendUnavailable(_)
            | AppRuntimePrepareError::NotReadyForEvaluation(_)
            | AppRuntimePrepareError::UnsupportedBackend(_),
        ) => {}
        // Without the OCR backend no runtime can start.
        #[cfg(not(feature = "leptess-backend"))]
        Err(AppRuntimePrepareError::OcrBackendNotAvailable) => {}
        Err(AppRuntimePrepareError::WaylandCaptureFailed(msg)) => {
            assert!(
                !msg.is_empty(),
                "WaylandCaptureFailed should carry the portal capture error: {msg}"
            );
        }
        Err(AppRuntimePrepareError::ConfigLoadFailed(msg)) => {
            panic!("expected runtime prerequisite failure after config root resolution, got: {msg}")
        }
        Err(AppRuntimePrepareError::PortalSessionNotAllowed(caller)) => {
            panic!("prepare_app_runtime never forbids the portal, got a refusal for: {caller}")
        }
        Ok(_) => panic!("prepare_app_runtime unexpectedly succeeded for an empty config root"),
    }
}

#[cfg(feature = "real-runtime")]
#[test]
fn real_runtime_seam_stops_before_emitting_debug_frame_without_runtime_prereqs() {
    let dir = make_unique_temp_dir("d4lc-test-real-runtime-seam");
    let app_state = Arc::new(Mutex::new(AppState::default_startup()));
    let (event_tx, event_rx) = crate::runtime_bridge::RuntimeEventSink::new();

    let handle = crate::runtime_bridge::start_live_loop(
        app_state,
        event_tx,
        dir.clone(),
        d4lc_runtime::RuntimeReloadSignal::new(),
    );

    // The loop announces its preparation first; `Started` only follows
    // a successful one, so a failed preparation never claims it.
    let first = event_rx.recv_timeout(Duration::from_millis(500));
    match first {
        Ok(RuntimeEvent::Preparing { detail }) => {
            assert_eq!(detail, "preparing the runtime");
        }
        other => panic!("expected Preparing as the first event, got {other:?}"),
    }

    let mut saw_error = false;
    let mut saw_stopped = false;

    for _ in 0..4 {
        match event_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(RuntimeEvent::Error { message }) => {
                saw_error = true;
                assert!(
                    message.contains("runtime preparation failed"),
                    "expected structured preparation error, got: {message}"
                );
            }
            Ok(RuntimeEvent::Stopped { reason }) => {
                saw_stopped = true;
                assert_eq!(reason, "preparation failed");
                break;
            }
            Ok(RuntimeEvent::CurrentItemUpdate { .. }) => {
                panic!("real-runtime seam must not emit CurrentItemUpdate when preparation fails")
            }
            Ok(RuntimeEvent::Started) => {
                panic!("Started must not be sent when preparation fails")
            }
            Ok(RuntimeEvent::Preparing { .. }) => {}
            Ok(RuntimeEvent::SettingsReloaded { .. }) => panic!(
                "real-runtime seam must not emit settings reload events when preparation fails"
            ),
            Ok(RuntimeEvent::SettingsReloadFailed { .. }) => panic!(
                "real-runtime seam must not emit settings reload events when preparation fails"
            ),
            Ok(RuntimeEvent::GameWindowObservation { .. }) => panic!(
                "real-runtime seam must not start the game-window observer when preparation fails"
            ),
            // `RuntimeEvent::LogMessage` refreshes the Logging screen (see
            // `runtime_bridge::apply_runtime_event` and
            // `actions::runtime::apply_event_to_ui`). The
            // preparation-failure path does not currently emit
            // LogMessage events, but we keep the match exhaustive
            // by treating any observed LogMessage here as
            // acceptable — the contract under test is the
            // structured `Error` / `Stopped` sequence, not the
            // absence of every other variant. If the live loop
            // later starts emitting LogMessage during the
            // prepare-failure window, this arm will turn into a
            // soft `continue` and the test will still pass.
            Ok(RuntimeEvent::LogMessage { .. }) => {}
            Err(err) => panic!("timed out waiting for runtime failure events: {err}"),
        }
    }

    handle.stop();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        saw_error,
        "expected real-runtime seam to emit an Error event"
    );
    assert!(
        saw_stopped,
        "expected real-runtime seam to emit a Stopped event after failure"
    );
}

// ---- Backend choice, evaluation prerequisites, controller list ----

/// A config root carrying an extracted copy of the vendored 1080p_SMF
/// preset and the given settings.
pub(crate) fn preset_fixture(
    tag: &str,
    mutate: impl FnOnce(&mut d4lc_config::AppSettings),
) -> std::path::PathBuf {
    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("create preset dir");
        for entry in std::fs::read_dir(from).expect("read preset dir") {
            let entry = entry.expect("preset entry");
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).expect("copy preset file");
            }
        }
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("d4lc-prep-{tag}-{unique}"));
    let repo = crate::actions::affix::catalog_repo_root().expect("checkout with assets");
    copy_tree(
        &repo.join(d4lc_assets::SYSTEM_PRESETS_DIR).join("1080p_SMF"),
        &root
            .join(d4lc_assets::EXTRACTED_PRESETS_DIR)
            .join("1080p_SMF"),
    );
    let mut settings = d4lc_config::AppSettings {
        selected_system_preset: "1080p_SMF".to_string(),
        ..d4lc_config::AppSettings::default()
    };
    mutate(&mut settings);
    d4lc_config::save_app_settings(&root, &settings).expect("save settings");
    root
}

#[test]
fn an_explicit_capture_backend_is_respected() {
    let root = preset_fixture("backends", |settings| {
        settings.capture_backend = "x11-command".to_string();
    });
    let config = resolve_runtime_config(&root, None, None, None).expect("config");
    assert_eq!(config.capture_backend, CaptureBackend::X11Command);

    // A stored choice this build does not know is refused by name.
    let mut settings = d4lc_config::load_app_settings(&root).expect("settings");
    settings.capture_backend = "gdi".to_string();
    d4lc_config::save_app_settings(&root, &settings).expect("save");
    match resolve_runtime_config(&root, None, None, None) {
        Err(AppRuntimePrepareError::UnsupportedBackend(message)) => {
            assert!(message.contains("gdi"), "{message}");
        }
        other => panic!("expected an unsupported capture backend, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_x11_capture_choice_is_unsupported_on_wayland() {
    match resolve_capture_backend("x11-native", DisplayBackend::Wayland) {
        Err(AppRuntimePrepareError::UnsupportedBackend(message)) => {
            assert!(message.contains("ScreenCast portal"), "{message}");
        }
        other => panic!("expected unsupported, got {other:?}"),
    }
    assert_eq!(
        resolve_capture_backend("auto", DisplayBackend::Wayland).ok(),
        Some(CaptureBackend::Auto)
    );
    assert_eq!(
        resolve_capture_backend("", DisplayBackend::X11).ok(),
        Some(CaptureBackend::Auto)
    );
}

#[test]
fn an_x11_capture_choice_is_refused_when_auto_resolves_to_the_portal_engine() {
    // The parse-level gate only sees explicit settings; `auto` can
    // resolve to the portal engine after it, and the refusal must
    // follow the resolved engine.
    let mut config = dummy_runtime_config();
    config.capture_backend = CaptureBackend::X11Command;
    config.display_backend_status = DisplayBackendStatus::Wayland {
        reason: "no X11 display, wayland session".to_string(),
    };
    match resolved_capture_backend_gate(&config) {
        Err(AppRuntimePrepareError::UnsupportedBackend(message)) => {
            assert!(message.contains("ScreenCast portal"), "{message}");
        }
        other => panic!("expected the resolved-engine refusal, got {other:?}"),
    }
    config.capture_backend = CaptureBackend::Auto;
    resolved_capture_backend_gate(&config)
        .unwrap_or_else(|error| panic!("auto capture is honored on the portal engine: {error}"));
    // The X11 engine never trips the resolved-engine gate.
    config.display_backend_status = DisplayBackendStatus::X11 {
        reason: "Diablo window found".to_string(),
    };
    config.capture_backend = CaptureBackend::X11Command;
    resolved_capture_backend_gate(&config)
        .unwrap_or_else(|error| panic!("x11 capture is honored on the x11 engine: {error}"));
}

/// A missing catalog blocks the evaluation-ready state.
#[test]
fn missing_catalog_data_is_not_ready_for_evaluation() {
    let root = preset_fixture("catalog", |_| {});
    let config = resolve_runtime_config(&root, None, None, None).expect("config");
    let data_less = root.join("no-game-data");
    std::fs::create_dir_all(&data_less).expect("data-less root");
    match check_evaluation_prerequisites(&root, &data_less, &config) {
        Err(AppRuntimePrepareError::NotReadyForEvaluation(message)) => {
            assert!(message.contains("catalog data missing"), "{message}");
        }
        other => panic!("expected not ready, got {other:?}"),
    }
    let repo = crate::actions::affix::catalog_repo_root().expect("checkout");
    check_evaluation_prerequisites(&root, &repo, &config)
        .unwrap_or_else(|error| panic!("the vendored catalogs are complete: {error}"));
    let _ = std::fs::remove_dir_all(&root);
}

/// Through the composed preparation: a language without catalog
/// data fails the runtime preparation before any window is looked up.
/// The fixture pins the `x11` display backend explicitly: the default
/// `auto` is session-detected, and this test's decision point is the OCR prerequisite
/// ordering, not the backend choice.
#[cfg(feature = "leptess-backend")]
#[test]
fn preparation_for_a_language_without_catalogs_is_not_ready() {
    let root = preset_fixture("language", |settings| {
        settings.selected_affix_language = "xxYY".to_string();
        settings.display_backend = "x11".to_string();
    });
    match prepare_app_runtime(&root, None, None, None, None, None) {
        Err(AppRuntimePrepareError::NotReadyForEvaluation(message)) => {
            assert!(message.contains("xxYY"), "{message}");
        }
        Err(other) => panic!("expected not ready for evaluation, got {other}"),
        Ok(_) => panic!("a runtime without catalogs must not prepare"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The capture-free check (preset reload on the UI thread) fails with
/// the same structured prerequisite error the full preparation reports,
/// and never reaches a capture step.
#[cfg(feature = "leptess-backend")]
#[test]
fn capture_free_check_reports_the_preparation_prerequisite_error() {
    let root = preset_fixture("capture-free-language", |settings| {
        settings.selected_affix_language = "xxYY".to_string();
        // An explicit Wayland selection: a capture step would be a
        // portal request (or its structured unavailability).
        settings.display_backend = "wayland".to_string();
    });
    match check_runtime_prerequisites_without_capture(&root) {
        Err(AppRuntimePrepareError::NotReadyForEvaluation(message)) => {
            assert!(message.contains("xxYY"), "{message}");
        }
        other => panic!("expected not ready for evaluation, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A fresh install: the config root holds only the settings, the selected
/// preset ships beside the application. The detector loads it from there,
/// as the OCR preparation does, instead of reporting the preset as missing.
#[test]
fn the_detector_falls_back_to_the_bundled_preset_tree() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("d4lc-prep-bundled-preset-{unique}"));
    let settings = d4lc_config::AppSettings {
        selected_system_preset: "1080p_SMF".to_string(),
        ..d4lc_config::AppSettings::default()
    };
    d4lc_config::save_app_settings(&root, &settings).expect("save settings");
    assert!(!root.join(d4lc_assets::EXTRACTED_PRESETS_DIR).exists());

    let config = resolve_runtime_config(&root, None, None, None).expect("config");
    let detector = build_prepared_detector(&root, &config).expect("detector from the bundle");
    assert_eq!(detector.preset, "1080p_SMF");
    assert!(!detector.templates.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn capture_free_check_rejects_a_missing_config_root() {
    assert!(matches!(
        check_runtime_prerequisites_without_capture(Path::new("/nonexistent/d4lc-root")),
        Err(AppRuntimePrepareError::ConfigLoadFailed(_))
    ));
}

/// Without the OCR backend the check fails instead of
/// validating a runtime that cannot start.
#[cfg(not(feature = "leptess-backend"))]
#[test]
fn capture_free_check_names_the_missing_ocr_backend() {
    let root = preset_fixture("capture-free-no-ocr", |_| {});
    assert!(matches!(
        check_runtime_prerequisites_without_capture(&root),
        Err(AppRuntimePrepareError::OcrBackendNotAvailable)
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn progress_descriptions_name_the_wait() {
    let dialog = PrepareProgress::AwaitingScreenCastSelection {
        restore_token_offered: false,
    }
    .describe();
    assert!(dialog.contains("compositor dialog"), "{dialog}");
    for text in [
        "on another monitor",
        "behind the fullscreen game",
        "the monitor Diablo IV runs on",
    ] {
        assert!(dialog.contains(text), "{dialog}");
    }

    let restore = PrepareProgress::AwaitingScreenCastSelection {
        restore_token_offered: true,
    }
    .describe();
    assert!(restore.contains("restore the stored"), "{restore}");
    assert!(restore.contains("selection dialog"), "{restore}");
    for text in [
        "on another monitor",
        "behind the fullscreen game",
        "the monitor Diablo IV runs on",
    ] {
        assert!(restore.contains(text), "{restore}");
    }

    let frame = PrepareProgress::AwaitingFirstStreamFrame.describe();
    assert!(frame.contains("first stream frame"), "{frame}");

    let reuse = PrepareProgress::ReusingScreenCastSession.describe();
    assert!(reuse.contains("no new portal session"), "{reuse}");
}

/// Only a running ScreenCast session is carried into a re-preparation;
/// a non-Wayland runtime hands over nothing, so its re-preparation
/// re-resolves its capture as before.
#[test]
fn a_non_wayland_runtime_carries_no_capture_into_its_reload() {
    assert!(ReusableCapture::none().is_empty());
    let root = preset_fixture("reuse-synthetic", |_| {});
    let config = resolve_runtime_config(&root, None, None, None).expect("config");
    let detector = build_prepared_detector(&root, &config).expect("detector");
    let source =
        d4lc_capture::SyntheticCaptureFrameSource::manual_region(d4lc_core::CaptureFrame {
            width: 4,
            height: 4,
            stride: 16,
            pixels_bgra: vec![255; 64],
        })
        .expect("synthetic source");
    let runtime = PreparedRuntime::new(CaptureBackend::Auto, source.reference().clone(), detector)
        .with_synthetic_capture_source(source);
    assert!(ReusableCapture::from_runtime(&runtime).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// A reload carries the running stream's current frame size, not the
/// geometry of the first preparation: after a game resolution change
/// the reloaded runtime publishes what the frame results report.
#[cfg(feature = "wayland-portal")]
#[test]
fn a_reused_stream_reference_follows_the_newest_frame_size() {
    let first = portal_stream_reference(1920, 1080);
    let reloaded = reused_stream_reference(&first, Some((2560, 1440)));
    let expected = Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    };
    assert_eq!(reloaded.geometry, expected);
    assert!(matches!(
        reloaded.source,
        d4lc_capture::CaptureSource::PortalStream(rect) if rect == expected
    ));
    assert_eq!(
        reused_stream_reference(&first, Some((1920, 1080))),
        first,
        "an unchanged stream keeps the same reference"
    );
    assert_eq!(reused_stream_reference(&first, None), first);
}

/// Missing Tesseract language data is named at preparation.
#[cfg(feature = "leptess-backend")]
#[test]
fn missing_ocr_language_data_is_not_ready_for_evaluation() {
    let empty = make_unique_temp_dir("d4lc-no-tessdata");
    let profile = d4lc_ocr::OcrProfile {
        backend: d4lc_ocr::OcrBackendKind::Auto,
        language: "enUS".to_string(),
        tessdata_path: Some(empty.clone()),
    };
    match check_ocr_language_data(&profile) {
        Err(AppRuntimePrepareError::NotReadyForEvaluation(message)) => {
            assert!(message.contains("OCR language data for enUS"), "{message}");
        }
        other => panic!("expected not ready, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&empty);
}

/// Load one recorded reference frame as a BGRA capture frame.
#[cfg(feature = "leptess-backend")]
fn load_reference_frame(path: &Path) -> d4lc_core::CaptureFrame {
    let rgba = image::ImageReader::open(path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", path.display()))
        .decode()
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    d4lc_core::CaptureFrame {
        width,
        height,
        stride: usize::try_from(width).expect("frame width") * 4,
        pixels_bgra: rgba
            .pixels()
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
            .collect(),
    }
}

/// The OCR input preprocessing is a measured decision, not a
/// preference. Both variants run over the same recorded reference
/// frames with the same language model (the settings language, enUS ->
/// `eng`) and the same engine registry, and every run exports its
/// observable chain (tooltip crop, segment images, raw text, TSV,
/// normalized lines, item name) under
/// `debug-output/ocr-preprocess/<capture>/<variant>/`.
///
/// The comparison line per capture and variant is printed
/// (`cargo test ... -- --nocapture`); the artifacts are the evidence.
#[cfg(feature = "leptess-backend")]
#[test]
fn reference_captures_compare_ocr_preprocess_variants() {
    use d4lc_runtime::ocr_parity::{
        export_tooltip_ocr_chain, runtime_parity_result, PreparedTooltipOcr,
        TooltipOcrPreprocessVariant,
    };

    let repo = crate::actions::affix::catalog_repo_root().expect("checkout with assets");
    let profile = d4lc_ocr::OcrProfile {
        backend: d4lc_ocr::OcrBackendKind::Auto,
        language: "enUS".to_string(),
        tessdata_path: None,
    };
    let registry = d4lc_ocr::OcrEngineRegistry::new(|p: &d4lc_ocr::OcrProfile| {
        let engine = d4lc_ocr::LeptessOcrEngine::new(p)?;
        Ok(Box::new(engine) as d4lc_ocr::BoxedOcrEngine)
    });
    // The controller frame was captured with the gamepad UI, the keyboard
    // frames with the keyboard UI (tests/fixtures/runtime/manifest.json).
    let captures = [
        ("frame-KB.png", false),
        ("d4lc-livecap.png", false),
        ("frame-CTRL.png", true),
    ];

    let mut comparison = Vec::new();
    for (file, controller_mode) in captures {
        let root = preset_fixture(&format!("ocr-preprocess-{file}"), |settings| {
            settings.controller_mode = controller_mode;
        });
        if controller_mode {
            // The shipped `Config/Controllers.json` names the active
            // controller tooltip template; the fixture root carries only
            // the preset, so the controller list has to be written too.
            d4lc_config::save_controller_config(&root, &["tooltip_gc_x.png".to_string()])
                .expect("controller config");
        }
        let settings = d4lc_config::load_app_settings(&root).expect("settings");
        let live_variant = TooltipOcrPreprocessVariant::for_settings(&settings);
        let upstream_variant = TooltipOcrPreprocessVariant::UpstreamGrayBinaryInv {
            threshold_min: settings.threshold_min,
            threshold_max: settings.threshold_max,
        };
        assert_eq!(
            live_variant, upstream_variant,
            "the live path must run the upstream preprocessing this comparison measures"
        );
        let frame = load_reference_frame(&repo.join("tests/fixtures/live-frames").join(file));
        let placement = d4lc_core::coords::TooltipPlacement::whole_frame(
            d4lc_core::coords::FramePlacement::default(),
            frame.width,
            frame.height,
        );

        let mut plans_per_variant = Vec::new();
        for variant in [upstream_variant, TooltipOcrPreprocessVariant::RawBgra] {
            let prepared = PreparedTooltipOcr::prepare_with_game_data_root(
                &root,
                &repo,
                "1080p_SMF".to_string(),
                profile.clone(),
            )
            .unwrap_or_else(|error| panic!("{file}: prepared session failed: {error}"))
            .with_preprocess_variant(variant);
            let execution = prepared
                .execute(&registry, &frame)
                .unwrap_or_else(|error| panic!("{file}: execution failed: {error}"));
            let result =
                runtime_parity_result(&execution, 80, prepared.fuzzy_indexes(), &placement);
            let dir = repo
                .join("debug-output/ocr-preprocess")
                .join(file)
                .join(variant.as_str());
            let artifacts = export_tooltip_ocr_chain(&dir, file, &execution, &result)
                .unwrap_or_else(|error| panic!("{file}: chain export failed: {error}"));

            assert_eq!(artifacts.segment_inputs.len(), execution.results.len());
            assert!(artifacts.tooltip_crop.exists());
            for segment in &execution.results {
                assert_eq!(segment.ocr_input.variant, variant);
                assert_eq!(
                    segment.ocr_input.payload,
                    match variant {
                        TooltipOcrPreprocessVariant::UpstreamGrayBinaryInv { .. } => "gray",
                        TooltipOcrPreprocessVariant::RawBgra => "bgra",
                    },
                    "{file}: the recognized payload kind must match the variant"
                );
                assert!(
                    segment.tsv.is_some(),
                    "{file}: the structured output of segment {} must be observable",
                    segment.plan.index
                );
            }
            plans_per_variant.push(
                execution
                    .results
                    .iter()
                    .map(|segment| (segment.plan.index, segment.plan.kind, segment.plan.rect))
                    .collect::<Vec<_>>(),
            );
            comparison.push(format!(
                "capture={file} variant={} segments={} lines={} name={:?} affixes={} text={:?}",
                variant.as_str(),
                result.segment_count,
                result.line_count,
                result.item.display_name,
                result.item.affixes.len(),
                result.text_preview,
            ));
        }

        // Preprocessing must never change what is planned: the same
        // segments are recognized, only from different pixels.
        assert_eq!(
            plans_per_variant[0], plans_per_variant[1],
            "{file}: the preprocessing variant must not change the segment plan"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    for line in &comparison {
        println!("COMPARE {line}");
    }
}

/// One recorded mouse-mode tooltip and what the live chain must read
/// from it (`tests/fixtures/runtime/manifest.json`, `recorded-mouse-*`).
#[cfg(feature = "leptess-backend")]
struct RecordedTooltip {
    file: &'static str,
    /// Window-relative cursor at capture time.
    cursor: (i32, i32),
    item_type: &'static str,
    item_power: Option<u32>,
    /// Per affix row: the first catalog id of the match and whether the
    /// row is a greater affix.
    affixes: &'static [(&'static str, bool)],
}

/// Recorded tooltips of a live session read through the same chain the
/// live loop runs: the prepared detector finds the tooltip from the
/// cursor, the OCR worker reads the crop with Tesseract, and the parse
/// resolves the item type, the item power and every affix row. The
/// expectations hold what the captures show; OCR-dependent value text is
/// not asserted because it may differ between Tesseract versions.
#[cfg(feature = "leptess-backend")]
#[test]
fn recorded_tooltips_read_through_the_live_chain() {
    const CASES: &[RecordedTooltip] = &[
        RecordedTooltip {
            file: "mouse-legendary-ring-comparison.png",
            cursor: (469, 760),
            item_type: "ring",
            item_power: Some(900),
            affixes: &[
                ("CoreStat_Willpower", false),
                ("S04_LifePerHit", false),
                ("CritChance", false),
                ("S04_Damage_to_Vulnerable", false),
            ],
        },
        RecordedTooltip {
            file: "mouse-unique-focus-comparison.png",
            cursor: (458, 747),
            item_type: "offhand",
            item_power: Some(850),
            affixes: &[
                ("X2_Fast_Weapon_Damage_1HAxe", false),
                ("S04_CritDamage", false),
                ("X2_DamageType_Fire", false),
                (
                    "Talisman_Charm_SkillRankBonus_Warlock_Category_Demonology",
                    false,
                ),
            ],
        },
        RecordedTooltip {
            file: "mouse-legendary-gloves-comparison.png",
            cursor: (465, 738),
            item_type: "gloves",
            item_power: Some(850),
            affixes: &[
                ("CoreStat_Willpower", false),
                ("Life", false),
                ("AttackSpeed", false),
                ("X2_DamageType_Fire", false),
            ],
        },
        RecordedTooltip {
            file: "mouse-legendary-ring-greater-affixes.png",
            cursor: (465, 419),
            item_type: "ring",
            item_power: Some(900),
            affixes: &[
                ("CoreStat_Willpower", true),
                ("Life", true),
                ("AttackSpeed", false),
                ("S04_Damage_to_Vulnerable", false),
                ("Lucky_Hit_Resource", true),
            ],
        },
        RecordedTooltip {
            file: "mouse-legendary-boots.png",
            cursor: (477, 536),
            item_type: "boots",
            item_power: Some(900),
            affixes: &[
                ("Life", true),
                ("S04_Resource_Per_Second_Wrath", false),
                (
                    "Talisman_Charm_SkillRankBonus_Warlock_Category_Hellfire",
                    false,
                ),
                ("Evade_Charges_UniqueRandom", false),
                ("Movement_Speed", false),
            ],
        },
        // Charm and horadric-seal tooltips clear their affix areas like
        // upstream FindItemAffixes; the sigil reads its dungeon affixes
        // through the sigil conversion instead of affix rows.
        RecordedTooltip {
            file: "mouse-charm.png",
            cursor: (456, 770),
            item_type: "charm",
            item_power: Some(900),
            affixes: &[],
        },
        RecordedTooltip {
            file: "mouse-horadric-seal.png",
            cursor: (486, 743),
            item_type: "horadricseal",
            item_power: Some(850),
            affixes: &[],
        },
        RecordedTooltip {
            file: "mouse-nightmare-sigil.png",
            cursor: (449, 772),
            item_type: "sigil",
            item_power: None,
            affixes: &[],
        },
    ];

    let repo = crate::actions::affix::catalog_repo_root().expect("checkout with assets");
    let root = recorded_tooltip_root("recorded-tooltips");
    let config = resolve_runtime_config(&root, None, None, None).expect("config");
    let detector = build_prepared_detector(&root, &config).expect("detector");
    let worker = build_ocr_worker_with_roots(&root, &repo, &config).expect("OCR worker");
    let mut session = RuntimeSession::new(Some(worker), Duration::ZERO);

    for case in CASES {
        let (_, analysis) = read_recorded_tooltip(&mut session, &detector, &repo, case);
        let item = analysis
            .item
            .clone()
            .unwrap_or_else(|| panic!("{}: no current item", case.file));

        assert_eq!(
            item.item_type.as_deref(),
            Some(case.item_type),
            "{}",
            case.file
        );
        assert_eq!(item.item_power, case.item_power, "{}", case.file);
        let affixes = item
            .affixes
            .iter()
            .map(|affix| {
                let first_id = affix
                    .affix_id
                    .as_deref()
                    .and_then(|ids| ids.split(';').next())
                    .unwrap_or_default();
                (first_id, affix.is_greater)
            })
            .collect::<Vec<_>>();
        assert_eq!(affixes, case.affixes, "{}", case.file);
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A config root with the 1080p_SMF preset and the values that preset seeds
/// on a preset switch (assets/original/systempresets/1080p_SMF/config.json).
#[cfg(feature = "leptess-backend")]
fn recorded_tooltip_root(tag: &str) -> std::path::PathBuf {
    preset_fixture(tag, |settings| {
        settings.tooltip_width = 375;
        settings.tooltip_max_height = 200;
        settings.affix_area_height_offset_top = 8;
        settings.affix_area_height_offset_bottom = 8;
        settings.affix_aspect_area_width_offset = 18;
        settings.aspect_area_height_offset_top = 8;
        settings.threshold_min = 70;
        settings.threshold_max = 255;
    })
}

/// Feed one recorded frame through the live pipeline (detector from the
/// cursor, OCR worker, parse, coordinator) until its analysis is accepted.
/// Returns the frame and the accepted analysis.
#[cfg(feature = "leptess-backend")]
fn read_recorded_tooltip(
    session: &mut RuntimeSession,
    detector: &PreparedDetector,
    repo: &Path,
    case: &RecordedTooltip,
) -> (d4lc_core::CaptureFrame, Arc<d4lc_runtime::AnalysisSnapshot>) {
    let frame = load_reference_frame(&repo.join("tests/fixtures/live-frames").join(case.file));
    let source = d4lc_capture::SyntheticCaptureFrameSource::manual_region(frame.clone())
        .expect("synthetic source");
    let cursor = d4lc_core::Point {
        x: case.cursor.0,
        y: case.cursor.1,
    };
    let runtime = PreparedRuntime::new(
        CaptureBackend::Auto,
        source.reference().clone(),
        detector.clone(),
    )
    .with_synthetic_capture_source(source)
    .with_synthetic_cursor_source(d4lc_runtime::SyntheticCursorSource::new(move || {
        Some(cursor)
    }));

    // The crop stability gate and the OCR worker need a few frames; the
    // item is read once the analysis is accepted.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let result = d4lc_runtime::process_one_frame(session, &runtime)
            .unwrap_or_else(|error| panic!("{}: frame failed: {error}", case.file));
        if result.frame_status == d4lc_runtime::FrameStatus::Parsed && result.current_item.is_some()
        {
            return (frame, result.analysis);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{}: no parsed item, last status {:?}",
            case.file,
            result.frame_status
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A controller-list change alone — no mode switch, no
/// `Settings.json` edit — is observed and replaces the running
/// detector's templates through the hot-reload path.
#[test]
fn controller_list_change_replaces_the_running_templates() {
    use crate::runtime_bridge::settings_reload::{SettingsReloadDecision, SettingsReloadMonitor};

    let root = preset_fixture("controllers", |settings| settings.controller_mode = true);
    d4lc_config::save_controller_config(&root, &["tooltip_gc_x.png".to_string()])
        .expect("controllers");
    let repo = crate::actions::affix::catalog_repo_root().expect("checkout");
    let mut monitor = SettingsReloadMonitor::with_game_data_root(&root, &repo)
        .with_asset_interval(Duration::ZERO);

    let names = |detector: &PreparedDetector| {
        detector
            .templates
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
    };
    let prepare = |root: &Path| {
        let config = resolve_runtime_config(root, None, None, None).expect("config");
        build_prepared_detector(root, &config).expect("detector")
    };
    let detector = prepare(&root);
    assert_eq!(names(&detector), vec!["tooltip_gc_x.png".to_string()]);
    let source =
        d4lc_capture::SyntheticCaptureFrameSource::manual_region(d4lc_core::CaptureFrame {
            width: 4,
            height: 4,
            stride: 16,
            pixels_bgra: vec![255; 64],
        })
        .expect("synthetic source");
    let mut runtime =
        PreparedRuntime::new(CaptureBackend::Auto, source.reference().clone(), detector)
            .with_synthetic_capture_source(source.clone());
    let mut session = RuntimeSession::new(None, Duration::ZERO);

    // Only the controller list changes.
    assert_eq!(monitor.poll(), SettingsReloadDecision::NoChange);
    d4lc_config::save_controller_config(&root, &["tooltip_gc_square.png".to_string()])
        .expect("controllers");
    let signal = d4lc_runtime::RuntimeReloadSignal::new();
    match monitor.poll() {
        SettingsReloadDecision::Changed {
            reprepare, restart, ..
        } => {
            assert_eq!(reprepare, vec!["controller list".to_string()]);
            assert!(restart.is_empty());
            signal.request();
        }
        other => panic!("a controller-list change must re-prepare, got {other:?}"),
    }

    let outcome = d4lc_runtime::apply_pending_reload(&signal, &mut runtime, &mut session, || {
        let runtime = PreparedRuntime::new(
            CaptureBackend::Auto,
            source.reference().clone(),
            prepare(&root),
        )
        .with_synthetic_capture_source(source.clone());
        Ok((runtime, RuntimeSession::new(None, Duration::ZERO)))
    });
    assert_eq!(outcome, d4lc_runtime::ReloadOutcome::Applied);
    assert_eq!(
        names(runtime.detector()),
        vec!["tooltip_gc_square.png".to_string()],
        "the running detector uses the new controller list"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// One recorded ring tooltip through the whole chain the live loop runs:
/// real frame → tooltip detection from the cursor → Tesseract OCR → parse →
/// rule evaluation against an affix preset (the live loop's
/// `RulesSnapshot::evaluate`) → the overlay render ops both backends draw.
#[cfg(feature = "leptess-backend")]
#[test]
fn a_recorded_tooltip_is_evaluated_and_marked_on_the_overlay() {
    use d4lc_gui::{PresetEntry, PresetEntryKind};

    let case = RecordedTooltip {
        file: "mouse-legendary-ring-greater-affixes.png",
        cursor: (465, 419),
        item_type: "ring",
        item_power: Some(900),
        affixes: &[],
    };
    let repo = crate::actions::affix::catalog_repo_root().expect("checkout with assets");
    let root = recorded_tooltip_root("recorded-overlay");
    let config = resolve_runtime_config(&root, None, None, None).expect("config");
    let detector = build_prepared_detector(&root, &config).expect("detector");
    let worker = build_ocr_worker_with_roots(&root, &repo, &config).expect("OCR worker");
    let mut session = RuntimeSession::new(Some(worker), Duration::ZERO);
    let (frame, analysis) = read_recorded_tooltip(&mut session, &detector, &repo, &case);
    let item = analysis.item.as_ref().expect("parsed item");
    assert_eq!(item.item_type.as_deref(), Some(case.item_type));
    assert_eq!(item.item_power, case.item_power);

    // Preset entries carry the catalog id (the joined upstream `IdName`),
    // exactly as the affix screen writes them.
    let catalog = d4lc_rules::AffixCatalog::load(&d4lc_assets::AssetRepository::new(&repo), "enUS")
        .expect("affix catalog");
    let catalog_id = |short: &str| {
        catalog
            .entries(d4lc_rules::CatalogKind::Affix)
            .iter()
            .find(|entry| entry.id.split(';').any(|id| id == short))
            .unwrap_or_else(|| panic!("{short} is not in the affix catalog"))
            .id
            .clone()
    };
    let entry = |id: &str, color: &str, is_greater: bool| PresetEntry {
        kind: PresetEntryKind::Affix,
        id: catalog_id(id),
        label: None,
        item_type: Some("ring".to_string()),
        color: color.to_string(),
        is_any_type: false,
        is_greater,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    };
    let mut state = AppState::default_startup();
    // The overlay power switch the user turns on (off by default).
    state.shell.settings.overlay_enabled = true;
    state
        .affix_preset_editor
        .create_preset("Ring")
        .expect("preset");
    for wanted in [
        entry("CoreStat_Willpower", "#008000", true),
        entry("AttackSpeed", "#0000FF", false),
    ] {
        state
            .affix_preset_editor
            .edit_entry("Ring", wanted)
            .expect("preset entry");
    }
    let snapshot = crate::runtime_bridge::live_state::RulesSnapshot::capture(&state, 1);
    let window = d4lc_core::Rect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    };
    let evaluation = snapshot.evaluate_item(&analysis, Some(window));
    let ops = d4lc_overlay_x11::build_descriptor_render_ops(
        &evaluation.descriptors,
        snapshot.overlay_font_px(),
    );
    // Expected values read off the recorded frame (verified against the
    // image): one marker per affix row in tooltip order plus the aspect,
    // left of each row's bullet. Wanted rows take the preset color; the
    // greater Willpower entry is a triangle (upstream
    // `upstream_affix_marker_shape`); unwanted rows are red.
    let marker = |y: i32| d4lc_core::Rect {
        x: 30,
        y,
        width: 20,
        height: 20,
    };
    let expected: [(&str, d4lc_rules::RuleOverlayShape, &str, d4lc_core::Rect); 6] = [
        (
            "CoreStat_Willpower",
            d4lc_rules::RuleOverlayShape::Triangle,
            "#008000",
            marker(330),
        ),
        (
            "Life",
            d4lc_rules::RuleOverlayShape::Circle,
            "Red",
            marker(356),
        ),
        (
            "AttackSpeed",
            d4lc_rules::RuleOverlayShape::Circle,
            "#0000FF",
            marker(385),
        ),
        (
            "S04_Damage_to_Vulnerable",
            d4lc_rules::RuleOverlayShape::Circle,
            "Red",
            marker(414),
        ),
        (
            "Lucky_Hit_Resource",
            d4lc_rules::RuleOverlayShape::Circle,
            "Red",
            marker(467),
        ),
        (
            "legendary_warlock_009_x2",
            d4lc_rules::RuleOverlayShape::Circle,
            "Red",
            marker(521),
        ),
    ];
    let first_id = |target: &d4lc_rules::RuleOverlayTarget| match target {
        d4lc_rules::RuleOverlayTarget::Affix { affix_id, .. } => {
            affix_id.split(';').next().unwrap_or_default().to_string()
        }
        d4lc_rules::RuleOverlayTarget::Power { power_id, .. } => power_id.clone(),
        other => format!("{other:?}"),
    };
    let actual = evaluation
        .descriptors
        .iter()
        .map(|descriptor| {
            (
                first_id(&descriptor.target),
                descriptor.shape,
                descriptor.color.name().unwrap_or_default().to_string(),
                descriptor.geometry,
            )
        })
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|(id, shape, color, rect)| (id.to_string(), *shape, color.to_string(), *rect))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    let traced_matches = evaluation
        .affix_traces
        .iter()
        .filter(|trace| trace.is_match())
        .count();
    assert_eq!(
        traced_matches, 2,
        "Willpower and Attack Speed match the preset"
    );

    // The overlay draws exactly these markers, in this order.
    assert_eq!(
        ops.iter()
            .map(|op| (op.geometry, op.color.name().unwrap_or_default().to_string()))
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(_, _, color, rect)| (*rect, color.clone()))
            .collect::<Vec<_>>()
    );

    // "Hide unwanted" keeps only the two preset markers.
    state.shell.settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
    let hidden = crate::runtime_bridge::live_state::RulesSnapshot::capture(&state, 2)
        .evaluate_item(&analysis, Some(window));
    assert_eq!(
        hidden
            .descriptors
            .iter()
            .map(|descriptor| first_id(&descriptor.target))
            .collect::<Vec<_>>(),
        ["CoreStat_Willpower", "AttackSpeed"]
    );
    let _ = std::fs::remove_dir_all(&root);
}
