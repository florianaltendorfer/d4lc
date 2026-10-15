//! Provider download HTTP client and the InfinityBuilds / Maxroll
//! download flows.

use std::path::{Path, PathBuf};

use d4lc_config::AppSettings;
use d4lc_rules::{ImportUnmappedEntry, ImportWarning};

use super::{
    PROVIDER_D2CORE, PROVIDER_D4BUILDS, PROVIDER_INFINITY_BUILDS, PROVIDER_MAXROLL,
    PROVIDER_MOBALYTICS,
};
use crate::actions::build_download::{ImportError, ImportErrorKind};
use crate::actions::settings::{DownloadError, UrlPolicy};

/// Classify an HTTP fetch failure of a provider download.
pub(super) fn classify_download_error(error: DownloadError) -> ImportError {
    let kind = match &error {
        DownloadError::Transport(message)
            if message.contains("timed out") || message.contains("timeout") =>
        {
            ImportErrorKind::Timeout
        }
        DownloadError::Transport(_) | DownloadError::RedirectRefused(_) => ImportErrorKind::Network,
        // Cloudflare-style refusals and rate limits.
        DownloadError::HttpStatus {
            status: 403 | 429, ..
        } => ImportErrorKind::Blocked,
        DownloadError::HttpStatus { .. } => ImportErrorKind::Network,
        DownloadError::InvalidUrl(_) | DownloadError::UnexpectedHost(_) => {
            ImportErrorKind::InvalidUrl
        }
        DownloadError::BodyTooLarge { .. } => ImportErrorKind::InvalidResponse,
    };
    ImportError::new(kind, error.to_string())
}

// ---------------------------------------------------------------------------
// InfinityBuilds share-page download.
//
// Upstream `BuildsManagerInfinityBuilds.DownloadInfinityBuildsBuild` drives
// a Selenium/DevTools browser session to capture the Next.js flight payload
// of a share page. The Rust port is not browser-driven; the equivalent
// mechanism (parity `equivalent`) is a direct HTTP fetch of the same page
// through the approved `ureq` seam, followed by the exact upstream JSON
// navigation in `d4lc_rules::import_infinity_builds_share_html` (conversion
// parity `exact`). No live network runs in default tests: the fetch sits
// behind the `Downloader` seam and tests inject a mock.
// ---------------------------------------------------------------------------

/// Allowed share host for the InfinityBuilds download
/// (upstream `CanAddInfinityBuildsBuildExecute` checks
/// `BuildUrlInfinityBuilds.Contains("infinitybuilds.gg")`).
pub const INFINITY_BUILDS_SHARE_HOST_SUFFIX: &str = "infinitybuilds.gg";

/// Size cap of a fetched InfinityBuilds share page (a live page is about
/// 400 KiB).
pub const INFINITY_BUILDS_PAGE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Provider URL host suffix for the pasted-build-URL gate, mirroring the
/// documented upstream `Contains("<provider domain>")` gate. The
/// InfinityBuilds suffix is upstream-explicit; the other four mirror the
/// same gate shape using the documented website entries
/// (`AffixViewModel.InitBuildImportWebsites`).
pub fn provider_url_host_suffix(provider: &str) -> Option<&'static str> {
    match provider {
        PROVIDER_D2CORE => Some("d2core.com"),
        PROVIDER_D4BUILDS => Some("d4builds.gg"),
        PROVIDER_MAXROLL => Some("maxroll.gg"),
        PROVIDER_MOBALYTICS => Some("mobalytics.gg"),
        PROVIDER_INFINITY_BUILDS => Some(INFINITY_BUILDS_SHARE_HOST_SUFFIX),
        _ => None,
    }
}

/// Validate a pasted build URL for `provider` with the same [`UrlPolicy`]
/// the provider download applies: `https`, no credentials, default port, a
/// host name equal to the provider domain or a subdomain of it (the upstream
/// `contains` gate, pinned to scheme + host).
pub fn validate_provider_build_url(provider: &str, url: &str) -> Result<(), String> {
    let domain = provider_url_host_suffix(provider)
        .ok_or_else(|| format!("provider '{provider}' does not accept pasted build URLs"))?;
    UrlPolicy::domain_suffix(domain)
        .check(url)
        .map(|_| ())
        .map_err(|error| format!("build URL refused for {domain}: {error}"))
}

/// Validate the share URL: [`validate_provider_build_url`] for the
/// `infinitybuilds.gg` domain.
pub fn validate_infinity_builds_url(url: &str) -> Result<(), String> {
    validate_provider_build_url(PROVIDER_INFINITY_BUILDS, url)
}

// ---------------------------------------------------------------------------
// Provider download HTTP client.
//
// Upstream funnels every build download through the shared
// `HttpClientHandler` (`D4Companion.Services/HttpClientHandler.cs`),
// which pins `User-Agent: D4Companion` and `Cache-Control: no-cache`
// on every request (HttpClientHandler.cs:73-77). The port keeps that
// identity (see `settings::fetch_bounded`) and adds a stricter
// per-provider host gate: a [`ProviderDownloader`] fetches only over https
// from the domain of the one provider it was created for — the request
// URL and every redirect hop — so a download flow can never be pointed at
// a foreign host.
pub struct ProviderDownloader {
    policy: UrlPolicy,
}

impl ProviderDownloader {
    /// The downloader for `provider`, or `None` when the provider id is
    /// not one of the five build-import providers.
    pub fn for_provider(provider: &str) -> Option<Self> {
        provider_url_host_suffix(provider).map(|domain| Self {
            policy: UrlPolicy::domain_suffix(domain),
        })
    }
}

#[cfg(feature = "preset-download")]
impl crate::actions::settings::Downloader for ProviderDownloader {
    fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
        crate::actions::settings::fetch_bounded(&self.policy, url, max_bytes)
            .map(|fetched| fetched.bytes)
    }
}

/// Disabled-feature fallback: behave as a structured error so builds
/// without the `preset-download` feature report the capability
/// gap instead of faking a download.
#[cfg(not(feature = "preset-download"))]
impl crate::actions::settings::Downloader for ProviderDownloader {
    fn fetch(&self, url: &str, _max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
        self.policy.check(url)?;
        Err(DownloadError::Transport(
            "the `preset-download` feature (HTTP client) is not enabled in this build".to_string(),
        ))
    }
}

/// Outcome of a successful share-page download: the cache file written
/// plus the projection data the dialog surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfinityBuildsDownloadOutcome {
    pub cache_path: PathBuf,
    pub build_id: String,
    pub build_name: String,
    pub variant_names: Vec<String>,
    pub warnings: Vec<ImportWarning>,
    pub unmapped_entries: Vec<ImportUnmappedEntry>,
}

/// Headless InfinityBuilds share-page download flow: fetch the page via the
/// injected [`Downloader`](crate::actions::settings::Downloader), run the
/// upstream-equivalent parse + conversion, and cache the build under
/// `builds_dir/{id}.json`. Does **not** mutate `AppState`; the caller
/// projects the outcome to the UI.
#[cfg(feature = "real-runtime")]
pub fn download_infinity_builds_build(
    url: &str,
    builds_dir: &Path,
    repo_root: &Path,
    downloader: &dyn crate::actions::settings::Downloader,
    settings: &AppSettings,
    control: &crate::actions::build_download::ImportControl,
) -> Result<InfinityBuildsDownloadOutcome, ImportError> {
    use d4lc_assets::AssetRepository;
    use d4lc_rules::{
        export_infinity_builds_build_cache, import_infinity_builds_share_html,
        InfinityBuildsShareImport,
    };

    validate_infinity_builds_url(url).map_err(ImportError::of(ImportErrorKind::InvalidUrl))?;
    let bytes = control
        .timed("page_load", || {
            downloader.fetch(url, INFINITY_BUILDS_PAGE_MAX_BYTES)
        })
        .map_err(classify_download_error)?;
    control.check()?;
    let started = std::time::Instant::now();
    let html = String::from_utf8_lossy(&bytes).into_owned();
    let repo = AssetRepository::new(repo_root);
    let share_import: InfinityBuildsShareImport =
        import_infinity_builds_share_html(&repo, &html, url, settings)
            .map_err(ImportError::of(ImportErrorKind::Parse))?;
    if share_import.build.variants.is_empty() {
        return Err(ImportError::new(
            ImportErrorKind::InvalidResponse,
            format!("share page '{url}' contains no build variants; nothing to cache"),
        ));
    }
    control.record("convert", started.elapsed());
    let cache_path = control
        .timed("store", || {
            export_infinity_builds_build_cache(&share_import.build, builds_dir)
        })
        .map_err(ImportError::of(ImportErrorKind::Storage))?;
    Ok(InfinityBuildsDownloadOutcome {
        variant_names: share_import
            .build
            .variants
            .iter()
            .map(|variant| variant.name.clone())
            .collect(),
        cache_path,
        build_id: share_import.build.id,
        build_name: share_import.build.name,
        warnings: share_import.warnings,
        unmapped_entries: share_import.unmapped_entries,
    })
}

#[cfg(not(feature = "real-runtime"))]
pub fn download_infinity_builds_build(
    url: &str,
    _builds_dir: &Path,
    _repo_root: &Path,
    downloader: &dyn crate::actions::settings::Downloader,
    _settings: &AppSettings,
    _control: &crate::actions::build_download::ImportControl,
) -> Result<InfinityBuildsDownloadOutcome, ImportError> {
    // Validate the URL first so the share-host gate is observable in every
    // build; the capability gap is reported after validation (no
    // network call, no fake download).
    validate_infinity_builds_url(url).map_err(ImportError::of(ImportErrorKind::InvalidUrl))?;
    let _ = downloader;
    Err(ImportError::new(
        ImportErrorKind::BrowserUnavailable,
        "InfinityBuilds download requires the `real-runtime` feature \
         (asset data) and the `preset-download` feature (HTTP client); \
         rebuild with `--features real-runtime`",
    ))
}

// ---------------------------------------------------------------------------
// Maxroll planner download.
//
// Upstream `BuildsManagerMaxroll.DownloadMaxrollBuild`
// (BuildsManagerMaxroll.cs:473-491) is the one provider flow that needs no
// browser: a plain HTTP GET of `https://planners.maxroll.gg/profiles/d4/{id}`
// through the shared `HttpClientHandler`. Both JSON layers of the response —
// the `{data, date, id, name}` envelope (`MaxrollBuildJson`,
// `D4Companion.Entities/MaxrollBuildJson.cs:9-26`) and the nested planner
// JSON inside the `data` string (`MaxrollBuildDataJson`, `profiles[]` +
// `items`) — are deserialized first; anything else logs "Invalid response".
// The validated response is then stored **verbatim** as
// `.\Builds\Maxroll\{id}.json` (no re-serialization). The port mirrors each
// step; parity `exact`.
// ---------------------------------------------------------------------------

/// Upstream Maxroll id gate: the planner id is exactly 8 characters
/// (`CanAddMaxrollBuildExecute`, ImportAffixPresetViewModel.cs:737-747).
pub const MAXROLL_BUILD_ID_LEN: usize = 8;

/// Size cap of a Maxroll planner response (live responses are 11–100 KiB).
pub const MAXROLL_RESPONSE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The upstream Maxroll id gate: exactly [`MAXROLL_BUILD_ID_LEN`]
/// characters.
pub fn validate_maxroll_build_id(build_id: &str) -> Result<(), String> {
    if build_id.len() == MAXROLL_BUILD_ID_LEN {
        Ok(())
    } else {
        Err(format!(
            "Maxroll build id '{build_id}' must be exactly {MAXROLL_BUILD_ID_LEN} characters (upstream CanAddMaxrollBuildExecute)"
        ))
    }
}

/// Upstream download endpoint (BuildsManagerMaxroll.cs:477).
pub fn build_maxroll_download_url(build_id: &str) -> String {
    format!("https://planners.maxroll.gg/profiles/d4/{build_id}")
}

/// The upstream response envelope `MaxrollBuildJson`
/// (`D4Companion.Entities/MaxrollBuildJson.cs:9-26`): `data` carries the
/// nested planner JSON as a string. Unknown envelope fields (the live
/// endpoint also sends `class`) are ignored, like upstream's
/// `JsonSerializer`.
#[derive(serde::Deserialize)]
struct MaxrollProfileEnvelope {
    name: String,
    data: String,
}

/// Outcome of a successful Maxroll download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaxrollDownloadOutcome {
    pub path: PathBuf,
    pub build_id: String,
    pub build_name: String,
    pub download_url: String,
}

/// Download a Maxroll planner build: fetch the upstream endpoint through
/// the injected [`Downloader`](crate::actions::settings::Downloader),
/// validate both JSON layers like upstream, and store the raw response
/// verbatim as `{builds_dir}/{id}.json`. Does **not** mutate `AppState`;
/// the caller projects the outcome to the UI.
pub fn download_maxroll_build(
    build_id: &str,
    builds_dir: &Path,
    downloader: &dyn crate::actions::settings::Downloader,
    control: &crate::actions::build_download::ImportControl,
) -> Result<MaxrollDownloadOutcome, ImportError> {
    validate_maxroll_build_id(build_id).map_err(ImportError::of(ImportErrorKind::InvalidUrl))?;
    let download_url = build_maxroll_download_url(build_id);
    let bytes = control
        .timed("page_load", || {
            downloader.fetch(&download_url, MAXROLL_RESPONSE_MAX_BYTES)
        })
        .map_err(classify_download_error)?;
    control.check()?;
    let started = std::time::Instant::now();
    // Upstream deserializes the envelope and the nested planner JSON and
    // logs "Invalid response. uri: …" when either layer fails
    // (BuildsManagerMaxroll.cs:482-498).
    let invalid_response = |detail: String| {
        ImportError::new(
            ImportErrorKind::InvalidResponse,
            format!("Invalid response. uri: {download_url} ({detail})"),
        )
    };
    if bytes.is_empty() {
        return Err(invalid_response("empty response".to_string()));
    }
    let envelope: MaxrollProfileEnvelope =
        serde_json::from_slice(&bytes).map_err(|error| invalid_response(error.to_string()))?;
    let planner: serde_json::Value = serde_json::from_str(&envelope.data)
        .map_err(|error| invalid_response(error.to_string()))?;
    if planner.get("profiles").and_then(|p| p.as_array()).is_none() {
        return Err(invalid_response("missing profiles array".to_string()));
    }
    control.record("extract", started.elapsed());
    // Stored verbatim — upstream `File.WriteAllText(…, json)` keeps the
    // raw response bytes unmodified (BuildsManagerMaxroll.cs:490-491).
    let started = std::time::Instant::now();
    let storage = |message: String| ImportError::new(ImportErrorKind::Storage, message);
    std::fs::create_dir_all(builds_dir).map_err(|error| {
        storage(format!(
            "create builds dir {}: {error}",
            builds_dir.display()
        ))
    })?;
    let path = builds_dir.join(format!("{build_id}.json"));
    std::fs::write(&path, &bytes)
        .map_err(|error| storage(format!("write {}: {error}", path.display())))?;
    control.record("store", started.elapsed());
    Ok(MaxrollDownloadOutcome {
        path,
        build_id: build_id.to_string(),
        build_name: envelope.name,
        download_url,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::actions::build_download::ImportControl;

    #[test]
    fn http_failures_are_classified_for_the_dialog() {
        let kind = |error| classify_download_error(error).kind;
        assert_eq!(
            kind(DownloadError::Transport("timed out after 100s".to_string())),
            ImportErrorKind::Timeout
        );
        assert_eq!(
            kind(DownloadError::Transport("dns error".to_string())),
            ImportErrorKind::Network
        );
        for (status, expected) in [
            (403, ImportErrorKind::Blocked),
            (429, ImportErrorKind::Blocked),
            (404, ImportErrorKind::Network),
            (500, ImportErrorKind::Network),
        ] {
            assert_eq!(
                kind(DownloadError::HttpStatus {
                    status,
                    reason: String::new(),
                    body: Vec::new()
                }),
                expected,
                "{status}"
            );
        }
        assert_eq!(
            kind(DownloadError::UnexpectedHost("evil.example".to_string())),
            ImportErrorKind::InvalidUrl
        );
    }

    #[test]
    fn a_cancelled_maxroll_job_stops_before_parsing_and_writes_nothing() {
        let stub = FetchStub::ok(maxroll_envelope("Claw LS"));
        let dir = tempfile::TempDir::new().unwrap();
        let control = ImportControl::new();
        control.cancel();
        let error =
            download_maxroll_build("5v9nc00u", dir.path(), &stub, &control).expect_err("cancelled");
        assert_eq!(error.kind, ImportErrorKind::Cancelled);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    use crate::actions::build_import_dialogs::paths::{
        load_builds_for_provider, workspace_root_dir,
    };
    use crate::actions::build_import_dialogs::{
        PROVIDER_D2CORE, PROVIDER_D4BUILDS, PROVIDER_MAXROLL, PROVIDER_MOBALYTICS,
    };

    #[test]
    fn validate_infinity_builds_url_accepts_share_host_only() {
        assert!(validate_infinity_builds_url("https://infinitybuilds.gg/build/x").is_ok());
        assert!(validate_infinity_builds_url("https://www.infinitybuilds.gg/build/x").is_ok());
        assert!(validate_infinity_builds_url("ftp://infinitybuilds.gg/build/x").is_err());
        assert!(validate_infinity_builds_url("https://evil.example/build/x").is_err());
        assert!(validate_infinity_builds_url("not-a-url").is_err());
        for bad in [
            "http://infinitybuilds.gg/build/x",
            "https://x@sub.infinitybuilds.gg/",
            "https://evil.example\\@x.infinitybuilds.gg/b",
            "https://evil.example:1\\.infinitybuilds.gg/",
            "https://infinitybuilds.gg:8443/build/x",
            "https://evilinfinitybuilds.gg/build/x",
            "https://infinitybuilds.gg.evil.example/build/x",
        ] {
            assert!(validate_infinity_builds_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn validate_provider_build_url_gates_each_provider_host() {
        assert!(validate_provider_build_url(PROVIDER_D2CORE, "https://d2core.com/build/x").is_ok());
        assert!(validate_provider_build_url(PROVIDER_D2CORE, "https://evil.example/x").is_err());
        assert!(validate_provider_build_url(
            PROVIDER_D4BUILDS,
            "https://d4builds.gg/builds/whirlwind"
        )
        .is_ok());
        assert!(validate_provider_build_url(PROVIDER_D4BUILDS, "http://evil.example/x").is_err());
        assert!(
            validate_provider_build_url(PROVIDER_MAXROLL, "https://maxroll.gg/d4/planner/x")
                .is_ok()
        );
        assert!(validate_provider_build_url(PROVIDER_MAXROLL, "not-a-url").is_err());
        assert!(validate_provider_build_url(
            PROVIDER_MOBALYTICS,
            "https://mobalytics.gg/diablo-4/builds/x"
        )
        .is_ok());
        assert!(validate_provider_build_url(PROVIDER_MOBALYTICS, "https://d4builds.gg/x").is_err());
        assert!(validate_provider_build_url("Unknown", "https://d2core.com/x").is_err());
    }

    // -----------------------------------------------------------------------
    // Maxroll planner download (upstream BuildsManagerMaxroll parity)
    // -----------------------------------------------------------------------

    /// Minimal test double standing in for the network: records fetched
    /// URLs and answers with a canned body or error, like the
    /// `MockDownloader` of the settings preset-download tests.
    struct FetchStub {
        response: Vec<u8>,
        error: Option<DownloadError>,
        seen_urls: std::sync::Mutex<Vec<String>>,
        seen_limits: std::sync::Mutex<Vec<u64>>,
    }

    impl FetchStub {
        fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self {
                response: body.into(),
                error: None,
                seen_urls: std::sync::Mutex::new(Vec::new()),
                seen_limits: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn err(error: DownloadError) -> Self {
            Self {
                response: Vec::new(),
                error: Some(error),
                seen_urls: std::sync::Mutex::new(Vec::new()),
                seen_limits: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl crate::actions::settings::Downloader for FetchStub {
        fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
            self.seen_urls.lock().unwrap().push(url.to_string());
            self.seen_limits.lock().unwrap().push(max_bytes);
            match &self.error {
                Some(error) => Err(error.clone()),
                None => Ok(self.response.clone()),
            }
        }
    }

    /// A valid upstream envelope: `{data, date, id, name}` with the nested
    /// planner JSON inside `data` (shape per the live endpoint and
    /// `MaxrollBuildJson.cs`; the fixture files carry the same layout).
    fn maxroll_envelope(name: &str) -> String {
        let planner = serde_json::json!({
            "profiles": [{"name": "Variant 1", "items": {}}],
            "items": {}
        });
        format!(
            r#"{{"id":"5v9nc00u","date":"2026-05-07 16:10:40","name":"{name}","class":"Druid","data":{}}}"#,
            serde_json::to_string(&planner.to_string()).unwrap()
        )
    }

    #[test]
    fn provider_downloader_policy_allows_only_https_on_the_provider_domain() {
        let policy = ProviderDownloader::for_provider(PROVIDER_MAXROLL)
            .expect("maxroll downloader")
            .policy;
        policy
            .check("https://planners.maxroll.gg/profiles/d4/5v9nc00u")
            .expect("provider subdomain over https");
        // Plain http is refused (it was accepted before redirects were
        // policy-checked; every provider endpoint is https).
        assert!(matches!(
            policy.check("http://maxroll.gg/x"),
            Err(DownloadError::InvalidUrl(_))
        ));
        assert!(policy
            .check("https://evil.example/x")
            .unwrap_err()
            .to_string()
            .contains("unexpected download host"));
        assert!(policy.check("https://x@planners.maxroll.gg/x").is_err());
        assert!(policy.check("ftp://maxroll.gg/x").is_err());
        assert!(policy.check("not-a-url").is_err());
    }

    #[test]
    fn validate_maxroll_build_id_enforces_the_upstream_eight_character_gate() {
        // Upstream CanAddMaxrollBuildExecute accepts exactly 8 characters.
        assert!(validate_maxroll_build_id("5v9nc00u").is_ok());
        assert!(validate_maxroll_build_id("short").is_err());
        assert!(validate_maxroll_build_id("").is_err());
        assert!(validate_maxroll_build_id("9chars-id").is_err());
    }

    #[test]
    fn build_maxroll_download_url_matches_upstream_endpoint() {
        // BuildsManagerMaxroll.cs:477.
        assert_eq!(
            build_maxroll_download_url("5v9nc00u"),
            "https://planners.maxroll.gg/profiles/d4/5v9nc00u"
        );
    }

    #[test]
    fn download_maxroll_build_saves_the_response_verbatim_under_the_builds_dir() {
        let body = maxroll_envelope("Claw LS");
        let stub = FetchStub::ok(body.clone());
        let dir = std::env::temp_dir().join("d4lc-maxroll-download-ok-test");
        let _ = std::fs::remove_dir_all(&dir);

        let outcome = download_maxroll_build("5v9nc00u", &dir, &stub, &ImportControl::new())
            .expect("download");

        assert_eq!(
            stub.seen_urls.lock().unwrap().as_slice(),
            ["https://planners.maxroll.gg/profiles/d4/5v9nc00u"],
            "must fetch the exact upstream endpoint"
        );
        assert_eq!(
            stub.seen_limits.lock().unwrap().as_slice(),
            [MAXROLL_RESPONSE_MAX_BYTES]
        );
        assert_eq!(outcome.build_id, "5v9nc00u");
        assert_eq!(outcome.build_name, "Claw LS");
        assert_eq!(
            outcome.path,
            dir.join("5v9nc00u.json"),
            "file name must be the build id like upstream .\\Builds\\Maxroll\\{{id}}.json"
        );
        // Verbatim: the stored bytes equal the raw response, unmodified.
        let stored = std::fs::read(&outcome.path).expect("stored file");
        assert_eq!(stored, body.as_bytes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_maxroll_build_rejects_invalid_responses_without_writing() {
        // Upstream validates both JSON layers and logs "Invalid response"
        // before any file write (BuildsManagerMaxroll.cs:482-498).
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty response", Vec::new()),
            ("not json", b"<html>nope</html>".to_vec()),
            (
                "data not nested json",
                br#"{"id":"x","name":"x","date":"x","data":"nope"}"#.to_vec(),
            ),
            (
                "missing profiles array",
                br#"{"id":"x","name":"x","date":"x","data":"{\"items\":{}}"}"#.to_vec(),
            ),
        ];
        for (label, body) in cases {
            let stub = FetchStub::ok(body);
            let dir = std::env::temp_dir().join("d4lc-maxroll-download-invalid-test");
            let _ = std::fs::remove_dir_all(&dir);
            let error = download_maxroll_build("5v9nc00u", &dir, &stub, &ImportControl::new())
                .expect_err("invalid response must fail");
            assert!(
                error.message.contains("Invalid response"),
                "{label}: {error}"
            );
            assert_eq!(error.kind, ImportErrorKind::InvalidResponse, "{label}");
            assert!(
                !dir.join("5v9nc00u.json").exists(),
                "{label}: nothing may be written for a rejected response"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    #[cfg(not(feature = "real-runtime"))]
    fn download_infinity_builds_build_reports_the_capability_gap_without_fetching() {
        let stub = FetchStub::ok(b"<html></html>".to_vec());
        let dir = tempfile::TempDir::new().unwrap();
        let args = |url| {
            download_infinity_builds_build(
                url,
                dir.path(),
                &workspace_root_dir(),
                &stub,
                &AppSettings::default(),
                &ImportControl::new(),
            )
        };
        let error = args("https://infinitybuilds.gg/en/builds/x").expect_err("feature gap");
        assert!(error.message.contains("--features real-runtime"), "{error}");
        assert!(
            args("https://example.com/x").is_err(),
            "the host gate runs first"
        );
        assert!(stub.seen_urls.lock().unwrap().is_empty(), "no network call");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    #[cfg(feature = "real-runtime")]
    fn download_infinity_builds_build_fetches_with_the_page_cap() {
        let stub = FetchStub::ok(b"<html></html>".to_vec());
        let dir = tempfile::TempDir::new().unwrap();
        let url = "https://infinitybuilds.gg/en/builds/x";
        let result = download_infinity_builds_build(
            url,
            dir.path(),
            &workspace_root_dir(),
            &stub,
            &AppSettings::default(),
            &ImportControl::new(),
        );
        assert!(result.is_err(), "an empty page holds no build");
        assert_eq!(
            stub.seen_limits.lock().unwrap().as_slice(),
            [INFINITY_BUILDS_PAGE_MAX_BYTES]
        );
    }

    #[test]
    fn download_maxroll_build_propagates_transport_and_http_errors() {
        let dir = std::env::temp_dir().join("d4lc-maxroll-download-error-test");
        let transport = FetchStub::err(DownloadError::Transport("dns".to_string()));
        assert!(
            download_maxroll_build("5v9nc00u", &dir, &transport, &ImportControl::new()).is_err()
        );
        let http = FetchStub::err(DownloadError::HttpStatus {
            status: 404,
            reason: "Not Found".to_string(),
            body: Vec::new(),
        });
        assert!(download_maxroll_build("5v9nc00u", &dir, &http, &ImportControl::new()).is_err());
    }

    #[test]
    #[ignore = "manual smoke: requires real network and a writable temp dir"]
    fn maxroll_live_download_smoke() {
        // Opt-in end-to-end check against the live upstream endpoint
        // (plain HTTP, no browser — upstream parity `exact`). Run with:
        // cargo test -p d4lc-app maxroll_live_download_smoke -- --ignored
        let dir = std::env::temp_dir().join("d4lc-maxroll-live-smoke");
        let _ = std::fs::remove_dir_all(&dir);
        let downloader = ProviderDownloader::for_provider(PROVIDER_MAXROLL)
            .expect("maxroll provider downloader");
        let outcome = download_maxroll_build("5v9nc00u", &dir, &downloader, &ImportControl::new())
            .expect("live maxroll download");
        assert!(outcome.path.is_file());
        assert!(!outcome.build_name.is_empty());
        // The dialog refresh reads the stored file back through the same
        // build loader the import path uses — it must enumerate the
        // downloaded build's variants.
        let builds =
            load_builds_for_provider(PROVIDER_MAXROLL, &dir).expect("enumerate downloaded build");
        assert!(
            !builds.is_empty(),
            "the downloaded planner build must yield an importable build row"
        );
        eprintln!(
            "[smoke] maxroll build '{}' stored at {} ({} variant row(s))",
            outcome.build_name,
            outcome.path.display(),
            builds[0].variant_names.len()
        );
    }

    // The share-page parse exists only with `real-runtime`; the default
    // build reports the capability gap, which the gate test above covers.
    #[test]
    #[cfg(feature = "real-runtime")]
    #[ignore = "manual smoke: requires real network and a writable temp dir"]
    fn infinity_builds_live_download_smoke() {
        // Opt-in end-to-end check against a live share page (plain HTTP
        // of the share HTML — upstream parity `equivalent`). The URL is
        // upstream's own watermark example. Run with:
        // cargo test -p d4lc-app --bin d4lc-app --features real-runtime \
        //   infinity_builds_live_download_smoke -- --ignored
        let url = "https://infinitybuilds.gg/en/builds/pestilent-swarmskuna-spiritborn-vAB2c3aMt2";
        let dir = std::env::temp_dir().join("d4lc-infinity-builds-live-smoke");
        let _ = std::fs::remove_dir_all(&dir);
        let downloader = ProviderDownloader::for_provider(PROVIDER_INFINITY_BUILDS)
            .expect("infinity builds provider downloader");
        let repo_root = workspace_root_dir();
        let settings = AppSettings::default();
        let outcome = download_infinity_builds_build(
            url,
            &dir,
            &repo_root,
            &downloader,
            &settings,
            &ImportControl::new(),
        )
        .expect("live infinity builds download");
        assert!(outcome.cache_path.is_file());
        assert!(!outcome.build_name.is_empty());
        let builds = load_builds_for_provider(PROVIDER_INFINITY_BUILDS, &dir)
            .expect("enumerate downloaded build");
        assert!(
            !builds.is_empty(),
            "the downloaded share build must yield an importable build row"
        );
        eprintln!(
            "[smoke] infinity builds build '{}' stored at {} ({} variant row(s))",
            outcome.build_name,
            outcome.cache_path.display(),
            builds[0].variant_names.len()
        );
    }
}
