//! Background community-preset download.
//!
//! Upstream starts the download as an awaited task and extracts the archive
//! from the completion message, so the window stays responsive while the
//! zip is fetched (`SystemPresetManager.cs:107-111` `DownloadSystemPreset`,
//! `SettingsViewModel.cs:313-323` `HandleDownloadSystemPresetCompletedMessage`
//! → `ExtractSystemPreset`, `SystemPresetManager.cs:113-125`). The command is
//! disabled while a download runs and while the overlay is on
//! (`CanDownloadSystemPresetExecute`, `SettingsViewModel.cs:451-454`).
//!
//! The port runs the blocking fetch and the extraction on one worker thread
//! and projects the result on the Slint thread through
//! `Weak::upgrade_in_event_loop`, so the Slint thread never waits on the
//! network.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use slint::ComponentHandle as _;

use super::preset_download::{
    download_preset, preset_download_base_url, preset_download_override_notice, Downloader,
};
use super::preset_extract::{extract_preset, PresetDownloadResult};
use super::presets::apply_settings_preset_projection;
use super::root_props::system_preset_change_allowed;
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

/// How a download job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetDownloadCompletion {
    /// Downloaded and extracted: the preset is selectable.
    Extracted(PresetDownloadResult),
    /// The fetch, the write or the extraction failed.
    Failed(String),
}

/// The worker body: fetch the zip, then extract it. The extraction follows
/// the completed download unconditionally, as upstream's completion handler
/// does (`SettingsViewModel.cs:312-322`); `SystemPresetChangeAllowed` only
/// gates the start of a download.
pub fn run_preset_download_job(
    config_root: &Path,
    file_name: &str,
    base_url: &str,
    downloader: &dyn Downloader,
) -> PresetDownloadCompletion {
    let download = match download_preset(config_root, file_name, base_url, downloader) {
        Ok(download) => download,
        Err(error) => return PresetDownloadCompletion::Failed(error),
    };
    eprintln!(
        "[d4lc] download_preset: wrote {} bytes to {}",
        download.bytes,
        download.path.display()
    );
    match extract_preset(config_root, file_name) {
        Ok(extraction) => PresetDownloadCompletion::Extracted(PresetDownloadResult {
            download,
            extraction,
        }),
        Err(error) => PresetDownloadCompletion::Failed(error.to_string()),
    }
}

/// Run `job`, mapping a panic to [`PresetDownloadCompletion::Failed`] so the
/// screen always leaves its downloading state.
fn guard_job(job: impl FnOnce() -> PresetDownloadCompletion) -> PresetDownloadCompletion {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        PresetDownloadCompletion::Failed(format!("the download worker panicked: {detail}"))
    })
}

/// Handle the root `download-preset(string)` callback on the Slint thread.
///
/// Refuses while a download already runs (upstream's `_downloadInProgress`)
/// or the overlay is on, otherwise marks the screen as downloading and hands
/// the fetch to a worker. Returns without waiting for it.
pub fn start_preset_download(
    app: &AppWindow,
    state: &Arc<Mutex<AppState>>,
    config_root: &Path,
    file_name: &str,
    downloader: Arc<dyn Downloader + Send + Sync>,
) {
    if app.get_preset_downloading() {
        return;
    }
    {
        let mut state = lock(state);
        if !system_preset_change_allowed(&state.shell.settings) {
            state.append_log_message(
                format!(
                    "download_preset refused for '{file_name}': the system preset cannot \
                     change while the overlay is enabled"
                ),
                crate::clock::hh_mm_now(),
            );
            return;
        }
    }
    app.set_preset_downloading(true);
    app.set_preset_download_success(false);
    if let Some(notice) = preset_download_override_notice() {
        eprintln!("[d4lc] {notice}");
        lock(state).append_log_message(notice, crate::clock::hh_mm_now());
    }

    let app_weak = app.as_weak();
    let worker_state = Arc::clone(state);
    let worker_root: PathBuf = config_root.to_path_buf();
    let worker_file = file_name.to_string();
    let spawned = std::thread::Builder::new()
        .name("d4lc-preset-download".to_string())
        .spawn(move || {
            let base_url = preset_download_base_url();
            let completion = guard_job(|| {
                run_preset_download_job(&worker_root, &worker_file, &base_url, downloader.as_ref())
            });
            let _ = app_weak.upgrade_in_event_loop(move |app| {
                finish_preset_download(&app, &worker_state, &worker_root, &worker_file, completion);
            });
        });
    if let Err(error) = spawned {
        app.set_preset_downloading(false);
        lock(state).append_log_message(
            format!("download_preset failed for '{file_name}': cannot start the worker: {error}"),
            crate::clock::hh_mm_now(),
        );
    }
}

/// Project a finished download job on the Slint thread: log a failure, refresh the preset lists and re-enable the button.
pub fn finish_preset_download(
    app: &AppWindow,
    state: &Arc<Mutex<AppState>>,
    config_root: &Path,
    file_name: &str,
    completion: PresetDownloadCompletion,
) {
    let mut state = lock(state);
    let success = match completion {
        PresetDownloadCompletion::Extracted(_) => true,
        PresetDownloadCompletion::Failed(error) => {
            let message = format!("download_preset failed for '{file_name}': {error}");
            eprintln!("[d4lc] {message}");
            state.append_log_message(message, crate::clock::hh_mm_now());
            false
        }
    };
    apply_settings_preset_projection(app, &state, config_root, Some(file_name), success);
    app.set_preset_downloading(false);
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
    use crate::actions::settings::preset_download::{
        DownloadError, PRESETS_IMAGES_SUBDIR, PRESET_DOWNLOAD_BASE_URL,
    };
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeDownloader {
        response: Result<Vec<u8>, DownloadError>,
    }

    impl Downloader for FakeDownloader {
        fn fetch(&self, _url: &str, _max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
            self.response.clone()
        }
    }

    fn preset_zip_bytes() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, contents) in [
            (
                "9100p_JOB/config.json",
                b"{\"TooltipWidth\": 375}".as_slice(),
            ),
            ("9100p_JOB/tooltip_kb_all.png", b"png".as_slice()),
        ] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn tmp_root(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "d4lc-preset-job-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    struct PanicDownloader;

    impl Downloader for PanicDownloader {
        fn fetch(&self, _url: &str, _max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
            panic!("a refused download must never reach the network");
        }
    }

    #[test]
    fn start_refuses_while_a_download_runs_or_the_overlay_is_enabled() {
        let app = crate::test_window::new_app_window();
        let root = tmp_root("refuse");
        let state = Arc::new(Mutex::new(AppState::default_startup()));

        // A running download keeps the button disabled.
        app.set_preset_downloading(true);
        start_preset_download(
            &app,
            &state,
            &root,
            "9100p_JOB.zip",
            Arc::new(PanicDownloader),
        );
        assert!(app.get_preset_downloading());

        // An enabled overlay refuses the download and says so.
        app.set_preset_downloading(false);
        lock(&state).shell.settings.overlay_enabled = true;
        start_preset_download(
            &app,
            &state,
            &root,
            "9100p_JOB.zip",
            Arc::new(PanicDownloader),
        );
        assert!(!app.get_preset_downloading());
        let logged = lock(&state)
            .logging
            .messages
            .iter()
            .any(|entry| entry.message.contains("download_preset refused"));
        assert!(logged);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finish_re_enables_the_button_and_reports_a_failed_job() {
        let app = crate::test_window::new_app_window();
        let root = tmp_root("finish");
        let state = Arc::new(Mutex::new(AppState::default_startup()));
        app.set_preset_downloading(true);

        finish_preset_download(
            &app,
            &state,
            &root,
            "9100p_JOB.zip",
            PresetDownloadCompletion::Failed("connection refused".to_string()),
        );

        assert!(!app.get_preset_downloading());
        assert!(!app.get_preset_download_success());
        let logged = lock(&state).logging.messages.iter().any(|entry| {
            entry
                .message
                .contains("download_preset failed for '9100p_JOB.zip'")
        });
        assert!(logged, "a failed job must reach the Logging screen");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn job_downloads_and_extracts() {
        let root = tmp_root("extract");
        let downloader = FakeDownloader {
            response: Ok(preset_zip_bytes()),
        };

        let completion = run_preset_download_job(
            &root,
            "9100p_JOB.zip",
            PRESET_DOWNLOAD_BASE_URL,
            &downloader,
        );

        let PresetDownloadCompletion::Extracted(result) = completion else {
            panic!("expected an extracted preset, got {completion:?}");
        };
        assert_eq!(result.extraction.preset_name, "9100p_JOB");
        assert!(root
            .join(PRESETS_IMAGES_SUBDIR)
            .join("9100p_JOB/config.json")
            .is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_panicking_job_becomes_a_failure_so_the_button_re_enables() {
        let root = tmp_root("panic");
        let completion = guard_job(|| {
            run_preset_download_job(
                &root,
                "9100p_JOB.zip",
                PRESET_DOWNLOAD_BASE_URL,
                &PanicDownloader,
            )
        });

        let PresetDownloadCompletion::Failed(message) = completion else {
            panic!("expected a failure, got {completion:?}");
        };
        assert!(message.contains("panicked"), "got: {message}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn job_reports_a_transport_failure_without_writing() {
        let root = tmp_root("failed");
        let downloader = FakeDownloader {
            response: Err(DownloadError::Transport("connection refused".to_string())),
        };

        let completion = run_preset_download_job(
            &root,
            "9100p_JOB.zip",
            PRESET_DOWNLOAD_BASE_URL,
            &downloader,
        );

        let PresetDownloadCompletion::Failed(message) = completion else {
            panic!("expected a failure, got {completion:?}");
        };
        assert!(message.contains("connection refused"), "got: {message}");
        assert!(!root
            .join(PRESETS_IMAGES_SUBDIR)
            .join("9100p_JOB.zip")
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
