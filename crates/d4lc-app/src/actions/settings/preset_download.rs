//! Community preset download.

use std::path::{Path, PathBuf};

use super::http_fetch::{effective_authority, UrlPolicy};
use super::preset_install::trusted_images_dir;

// ── Community preset download ────────────────────────────────
//
// Upstream evidence:
//   - `SystemPresetManager.cs::DownloadSystemPreset` builds
//     `https://github.com/josdemmers/Diablo4Companion/raw/master/downloads/systempresets/{fileName}`
//     for a `FileName` from `systempresets.json`.
//   - `HttpClientHandler.cs::DownloadZipSystemPreset` streams the
//     response body and writes it to `./Images/{fileName}` (the
//     application working directory).
//
// Linux adaptation: the destination is the configured
// `D4LC_CONFIG_ROOT/Images/{fileName}` (preserves the upstream
// `Images/` directory naming under our XDG config root). The
// callback string is treated as a basename only; any path
// traversal is rejected. Network is performed by an injected
// `Downloader` trait so default tests can supply a mock and
// never touch the real network; the real wired flow is testable
// headless through the `D4LC_PRESET_DOWNLOAD_BASE_URL` override
// (base URL seam, see `preset_download_base_url`).

/// Subdirectory under the config root where downloaded community
/// presets are written. Mirrors the upstream `./Images/` write
/// location while staying inside the D4LC config root so the
/// download is captured by the existing `D4LC_CONFIG_ROOT`
/// test/development override.
pub const PRESETS_IMAGES_SUBDIR: &str = "Images";

/// Upstream GitHub raw base URL for community / system preset
/// downloads. The full URL is
/// `format!("{base}/{fileName}")` with `base` resolved by
/// [`preset_download_base_url`]. Mirrors
/// `SystemPresetManager.cs::DownloadSystemPreset` and the matching
/// `HttpClientHandler.cs::DownloadZipSystemPreset`.
pub const PRESET_DOWNLOAD_BASE_URL: &str =
    "https://github.com/josdemmers/Diablo4Companion/raw/master/downloads/systempresets";

/// Hosts of the upstream preset flow: `github.com` answers `/raw/` URLs with
/// a redirect to `raw.githubusercontent.com`, which also serves the catalog.
const PRESET_HOSTS: &[&str] = &["github.com", "raw.githubusercontent.com"];

/// Size cap of a downloaded (and later extracted) preset archive. The
/// published archives are 20–89 KiB; 16 MiB leaves over 100x headroom.
pub const PRESET_ARCHIVE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Environment override for the preset-download base URL, following
/// the `D4LC_UPDATE_CHECK_URL` seam pattern
/// ([`crate::update_check::ENV_OVERRIDE_URL`]). When set to a
/// non-empty http(s) URL, the preset download job ([`super::preset_download_job`]) fetches
/// `{override}/{file_name}` instead of
/// `{PRESET_DOWNLOAD_BASE_URL}/{file_name}`; unset or empty keeps
/// the upstream default unchanged. The value is used verbatim
/// (no trailing-slash normalization), so the operator/test supplies
/// the exact base. The escape hatch exists so headless tests can
/// point the real wired download flow at a local stub server while
/// the suite stays network-free.
pub const ENV_OVERRIDE_BASE_URL: &str = "D4LC_PRESET_DOWNLOAD_BASE_URL";

/// Serializes every test that mutates the `D4LC_PRESET_DOWNLOAD_BASE_URL`
/// process-global, shared crate-internally. The resolver unit tests
/// (`download_preset_tests` below) and the headless harness's preset
/// download scenarios (`crate::testing_harness`) both set/remove the
/// override, and `cargo test` runs them as parallel threads of one
/// binary, so each family holds this lock across its whole env
/// set→remove window to keep the windows mutually exclusive.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve the download base URL for this session: the
/// `D4LC_PRESET_DOWNLOAD_BASE_URL` override when set to a non-empty
/// value, otherwise the upstream [`PRESET_DOWNLOAD_BASE_URL`].
///
/// Read per download (not cached) so a test can point consecutive
/// downloads at different stub states; production never sets the
/// override, so the resolved value is the constant upstream URL.
pub fn preset_download_base_url() -> String {
    if let Ok(override_url) = std::env::var(ENV_OVERRIDE_BASE_URL) {
        let trimmed = override_url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    PRESET_DOWNLOAD_BASE_URL.to_string()
}

/// A Logging-screen line naming the active `D4LC_PRESET_DOWNLOAD_BASE_URL`
/// override, so a stray environment setting is visible; `None` without one.
pub fn preset_download_override_notice() -> Option<String> {
    let base = preset_download_base_url();
    (base != PRESET_DOWNLOAD_BASE_URL)
        .then(|| format!("System presets are downloaded from {base} ({ENV_OVERRIDE_BASE_URL})."))
}

/// Outcome of a successful preset download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetDownloadOutcome {
    /// Resolved absolute path of the written zip on disk.
    pub path: PathBuf,
    /// Number of bytes written.
    pub bytes: u64,
}

/// Network error returned by a [`Downloader`].
#[derive(Debug, Clone)]
pub enum DownloadError {
    /// Transport-level error (DNS, TLS handshake, connection reset, etc.).
    Transport(String),
    /// Server returned a non-success HTTP status. `body` is the error
    /// response's body (empty when it had none or could not be read), which
    /// upstream's `GetRequest` hands back like any other body.
    HttpStatus {
        status: u16,
        reason: String,
        body: Vec<u8>,
    },
    /// The URL is not an acceptable absolute URL: unparsable, not `https`
    /// (outside a local override authority), or carrying credentials.
    InvalidUrl(String),
    /// The URL's host (or port) is outside the downloader's allowlist.
    UnexpectedHost(String),
    /// The response body exceeds the caller's size limit. `declared` is the
    /// `Content-Length` when that alone exceeded it; otherwise the limit was
    /// hit while reading.
    BodyTooLarge { limit: u64, declared: Option<u64> },
    /// A redirect was not followed: target outside the URL policy, `https`
    /// to `http` downgrade, loop, too many hops, or no `Location`.
    RedirectRefused(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Transport(message) => write!(f, "transport error: {message}"),
            DownloadError::HttpStatus { status, reason, .. } => {
                write!(f, "HTTP {status}: {reason}")
            }
            DownloadError::InvalidUrl(message) => write!(f, "invalid URL: {message}"),
            DownloadError::UnexpectedHost(host) => {
                write!(f, "unexpected download host: {host}")
            }
            DownloadError::BodyTooLarge {
                limit,
                declared: Some(declared),
            } => write!(
                f,
                "response too large: {declared} bytes declared, limit is {limit} bytes"
            ),
            DownloadError::BodyTooLarge {
                limit,
                declared: None,
            } => write!(f, "response too large: more than {limit} bytes"),
            DownloadError::RedirectRefused(reason) => write!(f, "redirect refused: {reason}"),
        }
    }
}

impl std::error::Error for DownloadError {}

/// Network downloader seam used by [`download_preset`].
///
/// Production implementation is [`UreqDownloader`] (gated on the
/// `preset-download` Cargo feature); tests inject
/// `MockDownloader` so default validation never reaches the
/// network.
pub trait Downloader {
    /// Fetch the body at `url`. A body longer than `max_bytes` is a
    /// [`DownloadError::BodyTooLarge`], never a truncated success.
    /// Implementations must not perform any side effects beyond the HTTP
    /// call.
    fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError>;
}

/// Build the download URL for a community / system preset zip
/// filename under `base_url`. The filename is sanitized to its
/// basename (forward or backward slashes are stripped) and the
/// empty filename is rejected.
///
/// `base_url` is the resolved session base — the production entry
/// point passes [`preset_download_base_url`] so the
/// `D4LC_PRESET_DOWNLOAD_BASE_URL` override applies; tests pass
/// [`PRESET_DOWNLOAD_BASE_URL`] or their own stub base explicitly.
///
/// Returns `Err(message)` for any unsafe or empty filename so
/// the caller can surface a structured error to the UI.
pub fn build_preset_download_url(base_url: &str, file_name: &str) -> Result<String, String> {
    let sanitized = sanitize_preset_file_name(file_name)?;
    Ok(format!("{base_url}/{sanitized}"))
}

/// Strip a user-supplied filename down to a safe basename:
/// - any input containing a path separator (`/` or `\`) is
///   rejected outright — upstream community preset names are
///   flat basenames (`1080p_SMF.zip`, `1440p_SSF_zhCN.zip`).
/// - the literal names `.` and `..` are rejected.
/// - empty / whitespace-only inputs are rejected.
/// - null bytes are rejected.
/// - the result is required to end with `.zip` (case-insensitive)
///   to match the upstream community preset shape.
pub fn sanitize_preset_file_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("preset filename is empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("preset filename contains a null byte".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(format!(
            "preset filename '{raw}' contains a path separator; only flat basenames are allowed"
        ));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(format!("preset filename '{trimmed}' is not allowed"));
    }
    if !trimmed.to_ascii_lowercase().ends_with(".zip") {
        return Err(format!(
            "preset filename '{trimmed}' must end with .zip (upstream system presets are zips)"
        ));
    }
    Ok(trimmed.to_string())
}

/// Resolve the absolute destination path for a downloaded preset:
/// `{Images}/{basename}`, where the images directory is created when missing
/// and must resolve inside the config root (see
/// [`trusted_images_dir`]). The
/// final component is not resolved: the write replaces whatever directory
/// entry is there instead of following it.
pub fn resolve_preset_destination(config_root: &Path, file_name: &str) -> Result<PathBuf, String> {
    let sanitized = sanitize_preset_file_name(file_name)?;
    let images_dir = trusted_images_dir(config_root, true).map_err(|error| error.to_string())?;
    Ok(images_dir.join(sanitized))
}

/// Headless community-preset download flow.
///
/// Builds the download URL under `base_url` (the resolved session
/// base — see [`preset_download_base_url`]), fetches at most
/// [`PRESET_ARCHIVE_MAX_BYTES`] through the injected [`Downloader`],
/// requires the body to parse as a zip archive, replaces
/// `config_root/PRESETS_IMAGES_SUBDIR/{file_name}` with it, and returns a
/// [`PresetDownloadOutcome`]. The function does **not** mutate any
/// `AppState`. On any failure (fetch, archive check, write) it returns an
/// error and the previous archive, if any, is unchanged.
///
/// Completeness: the HTTP layer cannot detect every truncated body (see
/// `http_fetch::fetch_bounded_with_timeout`: a close-delimited body cut
/// short, or a chunked body cut inside chunk data under ureq 2). Such a
/// body is a strict prefix of the archive, and the zip
/// end-of-central-directory record is the last record of the file, so the
/// archive check refuses it; per-entry CRC-32 checks during extraction are
/// the second line. This proves completeness against transport truncation,
/// not authenticity: a server that controls the bytes can serve any valid
/// zip, which the extraction limits then bound.
pub fn download_preset(
    config_root: &Path,
    file_name: &str,
    base_url: &str,
    downloader: &dyn Downloader,
) -> Result<PresetDownloadOutcome, String> {
    let url = build_preset_download_url(base_url, file_name)?;
    let destination = resolve_preset_destination(config_root, file_name)?;

    let bytes = downloader
        .fetch(&url, PRESET_ARCHIVE_MAX_BYTES)
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Err(format!(
            "upstream returned an empty body for '{url}'; refusing to write a zero-byte preset"
        ));
    }
    zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).map_err(|error| {
        format!(
            "the download from '{url}' is not a complete zip archive ({error}); \
             the previous archive is kept"
        )
    })?;
    replace_file(&destination, &bytes)?;
    Ok(PresetDownloadOutcome {
        path: destination,
        bytes: bytes.len() as u64,
    })
}

/// Replace `destination` with `bytes`: write a fresh sibling temp file
/// (`create_new`, so an existing file or symlink at the temp name is never
/// followed), sync it, and rename it over `destination`. Readers see the
/// previous file or the complete new one; a symlink at `destination` is
/// replaced, not followed. The temp file is removed on failure.
fn replace_file(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    let (Some(dir), Some(name)) = (destination.parent(), destination.file_name()) else {
        return Err(format!("invalid destination {}", destination.display()));
    };
    let (staging, mut file) = loop {
        let candidate = dir.join(format!(
            ".{}.{}-{}.part",
            name.to_string_lossy(),
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to create preset staging file {}: {error}",
                    candidate.display()
                ))
            }
        }
    };
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "failed to write preset staging file {}: {error}",
                staging.display()
            )
        })
        .and_then(|()| {
            std::fs::rename(&staging, destination).map_err(|error| {
                format!(
                    "failed to rename {} -> {}: {error}",
                    staging.display(),
                    destination.display()
                )
            })
        });
    drop(file);
    if result.is_err() {
        if let Err(error) = std::fs::remove_file(&staging) {
            eprintln!(
                "[d4lc] download_preset: cannot remove staging file {}: {error}",
                staging.display()
            );
        }
    }
    result
}

/// `ureq`-backed [`Downloader`] of the preset flow (catalog and archives).
/// Only compiled with the `preset-download` Cargo feature; without it the
/// fallback below reports the capability gap.
#[cfg(feature = "preset-download")]
pub struct UreqDownloader;

#[cfg(feature = "preset-download")]
impl Downloader for UreqDownloader {
    fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
        fetch_preset_body(url, max_bytes).map(|fetched| fetched.bytes)
    }
}

/// The preset-flow fetch of [`UreqDownloader`], keeping the final URL:
/// [`preset_url_policy`] re-resolved per fetch (so it agrees with the URL
/// the download job built from the same session base) and
/// [`fetch_bounded`](super::http_fetch::fetch_bounded).
#[cfg(feature = "preset-download")]
pub(crate) fn fetch_preset_body(
    url: &str,
    max_bytes: u64,
) -> Result<super::http_fetch::FetchedBody, DownloadError> {
    let policy = preset_url_policy(&preset_download_base_url());
    let fetched = super::http_fetch::fetch_bounded(&policy, url, max_bytes)?;
    if fetched.final_url != url {
        eprintln!("[d4lc] download: {url} served by {}", fetched.final_url);
    }
    Ok(fetched)
}

/// Disabled-feature fallback: a structured error, so a build without
/// `preset-download` never reports a download that did not happen.
#[cfg(not(feature = "preset-download"))]
pub struct UreqDownloader;

#[cfg(not(feature = "preset-download"))]
impl Downloader for UreqDownloader {
    fn fetch(&self, _url: &str, _max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
        Err(DownloadError::Transport(
            "preset-download feature is not enabled in this build".to_string(),
        ))
    }
}

/// URL policy of the preset flow, applied to the request URL and every
/// redirect hop: `https` on [`PRESET_HOSTS`], plus — only while the
/// `D4LC_PRESET_DOWNLOAD_BASE_URL` override is active — exactly the
/// authority (host:port) of the configured session base, over `http` too,
/// so tests can target a local stub server. A malformed override base adds
/// nothing.
pub(crate) fn preset_url_policy(session_base_url: &str) -> UrlPolicy {
    let policy = UrlPolicy::exact_hosts(PRESET_HOSTS);
    if session_base_url == PRESET_DOWNLOAD_BASE_URL {
        return policy;
    }
    match effective_authority(session_base_url) {
        Some(authority) => policy.with_local_authority(&authority),
        None => policy,
    }
}

// ── Community preset download tests ───────────────────────
//
// Default tests must NOT hit the real network. The `MockDownloader`
// below fakes the network call; the shared HTTP path is tested against
// loopback servers in `http_fetch`, and `UreqDownloader` against the real
// host only in ignored live smoke tests.

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Captures the URL the most recent fetch saw.
    #[derive(Default)]
    struct MockDownloader {
        seen_urls: Mutex<Vec<String>>,
        seen_limits: Mutex<Vec<u64>>,
        response: Vec<u8>,
        /// If `Some`, the next fetch returns this error.
        error: Option<DownloadError>,
    }

    impl MockDownloader {
        fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self {
                seen_urls: Mutex::new(Vec::new()),
                seen_limits: Mutex::new(Vec::new()),
                response: body.into(),
                error: None,
            }
        }
        fn err(error: DownloadError) -> Self {
            Self {
                seen_urls: Mutex::new(Vec::new()),
                seen_limits: Mutex::new(Vec::new()),
                response: Vec::new(),
                error: Some(error),
            }
        }
    }

    impl Downloader for MockDownloader {
        fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
            self.seen_urls
                .lock()
                .expect("poisoned")
                .push(url.to_string());
            self.seen_limits.lock().expect("poisoned").push(max_bytes);
            match &self.error {
                Some(error) => Err(error.clone()),
                None => Ok(self.response.clone()),
            }
        }
    }

    static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
    fn unique_tmp_root(label: &str) -> std::path::PathBuf {
        let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("d4lc-download-preset-{label}-{pid}-{n}"))
    }

    #[test]
    fn build_preset_download_url_sanitizes_basename() {
        assert_eq!(
            build_preset_download_url(PRESET_DOWNLOAD_BASE_URL, "1080p_SMF.zip").unwrap(),
            format!("{PRESET_DOWNLOAD_BASE_URL}/1080p_SMF.zip")
        );
        // Mixed-case / extension-only changes: the basename is
        // preserved verbatim (we only strip path components).
        assert_eq!(
            build_preset_download_url(PRESET_DOWNLOAD_BASE_URL, "1440p_SSF_zhCN.zip").unwrap(),
            format!("{PRESET_DOWNLOAD_BASE_URL}/1440p_SSF_zhCN.zip")
        );
    }

    #[test]
    fn build_preset_download_url_rejects_path_traversal() {
        for bad in [
            "../etc/passwd",
            "..\\windows\\system32",
            "subdir/file.zip",
            "/absolute/file.zip",
            ".",
            "..",
            "",
            "   ",
        ] {
            assert!(
                build_preset_download_url(PRESET_DOWNLOAD_BASE_URL, bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn build_preset_download_url_rejects_non_zip_extension() {
        assert!(build_preset_download_url(PRESET_DOWNLOAD_BASE_URL, "1080p_SMF").is_err());
        assert!(build_preset_download_url(PRESET_DOWNLOAD_BASE_URL, "1080p_SMF.exe").is_err());
        assert!(build_preset_download_url(PRESET_DOWNLOAD_BASE_URL, "1080p_SMF.ZIP").is_ok());
    }

    #[test]
    fn build_preset_download_url_honors_stub_base() {
        // The `D4LC_PRESET_DOWNLOAD_BASE_URL` seam passes the stub
        // base explicitly; the URL is `{base}/{sanitized}` verbatim.
        let url = build_preset_download_url("http://127.0.0.1:4242", "1080p_SMF.zip")
            .expect("stub base must build");
        assert_eq!(url, "http://127.0.0.1:4242/1080p_SMF.zip");
    }

    #[test]
    fn download_preset_writes_bytes_under_images_dir() {
        let root = unique_tmp_root("happy");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let archive = tiny_zip(b"content");
        let mock = MockDownloader::ok(archive.clone());
        let outcome = download_preset(&root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock)
            .expect("download ok");

        // The mock should have been called exactly once with
        // the expected upstream URL.
        let seen = mock.seen_urls.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0], format!("{PRESET_DOWNLOAD_BASE_URL}/1080p_SMF.zip"));
        drop(seen);
        assert_eq!(
            mock.seen_limits.lock().unwrap().as_slice(),
            [PRESET_ARCHIVE_MAX_BYTES]
        );

        // The file should exist on disk under
        // `config_root/Images/` with the expected contents.
        assert!(outcome.path.exists());
        assert!(outcome.path.starts_with(root.join(PRESETS_IMAGES_SUBDIR)));
        assert_eq!(outcome.bytes, archive.len() as u64);
        let written = std::fs::read(&outcome.path).expect("read back");
        assert_eq!(written, archive);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn download_preset_rejects_path_traversal_destination() {
        let root = unique_tmp_root("traversal");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let mock = MockDownloader::ok(b"bytes".to_vec());
        // Sanitizer rejects the input; the mock should not be
        // called at all.
        let result = download_preset(&root, "../escape.zip", PRESET_DOWNLOAD_BASE_URL, &mock);
        assert!(result.is_err());
        let seen = mock.seen_urls.lock().unwrap();
        assert!(
            seen.is_empty(),
            "expected zero network calls for traversal filename"
        );
        drop(seen);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn download_preset_propagates_transport_error() {
        let root = unique_tmp_root("transport");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let mock = MockDownloader::err(DownloadError::Transport("dns failure".to_string()));
        let result = download_preset(&root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock);
        let message = result.expect_err("transport must fail");
        assert!(message.contains("transport error"), "got: {message}");
        assert!(message.contains("dns failure"), "got: {message}");

        // No file should be written on a transport error.
        assert!(!root
            .join(PRESETS_IMAGES_SUBDIR)
            .join("1080p_SMF.zip")
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn download_preset_propagates_http_error() {
        let root = unique_tmp_root("http");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let mock = MockDownloader::err(DownloadError::HttpStatus {
            status: 404,
            reason: "Not Found".to_string(),
            body: Vec::new(),
        });
        let result = download_preset(&root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock);
        let message = result.expect_err("http error must fail");
        assert!(message.contains("HTTP 404"), "got: {message}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn download_preset_rejects_empty_response() {
        let root = unique_tmp_root("empty");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        let mock = MockDownloader::ok(Vec::new());
        let result = download_preset(&root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock);
        let message = result.expect_err("empty body must fail");
        assert!(message.contains("empty body"), "got: {message}");
        assert!(!root
            .join(PRESETS_IMAGES_SUBDIR)
            .join("1080p_SMF.zip")
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A minimal valid preset-shaped archive holding `content`.
    fn tiny_zip(content: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                "1080p_SMF/config.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn download_preset_refuses_every_truncated_archive_and_keeps_the_previous_one() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = temp.path().join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        let previous = tiny_zip(b"previous");
        std::fs::write(images_dir.join("1080p_SMF.zip"), &previous).unwrap();

        let small = tiny_zip(b"new");
        let vendored = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/original/systempresets/zips/1080p_SMF.zip"),
        )
        .unwrap();
        // Every strict prefix of a small archive, and a spread of prefixes
        // of a real published one (each check scans up to 64 KiB).
        let prefixes = (1..small.len())
            .map(|len| &small[..len])
            .chain((1..vendored.len()).step_by(211).map(|len| &vendored[..len]))
            .chain([&b"NOT-A-ZIP"[..]]);
        for body in prefixes {
            let mock = MockDownloader::ok(body.to_vec());
            let error = download_preset(
                temp.path(),
                "1080p_SMF.zip",
                PRESET_DOWNLOAD_BASE_URL,
                &mock,
            )
            .expect_err("a truncated archive is refused");
            assert!(
                error.contains("not a complete zip archive"),
                "{} bytes: {error}",
                body.len()
            );
        }
        assert_eq!(
            std::fs::read(images_dir.join("1080p_SMF.zip")).unwrap(),
            previous
        );
        assert!(part_files(&images_dir).is_empty());
    }

    #[test]
    fn download_preset_overwrites_an_existing_archive() {
        let root = unique_tmp_root("overwrite");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        // Pre-populate the destination with a previous content
        // and assert the new fetch replaces it.
        let target = root.join(PRESETS_IMAGES_SUBDIR).join("1080p_SMF.zip");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"OLD-CONTENT").unwrap();

        let archive = tiny_zip(b"new");
        let mock = MockDownloader::ok(archive.clone());
        let outcome = download_preset(&root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock)
            .expect("download ok");
        let read = std::fs::read(&outcome.path).expect("read back");
        assert_eq!(read, archive);
        // No staging file survives the rename.
        assert!(part_files(&root.join(PRESETS_IMAGES_SUBDIR)).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    fn part_files(images_dir: &Path) -> Vec<String> {
        std::fs::read_dir(images_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".part"))
            .collect()
    }

    #[test]
    fn download_preset_replaces_a_symlink_at_the_destination_without_following_it() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("config");
        let images_dir = root.join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        let outside = temp.path().join("outside.zip");
        std::fs::write(&outside, b"OUTSIDE").unwrap();
        std::os::unix::fs::symlink(&outside, images_dir.join("1080p_SMF.zip")).unwrap();

        let archive = tiny_zip(b"new");
        let mock = MockDownloader::ok(archive.clone());
        download_preset(&root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock)
            .expect("download ok");

        assert_eq!(std::fs::read(&outside).unwrap(), b"OUTSIDE");
        let destination = images_dir.join("1080p_SMF.zip");
        assert!(!destination.symlink_metadata().unwrap().is_symlink());
        assert_eq!(std::fs::read(&destination).unwrap(), archive);
        assert!(part_files(&images_dir).is_empty());
    }

    #[test]
    fn download_preset_refuses_an_images_symlink_leaving_the_config_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("config");
        let outside = temp.path().join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(PRESETS_IMAGES_SUBDIR)).unwrap();

        let mock = MockDownloader::ok(tiny_zip(b"x"));
        let error = download_preset(&root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock)
            .expect_err("an images dir outside the config root is refused");
        assert!(error.contains("outside the config root"), "{error}");
        assert!(mock.seen_urls.lock().unwrap().is_empty(), "no fetch");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn download_preset_failure_keeps_the_previous_archive_and_leaves_no_staging_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let images_dir = root.join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        std::fs::write(images_dir.join("1080p_SMF.zip"), b"OLD").unwrap();

        let mock = MockDownloader::err(DownloadError::BodyTooLarge {
            limit: PRESET_ARCHIVE_MAX_BYTES,
            declared: None,
        });
        let error = download_preset(root, "1080p_SMF.zip", PRESET_DOWNLOAD_BASE_URL, &mock)
            .expect_err("oversized body fails");
        assert!(error.contains("response too large"), "{error}");
        assert_eq!(
            std::fs::read(images_dir.join("1080p_SMF.zip")).unwrap(),
            b"OLD"
        );
        assert!(part_files(&images_dir).is_empty());
    }

    #[test]
    #[cfg(not(feature = "preset-download"))]
    fn ureq_downloader_disabled_feature_returns_structured_error() {
        // When the `preset-download` feature is **not** enabled,
        // the production `UreqDownloader` must refuse the
        // request with a structured error so the UI can surface
        // a capability gap. This test runs in builds
        // that explicitly opt out of the feature (e.g.
        // `cargo test -p d4lc-app --no-default-features`) and
        // never touches the network.
        //
        // In the default build (`preset-download` is a default
        // feature) the test is compiled out — calling
        // `downloader.fetch(...)` here would otherwise open a
        // real socket, which is unacceptable for the default
        // validation suite. The enabled-build path is covered
        // by `ureq_downloader_enabled_rejects_unexpected_host`
        // (host validator, no socket opened) and by the
        // `#[ignore]`-d manual smoke
        // `ureq_downloader_enabled_smoke_is_ignored` below.
        let downloader = UreqDownloader;
        let result = downloader.fetch(
            "https://github.com/josdemmers/Diablo4Companion/raw/master/downloads/systempresets/1080p_SMF.zip",
            PRESET_ARCHIVE_MAX_BYTES,
        );
        let message = result.expect_err("disabled build must not call network");
        assert!(
            message
                .to_string()
                .contains("preset-download feature is not enabled"),
            "got: {message}"
        );
    }

    #[test]
    fn ureq_downloader_enabled_rejects_unexpected_host() {
        // `fetch_bounded` checks the URL policy before any request is
        // built, so the rejection path opens no socket. Only meaningful
        // when the `preset-download` feature is enabled.
        #[cfg(feature = "preset-download")]
        {
            let downloader = UreqDownloader;
            let result = downloader.fetch("https://example.com/1080p_SMF.zip", 1024);
            let message = result.expect_err("example.com must be rejected");
            assert!(
                message.to_string().contains("unexpected download host"),
                "got: {message}"
            );
        }
    }

    /// Manual smoke: only meaningful when the `preset-download`
    /// feature is enabled AND a real network is available. Runs
    /// with `--ignored` so the default `cargo test` validation
    /// stays network-free. Run via
    /// `cargo test -p d4lc-app --features preset-download download_preset -- --ignored`.
    #[test]
    #[cfg(feature = "preset-download")]
    #[ignore = "manual smoke: requires real network and a writable temp dir"]
    fn ureq_downloader_enabled_smoke_is_ignored() {
        let downloader = UreqDownloader;
        let result = downloader.fetch(
            "https://github.com/josdemmers/Diablo4Companion/raw/master/downloads/systempresets/systempresets.json",
            1024 * 1024,
        );
        // We do not assert success because the upstream
        // content / availability may change; the contract is
        // that the call must produce either bytes or a
        // structured `DownloadError`, never a panic.
        match result {
            Ok(bytes) => assert!(!bytes.is_empty(), "smoke body must be non-empty"),
            Err(error) => {
                let message = error.to_string();
                assert!(
                    !message.contains("unexpected download host"),
                    "host validation regression: {message}"
                );
            }
        }
    }

    fn validate_preset_url(url: &str, session_base_url: &str) -> Result<(), DownloadError> {
        preset_url_policy(session_base_url).check(url).map(|_| ())
    }

    #[test]
    fn validate_preset_url_accepts_github_hosts() {
        let base = PRESET_DOWNLOAD_BASE_URL;
        validate_preset_url(
            "https://github.com/josdemmers/Diablo4Companion/raw/master/downloads/systempresets/1080p_SMF.zip",
            base,
        )
        .expect("github.com must be accepted");
        // Plain http is refused for the upstream hosts (it was accepted
        // before redirects were policy-checked).
        let err = validate_preset_url(
            "http://github.com/josdemmers/Diablo4Companion/raw/master/downloads/systempresets/1080p_SMF.zip",
            base,
        )
        .expect_err("plain http to github.com is refused");
        assert!(matches!(err, DownloadError::InvalidUrl(_)));
        validate_preset_url(
            "https://raw.githubusercontent.com/josdemmers/Diablo4Companion/master/downloads/systempresets/1080p_SMF.zip",
            base,
        )
        .expect("raw.githubusercontent.com must be accepted");
    }

    #[test]
    fn validate_preset_url_rejects_other_hosts_and_schemes() {
        let base = PRESET_DOWNLOAD_BASE_URL;
        let err =
            validate_preset_url("file:///etc/passwd", base).expect_err("file scheme rejected");
        assert!(matches!(err, DownloadError::InvalidUrl(_)));
        let err = validate_preset_url("ftp://github.com/foo.zip", base).expect_err("ftp rejected");
        assert!(matches!(err, DownloadError::InvalidUrl(_)));
        let err =
            validate_preset_url("https://example.com/foo.zip", base).expect_err("host rejected");
        assert!(matches!(err, DownloadError::UnexpectedHost(_)));
        let err = validate_preset_url("not-a-url", base).expect_err("bare string rejected");
        assert!(matches!(err, DownloadError::InvalidUrl(_)));
    }

    #[test]
    fn validate_preset_url_allows_exactly_the_override_host() {
        // While the `D4LC_PRESET_DOWNLOAD_BASE_URL` override is
        // active, the session base's authority (host:port) is
        // accepted — a local stub server on a dynamic port — while
        // every other non-GitHub host stays rejected.
        let stub_base = "http://127.0.0.1:4242";
        validate_preset_url("http://127.0.0.1:4242/1080p_SMF.zip", stub_base)
            .expect("the configured stub authority must be accepted");
        let err = validate_preset_url("http://127.0.0.1:9999/1080p_SMF.zip", stub_base)
            .expect_err("a different port is a different authority");
        assert!(matches!(err, DownloadError::UnexpectedHost(_)));
        let err = validate_preset_url("http://localhost:4242/1080p_SMF.zip", stub_base)
            .expect_err("a different host stays rejected even with the override");
        assert!(matches!(err, DownloadError::UnexpectedHost(_)));
    }

    #[test]
    fn validate_preset_url_stays_fail_closed_on_malformed_override_base() {
        // An override that is not an absolute http(s) URL never widens
        // the host allowlist: the fetch fails closed with the usual
        // host error instead of opening an unvalidated socket.
        let err = validate_preset_url("https://example.com/foo.zip", "not-a-url")
            .expect_err("malformed override must not widen the allowlist");
        assert!(matches!(err, DownloadError::UnexpectedHost(_)));
        let err = validate_preset_url("https://example.com/foo.zip", "ftp://127.0.0.1:1/x.zip")
            .expect_err("non-http override must not widen the allowlist");
        assert!(matches!(err, DownloadError::UnexpectedHost(_)));
    }

    // ── D4LC_PRESET_DOWNLOAD_BASE_URL resolver (seam matrix) ────────────
    //
    // `preset_download_base_url()` reads the process-global env
    // override, so these tests serialize on `ENV_LOCK` — the
    // crate-shared static defined next to `ENV_OVERRIDE_BASE_URL`.
    // The harness's preset-download scenario family also sets/removes the
    // same override and holds the same lock across its whole env
    // set→remove window, so the two families' env windows are
    // mutually exclusive inside the parallel test binary. The
    // resolver/validator under test here take the base as a
    // parameter everywhere else (env-free by design).

    #[test]
    fn preset_download_base_url_defaults_to_upstream() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        std::env::remove_var(ENV_OVERRIDE_BASE_URL);
        assert_eq!(preset_download_base_url(), PRESET_DOWNLOAD_BASE_URL);

        // Empty / whitespace overrides fall back to the default too.
        std::env::set_var(ENV_OVERRIDE_BASE_URL, "");
        assert_eq!(preset_download_base_url(), PRESET_DOWNLOAD_BASE_URL);
        std::env::set_var(ENV_OVERRIDE_BASE_URL, "   ");
        assert_eq!(preset_download_base_url(), PRESET_DOWNLOAD_BASE_URL);
        std::env::remove_var(ENV_OVERRIDE_BASE_URL);
    }

    #[test]
    fn the_override_is_announced_only_while_active() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        std::env::remove_var(ENV_OVERRIDE_BASE_URL);
        assert_eq!(preset_download_override_notice(), None);
        std::env::set_var(ENV_OVERRIDE_BASE_URL, "http://127.0.0.1:4242");
        let notice = preset_download_override_notice().expect("override is announced");
        assert!(
            notice.contains("http://127.0.0.1:4242") && notice.contains(ENV_OVERRIDE_BASE_URL),
            "{notice}"
        );
        std::env::remove_var(ENV_OVERRIDE_BASE_URL);
    }

    #[test]
    fn preset_download_base_url_override_takes_precedence() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let stub = "http://127.0.0.1:4242";
        std::env::set_var(ENV_OVERRIDE_BASE_URL, stub);
        assert_eq!(preset_download_base_url(), stub);
        std::env::remove_var(ENV_OVERRIDE_BASE_URL);
        assert_eq!(preset_download_base_url(), PRESET_DOWNLOAD_BASE_URL);
    }
}
