//! Runtime preparation for d4lc-app.
//!
//! Bridges app config/settings → window discovery → template loading →
//! OCR worker creation → PreparedRuntime + RuntimeSession construction.
//!
//! Each step is a separate function so the app can handle errors before
//! starting the live loop.  No Slint types, no GUI state, no fake data.
//!
//! ## Architecture
//!
//! ```text
//! config_root (Path)
//!   └─ resolve_runtime_config() → RuntimeConfig
//!   ├─ build_prepared_detector() → PreparedDetector
//!   │   └─ AssetRepository::load_preset_templates()
//!   │   └─ load_template_image() (PNG → GrayImage)
//!   ├─ resolve_capture_source()  → CaptureReference
//!   │   └─ d4lc_capture::resolve_capture_reference()
//!   └─ build_ocr_worker()        → OcrWorker (or error)
//!       ├─ resolve_game_data_root() → repo/bundle root carrying assets/
//!       └─ OcrEngineRegistry with LeptessOcrEngine
//!
//! RuntimeConfig + PreparedDetector + CaptureReference + OcrWorker
//!   └─ PreparedRuntime::new()
//!   └─ RuntimeSession::new(Some(worker), ocr_interval)
//! ```
//!
//! ## Invariants
//!
//! - No Slint imports, no GUI state.
//! - No fake/placeholder data — if preparation cannot complete, return Err.
//! - OCR worker is required when the leptess-backend feature is enabled.
//!   Without leptess-backend, preparation fails with `OcrBackendNotAvailable`.
//! - Template loading reads from disk (needs `image` crate).
//! - Window discovery uses d4lc-capture (needs X11 client libraries at link time).

use std::path::Path;
use std::time::Duration;

use d4lc_capture::{resolve_capture_reference, CaptureBackend, CaptureReference, WindowQuery};
use d4lc_config::load_app_settings;
use d4lc_core::{GrayImage, InputMode, Rect};
use d4lc_detect::prepared::{PreparedDetector, ThresholdSettings};
use d4lc_runtime::{PreparedRuntime, RuntimeSession};

// The display-backend selection seam lives in the
// always-compiled `crate::display_backend` module so the settings
// persistence path can share it; this module applies it on the
// runtime-prep path, including the once-per-start `auto` detection.
use crate::display_backend::{
    resolve_display_backend, resolve_display_backend_for_session, CaptureEngineResolution,
    DisplayBackend, DisplayBackendStatus, SessionEnvironment, X11GameWindowProbe,
    WAYLAND_CAPTURE_BUILD,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Structured preparation error returned by `prepare_app_runtime`.
/// `WaylandCaptureFailed` and `PortalSessionNotAllowed` are only constructed
/// by the `wayland-portal` capture path, so they are dead code in a build
/// without that feature.
#[cfg_attr(not(feature = "wayland-portal"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRuntimePrepareError {
    /// Could not load or parse app settings from disk.
    ConfigLoadFailed(String),
    /// The selected display backend has no implementation in this build.
    /// Structured and user-visible by design: selecting Wayland must
    /// report unavailability, never fall back to X11 silently.
    DisplayBackendUnavailable(String),
    /// The Wayland ScreenCast capture session could not be established
    /// (portal unreachable, dialog cancelled, PipeWire setup failed).
    /// The reason names the missing piece; there is no silent X11
    /// fallback.
    WaylandCaptureFailed(String),
    /// No tooltip templates found for the selected preset/mode.
    TemplateLoadFailed(String),
    /// No X11 window or manual capture region could be resolved.
    CaptureSourceFailed(String),
    /// The OCR backend is not available (leptess-backend feature not enabled).
    #[cfg(not(feature = "leptess-backend"))]
    OcrBackendNotAvailable,
    /// A prerequisite of a canonical evaluation is missing (catalog data,
    /// OCR language data): the runtime must not claim to evaluate items
    NotReadyForEvaluation(String),
    /// A stored backend choice this build or session cannot honor.
    UnsupportedBackend(String),
    /// The resolved engine is the Wayland ScreenCast portal, but this
    /// preparation must not open a portal session (it runs beside a live
    /// loop that owns the process's exclusive session). The reason names
    /// the caller.
    PortalSessionNotAllowed(String),
}

impl std::fmt::Display for AppRuntimePrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigLoadFailed(msg) => write!(f, "config load failed: {msg}"),
            Self::DisplayBackendUnavailable(msg) => {
                write!(f, "display backend unavailable: {msg}")
            }
            Self::WaylandCaptureFailed(msg) => write!(f, "wayland capture failed: {msg}"),
            Self::TemplateLoadFailed(msg) => write!(f, "template load failed: {msg}"),
            Self::CaptureSourceFailed(msg) => write!(f, "capture source failed: {msg}"),
            #[cfg(not(feature = "leptess-backend"))]
            Self::OcrBackendNotAvailable => {
                write!(
                    f,
                    "OCR backend not available; rebuild with --features leptess-backend and \
                     install libtesseract-dev/libleptonica-dev"
                )
            }
            Self::NotReadyForEvaluation(msg) => write!(f, "not ready for evaluation: {msg}"),
            Self::UnsupportedBackend(msg) => write!(f, "unsupported backend: {msg}"),
            Self::PortalSessionNotAllowed(msg) => write!(
                f,
                "the capture engine resolved to the Wayland portal, which this preparation must not open: {msg}"
            ),
        }
    }
}

impl From<String> for AppRuntimePrepareError {
    fn from(msg: String) -> Self {
        Self::ConfigLoadFailed(msg)
    }
}

/// A preparation step that can take user-visible time. Preparation reports
/// it before the step starts so the live loop can say what it waits on
/// instead of claiming a running pipeline with no frame yet. Every variant
/// is emitted by the `wayland-portal` capture path, so they are dead code in
/// a build without that feature.
#[cfg_attr(not(feature = "wayland-portal"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareProgress {
    /// The ScreenCast portal was asked to start a capture session. With a
    /// stored restore token the compositor may restore the previous
    /// selection silently; otherwise (or when the restore fails) it shows
    /// its screen-share selection dialog and preparation blocks until the
    /// user picks a source or cancels.
    AwaitingScreenCastSelection {
        /// Whether a stored restore token was offered to the portal.
        restore_token_offered: bool,
    },
    /// The portal session is established; preparation waits (bounded) for
    /// the stream's first frame.
    AwaitingFirstStreamFrame,
    /// A settings re-preparation keeps the running ScreenCast session: no
    /// portal request, no dialog, no restore-token rotation.
    ReusingScreenCastSession,
}

impl PrepareProgress {
    /// User-facing description of what preparation is waiting on.
    pub fn describe(&self) -> String {
        match self {
            Self::AwaitingScreenCastSelection {
                restore_token_offered: true,
            } => "waiting for the compositor to restore the stored screen-share selection; if it \
                  cannot, it shows its selection dialog, possibly on another monitor or behind \
                  the fullscreen game — pick the monitor Diablo IV runs on (or its window)"
                .to_string(),
            Self::AwaitingScreenCastSelection {
                restore_token_offered: false,
            } => "waiting for the screen-share selection in the compositor dialog, possibly on \
                  another monitor or behind the fullscreen game — pick the monitor Diablo IV runs \
                  on (or its window)"
                .to_string(),
            Self::AwaitingFirstStreamFrame => {
                "screen share established; waiting for the first stream frame".to_string()
            }
            Self::ReusingScreenCastSession => {
                "keeping the running screen share (no new portal session)".to_string()
            }
        }
    }
}

/// The capture a running runtime already holds, carried into its settings
/// re-preparation.
///
/// ScreenCast portal sessions are exclusive per process, and a new one
/// would consume the single-use restore token or show the compositor
/// dialog again. A re-preparation that resolves to the Wayland engine
/// therefore keeps the running session and its capture reference; one
/// that resolves to another engine ignores this value and prepares that
/// engine normally. Every other source carries nothing: the X11 paths
/// re-resolve their target on each preparation.
#[derive(Default)]
pub struct ReusableCapture {
    #[cfg(feature = "wayland-portal")]
    wayland: Option<(
        std::sync::Arc<d4lc_capture::WaylandFrameSource>,
        CaptureReference,
    )>,
}

impl ReusableCapture {
    /// Nothing to reuse: a fresh start.
    pub fn none() -> Self {
        Self::default()
    }

    /// What `runtime` can hand to its re-preparation.
    pub fn from_runtime(runtime: &PreparedRuntime) -> Self {
        #[cfg(feature = "wayland-portal")]
        if let Some(source) = runtime.shared_wayland_capture_source() {
            // The stream keeps running across the reload, so its current
            // frame size is the geometry a fresh preparation would read
            // (a game resolution change since the first preparation). A
            // poisoned frame slot keeps the running reference: the loop's
            // next capture reports that slot as a fatal stream error.
            let latest = source.latest_frame_size().ok().flatten();
            let reference = reused_stream_reference(runtime.capture_reference(), latest);
            return Self {
                wayland: Some((source, reference)),
            };
        }
        #[cfg(not(feature = "wayland-portal"))]
        let _ = runtime;
        Self::none()
    }

    /// Whether this value carries a running capture session.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        #[cfg(feature = "wayland-portal")]
        if self.wayland.is_some() {
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Display-backend selection seam — applied on the runtime-prep path
// ---------------------------------------------------------------------------

// The `DisplayBackend` / `DisplayBackendStatus` types and the single
// `resolve_display_backend` resolution point live in
// `crate::display_backend` (imported above); this module is where the
// resolution is applied to runtime preparation.

// ---------------------------------------------------------------------------
// RuntimeConfig — resolved from app settings on disk
// ---------------------------------------------------------------------------

/// Resolved runtime configuration, extracted from app settings on disk.
///
/// Each field has been resolved through `d4lc-config` defaults and any
/// optional overrides the caller supplies.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeConfig {
    /// Selected system preset name (e.g. "1440p_SMF").
    pub preset: String,
    /// Selected affix language (e.g. "enUS").
    pub language: String,
    /// Detection mode: Mouse or Controller.
    pub mode: InputMode,
    /// Scan height percentage for mouse ROI planning.
    pub scan_height_percent: f64,
    /// Tooltip width from the settings (upstream reads
    /// `Settings.TooltipWidth`, seeded from the system preset,
    /// ScreenProcessHandler.cs:565,670).
    pub tooltip_width: i32,
    /// Threshold settings (min, max, similarity_tooltip).
    pub thresholds: ThresholdSettings,
    /// Controller image filenames (empty list = auto-detect).
    pub controller_config: Vec<String>,
    /// Capture backend to use.
    pub capture_backend: CaptureBackend,
    /// Selected display backend, resolved through the
    /// [`resolve_display_backend`] seam.
    pub display_backend: DisplayBackend,
    /// Status of the selected display backend. For explicit
    /// settings this is final at the parse level; for `auto` it starts
    /// as [`DisplayBackendStatus::Auto`] and is replaced by the
    /// once-per-runtime-start session resolution in
    /// `prepare_app_runtime`.
    pub display_backend_status: DisplayBackendStatus,
    /// Stored single-use ScreenCast restore token from the last
    /// successful Wayland session, if any.
    pub wayland_restore_token: Option<String>,
    /// Minimum interval between OCR submissions for the same tooltip.
    pub ocr_interval: Duration,
    /// Persisted `Settings.IsDebugInfoEnabled` at runtime start (debug UI
    /// A): the initial value of the OCR worker's live debug-bundle
    /// gate. A toggle while the runtime runs reaches the worker through the
    /// rules snapshot, not through a reload.
    pub is_debug_info_enabled: bool,
}

// ---------------------------------------------------------------------------
// Step 1: Load and resolve runtime configuration
// ---------------------------------------------------------------------------

/// Load `AppSettings` from disk and resolve into a `RuntimeConfig`.
///
/// # Errors
///
/// Returns `AppRuntimePrepareError::ConfigLoadFailed` if the config root
/// does not exist or if `d4lc_config::load_app_settings()` fails.
pub fn resolve_runtime_config(
    config_root: &Path,
    preset_override: Option<String>,
    language_override: Option<String>,
    mode_override: Option<InputMode>,
) -> Result<RuntimeConfig, AppRuntimePrepareError> {
    // Real-runtime requires an existing config root directory.
    // load_app_settings() is intentionally permissive (returns defaults for
    // missing files), but resolve_runtime_config() is the real-runtime boundary
    // and must reject nonexistent or non-directory paths.
    if !config_root.is_dir() {
        return Err(AppRuntimePrepareError::ConfigLoadFailed(format!(
            "config root does not exist or is not a directory: {}",
            config_root.display()
        )));
    }

    let settings =
        load_app_settings(config_root).map_err(AppRuntimePrepareError::ConfigLoadFailed)?;

    let preset = preset_override.unwrap_or(settings.selected_system_preset);
    let language = language_override.unwrap_or(settings.selected_affix_language);
    let mode = match mode_override {
        Some(m) => m,
        None if settings.controller_mode => InputMode::Controller,
        None => InputMode::Mouse,
    };

    // Display-backend selection seam: the parse-level
    // resolution of the persisted `DisplayBackend` setting. An invalid
    // stored value is rejected here at the parse point with the
    // structured error; `auto` resolves to the undetected
    // [`DisplayBackendStatus::Auto`] — the real detection runs once per
    // runtime start in [`prepare_app_runtime`], never on this settings
    // path (it must not probe).
    let (display_backend, display_backend_status) =
        resolve_display_backend(&settings.display_backend)
            .map_err(AppRuntimePrepareError::ConfigLoadFailed)?;

    // Controller image selection: the active controller
    // images come from `Config/Controllers.json`, the same file the
    // ControllerConfig dialog writes and the CLI/detect paths already
    // read. An unreadable file is a hard preparation error — a
    // controller-mode run must not silently start from an empty template
    // set.
    let controller_config = d4lc_config::load_controller_config(config_root)
        .map_err(AppRuntimePrepareError::ConfigLoadFailed)?;

    // An explicit capture choice is never silently replaced by `Auto`, and a
    // choice this session cannot use is refused by name.
    let capture_backend = resolve_capture_backend(&settings.capture_backend, display_backend)?;

    Ok(RuntimeConfig {
        preset,
        language,
        mode,
        scan_height_percent: settings.scan_height_percent,
        // Upstream reads `Settings.TooltipWidth` (seeded from the preset's
        // config on a preset change, SettingsViewModel.cs:272); the settings
        // field is the live value the detector and the OCR preparation use.
        tooltip_width: i32::try_from(settings.tooltip_width).unwrap_or(i32::MAX),
        thresholds: ThresholdSettings {
            min: settings.threshold_min,
            max: settings.threshold_max,
            similarity_tooltip: settings.threshold_similarity_tooltip,
        },
        controller_config,
        capture_backend,
        display_backend,
        display_backend_status,
        wayland_restore_token: settings.wayland_restore_token.clone(),
        ocr_interval: Duration::from_millis(250),
        is_debug_info_enabled: settings.is_debug_info_enabled,
    })
}

/// The structured refusal of an explicit X11 capture choice on the
/// Wayland portal path: refused by name, never silently replaced.
fn x11_capture_backend_on_wayland_error(backend: CaptureBackend) -> AppRuntimePrepareError {
    AppRuntimePrepareError::UnsupportedBackend(format!(
        "capture backend '{backend}' is an X11 capture path; the Wayland display backend \
         captures through the ScreenCast portal only (select 'auto')"
    ))
}

/// The stored capture backend for the selected display backend.
///
/// The X11 capture backends (`auto`, `x11-native`, `x11-command`) apply to
/// the X11 capture path. The Wayland path captures through the ScreenCast
/// portal only, so an explicit X11 capture choice cannot be honored there
/// and is reported instead of ignored. The same gate runs on the RESOLVED
/// engine in `prepare_app_runtime`: `auto` can resolve to the portal
/// engine after the parse level.
fn resolve_capture_backend(
    stored: &str,
    display_backend: DisplayBackend,
) -> Result<CaptureBackend, AppRuntimePrepareError> {
    let stored = stored.trim();
    let backend = CaptureBackend::parse(if stored.is_empty() { "auto" } else { stored })
        .map_err(|error| AppRuntimePrepareError::UnsupportedBackend(format!("capture: {error}")))?;
    if matches!(display_backend, DisplayBackend::Wayland) && backend != CaptureBackend::Auto {
        return Err(x11_capture_backend_on_wayland_error(backend));
    }
    Ok(backend)
}

/// The capture-backend gate on the RESOLVED engine: an explicit
/// X11 capture choice cannot be honored when the display backend
/// resolves to the Wayland portal engine — including through `auto`.
/// Both call sites are behind the `wayland-portal` feature (the portal
/// engine cannot be resolved without it); the pure gate itself stays
/// testable in every build.
#[cfg_attr(not(feature = "wayland-portal"), allow(dead_code))]
fn resolved_capture_backend_gate(config: &RuntimeConfig) -> Result<(), AppRuntimePrepareError> {
    if config.display_backend_status.is_wayland_engine()
        && config.capture_backend != CaptureBackend::Auto
    {
        return Err(x11_capture_backend_on_wayland_error(config.capture_backend));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2: Build a PreparedDetector by loading templates from disk
// ---------------------------------------------------------------------------

/// Load preset templates from disk and construct a `PreparedDetector`.
///
/// # Errors
///
/// Returns `AppRuntimePrepareError::TemplateLoadFailed` if the preset does not
/// exist, templates cannot be loaded, or the matcher backend cannot be resolved.
pub fn build_prepared_detector(
    config_root: &Path,
    config: &RuntimeConfig,
) -> Result<PreparedDetector, AppRuntimePrepareError> {
    use d4lc_assets::{AssetRepository, TemplateKind};

    let preset_root = preset_root(config_root, &config.preset);
    let repo = AssetRepository::new(&preset_root);
    let loaded = repo
        .load_preset_templates(&config.preset)
        .map_err(AppRuntimePrepareError::TemplateLoadFailed)?;

    // Select template names matching the active mode.
    let template_names: Vec<String> = match config.mode {
        InputMode::Mouse => {
            let all_names = repo
                .controller_image_names(&config.preset, &config.language)
                .map_err(AppRuntimePrepareError::TemplateLoadFailed)?;
            all_names
                .into_iter()
                .filter(|name| name.starts_with("tooltip_kb_"))
                .collect()
        }
        InputMode::Controller => repo
            .active_controller_image_names(
                &config.preset,
                &config.language,
                &config.controller_config,
            )
            .map_err(AppRuntimePrepareError::TemplateLoadFailed)?,
    };

    if template_names.is_empty() {
        return Err(AppRuntimePrepareError::TemplateLoadFailed(format!(
            "no tooltip templates selected for preset '{}' mode {:?}",
            config.preset, config.mode
        )));
    }

    // Load each selected template from disk.
    let mut templates = Vec::new();
    for template_asset in &loaded.templates {
        if template_asset.kind != TemplateKind::Tooltip {
            continue;
        }
        if !template_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&template_asset.file_name))
        {
            continue;
        }
        let image = load_template_image(&preset_root.join(&template_asset.relative_path))
            .map_err(AppRuntimePrepareError::TemplateLoadFailed)?;
        templates.push((template_asset.file_name.clone(), image));
    }

    if templates.is_empty() {
        return Err(AppRuntimePrepareError::TemplateLoadFailed(format!(
            "loaded preset '{}' has no tooltip template images matching mode {:?}",
            config.preset, config.mode
        )));
    }

    Ok(PreparedDetector {
        preset: config.preset.clone(),
        mode: config.mode,
        scan_height_percent: config.scan_height_percent,
        thresholds: config.thresholds,
        // Upstream FindTooltip bounds the tooltip crop to
        // `Settings.TooltipWidth` (ScreenProcessHandler.cs:670); the settings
        // value (preset-seeded) is authoritative, not the preset config.
        tooltip_width: config.tooltip_width,
        templates,
        scan: d4lc_detect::prepared::DetectScan::default(),
    })
}

/// The root the detector reads `preset` from, with the precedence of the
/// OCR preparation (`PreparedTooltipOcr::prepare_with_game_data_root`): the
/// config root when it holds the preset (extracted `Images/{preset}` or a
/// vendored tree), else the game-data root beside the application. Without
/// the fallback a fresh install could select a bundled preset whose OCR
/// templates load while the detector reports the preset as missing.
fn preset_root(config_root: &Path, preset: &str) -> std::path::PathBuf {
    let in_config_root = [
        d4lc_assets::EXTRACTED_PRESETS_DIR,
        d4lc_assets::SYSTEM_PRESETS_DIR,
    ]
    .iter()
    .any(|dir| config_root.join(dir).join(preset).is_dir());
    if in_config_root {
        config_root.to_path_buf()
    } else {
        resolve_game_data_root(config_root)
    }
}

/// Load a grayscale template PNG through the size-limited
/// [`d4lc_runtime::load_template_gray_image`] (OpenCV's grayscale
/// conversion, upstream `new Image<Gray, byte>(fileName)`,
/// ScreenProcessHandler.cs:248, the same one the captured frame gets).
fn load_template_image(path: &Path) -> Result<GrayImage, String> {
    d4lc_runtime::load_template_gray_image(path)
        .map_err(|error| format!("failed to load template image: {error}"))
}

// ---------------------------------------------------------------------------
// Step 3: Resolve capture source (window discovery or manual region)
// ---------------------------------------------------------------------------

/// Resolve a capture source — discover a running Diablo IV window or use an
/// explicitly provided manual region.
///
/// # Errors
///
/// Returns `AppRuntimePrepareError::CaptureSourceFailed` if no X11 display is
/// available, no matching window is found, and no manual region is provided.
pub fn resolve_capture_source(
    query: Option<&WindowQuery>,
    manual_region: Option<Rect>,
) -> Result<CaptureReference, AppRuntimePrepareError> {
    let query = query
        .cloned()
        .unwrap_or_default()
        .with_default_diablo_title();

    resolve_capture_reference(&query, manual_region)
        .map_err(AppRuntimePrepareError::CaptureSourceFailed)
}

// ---------------------------------------------------------------------------
// Step 4: Build OCR worker (requires leptess-backend feature)
// ---------------------------------------------------------------------------

/// The root the live runtime reads the vendored game data from
/// (`assets/original/data`, `assets/original/systempresets`).
///
/// The app config root carries the settings, the builds, and the extracted
/// user presets — never the vendored `assets/` tree — so the game-data root
/// resolves through the seam the app's catalog, build-import, and preset
/// surfaces already use ([`crate::actions::affix::catalog_repo_root`]): the
/// current directory first (packaging copies `assets/` into the bundle and
/// starts the binary with the bundle root as CWD), then the compile-time
/// checkout root. When neither location carries the tree (config-root-only
/// layouts), the config root is kept, i.e. the historical single-root
/// behavior: a tree without the catalog data then fails the evaluation
/// prerequisites ([`check_evaluation_prerequisites`]) instead of running a
/// runtime that parses without catalogs.
fn resolve_game_data_root(config_root: &Path) -> std::path::PathBuf {
    crate::actions::affix::catalog_repo_root().unwrap_or_else(|| config_root.to_path_buf())
}

/// Build an OCR worker for the live loop using the leptess OCR engine.
///
/// # Errors
///
/// Returns `OcrBackendNotAvailable` when the `leptess-backend` feature is not
/// enabled.  Returns `OcrEngineFailed` or `OcrWorkerFailed` when engine setup
/// or worker spawning fails.
#[cfg(feature = "leptess-backend")]
pub fn build_ocr_worker(
    config_root: &Path,
    config: &RuntimeConfig,
) -> Result<d4lc_runtime::OcrWorker, AppRuntimePrepareError> {
    build_ocr_worker_with_roots(config_root, &resolve_game_data_root(config_root), config)
}

/// The OCR profile of the live worker for `language`.
#[cfg(feature = "leptess-backend")]
fn live_ocr_profile(language: &str) -> d4lc_ocr::OcrProfile {
    d4lc_ocr::OcrProfile {
        backend: d4lc_ocr::OcrBackendKind::Auto,
        language: language.to_string(),
        tessdata_path: None,
    }
}

/// Check that the OCR preparation for `config` can evaluate items: the
/// preset's OCR templates load and the catalog data behind the fuzzy
/// indexes exists for the settings language. A missing piece
/// is a visible "not ready for evaluation", never a runtime that parses
/// without catalogs.
#[cfg_attr(not(feature = "leptess-backend"), allow(dead_code))]
pub fn check_evaluation_prerequisites(
    config_root: &Path,
    game_data_root: &Path,
    config: &RuntimeConfig,
) -> Result<(), AppRuntimePrepareError> {
    prepare_tooltip_ocr(config_root, game_data_root, config).map(drop)
}

/// The OCR preparation behind [`check_evaluation_prerequisites`]: settings,
/// templates and fuzzy catalogs, loaded once. The live worker keeps the
/// returned preparation for every job; a change of anything it read
/// re-prepares the runtime and builds a new worker (consumer matrix,
/// `settings_consumers`).
#[cfg_attr(not(feature = "leptess-backend"), allow(dead_code))]
fn prepare_tooltip_ocr(
    config_root: &Path,
    game_data_root: &Path,
    config: &RuntimeConfig,
) -> Result<d4lc_runtime::ocr_parity::PreparedTooltipOcr, AppRuntimePrepareError> {
    let profile = d4lc_ocr::OcrProfile {
        backend: d4lc_ocr::OcrBackendKind::Auto,
        language: config.language.clone(),
        tessdata_path: None,
    };
    let prepared = d4lc_runtime::ocr_parity::PreparedTooltipOcr::prepare_with_game_data_root(
        config_root,
        game_data_root,
        config.preset.clone(),
        profile,
    )
    .map_err(|error| {
        AppRuntimePrepareError::NotReadyForEvaluation(format!("OCR preparation: {error}"))
    })?;
    match prepared.catalog_error() {
        Some(error) => Err(AppRuntimePrepareError::NotReadyForEvaluation(format!(
            "catalog data missing: {error}"
        ))),
        None => Ok(prepared),
    }
}

/// Check that Tesseract has the language data for `profile` by starting
/// one engine. The worker would otherwise only fail on its first
/// job, after the runtime already claimed to be running.
#[cfg(feature = "leptess-backend")]
pub fn check_ocr_language_data(
    profile: &d4lc_ocr::OcrProfile,
) -> Result<(), AppRuntimePrepareError> {
    d4lc_ocr::LeptessOcrEngine::new(profile)
        .map(drop)
        .map_err(|error| {
            AppRuntimePrepareError::NotReadyForEvaluation(format!(
                "OCR language data for {} (Tesseract '{}') is not available: {error}",
                profile.language,
                profile.tesseract_language()
            ))
        })
}

/// [`build_ocr_worker`] with an explicit game-data root.
#[cfg(feature = "leptess-backend")]
pub fn build_ocr_worker_with_roots(
    config_root: &Path,
    game_data_root: &Path,
    config: &RuntimeConfig,
) -> Result<d4lc_runtime::OcrWorker, AppRuntimePrepareError> {
    use d4lc_ocr::{LeptessOcrEngine, OcrEngine, OcrEngineRegistry, OcrProfile};
    use d4lc_runtime::{OcrJob, TooltipTextSegmentSnapshot, TooltipTextSnapshot};

    let profile = live_ocr_profile(&config.language);
    let prepared = prepare_tooltip_ocr(config_root, game_data_root, config)?;
    check_ocr_language_data(&profile)?;

    let registry = OcrEngineRegistry::new(move |p: &OcrProfile| {
        let engine = LeptessOcrEngine::new(p)
            .map_err(|e| d4lc_ocr::OcrError::Backend(format!("leptess init: {e}")))?;
        Ok(Box::new(engine) as Box<dyn OcrEngine + Send>)
    });
    // The segments of a tooltip are recognized on this many engines at
    // once; starting them here keeps engine start-up out of the first read.
    registry
        .pool_for(profile.clone())
        .and_then(|pool| pool.prewarm(d4lc_runtime::ocr_parity::OCR_SEGMENT_THREADS))
        .map_err(|error| {
            AppRuntimePrepareError::NotReadyForEvaluation(format!(
                "OCR engines for {} could not start: {error}",
                profile.language
            ))
        })?;

    let preview_chars: usize = 80;
    // Live debug-bundle gate. Seeded from the persisted
    // `Settings.IsDebugInfoEnabled` bit and flipped live by the loop when the
    // toggle changes; with it off the job closure allocates no stage bitmap
    // for the accepted analysis at all.
    let debug_bundle = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        config.is_debug_info_enabled,
    ));
    let job_debug_bundle = std::sync::Arc::clone(&debug_bundle);

    let worker = d4lc_runtime::OcrWorker::spawn_with_debug_flag(
        move |job: OcrJob| {
            // 1. Run the full upstream-parity OCR pipeline on the job's own
            //    tooltip image — the crop this frame's detection ran on. The
            //    worker never captures: a later frame (X11 re-capture or the next
            //    Wayland stream buffer) cannot replace the pixels the job was
            //    scheduled for. Settings, templates and catalogs come from the
            //    preparation above, loaded once for this worker.
            // Row geometry comes back in window space, converted once through
            // the image's capture-time placement.
            let execution = prepared
                .execute_detected_crop(&registry, job.image.pixels())
                .map_err(|e| format!("ocr-worker parity: {e}"))?;
            // The stage bitmaps are rebuilt from the execution's own crop
            // trace, marker plan and segment results — no second detection, no
            // second OCR. Built once per job, off the
            // frame hot path; only while the debug bit is on.
            let debug = job_debug_bundle
                .load(std::sync::atomic::Ordering::Relaxed)
                .then(|| {
                    std::sync::Arc::new(d4lc_runtime::TooltipDebugBundle::from_execution(
                        &execution,
                    ))
                });
            let parity = d4lc_runtime::ocr_parity::runtime_parity_result(
                &execution,
                preview_chars,
                prepared.fuzzy_indexes(),
                &job.image.capture().placement,
            );

            // 3. Build TooltipTextSnapshot from the parity result.
            let segments_preview: Vec<TooltipTextSegmentSnapshot> = parity
                .segments_preview
                .into_iter()
                .map(|s| TooltipTextSegmentSnapshot {
                    index: s.index,
                    segment_kind: s.segment_kind,
                    line_count: s.line_count,
                    raw_text_preview: s.raw_text_preview,
                    converted_text_preview: s.converted_text_preview,
                })
                .collect();

            Ok(TooltipTextSnapshot {
                sequence: 0,
                source_frame_sequence: job.frame_id(),
                tooltip_signature: job.tooltip_signature,
                content_fingerprint: job.content_fingerprint(),
                token: job.token,
                ocr_read_top: parity.read_top,
                ocr_read_extent: parity.read_extent,
                parity_path: parity.parity_path.to_string(),
                ocr_ms: parity.ocr_ms,
                // The crop is cut by the frame loop's detection; the worker
                // spends no time capturing.
                capture_ms: 0,
                conversion_ms: parity.conversion_ms,
                splitter_detection_ms: Some(parity.splitter_detection_ms),
                segment_count: parity.segment_count,
                line_count: parity.line_count,
                raw_text: parity.raw_text,
                normalized_lines: parity.normalized_lines,
                segments_preview,
                title_preview: parity.title_preview,
                text_preview: parity.text_preview,
                item: Some(parity.item),
                error: None,
                debug,
            })
        },
        debug_bundle,
    );

    Ok(worker)
}

/// Check what a runtime start needs apart from its capture: the settings,
/// the preset's detector templates, and the OCR and evaluation
/// prerequisites.
///
/// Opens no capture session and resolves no capture target, so it is safe
/// on the UI thread on every display backend (on Wayland a capture check
/// would open a portal session and possibly the compositor dialog). The
/// capture is established — and its failure reported — by the live loop's
/// own preparation.
///
/// # Errors
///
/// The same structured errors the corresponding steps of
/// `prepare_app_runtime` return.
pub fn check_runtime_prerequisites_without_capture(
    config_root: &Path,
) -> Result<(), AppRuntimePrepareError> {
    let config = resolve_runtime_config(config_root, None, None, None)?;
    let _presets = d4lc_assets::lock_preset_trees_shared();
    build_prepared_detector(config_root, &config)?;
    check_ocr_prerequisites(config_root, &config)
}

/// The tooltip detector and the OCR worker (whose marker templates come from
/// the same preset), read from one consistent preset tree: a preset install
/// running at the same time is applied completely before or after this read
/// (see [`d4lc_assets::lock_preset_trees_shared`]).
///
/// # Errors
///
/// The errors of [`build_prepared_detector`] and [`build_ocr_worker`].
pub(crate) fn build_detector_and_ocr_worker(
    config_root: &Path,
    config: &RuntimeConfig,
) -> Result<(PreparedDetector, d4lc_runtime::OcrWorker), AppRuntimePrepareError> {
    let _presets = d4lc_assets::lock_preset_trees_shared();
    let detector = build_prepared_detector(config_root, config)?;
    let ocr_worker = build_ocr_worker(config_root, config)?;
    Ok((detector, ocr_worker))
}

/// The checks [`build_ocr_worker`] runs before it spawns the worker.
#[cfg(feature = "leptess-backend")]
fn check_ocr_prerequisites(
    config_root: &Path,
    config: &RuntimeConfig,
) -> Result<(), AppRuntimePrepareError> {
    check_evaluation_prerequisites(config_root, &resolve_game_data_root(config_root), config)?;
    check_ocr_language_data(&live_ocr_profile(&config.language))
}

/// Without the OCR backend no runtime can start (see [`build_ocr_worker`]).
#[cfg(not(feature = "leptess-backend"))]
fn check_ocr_prerequisites(
    _config_root: &Path,
    _config: &RuntimeConfig,
) -> Result<(), AppRuntimePrepareError> {
    Err(AppRuntimePrepareError::OcrBackendNotAvailable)
}

/// Build OCR worker — feature-gated fallback for non-leptess builds.
#[cfg(not(feature = "leptess-backend"))]
pub fn build_ocr_worker(
    _config_root: &Path,
    _config: &RuntimeConfig,
) -> Result<d4lc_runtime::OcrWorker, AppRuntimePrepareError> {
    Err(AppRuntimePrepareError::OcrBackendNotAvailable)
}

// ---------------------------------------------------------------------------
// Step 5: Complete runtime preparation (Steps 1 + 2 + 3 + 4 combined)
// ---------------------------------------------------------------------------

/// Full runtime preparation: load config, detect the capture engine,
/// build detector, discover window, create OCR worker.
///
/// This is the main entry point for `start_live_loop` (real-runtime path).
/// Returns a `PreparedRuntime` (immutable per-session resources), a
/// `RuntimeSession` (mutable per-session state with OCR enabled), and the
/// resolved capture-engine selection ([`CaptureEngineResolution`]) the
/// caller publishes to the Debug screen.
///
/// The `auto` display backend is detected ONCE per runtime start, at two
/// points:
///
/// 1. Session routing without probing (no `DISPLAY`): the session facts
///    alone decide between the portal engine, the structured
///    unavailability, and "no usable engine".
/// 2. The capture-source step (with `DISPLAY`): its single game-window
///    discovery IS the detection probe — a resolved reference selects the
///    X11 engine and is reused as the capture reference, a miss falls
///    back to the portal engine on a Wayland session, and otherwise
///    today's capture-source failure is carried. The probe deliberately
///    runs AFTER the detector/OCR prerequisite checks: a missing catalog
///    or language pack is the reason to report even when the game is not
///    running yet.
///
/// Explicit `x11`/`wayland` settings stay strictly honored:
/// detection never replaces them.
///
/// OCR worker is created via `build_ocr_worker()` which requires the
/// `leptess-backend` feature.  Without it, preparation fails with
/// `OcrBackendNotAvailable`.
///
/// # Errors
///
/// Returns `AppRuntimePrepareError` if any step fails: config load, engine
/// resolution, template loading, window discovery, detector construction,
/// or OCR worker creation. The caller (typically `start_live_loop`) should
/// emit a `RuntimeEvent::Error` and abort the live loop on failure.
#[cfg(test)]
pub fn prepare_app_runtime(
    config_root: &Path,
    window_query: Option<&WindowQuery>,
    manual_region: Option<Rect>,
    preset_override: Option<String>,
    language_override: Option<String>,
    mode_override: Option<InputMode>,
) -> Result<(PreparedRuntime, RuntimeSession, CaptureEngineResolution), AppRuntimePrepareError> {
    prepare_app_runtime_with_progress(
        config_root,
        window_query,
        manual_region,
        preset_override,
        language_override,
        mode_override,
        ReusableCapture::none(),
        &|_| {},
    )
}

/// `prepare_app_runtime` that reports each long-running step through
/// `progress` before the step starts (see [`PrepareProgress`]) and, when
/// the resolved engine is Wayland, keeps the running capture session in
/// `reuse` instead of opening a new one (see [`ReusableCapture`]).
///
/// # Errors
///
/// Same as `prepare_app_runtime`.
#[allow(clippy::too_many_arguments)]
pub fn prepare_app_runtime_with_progress(
    config_root: &Path,
    window_query: Option<&WindowQuery>,
    manual_region: Option<Rect>,
    preset_override: Option<String>,
    language_override: Option<String>,
    mode_override: Option<InputMode>,
    reuse: ReusableCapture,
    progress: &dyn Fn(PrepareProgress),
) -> Result<(PreparedRuntime, RuntimeSession, CaptureEngineResolution), AppRuntimePrepareError> {
    prepare_app_runtime_inner(
        config_root,
        window_query,
        manual_region,
        preset_override,
        language_override,
        mode_override,
        reuse,
        None,
        progress,
    )
}

/// `prepare_app_runtime` for a caller that runs beside a live loop: it
/// never opens a ScreenCast portal session. The portal session is
/// exclusive per process and belongs to the live loop, so a Wayland
/// engine resolution stops with
/// [`AppRuntimePrepareError::PortalSessionNotAllowed`] (carrying `caller`)
/// before any portal call instead of waiting on the session slot and
/// failing with a conflict — or, while the loop is still preparing,
/// taking the slot from it.
///
/// # Errors
///
/// Same as `prepare_app_runtime`, plus the portal refusal above.
pub fn prepare_app_runtime_without_portal(
    config_root: &Path,
    caller: &str,
) -> Result<(PreparedRuntime, RuntimeSession, CaptureEngineResolution), AppRuntimePrepareError> {
    prepare_app_runtime_inner(
        config_root,
        None,
        None,
        None,
        None,
        None,
        ReusableCapture::none(),
        Some(caller),
        &|_| {},
    )
}

/// The shared preparation body. `portal_forbidden_for` names the caller
/// that must not open a portal session (see
/// [`prepare_app_runtime_without_portal`]); `None` allows it.
#[allow(clippy::too_many_arguments)]
fn prepare_app_runtime_inner(
    config_root: &Path,
    window_query: Option<&WindowQuery>,
    manual_region: Option<Rect>,
    preset_override: Option<String>,
    language_override: Option<String>,
    mode_override: Option<InputMode>,
    reuse: ReusableCapture,
    portal_forbidden_for: Option<&str>,
    progress: &dyn Fn(PrepareProgress),
) -> Result<(PreparedRuntime, RuntimeSession, CaptureEngineResolution), AppRuntimePrepareError> {
    #[cfg(not(feature = "wayland-portal"))]
    let _ = (reuse, portal_forbidden_for, progress);
    let mut config = resolve_runtime_config(
        config_root,
        preset_override,
        language_override,
        mode_override,
    )?;
    let env = SessionEnvironment::from_process();

    // Engine resolution, stage 1: session routing that needs no probe.
    // With `DISPLAY` absent the session facts alone decide; with `DISPLAY`
    // present (or an explicit setting) the status stays as resolved at the
    // parse level and `auto` defers to the capture-source step below.
    let mut resolution =
        if config.display_backend_status == DisplayBackendStatus::Auto && !env.x11_display {
            resolve_display_backend_for_session(
                config.display_backend,
                WAYLAND_CAPTURE_BUILD,
                env,
                &X11GameWindowProbe::NotRun,
            )
        } else {
            CaptureEngineResolution {
                backend: config.display_backend,
                status: config.display_backend_status.clone(),
            }
        };
    config.display_backend_status = resolution.status.clone();

    // Wayland guard: a Wayland engine selection (explicit, or resolved by
    // auto) without the capture path in this binary stops preparation with
    // the structured unavailability instead of silently running the X11
    // paths while claiming Wayland. An auto selection with no usable
    // engine stops with today's capture-source failure — never a silent
    // fallback onto a path that cannot see the game.
    match &config.display_backend_status {
        DisplayBackendStatus::WaylandUnavailable { reason } => {
            return Err(AppRuntimePrepareError::DisplayBackendUnavailable(
                reason.clone(),
            ));
        }
        DisplayBackendStatus::NoEngine { reason } => {
            return Err(AppRuntimePrepareError::CaptureSourceFailed(reason.clone()));
        }
        _ => {}
    }

    // The real Wayland capture path: establish the ScreenCast portal
    // session and route the runtime frame dispatch through the
    // PipeWire stream source. Keyed on the RESOLVED engine, not the raw
    // setting: `auto` on a Wayland session routes here too.
    #[cfg(feature = "wayland-portal")]
    if config.display_backend_status.is_wayland_engine() {
        refuse_forbidden_portal(portal_forbidden_for)?;
        resolved_capture_backend_gate(&config)?;
        let (runtime, session) = prepare_wayland_runtime(config_root, &config, reuse, progress)?;
        return Ok((runtime, session, resolution));
    }

    // The evaluation prerequisites are checked before the window lookup: a
    // missing catalog or language pack is the reason to report even when
    // the game is not running yet.
    let (detector, ocr_worker) = build_detector_and_ocr_worker(config_root, &config)?;

    // Engine resolution, stage 2 + capture source: for `auto` with a
    // display, the capture-source step's single discovery is the detection
    // probe, and its reference is reused as the capture reference (one X11
    // discovery per start). Explicit settings resolve here as before.
    let capture_ref = if config.display_backend_status == DisplayBackendStatus::Auto {
        let query = window_query
            .cloned()
            .unwrap_or_default()
            .with_default_diablo_title();
        let mut probe_reference: Option<CaptureReference> = None;
        let probe = match resolve_capture_reference(&query, manual_region) {
            Ok(reference) => {
                let matched_window =
                    matches!(reference.source, d4lc_capture::CaptureSource::Window(_));
                probe_reference = Some(reference);
                X11GameWindowProbe::Resolved { matched_window }
            }
            Err(error) => X11GameWindowProbe::Failed { error },
        };
        resolution = resolve_display_backend_for_session(
            config.display_backend,
            WAYLAND_CAPTURE_BUILD,
            env,
            &probe,
        );
        config.display_backend_status = resolution.status.clone();
        match &config.display_backend_status {
            DisplayBackendStatus::NoEngine { reason } => {
                return Err(AppRuntimePrepareError::CaptureSourceFailed(reason.clone()));
            }
            DisplayBackendStatus::WaylandUnavailable { reason } => {
                return Err(AppRuntimePrepareError::DisplayBackendUnavailable(
                    reason.clone(),
                ));
            }
            _ => {}
        }
        // A probe miss on a Wayland session falls back to the portal
        // engine; the portal path prepares its own detector and OCR
        // worker, so the ones built above are dropped here.
        #[cfg(feature = "wayland-portal")]
        if config.display_backend_status.is_wayland_engine() {
            refuse_forbidden_portal(portal_forbidden_for)?;
            resolved_capture_backend_gate(&config)?;
            let (runtime, session) =
                prepare_wayland_runtime(config_root, &config, reuse, progress)?;
            return Ok((runtime, session, resolution));
        }
        match probe_reference {
            Some(reference) => reference,
            // Unreachable: an X11 resolution from the probe carries its
            // reference. The standard resolution is the fallback.
            None => resolve_capture_source(window_query, manual_region)?,
        }
    } else {
        resolve_capture_source(window_query, manual_region)?
    };

    // The X11 loop captures on demand and paces
    // itself at the captured output's refresh rate; when RandR cannot name
    // a rate, the announced 60 Hz fallback applies.
    let pacing = d4lc_capture::x11_refresh_interval(capture_ref.geometry)
        .map(|interval| d4lc_runtime::FramePacing {
            interval,
            source: d4lc_runtime::PacingSource::RandR,
        })
        .unwrap_or_else(d4lc_runtime::FramePacing::fallback);

    let runtime = PreparedRuntime::new(config.capture_backend, capture_ref, detector)
        .with_frame_pacing(pacing);

    let session = RuntimeSession::new(Some(ocr_worker), config.ocr_interval);

    Ok((runtime, session, resolution))
}

#[cfg(feature = "wayland-portal")]
mod wayland;
#[cfg(feature = "wayland-portal")]
use wayland::{prepare_wayland_runtime, refuse_forbidden_portal, reused_stream_reference};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
