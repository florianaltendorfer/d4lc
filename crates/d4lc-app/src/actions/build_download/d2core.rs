//! D2Core: complete download flow.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use d4lc_rules::ensure_cache_filename_safe;

use super::{bot_protection_failure, BrowserSession, ImportError, ImportErrorKind};

/// Upstream inactivity timeout (`BuildsManagerD2Core.cs:52-53`).
pub const D2CORE_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(10);

/// Upstream id gate length
/// (`ImportAffixPresetViewModel.cs:640-655`): the `bd=` query value,
/// exactly 4 characters.
pub const D2CORE_BUILD_ID_LEN: usize = 4;

/// The response-URL substring upstream sniffs for
/// (`BuildsManagerD2Core.cs:173`).
pub const D2CORE_SNIFF_URL_FILTER: &str = "web?env=diablocore";

/// The exact body substring upstream accepts
/// (`BuildsManagerD2Core.cs:203`): the planner RPC response carrying
/// this build's `response_data`. Every other JSON body (e.g. the
/// `function-user-querycurrentuserinfo` response) is ignored.
fn d2core_envelope_needle(build_id: &str) -> String {
    format!("{{\"response_data\":\"{{\\\"data\\\":{{\\\"_id\\\":\\\"{build_id}\\\"")
}

/// The upstream D2Core id gate: exactly [`D2CORE_BUILD_ID_LEN`]
/// characters.
pub fn validate_d2core_build_id(build_id: &str) -> Result<(), String> {
    if build_id.len() == D2CORE_BUILD_ID_LEN {
        Ok(())
    } else {
        Err(format!(
            "D2Core build id '{build_id}' must be exactly {D2CORE_BUILD_ID_LEN} characters \
             (upstream CanAddD2CoreBuildExecute)"
        ))
    }
}

/// The page upstream navigates (`BuildsManagerD2Core.cs:532`).
pub fn build_d2core_planner_url(build_id: &str) -> String {
    format!("https://www.d2core.com/d4/planner?bd={build_id}")
}

/// Outcome of a successful D2Core download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D2CoreDownloadOutcome {
    pub path: PathBuf,
    pub build_id: String,
    /// `data.title` of the planner JSON when it carries one.
    pub build_name: Option<String>,
}

/// Download a D2Core planner build through the injected browser session:
/// navigate the planner page, sniff the JSON traffic, accept only the
/// `response_data` envelope of this build, and store the inner
/// `response_data` string verbatim as `{builds_dir}/{id}.json`
/// (`BuildsManagerD2Core.cs:521-536`). The capture ends the moment that
/// envelope arrived. The stored shape is exactly what
/// `d4lc_rules::import_d2core_build_variant` parses. Nothing is written
/// for a rejected response. Does **not** mutate `AppState`; the caller
/// projects the outcome to the UI.
pub fn download_d2core_build(
    build_id: &str,
    builds_dir: &Path,
    session: &dyn BrowserSession,
) -> Result<D2CoreDownloadOutcome, ImportError> {
    validate_d2core_build_id(build_id).map_err(ImportError::of(ImportErrorKind::InvalidUrl))?;
    ensure_cache_filename_safe(build_id).map_err(ImportError::of(ImportErrorKind::InvalidUrl))?;
    let planner_url = build_d2core_planner_url(build_id);
    // Upstream accepts only the one response whose raw body carries this
    // build's envelope; everything else is ignored before any parse or
    // write (`BuildsManagerD2Core.cs:203`).
    let needle = d2core_envelope_needle(build_id);
    let accept = |body: &str| body.contains(&needle);
    let responses = session
        .navigate_collecting_json(
            &planner_url,
            D2CORE_SNIFF_URL_FILTER,
            D2CORE_INACTIVITY_TIMEOUT,
            Some(&accept),
        )
        .map_err(|error| {
            ImportError::from(error).context(format!("D2Core planner download for '{build_id}'"))
        })?;
    let started = Instant::now();
    let matched = responses
        .iter()
        .find(|response| accept(&response.body))
        .ok_or_else(|| {
            match bot_protection_failure(session, "D2Core", &planner_url, "Try again later.") {
                Some(message) => ImportError::new(ImportErrorKind::Blocked, message),
                None => ImportError::new(
                    ImportErrorKind::Timeout,
                    format!(
                        "D2Core sent no planner data for build '{build_id}' before the page went \
                         quiet ({} matching JSON response(s) without the expected \
                         response_data envelope). The planner page requests the build only after \
                         it finished loading; when one of its resources hangs it never does. Try \
                         again.",
                        responses.len()
                    ),
                ),
            }
        })?;
    let envelope: serde_json::Value = serde_json::from_str(&matched.body).map_err(|error| {
        ImportError::new(
            ImportErrorKind::InvalidResponse,
            format!("planner response was not valid JSON: {error}"),
        )
    })?;
    // Upstream wraps the envelope as `D2CoreBuildJson { requestId, data }`
    // with the planner JSON string at `data.ResponseData`
    // (`D4Companion.Entities/D2CoreBuildJson.cs:9-22`,
    // `BuildsManagerD2Core.cs:206-214`).
    let response_data = envelope
        .get("data")
        .and_then(|data| data.get("response_data"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorKind::InvalidResponse,
                format!(
                    "planner response carried no data.response_data string (body starts with: {})",
                    matched.body.chars().take(200).collect::<String>()
                ),
            )
        })?;
    // Build name: `data.title` when the planner JSON carries it. A
    // `response_data` that is not JSON is rejected before anything is
    // stored: the build list could not read it back.
    let planner: serde_json::Value = serde_json::from_str(response_data).map_err(|error| {
        ImportError::new(
            ImportErrorKind::InvalidResponse,
            format!("planner response_data was not valid JSON: {error}"),
        )
    })?;
    let build_name = planner
        .get("data")
        .and_then(|data| data.get("title"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    session.record_phase("extract", started.elapsed());
    // Stored verbatim — the raw CDP JSON envelope is discarded and the
    // inner `response_data` string is written unmodified
    // (`BuildsManagerD2Core.cs:213-214`).
    let started = Instant::now();
    std::fs::create_dir_all(builds_dir).map_err(|error| {
        ImportError::new(
            ImportErrorKind::Storage,
            format!("create builds dir {}: {error}", builds_dir.display()),
        )
    })?;
    let path = builds_dir.join(format!("{build_id}.json"));
    std::fs::write(&path, response_data).map_err(|error| {
        ImportError::new(
            ImportErrorKind::Storage,
            format!("write {}: {error}", path.display()),
        )
    })?;
    session.record_phase("store", started.elapsed());
    Ok(D2CoreDownloadOutcome {
        path,
        build_id: build_id.to_string(),
        build_name,
    })
}
