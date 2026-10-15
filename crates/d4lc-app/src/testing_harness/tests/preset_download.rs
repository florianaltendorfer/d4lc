use super::*;

// ── preset-download scenarios: Settings preset download (local stub) ──

/// Stub response modes, switched by the scenario between real
/// DOWNLOAD clicks.
const STUB_MODE_SERVE_ZIP: usize = 0;
const STUB_MODE_NOT_FOUND: usize = 1;
const STUB_MODE_GARBAGE: usize = 2;
/// Valid HTTP 200 whose body is not a zip archive:
/// the download step writes the bytes, the extraction step must
/// then fail with a structured error and never leave a partial
/// preset directory.
const STUB_MODE_CORRUPT_ZIP: usize = 3;

/// Minimal loopback HTTP stub for the real wired download flow
/// (std::net only, no new crates). Serves the vendored zip body
/// for every request, or the configured failure mode. Binds
/// `127.0.0.1:0` (dynamic port, loopback only) so the suite's
/// no-network guarantee holds: the only traffic ever exchanged is
/// between the harness process and itself.
struct PresetDownloadStub {
    base_url: String,
    inner: Arc<StubShared>,
}

struct StubShared {
    mode: std::sync::atomic::AtomicUsize,
    shutdown: std::sync::atomic::AtomicBool,
    /// Request targets (path lines) actually served — the receipt
    /// that each click fetched from THIS stub, not from upstream.
    served: Mutex<Vec<String>>,
}

impl PresetDownloadStub {
    fn start(zip_body: Vec<u8>) -> Self {
        use std::io::ErrorKind;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the preset-download stub");
        let port = listener.local_addr().expect("stub local addr").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking stub listener");
        let inner = Arc::new(StubShared {
            mode: std::sync::atomic::AtomicUsize::new(STUB_MODE_SERVE_ZIP),
            shutdown: std::sync::atomic::AtomicBool::new(false),
            served: Mutex::new(Vec::new()),
        });
        let worker_inner = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("d4lc-preset-stub".into())
            .spawn(move || {
                loop {
                    if worker_inner
                        .shutdown
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        break;
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let target = read_request_target(&mut stream);
                            worker_inner
                                .served
                                .lock()
                                .expect("stub served lock")
                                .push(target);
                            let body =
                                match worker_inner.mode.load(std::sync::atomic::Ordering::SeqCst) {
                                    STUB_MODE_SERVE_ZIP => {
                                        let mut response = format!(
                                            "HTTP/1.1 200 OK\r\nContent-Type: \
                                         application/zip\r\nContent-Length: {}\r\n\
                                         Connection: close\r\n\r\n",
                                            zip_body.len()
                                        )
                                        .into_bytes();
                                        response.extend_from_slice(&zip_body);
                                        response
                                    }
                                    STUB_MODE_NOT_FOUND => {
                                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                                      Connection: close\r\n\r\n"
                                            .to_vec()
                                    }
                                    STUB_MODE_CORRUPT_ZIP => {
                                        let body = b"NOT-A-ZIP-ARCHIVE-BODY".to_vec();
                                        let mut response = format!(
                                            "HTTP/1.1 200 OK\r\nContent-Type: \
                                         application/zip\r\nContent-Length: {}\r\n\
                                         Connection: close\r\n\r\n",
                                            body.len()
                                        )
                                        .into_bytes();
                                        response.extend_from_slice(&body);
                                        response
                                    }
                                    // Not a valid HTTP response at all: the
                                    // transport must reject it without writing
                                    // anything to disk.
                                    _ => b"NOT-HTTP-GARBAGE-RESPONSE\r\n\r\n".to_vec(),
                                };
                            let _ = stream.write_all(&body);
                            let _ = stream.flush();
                            // Dropping the stream closes the connection
                            // (Connection: close was announced).
                        }
                        Err(ref error) if error.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("spawn the preset-download stub thread");
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            inner,
        }
    }

    fn set_mode(&self, mode: usize) {
        self.inner
            .mode
            .store(mode, std::sync::atomic::Ordering::SeqCst);
    }

    fn served(&self) -> Vec<String> {
        self.inner.served.lock().expect("stub served lock").clone()
    }

    fn shutdown(&self) {
        self.inner
            .shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Read one HTTP request head and return the request target
/// (`GET /path HTTP/1.1` → `/path`); empty when unreadable.
fn read_request_target(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|window| window == b"\r\n\r\n") || buf.len() > 8192 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let request_line =
        String::from_utf8_lossy(buf.split(|byte| *byte == b'\n').next().unwrap_or(&[]))
            .trim()
            .to_string();
    request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string()
}

/// Click the real download button and wait for its worker: the download runs
/// off the Slint thread, so the result lands through the event loop and the
/// screen's downloading state ends the wait.
fn click_download_and_wait() {
    click_on_ui("Download preset");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while read_ui(|app| app.get_preset_downloading()) {
        assert!(
            std::time::Instant::now() < deadline,
            "preset-download: the download worker did not finish within 60 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The whole observable download projection: community list,
/// system list, selected community preset, download-success flag.
fn projected_state() -> (Vec<String>, Vec<String>, String, bool) {
    read_ui(|app| {
        fn model_to_vec(model: &slint::ModelRc<slint::SharedString>) -> Vec<String> {
            (0..model.row_count())
                .map(|row| model.row_data(row).expect("preset row").to_string())
                .collect()
        }
        (
            model_to_vec(&app.get_community_presets()),
            model_to_vec(&app.get_system_presets()),
            app.get_selected_community_preset().to_string(),
            app.get_preset_download_success(),
        )
    })
}

// ── import-delete-edges: import/delete end-to-end edge scenarios ─────────

/// Read the stored-build rows the open dialog seeded offline from the
/// bundled fixture corpus (`resolve_builds_dir` →
/// `load_available_{provider}_builds`) — the same rows the dialog
/// renders in its left list.
pub(super) fn seeded_build_rows(provider: &str) -> Vec<String> {
    let provider = provider.to_string();
    read_ui(move |app| {
        let model = match provider.as_str() {
            "D2Core" => app.get_import_d2core_builds(),
            "D4Builds" => app.get_import_d4builds_builds(),
            "InfinityBuilds" => app.get_import_infinity_builds(),
            "Maxroll" => app.get_import_maxroll_builds(),
            "Mobalytics" => app.get_import_mobalytics_builds(),
            other => panic!("unknown provider {other}"),
        };
        (0..model.row_count())
            .map(|row| model.row_data(row).expect("build row").id.to_string())
            .collect::<Vec<_>>()
    })
}

/// Read the preset projection straight out of `AppState` (editor +
/// shell) — the orphan readback the projected UI list cannot prove.
pub(super) fn app_state_presets() -> (Vec<String>, String, usize, Option<String>) {
    on_ui(|_| {
        let arc = HARNESS_APP_STATE.get().expect("harness app state");
        let state = arc.lock().expect("app state lock");
        (
            state
                .affix_preset_editor
                .presets
                .iter()
                .map(|preset| preset.name.clone())
                .collect(),
            state
                .affix_preset_editor
                .selected_preset_name
                .clone()
                .unwrap_or_default(),
            state.shell.affix_presets.preset_count,
            state.shell.affix_presets.selected_preset.clone(),
        )
    })
}

/// The preset-download scenario family: the real Settings download flow
/// (nav click → real DOWNLOAD click → wired
/// `start_preset_download` worker → `UreqDownloader`) against a
/// loopback-only stub server via the additive
/// `D4LC_PRESET_DOWNLOAD_BASE_URL` seam.
///
/// Happy path: the selected catalog zip is fetched from the stub
/// (receipt-asserted), written under the harness config root's
/// `Images/`, and stays available + selected with the success
/// marker shown. Failure modes (HTTP 404, malformed non-HTTP
/// response body, connection refused) must surface the
/// in-UI error state — success marker off, projected lists and
/// selection unchanged, no bytes written, no panic.
///
/// Note on list growth: a UI download can only target a name the
/// ComboBox already lists (catalog ∪ downloaded zips — upstream
/// parity), so a successful download never grows the projected
/// list; the list-growth projection is covered by the
/// `apply_settings_preset_projection` unit tests. The observable
/// happy-path delta here is the stub receipt, the file on disk,
/// the success marker, and the retained selection.
pub(super) fn preset_download_scenarios(client: &McpClient, window: McpHandle) {
    use crate::actions::settings::ENV_OVERRIDE_BASE_URL;
    eprintln!("[preset-download] starting Settings preset-download scenarios");

    // Real click through the nav to the Settings screen.
    click_on_ui("Settings");
    assert_eq!(
        read_ui(|app| app.get_active_tab()),
        4,
        "preset-download: nav click must switch to Settings"
    );

    // The download click fetches the currently selected community
    // preset; the startup projection selected the first catalog zip.
    let selected = read_ui(|app| app.get_selected_community_preset().to_string());
    assert!(
        !selected.is_empty() && selected.to_ascii_lowercase().ends_with(".zip"),
        "preset-download: a catalog zip must be preselected for the download click, got \
         {selected:?}"
    );

    // Real vendored bytes for the selected catalog entry — the stub
    // serves exactly what the upstream host would.
    let vendored_zip = workspace_root()
        .join("assets/original/systempresets/zips")
        .join(&selected);
    let zip_bytes = std::fs::read(&vendored_zip)
        .unwrap_or_else(|error| harness_panic("read vendored catalog zip", &error.to_string()));

    let stub = PresetDownloadStub::start(zip_bytes.clone());
    // Hold the settings module's crate-shared ENV_LOCK across the
    // whole env set→remove window below: the resolver unit tests
    // in `actions::settings` mutate the same process-global env
    // var on parallel test threads of this binary, and an
    // interleaved window would leak the stub base into their
    // asserts (or their windows into this flow, triggering a real
    // network attempt). Poisoning is tolerated exactly as there.
    let _env_guard = crate::actions::settings::ENV_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // Activate the seam for the real wired flow (read on the UI
    // thread inside each click, ordered by the event-loop round
    // trips; removed before the family returns).
    std::env::set_var(ENV_OVERRIDE_BASE_URL, &stub.base_url);
    let images_zip = crate::actions::settings::resolve_config_root()
        .join(crate::actions::settings::PRESETS_IMAGES_SUBDIR)
        .join(&selected);
    let _ = std::fs::remove_file(&images_zip);

    // ── Happy path: 200 + a real preset zip ─────────────────────
    // A successful download now also extracts the zip,
    // so the preset becomes selectable in the system list and the
    // description panel / language-ready flags seed from the
    // catalog metadata.
    let before_download = projected_state();
    stub.set_mode(STUB_MODE_SERVE_ZIP);
    click_download_and_wait();
    let (community, system, selected_after, success) = projected_state();
    assert!(
        success,
        "preset-download: a successful download must show the in-UI success marker"
    );
    assert_eq!(
        selected_after, selected,
        "preset-download: the downloaded preset must be the selected community preset"
    );
    assert!(
        community.contains(&selected),
        "preset-download: the downloaded preset must be available in the projected community \
         list"
    );
    assert_eq!(
        std::fs::read(&images_zip).ok(),
        Some(zip_bytes.clone()),
        "preset-download: the stub zip bytes must be written under Images/"
    );
    assert_eq!(
        stub.served(),
        vec![format!("/{selected}")],
        "preset-download: the click must fetch exactly the selected preset from the stub"
    );

    // Extraction: the archive's single root directory appears as an
    // extracted preset dir under Images/ (busy-state note: the button is
    // disabled while the worker runs, so exactly one stub fetch per click).
    let preset_stem = selected
        .strip_suffix(".zip")
        .unwrap_or(selected.as_str())
        .to_string();
    let extracted_dir = crate::actions::settings::resolve_config_root()
        .join(crate::actions::settings::PRESETS_IMAGES_SUBDIR)
        .join(&preset_stem);
    assert!(
        extracted_dir.join("config.json").is_file(),
        "preset-download: the download must extract the preset dir with its config.json \
         at {}",
        extracted_dir.display()
    );
    let mut expected_system = before_download.1.clone();
    expected_system.push(preset_stem.clone());
    expected_system.sort();
    expected_system.dedup();
    assert_eq!(
        system, expected_system,
        "preset-download: the extracted preset must join the projected system preset list"
    );

    // Description panel + language-ready flags seeded from the
    // vendored catalog entry of the downloaded preset (independent
    // oracle: the vendored systempresets.json, re-parsed here).
    let catalog_text = std::fs::read_to_string(
        workspace_root().join("assets/original/systempresets/systempresets.json"),
    )
    .expect("vendored systempresets.json must exist");
    let metadata = d4lc_assets::parse_system_preset_catalog(&catalog_text)
        .expect("parse vendored catalog")
        .into_iter()
        .find(|entry| entry.file_name == selected)
        .unwrap_or_else(|| harness_panic("preset-download catalog lookup", &selected));
    let details = read_ui(|app| app.get_preset_details());
    assert_eq!(
        details.filename.as_str(),
        selected.as_str(),
        "preset-download: the panel filename must be the downloaded preset zip"
    );
    assert_eq!(
        details.resolution.as_str(),
        metadata.resolution.as_str(),
        "preset-download: the panel resolution must mirror the catalog"
    );
    assert_eq!(
        details.brightness_threshold.as_str(),
        metadata.brightness_threshold.as_str(),
        "preset-download: the threshold row must mirror the catalog"
    );
    assert_eq!(
        details.language_ready_de_de,
        metadata.ready_languages.iter().any(|lang| lang == "deDE"),
        "preset-download: the DE flag must mirror the catalog LanguageReadydeDE"
    );
    assert_eq!(
        details.language_ready_it_it,
        metadata.ready_languages.iter().any(|lang| lang == "itIT"),
        "preset-download: the IT flag must mirror the catalog LanguageReadyitIT"
    );
    take_evidence_screenshot(client, window, "preset-download", "download-success.png");

    // Failure modes: error state, no panic, unchanged prior
    // lists, no bytes written (the happy-path file stays intact).
    let assert_failure =
        |name: &str,
         stub: &PresetDownloadStub,
         expected_served: usize,
         before: &(Vec<String>, Vec<String>, String, bool)| {
            let after = projected_state();
            assert!(
                !after.3,
                "preset-download {name}: the download success marker must stay off"
            );
            assert_eq!(
                after.0, before.0,
                "preset-download {name}: the community list must stay unchanged"
            );
            assert_eq!(
                after.1, before.1,
                "preset-download {name}: the system preset list must stay unchanged"
            );
            assert_eq!(
                after.2, before.2,
                "preset-download {name}: the community selection must stay unchanged"
            );
            assert_eq!(
                std::fs::read(&images_zip).ok(),
                Some(zip_bytes.clone()),
                "preset-download {name}: a failed download must not write any bytes"
            );
            let served = stub.served();
            assert_eq!(
                served.len(),
                expected_served,
                "preset-download {name}: unexpected stub receipts: {served:?}"
            );
            take_evidence_screenshot(
                client,
                window,
                "preset-download",
                &format!("download-error-{name}.png"),
            );
        };

    // ── HTTP 404 ─────────────────────────────────────────────────
    stub.set_mode(STUB_MODE_NOT_FOUND);
    let before = projected_state();
    click_download_and_wait();
    assert_failure("404", &stub, 2, &before);

    // ── Malformed (non-HTTP) response body ──────────────────────
    stub.set_mode(STUB_MODE_GARBAGE);
    let before = projected_state();
    click_download_and_wait();
    assert_failure("garbage", &stub, 3, &before);

    // ── Connection refused (no listener on the port) ────────────
    // A freshly reserved-and-dropped loopback port; the transport
    // must fail without reaching the stub (receipt count frozen).
    let refused_port = reserve_free_port();
    std::env::set_var(
        ENV_OVERRIDE_BASE_URL,
        format!("http://127.0.0.1:{refused_port}"),
    );
    let before = projected_state();
    click_download_and_wait();
    assert_failure("refused", &stub, 3, &before);

    // ── Corrupt zip body ──────────────────────────
    // A valid HTTP 200 whose body is not a zip archive: the download
    // step refuses it before writing (the previous archive stays), the
    // success marker stays off and the previously extracted preset
    // directory stays complete — never a partial silent state.
    std::env::set_var(ENV_OVERRIDE_BASE_URL, &stub.base_url);
    stub.set_mode(STUB_MODE_CORRUPT_ZIP);
    let before = projected_state();
    click_download_and_wait();
    let after = projected_state();
    assert!(
        !after.3,
        "preset-download corrupt: the download success marker must stay off"
    );
    assert_eq!(
        after.0, before.0,
        "preset-download corrupt: the community list must stay unchanged"
    );
    assert_eq!(
        after.1, before.1,
        "preset-download corrupt: the previously extracted preset must stay listed"
    );
    assert_eq!(
        after.2, before.2,
        "preset-download corrupt: the community selection must stay unchanged"
    );
    assert_eq!(
        std::fs::read(&images_zip).ok(),
        Some(zip_bytes.clone()),
        "preset-download corrupt: a body that is not a zip never replaces the previous archive"
    );
    assert_eq!(
        stub.served().len(),
        4,
        "preset-download corrupt: unexpected stub receipts: {:?}",
        stub.served()
    );
    assert!(
        extracted_dir.join("config.json").is_file(),
        "preset-download corrupt: the previous extraction must survive complete"
    );
    take_evidence_screenshot(
        client,
        window,
        "preset-download",
        "download-error-corrupt-zip.png",
    );

    // ── Recovery: re-download the real zip after the refused body ─
    // The overwrite policy replaces the files of the extracted preset
    // directory through the same wired flow.
    stub.set_mode(STUB_MODE_SERVE_ZIP);
    click_download_and_wait();
    let (community, _system, selected_after, success) = projected_state();
    assert!(
        success,
        "preset-download recovery: re-downloading a valid zip must succeed"
    );
    assert_eq!(
        std::fs::read(&images_zip).ok(),
        Some(zip_bytes.clone()),
        "preset-download recovery: the archive is the served one"
    );
    assert!(
        extracted_dir.join("config.json").is_file(),
        "preset-download recovery: the preset directory is re-extracted"
    );
    assert!(
        community.contains(&selected) && selected_after == selected,
        "preset-download recovery: the selection survives the recovery download"
    );
    assert_eq!(
        stub.served().len(),
        5,
        "preset-download recovery: unexpected stub receipts: {:?}",
        stub.served()
    );
    take_evidence_screenshot(client, window, "preset-download", "download-recovery.png");

    // Deactivate the seam before any later suite runs.
    std::env::remove_var(ENV_OVERRIDE_BASE_URL);
    stub.shutdown();
    eprintln!(
        "[preset-download] download scenarios complete: happy + 404 + garbage + refused + \
         corrupt-zip + recovery, stub receipts at {}",
        stub.served().len()
    );
}
