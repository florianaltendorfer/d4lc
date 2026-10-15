//! The system preset catalog (`systempresets.json`).
//!
//! Upstream fetches the list of published system presets once the application
//! has loaded and fills the "available system presets" box from it
//! (`SystemPresetManager.cs:82-85` `HandleApplicationLoadedMessage`,
//! `SystemPresetManager.cs:232-253` `UpdateSystemPresetInfo`,
//! `SettingsViewModel.cs:348-355`). When the request yields nothing it
//! reports `Not able to download the latest system presets.` on the Logging
//! screen (`ErrorOccurredMessage`, `LoggingViewModel.cs:32`).
//!
//! The fetch runs on a worker thread and is projected on the Slint thread
//! through `Weak::upgrade_in_event_loop`. Upstream leaves the list empty after
//! a failed fetch; the port falls back to the vendored copy of the file and
//! logs that it did, so the box is never empty without an explanation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use d4lc_assets::SystemPresetMetadata;
use slint::ComponentHandle as _;

use super::preset_download::{
    preset_download_base_url, preset_download_override_notice, DownloadError, Downloader,
    PRESET_DOWNLOAD_BASE_URL,
};
use super::presets::{apply_settings_preset_projection, bundled_system_preset_catalog};
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

/// The catalog URL (`SystemPresetManager.cs:236-237`).
pub const SYSTEM_PRESETS_CATALOG_URL: &str = "https://raw.githubusercontent.com/josdemmers/Diablo4Companion/master/downloads/systempresets/systempresets.json";

/// Size cap of the catalog response. The live file is about 9 KiB; 1 MiB
/// leaves over 100x headroom.
pub const SYSTEM_PRESETS_CATALOG_MAX_BYTES: u64 = 1024 * 1024;

/// The message upstream reports when the catalog could not be downloaded
/// (`SystemPresetManager.cs:240-243`).
pub const CATALOG_DOWNLOAD_FAILED_MESSAGE: &str = "Not able to download the latest system presets.";

/// The catalog URL of this session: upstream's, or
/// `{D4LC_PRESET_DOWNLOAD_BASE_URL}/systempresets.json` while the download
/// base override is active, so one override points the whole preset flow at a
/// local stub server.
pub fn system_presets_catalog_url() -> String {
    let base = preset_download_base_url();
    if base == PRESET_DOWNLOAD_BASE_URL {
        SYSTEM_PRESETS_CATALOG_URL.to_string()
    } else {
        format!("{base}/systempresets.json")
    }
}

/// Where the session catalog came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    /// Fetched from upstream.
    Upstream,
    /// The vendored copy, after a failed fetch.
    Bundled,
}

/// The catalog the settings screen lists presets from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPresetCatalog {
    pub entries: Vec<SystemPresetMetadata>,
    pub source: CatalogSource,
}

/// The outcome of one catalog load: the catalog to use, if any, and the
/// Logging-screen lines to append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLoad {
    pub catalog: Option<SystemPresetCatalog>,
    pub log_lines: Vec<String>,
}

/// Why the catalog could not be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogFetchError {
    /// Nothing usable came back: the request failed or the body was empty or
    /// whitespace. `GetRequest` returns an empty string then, and upstream
    /// reports [`CATALOG_DOWNLOAD_FAILED_MESSAGE`] (`SystemPresetManager.cs:238-243`).
    NoResponse(String),
    /// A body came back that is not the catalog (an HTTP error page, bad
    /// JSON). Upstream's deserialization throws and only `_logger` hears of
    /// it (`SystemPresetManager.cs:246-250`): no Logging message.
    Unusable(String),
}

impl CatalogFetchError {
    fn reason(&self) -> &str {
        match self {
            Self::NoResponse(reason) | Self::Unusable(reason) => reason,
        }
    }
}

/// Fetch and parse the upstream catalog.
pub fn fetch_system_preset_catalog(
    downloader: &dyn Downloader,
    url: &str,
) -> Result<Vec<SystemPresetMetadata>, CatalogFetchError> {
    let body = downloader
        .fetch(url, SYSTEM_PRESETS_CATALOG_MAX_BYTES)
        .map_err(|error| match &error {
            // An error status still carries its body upstream: a blank one is
            // the "" of every other failure, anything else fails to parse.
            DownloadError::HttpStatus { body, .. }
                if !String::from_utf8_lossy(body).trim().is_empty() =>
            {
                CatalogFetchError::Unusable(error.to_string())
            }
            _ => CatalogFetchError::NoResponse(error.to_string()),
        })?;
    let text = String::from_utf8_lossy(&body);
    if text.trim().is_empty() {
        return Err(CatalogFetchError::NoResponse(
            "the response was empty".to_string(),
        ));
    }
    d4lc_assets::parse_system_preset_catalog(&text).map_err(CatalogFetchError::Unusable)
}

/// Resolve the session catalog from a fetch result: the fetched entries, or
/// the notice that the bundled list stands in, preceded by upstream's failure
/// message when upstream sends one ([`CatalogFetchError::NoResponse`]).
pub fn resolve_system_preset_catalog(
    fetched: Result<Vec<SystemPresetMetadata>, CatalogFetchError>,
    bundled: impl FnOnce() -> Option<Vec<SystemPresetMetadata>>,
) -> CatalogLoad {
    match fetched {
        Ok(entries) => CatalogLoad {
            catalog: Some(SystemPresetCatalog {
                entries,
                source: CatalogSource::Upstream,
            }),
            log_lines: Vec::new(),
        },
        Err(error) => {
            let reason = error.reason().to_string();
            let mut log_lines = Vec::new();
            if matches!(error, CatalogFetchError::NoResponse(_)) {
                log_lines.push(CATALOG_DOWNLOAD_FAILED_MESSAGE.to_string());
            }
            let catalog = bundled().map(|entries| SystemPresetCatalog {
                entries,
                source: CatalogSource::Bundled,
            });
            log_lines.push(match catalog {
                Some(_) => {
                    format!("System preset list: using the bundled list instead ({reason}).")
                }
                None => format!(
                    "System preset list: the bundled list is not available either ({reason})."
                ),
            });
            CatalogLoad { catalog, log_lines }
        }
    }
}

/// Start the catalog fetch on a worker thread and return without waiting for
/// it. The result is applied on the Slint thread: the catalog goes into the
/// app state, the log lines onto the Logging screen, and the settings screen
/// re-lists its presets.
pub fn start_system_preset_catalog_fetch(
    app: &AppWindow,
    state: &Arc<Mutex<AppState>>,
    config_root: &Path,
    downloader: Arc<dyn Downloader + Send + Sync>,
) {
    let app_weak = app.as_weak();
    let worker_state = Arc::clone(state);
    let worker_root: PathBuf = config_root.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("d4lc-system-presets".to_string())
        .spawn(move || {
            let url = system_presets_catalog_url();
            let fetched = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                fetch_system_preset_catalog(downloader.as_ref(), &url)
            }))
            .unwrap_or_else(|_| {
                Err(CatalogFetchError::NoResponse(
                    "the fetch worker panicked".to_string(),
                ))
            });
            let mut load = resolve_system_preset_catalog(fetched, bundled_system_preset_catalog);
            if let Some(notice) = preset_download_override_notice() {
                load.log_lines.insert(0, notice);
            }
            let _ = app_weak.upgrade_in_event_loop(move |app| {
                apply_catalog_load(&app, &worker_state, &worker_root, load);
            });
        });
    if let Err(error) = spawned {
        lock(state).append_log_message(
            format!("{CATALOG_DOWNLOAD_FAILED_MESSAGE} Cannot start the worker: {error}"),
            crate::clock::hh_mm_now(),
        );
    }
}

fn apply_catalog_load(
    app: &AppWindow,
    state: &Arc<Mutex<AppState>>,
    config_root: &Path,
    load: CatalogLoad,
) {
    let mut state = lock(state);
    for line in load.log_lines {
        eprintln!("[d4lc] {line}");
        state.append_log_message(line, crate::clock::hh_mm_now());
    }
    state.system_preset_catalog = load.catalog;
    let selected = app.get_selected_community_preset().to_string();
    apply_settings_preset_projection(
        app,
        &state,
        config_root,
        Some(&selected),
        app.get_preset_download_success(),
    );
}

fn lock(state: &Arc<Mutex<AppState>>) -> std::sync::MutexGuard<'_, AppState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::actions::settings::preset_download::{DownloadError, ENV_LOCK};

    struct FakeDownloader {
        response: Result<Vec<u8>, DownloadError>,
        requested: Mutex<Vec<String>>,
    }

    impl FakeDownloader {
        fn new(response: Result<Vec<u8>, DownloadError>) -> Self {
            Self {
                response,
                requested: Mutex::new(Vec::new()),
            }
        }
    }

    impl Downloader for FakeDownloader {
        fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
            assert_eq!(max_bytes, SYSTEM_PRESETS_CATALOG_MAX_BYTES);
            self.requested.lock().unwrap().push(url.to_string());
            self.response.clone()
        }
    }

    const CATALOG: &str = r#"[
  { "FileName": "9100p_JOB.zip", "Resolution": "9100p", "Config": "Fresh", "Status": "ok" },
  { "FileName": "9200p_JOB.zip", "Resolution": "9200p", "Config": "Fresh", "Status": "ok" }
]"#;

    fn bundled() -> Option<Vec<SystemPresetMetadata>> {
        bundled_system_preset_catalog()
    }

    #[test]
    fn the_catalog_url_is_upstreams_unless_the_download_base_is_overridden() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::remove_var(super::super::preset_download::ENV_OVERRIDE_BASE_URL);
        assert_eq!(
            system_presets_catalog_url(),
            "https://raw.githubusercontent.com/josdemmers/Diablo4Companion/master/downloads/systempresets/systempresets.json"
        );
        std::env::set_var(
            super::super::preset_download::ENV_OVERRIDE_BASE_URL,
            "http://127.0.0.1:9/presets",
        );
        assert_eq!(
            system_presets_catalog_url(),
            "http://127.0.0.1:9/presets/systempresets.json"
        );
        std::env::remove_var(super::super::preset_download::ENV_OVERRIDE_BASE_URL);
    }

    #[test]
    fn a_fetched_catalog_replaces_the_bundled_list_without_a_notice() {
        let downloader = FakeDownloader::new(Ok(CATALOG.as_bytes().to_vec()));
        let fetched = fetch_system_preset_catalog(&downloader, SYSTEM_PRESETS_CATALOG_URL);
        assert_eq!(
            downloader.requested.lock().unwrap().as_slice(),
            [SYSTEM_PRESETS_CATALOG_URL]
        );

        let load = resolve_system_preset_catalog(fetched, || panic!("bundled list not needed"));

        assert!(load.log_lines.is_empty());
        let catalog = load.catalog.expect("catalog");
        assert_eq!(catalog.source, CatalogSource::Upstream);
        let names: Vec<&str> = catalog
            .entries
            .iter()
            .map(|e| e.file_name.as_str())
            .collect();
        assert_eq!(names, ["9100p_JOB.zip", "9200p_JOB.zip"]);
    }

    #[test]
    fn a_failed_fetch_reports_upstreams_message_and_falls_back_with_a_notice() {
        let downloader = FakeDownloader::new(Err(DownloadError::Transport("offline".into())));
        let fetched = fetch_system_preset_catalog(&downloader, SYSTEM_PRESETS_CATALOG_URL);

        let load = resolve_system_preset_catalog(fetched, bundled);

        assert_eq!(
            load.log_lines[0],
            "Not able to download the latest system presets."
        );
        assert!(
            load.log_lines[1].contains("using the bundled list")
                && load.log_lines[1].contains("offline"),
            "{:?}",
            load.log_lines
        );
        let catalog = load.catalog.expect("the vendored catalog exists");
        assert_eq!(catalog.source, CatalogSource::Bundled);
        assert!(catalog
            .entries
            .iter()
            .any(|e| e.file_name == "1080p_SMF.zip"));
    }

    #[test]
    fn an_empty_body_reports_upstreams_message() {
        for body in ["", "  \n"] {
            let downloader = FakeDownloader::new(Ok(body.as_bytes().to_vec()));
            let fetched = fetch_system_preset_catalog(&downloader, SYSTEM_PRESETS_CATALOG_URL);
            assert!(
                matches!(fetched, Err(CatalogFetchError::NoResponse(_))),
                "{body:?}"
            );
            let load = resolve_system_preset_catalog(fetched, bundled);
            assert_eq!(load.log_lines[0], CATALOG_DOWNLOAD_FAILED_MESSAGE);
            assert_eq!(load.log_lines.len(), 2);
            assert_eq!(
                load.catalog.expect("fallback").source,
                CatalogSource::Bundled
            );
        }
    }

    #[test]
    fn an_error_status_with_an_empty_body_reports_upstreams_message() {
        // Upstream's GetRequest returns the error response's body, which is
        // "" here, so the failure message is the one for an empty response.
        for body in [&b""[..], &b" \n"[..]] {
            let downloader = FakeDownloader::new(Err(DownloadError::HttpStatus {
                status: 503,
                reason: "Service Unavailable".to_string(),
                body: body.to_vec(),
            }));
            let fetched = fetch_system_preset_catalog(&downloader, SYSTEM_PRESETS_CATALOG_URL);
            assert!(
                matches!(fetched, Err(CatalogFetchError::NoResponse(_))),
                "{body:?}"
            );
            let load = resolve_system_preset_catalog(fetched, bundled);
            assert_eq!(load.log_lines[0], CATALOG_DOWNLOAD_FAILED_MESSAGE);
            assert_eq!(load.log_lines.len(), 2);
        }
    }

    #[test]
    fn an_unusable_body_logs_only_the_fallback_notice_naming_the_error() {
        // Upstream's deserialization throws and only `_logger` hears of it
        // (SystemPresetManager.cs:246-250): no ErrorOccurredMessage.
        let bodies: [Result<Vec<u8>, DownloadError>; 2] = [
            Ok(b"404: Not Found".to_vec()),
            Err(DownloadError::HttpStatus {
                status: 404,
                reason: "Not Found".to_string(),
                body: b"404: Not Found".to_vec(),
            }),
        ];
        for response in bodies {
            let downloader = FakeDownloader::new(response);
            let fetched = fetch_system_preset_catalog(&downloader, SYSTEM_PRESETS_CATALOG_URL);
            assert!(matches!(fetched, Err(CatalogFetchError::Unusable(_))));
            let load = resolve_system_preset_catalog(fetched, bundled);
            assert_eq!(load.log_lines.len(), 1, "{:?}", load.log_lines);
            assert!(
                load.log_lines[0].contains("using the bundled list")
                    && !load.log_lines[0].contains(CATALOG_DOWNLOAD_FAILED_MESSAGE),
                "{:?}",
                load.log_lines
            );
            assert_eq!(
                load.catalog.expect("fallback").source,
                CatalogSource::Bundled
            );
        }
    }

    #[test]
    fn a_failed_fetch_without_a_bundled_list_says_so() {
        let load = resolve_system_preset_catalog(
            Err(CatalogFetchError::NoResponse("offline".to_string())),
            || None,
        );
        assert!(load.catalog.is_none());
        assert_eq!(load.log_lines[0], CATALOG_DOWNLOAD_FAILED_MESSAGE);
        assert!(load.log_lines[1].contains("not available either"));
    }
}
