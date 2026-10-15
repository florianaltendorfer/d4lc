//! X11 window discovery, one-shot capture, manual regions, and cursor state.
//!
//! The original Windows implementation uses process/window title checks plus
//! desktop BitBlt capture. This crate keeps the same behavioral split for the
//! Linux port: find a candidate Diablo window, capture one frame, and keep cursor
//! coordinates convertible to the selected window or manual region.

use d4lc_core::{CaptureFrame, Point, Rect, WindowInfo};
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

mod native_x11;
mod synthetic;
#[cfg(test)]
mod test_xvfb;
mod x11_command;
mod x11_discovery;

pub use synthetic::SyntheticCaptureFrameSource;
pub use x11_command::{SystemX11CommandRunner, X11CommandRunner};
pub use x11_discovery::{discover_x11_windows, window_matches_query};

/// New-frame signal of a capture pump: the live
/// loop waits on it and starts processing the moment a new frame arrives,
/// instead of sleeping a fixed interval. The signal carries no pixels — the
/// frame slot stays latest-wins, so a loop that overran its interval
/// consumes only the newest frame. Cloning shares the signal.
#[derive(Clone, Default)]
pub struct FrameSignal {
    state: Arc<(Mutex<u64>, Condvar)>,
}

impl FrameSignal {
    pub fn new() -> Self {
        Self {
            state: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    /// Record a new frame and wake the waiters; returns the generation.
    pub fn bump(&self) -> u64 {
        let (lock, condvar) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = guard.wrapping_add(1);
        let generation = *guard;
        condvar.notify_all();
        generation
    }

    /// The newest observed frame generation.
    pub fn generation(&self) -> u64 {
        *self
            .state
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Wait until a frame newer than `seen` arrives or `timeout` elapses.
    /// Returns the newest generation when one arrived, `None` on timeout
    /// (a zero timeout polls once, so an overrunning loop never blocks).
    pub fn wait_for(&self, seen: u64, timeout: Duration) -> Option<u64> {
        let (lock, condvar) = &*self.state;
        let deadline = Instant::now().checked_add(timeout);
        let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while *guard == seen {
            let deadline = deadline?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (woken, wait_result) = condvar
                .wait_timeout(guard, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard = woken;
            if wait_result.timed_out() && *guard == seen {
                return None;
            }
        }
        Some(*guard)
    }
}

/// The refresh interval of the output carrying `geometry` on the default
/// X11 display, queried through RandR (the X11 loop paces
/// itself at the monitor's refresh). `None` when RandR or the display is
/// unavailable — the caller falls back to 60 Hz and names that in the
/// debug line.
pub fn x11_refresh_interval(geometry: Rect) -> Option<Duration> {
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;

    let (connection, screen_number) = x11rb::rust_connection::RustConnection::connect(None).ok()?;
    let screen = connection.setup().roots.get(screen_number)?.root;
    let resources = connection
        .randr_get_screen_resources(screen)
        .ok()?
        .reply()
        .ok()?;

    // The output the captured window sits on: the CRTC whose box contains
    // the capture geometry's center. Sanity band for the derived rate:
    // below it the mode is not a live monitor mode, above it the dot clock
    // was misread.
    fn mode_interval(modes: &[x11rb::protocol::randr::ModeInfo], mode_id: u32) -> Option<Duration> {
        let mode = modes.iter().find(|mode| mode.id == mode_id)?;
        if mode.htotal == 0 || mode.vtotal == 0 {
            return None;
        }
        let hz = f64::from(mode.dot_clock) / (f64::from(mode.htotal) * f64::from(mode.vtotal));
        if (20.0..=500.0).contains(&hz) {
            Some(Duration::from_secs_f64(1.0 / hz))
        } else {
            None
        }
    }

    let center = (
        geometry.x + geometry.width as i32 / 2,
        geometry.y + geometry.height as i32 / 2,
    );
    for crtc in resources.crtcs.iter().copied() {
        let info = connection.randr_get_crtc_info(crtc, 0).ok()?.reply().ok()?;
        if info.mode == 0 {
            continue;
        }
        let crtc_x = i32::from(info.x);
        let crtc_y = i32::from(info.y);
        let on_this_output = info.width > 0
            && info.height > 0
            && center.0 >= crtc_x
            && center.0 < crtc_x + i32::from(info.width)
            && center.1 >= crtc_y
            && center.1 < crtc_y + i32::from(info.height);
        if !on_this_output {
            continue;
        }
        if let Some(interval) = mode_interval(&resources.modes, info.mode) {
            return Some(interval);
        }
    }

    // No containing CRTC: a single-output machine with an offset window —
    // fall back to the first CRTC with a usable mode.
    for crtc in resources.crtcs.iter().copied() {
        let Ok(info) = connection.randr_get_crtc_info(crtc, 0).ok()?.reply() else {
            continue;
        };
        if info.mode == 0 {
            continue;
        }
        if let Some(interval) = mode_interval(&resources.modes, info.mode) {
            return Some(interval);
        }
    }
    None
}

/// Wayland capture via the ScreenCast portal + PipeWire. The pure parts
/// (session state machine, frame conversion) compile in every build; the
/// `ashpd`/`pipewire` real path is gated behind the `wayland-portal`
/// build feature.
pub mod wayland;

#[cfg(feature = "wayland-portal")]
pub use wayland::{
    WaylandCapture, WaylandCursorStatus, WaylandFrameSource, WaylandFrameSourceConfig,
};

const XWD_FILE_VERSION: u32 = 7;
const XWD_Z_PIXMAP: u32 = 2;
const BYTE_ORDER_LSB_FIRST: u32 = 0;
const BYTE_ORDER_MSB_FIRST: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowQuery {
    pub title: Option<String>,
    pub class_name: Option<String>,
    pub pid: Option<u32>,
    pub exact_title: bool,
}

impl WindowQuery {
    pub fn with_default_diablo_title(mut self) -> Self {
        if self.title.is_none() && self.class_name.is_none() && self.pid.is_none() {
            self.title = Some("Diablo IV".to_string());
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.class_name.is_none() && self.pid.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCandidate {
    pub info: WindowInfo,
    pub pid: Option<u32>,
    pub is_active: bool,
    pub matches_query: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSelection {
    pub candidates: Vec<WindowCandidate>,
    pub selected: Option<WindowCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorState {
    pub global: Point,
    pub window_relative: Option<Point>,
    pub inside_reference_rect: Option<bool>,
    pub scale: ScaleFactor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleFactor {
    value_milli: u32,
}

impl ScaleFactor {
    pub const fn one() -> Self {
        Self { value_milli: 1000 }
    }

    pub const fn from_milli(value_milli: u32) -> Self {
        Self { value_milli }
    }

    pub const fn value_milli(self) -> u32 {
        self.value_milli
    }

    pub fn as_decimal_string(self) -> String {
        let whole = self.value_milli / 1000;
        let fraction = self.value_milli % 1000;
        if fraction == 0 {
            format!("{whole}.0")
        } else if fraction.is_multiple_of(100) {
            format!("{whole}.{}", fraction / 100)
        } else if fraction.is_multiple_of(10) {
            format!("{whole}.{:02}", fraction / 10)
        } else {
            format!("{whole}.{fraction:03}")
        }
    }
}

impl Default for ScaleFactor {
    fn default() -> Self {
        Self::one()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    Window(WindowCandidate),
    ManualRegion(Rect),
    /// A ScreenCast portal stream (Wayland display backend): the rect is
    /// the stream rect the connected session delivers (origin 0,0 in stream
    /// space plus the delivered size) — the same geometry the stream's frame
    /// results report. Frames come from the Wayland frame source; the X11
    /// capture paths cannot read a PipeWire stream and refuse the reference
    /// by name instead of guessing at an X11 drawable.
    PortalStream(Rect),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    pub backend: CaptureBackend,
    pub fallback_reason: Option<String>,
    pub source: CaptureSource,
    /// Full reference geometry in screen coordinates. For window captures this is
    /// the selected client window; for manual captures it is the manual region.
    pub geometry: Rect,
    pub scale: ScaleFactor,
    /// Requested capture rectangle in the coordinate space described by
    /// `roi_origin`. Full-frame captures use the full reference rect translated
    /// to origin 0,0.
    pub requested_capture_rect: Rect,
    /// Actual rectangle represented by `frame`, in the same coordinate space as
    /// `requested_capture_rect`. Backends may clamp this rect or fall back to the
    /// full reference frame.
    pub actual_capture_rect: Rect,
    pub roi_origin: RoiOrigin,
    pub capture_scope: CaptureScopeResult,
    pub overlay_window: OverlayWindowState,
    pub capture_method: CaptureMethod,
    pub frame: CaptureFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayWindowState {
    Visible,
    Hidden,
    Excluded,
    #[default]
    Ignored,
}

impl OverlayWindowState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
            Self::Excluded => "excluded",
            Self::Ignored => "ignored",
        }
    }
}

impl fmt::Display for OverlayWindowState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureScope {
    #[default]
    Auto,
    Full,
    Roi,
}

impl CaptureScope {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "full" => Ok(Self::Full),
            "roi" => Ok(Self::Roi),
            other => Err(format!(
                "unsupported --capture-scope '{other}'; expected auto|full|roi"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Full => "full",
            Self::Roi => "roi",
        }
    }
}

impl fmt::Display for CaptureScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRequest {
    Full,
    WindowRelativeRoi(Rect),
    ScreenGlobalRoi(Rect),
}

impl CaptureRequest {
    pub const fn is_roi(self) -> bool {
        matches!(self, Self::WindowRelativeRoi(_) | Self::ScreenGlobalRoi(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureScopeResult {
    #[default]
    FullFrame,
    Roi,
    FullFrameFallback,
}

impl CaptureScopeResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullFrame => "full_frame",
            Self::Roi => "roi",
            Self::FullFrameFallback => "full_frame_fallback",
        }
    }
}

impl fmt::Display for CaptureScopeResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureMethod {
    #[default]
    X11Command,
    X11NativeXGetImage,
    X11NativeXShm,
    /// ScreenCast portal + PipeWire stream. Only produced by the
    /// Wayland frame source (behind the `wayland-portal` feature); the
    /// X11-internal [`CaptureBackend`] does not describe this path.
    WaylandPortal,
}

impl CaptureMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X11Command => "x11-command",
            Self::X11NativeXGetImage => "x11-native-xgetimage",
            Self::X11NativeXShm => "x11-native-xshm",
            Self::WaylandPortal => "wayland-portal",
        }
    }
}

impl fmt::Display for CaptureMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoiOrigin {
    #[default]
    FullFrame,
    WindowRelative,
    ScreenGlobal,
}

impl RoiOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullFrame => "full_frame",
            Self::WindowRelative => "window_relative",
            Self::ScreenGlobal => "screen_global",
        }
    }
}

impl fmt::Display for RoiOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureReference {
    pub source: CaptureSource,
    pub geometry: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureBackend {
    #[default]
    Auto,
    X11Native,
    X11Command,
}

impl CaptureBackend {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "x11-native" => Ok(Self::X11Native),
            "x11-command" | "raw-window" => Ok(Self::X11Command),
            other => Err(format!(
                "unsupported --backend '{other}'; expected auto|x11-native|x11-command"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::X11Native => "x11-native",
            Self::X11Command => "x11-command",
        }
    }
}

impl fmt::Display for CaptureBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub fn capture_selected_window(query: &WindowQuery) -> Result<CaptureResult, String> {
    capture_selected_window_with(&SystemX11CommandRunner, query)
}

pub fn capture_selected_window_with(
    runner: &impl X11CommandRunner,
    query: &WindowQuery,
) -> Result<CaptureResult, String> {
    capture_selected_window_with_request_with(runner, query, CaptureRequest::Full)
}

pub fn capture_selected_window_with_request(
    query: &WindowQuery,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    capture_selected_window_with_request_with(&SystemX11CommandRunner, query, request)
}

pub fn capture_selected_window_with_request_with(
    runner: &impl X11CommandRunner,
    query: &WindowQuery,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    let selection = discover_x11_windows(query)?;
    let selected = selection.selected.ok_or_else(|| {
        if selection.candidates.is_empty() {
            "no X11 client windows were discovered".to_string()
        } else {
            "no X11 window matched the requested query".to_string()
        }
    })?;
    capture_window_candidate_with_request_with(runner, selected, request)
}

pub fn capture_window_candidate_with_request(
    selected: WindowCandidate,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    capture_window_candidate_with_request_with(&SystemX11CommandRunner, selected, request)
}

fn capture_window_candidate_with_request_with(
    runner: &impl X11CommandRunner,
    selected: WindowCandidate,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    let geometry = selected.info.rect;
    let full_rect = reference_full_rect(geometry);
    let (requested_capture_rect, actual_capture_rect, roi_origin, capture_scope, fallback_reason) =
        command_capture_metadata(geometry, request)?;
    let bytes = runner.run(
        "xwd",
        &[
            "-silent".to_string(),
            "-id".to_string(),
            format_window_id(selected.info.id),
        ],
    )?;
    let frame = parse_xwd_frame(&bytes)?;
    Ok(CaptureResult {
        backend: CaptureBackend::X11Command,
        capture_method: CaptureMethod::X11Command,
        fallback_reason,
        source: CaptureSource::Window(selected),
        geometry,
        scale: ScaleFactor::one(),
        requested_capture_rect: if requested_capture_rect.width == 0 {
            full_rect
        } else {
            requested_capture_rect
        },
        actual_capture_rect,
        roi_origin,
        capture_scope,
        overlay_window: OverlayWindowState::Ignored,
        frame,
    })
}

pub fn capture_manual_region(region: Rect) -> Result<CaptureResult, String> {
    capture_manual_region_with(&SystemX11CommandRunner, region)
}

pub fn capture_manual_region_with(
    runner: &impl X11CommandRunner,
    region: Rect,
) -> Result<CaptureResult, String> {
    capture_manual_region_with_request_with(runner, region, CaptureRequest::Full)
}

pub fn capture_manual_region_with_request(
    region: Rect,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    capture_manual_region_with_request_with(&SystemX11CommandRunner, region, request)
}

pub fn capture_manual_region_with_request_with(
    runner: &impl X11CommandRunner,
    region: Rect,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    validate_rect(region)?;
    let full_rect = reference_full_rect(region);
    let (requested_capture_rect, actual_capture_rect, roi_origin, capture_scope, fallback_reason) =
        command_capture_metadata(region, request)?;
    let bytes = runner.run("xwd", &["-silent".to_string(), "-root".to_string()])?;
    let root_frame = parse_xwd_frame(&bytes)?;
    let frame = crop_frame(&root_frame, region)?;
    Ok(CaptureResult {
        backend: CaptureBackend::X11Command,
        capture_method: CaptureMethod::X11Command,
        fallback_reason,
        source: CaptureSource::ManualRegion(region),
        geometry: region,
        scale: ScaleFactor::one(),
        requested_capture_rect: if requested_capture_rect.width == 0 {
            full_rect
        } else {
            requested_capture_rect
        },
        actual_capture_rect,
        roi_origin,
        capture_scope,
        overlay_window: OverlayWindowState::Ignored,
        frame,
    })
}

pub fn capture_window_or_manual_region(
    query: &WindowQuery,
    manual_region: Option<Rect>,
) -> Result<CaptureResult, String> {
    capture_window_or_manual_region_with_backend(query, manual_region, CaptureBackend::X11Command)
}

pub fn capture_window_or_manual_region_with_backend(
    query: &WindowQuery,
    manual_region: Option<Rect>,
    backend: CaptureBackend,
) -> Result<CaptureResult, String> {
    capture_window_or_manual_region_with_backend_and_request(
        query,
        manual_region,
        backend,
        CaptureRequest::Full,
    )
}

pub fn capture_reference_with_backend_and_request(
    reference: &CaptureReference,
    backend: CaptureBackend,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    match backend {
        CaptureBackend::Auto => match capture_reference_native(reference, request, None) {
            Ok(result) => Ok(result),
            Err(native_error) => capture_reference_command(reference, request, Some(native_error)),
        },
        CaptureBackend::X11Native => capture_reference_native(reference, request, None),
        CaptureBackend::X11Command => capture_reference_command(reference, request, None),
    }
}

fn capture_reference_native(
    reference: &CaptureReference,
    request: CaptureRequest,
    fallback_reason: Option<String>,
) -> Result<CaptureResult, String> {
    let mut result = match &reference.source {
        CaptureSource::Window(candidate) => {
            capture_window_candidate_native_with_request(candidate.clone(), request)?
        }
        CaptureSource::ManualRegion(region) => {
            capture_manual_region_native_with_request(*region, request)?
        }
        // The portal stream is not an X11 drawable: its frames are served by
        // the Wayland frame source (runtime dispatch on the Wayland engine),
        // so the X11-internal backends refuse the reference by name.
        CaptureSource::PortalStream(_) => {
            return Err(
                "the ScreenCast portal stream is captured by the Wayland frame source, \
                 not the X11 capture backends"
                    .to_string(),
            )
        }
    };
    result.fallback_reason = combine_fallback_reasons(fallback_reason, result.fallback_reason);
    Ok(result)
}

fn capture_reference_command(
    reference: &CaptureReference,
    request: CaptureRequest,
    fallback_reason: Option<String>,
) -> Result<CaptureResult, String> {
    let mut result = match &reference.source {
        CaptureSource::Window(candidate) => {
            capture_window_candidate_with_request(candidate.clone(), request)?
        }
        CaptureSource::ManualRegion(region) => {
            capture_manual_region_with_request(*region, request)?
        }
        // The portal stream is not an X11 drawable: its frames are served by
        // the Wayland frame source (runtime dispatch on the Wayland engine),
        // so the X11-internal backends refuse the reference by name.
        CaptureSource::PortalStream(_) => {
            return Err(
                "the ScreenCast portal stream is captured by the Wayland frame source, \
                 not the X11 capture backends"
                    .to_string(),
            )
        }
    };
    result.fallback_reason = combine_fallback_reasons(fallback_reason, result.fallback_reason);
    Ok(result)
}

pub fn capture_window_or_manual_region_with_backend_and_request(
    query: &WindowQuery,
    manual_region: Option<Rect>,
    backend: CaptureBackend,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    let query = query.clone().with_default_diablo_title();
    match backend {
        CaptureBackend::Auto => capture_auto(&query, manual_region, request),
        CaptureBackend::X11Native => capture_native_or_manual(&query, manual_region, request, None),
        CaptureBackend::X11Command => {
            capture_command_or_manual(&query, manual_region, request, None)
        }
    }
}

fn capture_auto(
    query: &WindowQuery,
    manual_region: Option<Rect>,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    match capture_native_or_manual(query, manual_region, request, None) {
        Ok(result) => Ok(result),
        Err(native_error) => {
            capture_command_or_manual(query, manual_region, request, Some(native_error))
        }
    }
}

fn capture_command_or_manual(
    query: &WindowQuery,
    manual_region: Option<Rect>,
    request: CaptureRequest,
    fallback_reason: Option<String>,
) -> Result<CaptureResult, String> {
    match capture_selected_window_with_request(query, request) {
        Ok(mut result) => {
            result.fallback_reason =
                combine_fallback_reasons(fallback_reason, result.fallback_reason);
            Ok(result)
        }
        Err(window_error) => match manual_region {
            Some(region) => match capture_manual_region_with_request(region, request) {
                Ok(mut result) => {
                    result.fallback_reason =
                        combine_fallback_reasons(fallback_reason, result.fallback_reason);
                    Ok(result)
                }
                Err(manual_error) => Err(format!(
                    "window capture failed ({window_error}); manual region capture also failed ({manual_error})"
                )),
            },
            None => Err(window_error),
        },
    }
}

fn capture_native_or_manual(
    query: &WindowQuery,
    manual_region: Option<Rect>,
    request: CaptureRequest,
    fallback_reason: Option<String>,
) -> Result<CaptureResult, String> {
    match capture_selected_window_native_with_request(query, request) {
        Ok(mut result) => {
            result.fallback_reason =
                combine_fallback_reasons(fallback_reason, result.fallback_reason);
            Ok(result)
        }
        Err(window_error) => match manual_region {
            Some(region) => match capture_manual_region_native_with_request(region, request) {
                Ok(mut result) => {
                    result.fallback_reason =
                        combine_fallback_reasons(fallback_reason, result.fallback_reason);
                    Ok(result)
                }
                Err(manual_error) => Err(format!(
                    "native window capture failed ({window_error}); native manual region capture also failed ({manual_error})"
                )),
            },
            None => Err(window_error),
        },
    }
}

pub fn capture_selected_window_native(query: &WindowQuery) -> Result<CaptureResult, String> {
    capture_selected_window_native_with_request(query, CaptureRequest::Full)
}

pub fn capture_selected_window_native_with_request(
    query: &WindowQuery,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    let selection = discover_x11_windows(query)?;
    let selected = selection.selected.ok_or_else(|| {
        if selection.candidates.is_empty() {
            "no X11 client windows were discovered".to_string()
        } else {
            "no X11 window matched the requested query".to_string()
        }
    })?;
    capture_window_candidate_native_with_request(selected, request)
}

pub fn capture_window_candidate_native_with_request(
    selected: WindowCandidate,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    let geometry = selected.info.rect;
    let (requested_capture_rect, actual_capture_rect, roi_origin, capture_scope) =
        native_capture_metadata(geometry, request)?;
    let native = native_x11::capture_drawable(
        selected.info.id,
        actual_capture_rect.x,
        actual_capture_rect.y,
        actual_capture_rect.width,
        actual_capture_rect.height,
    )?;
    Ok(CaptureResult {
        backend: CaptureBackend::X11Native,
        capture_method: native.method,
        fallback_reason: native.fallback_reason,
        source: CaptureSource::Window(selected),
        geometry,
        scale: ScaleFactor::one(),
        requested_capture_rect,
        actual_capture_rect,
        roi_origin,
        capture_scope,
        overlay_window: OverlayWindowState::Ignored,
        frame: native.frame,
    })
}

pub fn capture_manual_region_native(region: Rect) -> Result<CaptureResult, String> {
    capture_manual_region_native_with_request(region, CaptureRequest::Full)
}

pub fn capture_manual_region_native_with_request(
    region: Rect,
    request: CaptureRequest,
) -> Result<CaptureResult, String> {
    validate_rect(region)?;
    if region.x < 0 || region.y < 0 {
        return Err("manual region cannot start outside the root window".to_string());
    }
    let full_rect = reference_full_rect(region);
    let (requested_capture_rect, actual_capture_rect, roi_origin, capture_scope) =
        native_capture_metadata(region, request)?;
    let global_rect = Rect {
        x: region.x + actual_capture_rect.x,
        y: region.y + actual_capture_rect.y,
        width: actual_capture_rect.width,
        height: actual_capture_rect.height,
    };
    let native = native_x11::capture_root_region(global_rect)?;
    Ok(CaptureResult {
        backend: CaptureBackend::X11Native,
        capture_method: native.method,
        fallback_reason: native.fallback_reason,
        source: CaptureSource::ManualRegion(region),
        geometry: region,
        scale: ScaleFactor::one(),
        requested_capture_rect: if requested_capture_rect.width == 0 {
            full_rect
        } else {
            requested_capture_rect
        },
        actual_capture_rect,
        roi_origin,
        capture_scope,
        overlay_window: OverlayWindowState::Ignored,
        frame: native.frame,
    })
}

pub fn resolve_capture_reference(
    query: &WindowQuery,
    manual_region: Option<Rect>,
) -> Result<CaptureReference, String> {
    let query = query.clone().with_default_diablo_title();
    match discover_x11_windows(&query) {
        Ok(selection) => match selection.selected {
            Some(selected) => Ok(CaptureReference {
                geometry: selected.info.rect,
                source: CaptureSource::Window(selected),
            }),
            None => match manual_region {
                Some(region) => Ok(CaptureReference {
                    geometry: region,
                    source: CaptureSource::ManualRegion(region),
                }),
                None if selection.candidates.is_empty() => {
                    Err("no X11 client windows were discovered".to_string())
                }
                None => Err("no X11 window matched the requested query".to_string()),
            },
        },
        Err(window_error) => match manual_region {
            Some(region) => Ok(CaptureReference {
                geometry: region,
                source: CaptureSource::ManualRegion(region),
            }),
            None => Err(window_error),
        },
    }
}

fn command_capture_metadata(
    geometry: Rect,
    request: CaptureRequest,
) -> Result<(Rect, Rect, RoiOrigin, CaptureScopeResult, Option<String>), String> {
    let full_rect = reference_full_rect(geometry);
    match request {
        CaptureRequest::Full => Ok((
            full_rect,
            full_rect,
            RoiOrigin::FullFrame,
            CaptureScopeResult::FullFrame,
            None,
        )),
        CaptureRequest::WindowRelativeRoi(rect) => {
            validate_rect(rect)?;
            Ok((
                rect,
                full_rect,
                RoiOrigin::WindowRelative,
                CaptureScopeResult::FullFrameFallback,
                Some("x11-command ROI capture not supported".to_string()),
            ))
        }
        CaptureRequest::ScreenGlobalRoi(rect) => {
            validate_rect(rect)?;
            Ok((
                rect,
                full_rect,
                RoiOrigin::ScreenGlobal,
                CaptureScopeResult::FullFrameFallback,
                Some("x11-command ROI capture not supported".to_string()),
            ))
        }
    }
}

fn native_capture_metadata(
    geometry: Rect,
    request: CaptureRequest,
) -> Result<(Rect, Rect, RoiOrigin, CaptureScopeResult), String> {
    let full_rect = reference_full_rect(geometry);
    match request {
        CaptureRequest::Full => Ok((
            full_rect,
            full_rect,
            RoiOrigin::FullFrame,
            CaptureScopeResult::FullFrame,
        )),
        CaptureRequest::WindowRelativeRoi(rect) => {
            let actual = clamp_rect_to_reference(rect, geometry)?;
            Ok((
                rect,
                actual,
                RoiOrigin::WindowRelative,
                CaptureScopeResult::Roi,
            ))
        }
        CaptureRequest::ScreenGlobalRoi(rect) => {
            let relative = Rect {
                x: rect.x - geometry.x,
                y: rect.y - geometry.y,
                width: rect.width,
                height: rect.height,
            };
            let actual = clamp_rect_to_reference(relative, geometry)?;
            Ok((
                rect,
                actual,
                RoiOrigin::ScreenGlobal,
                CaptureScopeResult::Roi,
            ))
        }
    }
}

pub fn reference_full_rect(geometry: Rect) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: geometry.width,
        height: geometry.height,
    }
}

pub fn clamp_rect_to_reference(rect: Rect, geometry: Rect) -> Result<Rect, String> {
    validate_rect(rect)?;
    validate_rect(geometry)?;
    let max_x = i64::from(geometry.width);
    let max_y = i64::from(geometry.height);
    let start_x = i64::from(rect.x).clamp(0, max_x);
    let start_y = i64::from(rect.y).clamp(0, max_y);
    let end_x = i64::from(rect.x)
        .saturating_add(i64::from(rect.width))
        .clamp(0, max_x);
    let end_y = i64::from(rect.y)
        .saturating_add(i64::from(rect.height))
        .clamp(0, max_y);
    if end_x <= start_x || end_y <= start_y {
        return Err(format!(
            "ROI {} does not intersect reference frame {}",
            rect_to_string(rect),
            rect_to_string(reference_full_rect(geometry))
        ));
    }
    Ok(Rect {
        x: i32::try_from(start_x).map_err(|error| error.to_string())?,
        y: i32::try_from(start_y).map_err(|error| error.to_string())?,
        width: u32::try_from(end_x - start_x).map_err(|error| error.to_string())?,
        height: u32::try_from(end_y - start_y).map_err(|error| error.to_string())?,
    })
}

fn combine_fallback_reasons(primary: Option<String>, secondary: Option<String>) -> Option<String> {
    match (primary, secondary) {
        (Some(primary), Some(secondary)) => Some(format!("{primary}; {secondary}")),
        (Some(primary), None) => Some(primary),
        (None, Some(secondary)) => Some(secondary),
        (None, None) => None,
    }
}

/// Read the pointer position in-process (`XQueryPointer` on a persistent
/// per-thread display connection). The live loop polls this every frame, so
/// it must not spawn a helper process.
pub fn read_cursor_state(reference_rect: Option<Rect>) -> Result<CursorState, String> {
    let global = native_x11::query_pointer()?;
    Ok(cursor_state_from_global(global, reference_rect))
}

pub fn cursor_state_from_global(global: Point, reference_rect: Option<Rect>) -> CursorState {
    let (window_relative, inside_reference_rect) = match reference_rect {
        Some(rect) => {
            let relative = Point {
                x: global.x - rect.x,
                y: global.y - rect.y,
            };
            let inside = relative.x >= 0
                && relative.y >= 0
                && u32::try_from(relative.x)
                    .ok()
                    .is_some_and(|x| x < rect.width)
                && u32::try_from(relative.y)
                    .ok()
                    .is_some_and(|y| y < rect.height);
            (Some(relative), Some(inside))
        }
        None => (None, None),
    };

    CursorState {
        global,
        window_relative,
        inside_reference_rect,
        scale: ScaleFactor::one(),
    }
}

pub fn parse_manual_region(value: &str) -> Result<Rect, String> {
    let normalized = value.replace(['x', 'X', ':'], ",");
    let parts = normalized
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(format!(
            "manual region must have four values x,y,width,height; got '{value}'"
        ));
    }
    let x = parts[0]
        .parse::<i32>()
        .map_err(|error| format!("invalid manual region x '{}': {error}", parts[0]))?;
    let y = parts[1]
        .parse::<i32>()
        .map_err(|error| format!("invalid manual region y '{}': {error}", parts[1]))?;
    let width = parts[2]
        .parse::<u32>()
        .map_err(|error| format!("invalid manual region width '{}': {error}", parts[2]))?;
    let height = parts[3]
        .parse::<u32>()
        .map_err(|error| format!("invalid manual region height '{}': {error}", parts[3]))?;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    validate_rect(rect)?;
    Ok(rect)
}

pub fn crop_frame(frame: &CaptureFrame, region: Rect) -> Result<CaptureFrame, String> {
    validate_rect(region)?;
    if region.x < 0 || region.y < 0 {
        return Err("manual region cannot start outside the captured root frame".to_string());
    }
    let start_x = u32::try_from(region.x).map_err(|error| error.to_string())?;
    let start_y = u32::try_from(region.y).map_err(|error| error.to_string())?;
    let end_x = start_x
        .checked_add(region.width)
        .ok_or_else(|| "manual region width overflows frame bounds".to_string())?;
    let end_y = start_y
        .checked_add(region.height)
        .ok_or_else(|| "manual region height overflows frame bounds".to_string())?;
    if end_x > frame.width || end_y > frame.height {
        return Err(format!(
            "manual region {} exceeds captured frame {}x{}",
            rect_to_string(region),
            frame.width,
            frame.height
        ));
    }

    let width = usize::try_from(region.width).map_err(|error| error.to_string())?;
    let height = usize::try_from(region.height).map_err(|error| error.to_string())?;
    let start_x_usize = usize::try_from(start_x).map_err(|error| error.to_string())?;
    let start_y_usize = usize::try_from(start_y).map_err(|error| error.to_string())?;
    let mut pixels_bgra = vec![0u8; width * height * 4];

    for row in 0..height {
        let src_start = (start_y_usize + row)
            .checked_mul(frame.stride)
            .and_then(|offset| offset.checked_add(start_x_usize * 4))
            .ok_or_else(|| "source frame offset overflow".to_string())?;
        let src_end = src_start
            .checked_add(width * 4)
            .ok_or_else(|| "source frame row overflow".to_string())?;
        let dst_start = row
            .checked_mul(width * 4)
            .ok_or_else(|| "destination frame offset overflow".to_string())?;
        let dst_end = dst_start
            .checked_add(width * 4)
            .ok_or_else(|| "destination frame row overflow".to_string())?;
        pixels_bgra[dst_start..dst_end].copy_from_slice(&frame.pixels_bgra[src_start..src_end]);
    }

    Ok(CaptureFrame {
        width: region.width,
        height: region.height,
        stride: width * 4,
        pixels_bgra,
    })
}

pub fn parse_xwd_frame(bytes: &[u8]) -> Result<CaptureFrame, String> {
    if bytes.len() < 100 {
        return Err("XWD data is too short to contain a file header".to_string());
    }
    let header_order = detect_xwd_header_order(bytes)?;
    let header_size = read_u32_at(bytes, 0, header_order)?;
    let file_version = read_u32_at(bytes, 4, header_order)?;
    let pixmap_format = read_u32_at(bytes, 8, header_order)?;
    let pixmap_depth = read_u32_at(bytes, 12, header_order)?;
    let width = read_u32_at(bytes, 16, header_order)?;
    let height = read_u32_at(bytes, 20, header_order)?;
    let byte_order = read_u32_at(bytes, 28, header_order)?;
    let bits_per_pixel = read_u32_at(bytes, 44, header_order)?;
    let bytes_per_line = read_u32_at(bytes, 48, header_order)?;
    let red_mask = read_u32_at(bytes, 56, header_order)?;
    let green_mask = read_u32_at(bytes, 60, header_order)?;
    let blue_mask = read_u32_at(bytes, 64, header_order)?;
    let ncolors = read_u32_at(bytes, 76, header_order)?;

    if file_version != XWD_FILE_VERSION {
        return Err(format!("unsupported XWD file version {file_version}"));
    }
    if pixmap_format != XWD_Z_PIXMAP {
        return Err(format!("unsupported XWD pixmap format {pixmap_format}"));
    }
    if pixmap_depth == 0 {
        return Err("XWD pixmap depth must be non-zero".to_string());
    }

    let color_map_bytes = usize::try_from(ncolors)
        .map_err(|error| error.to_string())?
        .checked_mul(12)
        .ok_or_else(|| "XWD color map size overflow".to_string())?;
    let image_offset = usize::try_from(header_size)
        .map_err(|error| error.to_string())?
        .checked_add(color_map_bytes)
        .ok_or_else(|| "XWD image offset overflow".to_string())?;
    let image_data = bytes.get(image_offset..).ok_or_else(|| {
        format!(
            "XWD data is truncated: image data starts at byte {image_offset}, file has {}",
            bytes.len()
        )
    })?;
    let layout = ZPixmapLayout {
        width,
        height,
        stride: usize::try_from(bytes_per_line).map_err(|error| error.to_string())?,
        bits_per_pixel,
        byte_order,
        red_mask,
        green_mask,
        blue_mask,
    };
    zpixmap_to_bgra(image_data, &layout).map_err(|error| format!("XWD {error}"))
}

pub fn rect_to_string(rect: Rect) -> String {
    format!("{},{},{},{}", rect.x, rect.y, rect.width, rect.height)
}

pub fn format_window_id(id: u64) -> String {
    format!("0x{id:x}")
}

fn validate_rect(rect: Rect) -> Result<(), String> {
    if rect.width == 0 || rect.height == 0 {
        return Err(format!(
            "rectangle width and height must be non-zero; got {}",
            rect_to_string(rect)
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteOrder {
    Little,
    Big,
}

fn detect_xwd_header_order(bytes: &[u8]) -> Result<ByteOrder, String> {
    let little_header_size = read_u32_at(bytes, 0, ByteOrder::Little)?;
    let little_version = read_u32_at(bytes, 4, ByteOrder::Little)?;
    if little_version == XWD_FILE_VERSION && plausible_header_size(little_header_size, bytes.len())
    {
        return Ok(ByteOrder::Little);
    }

    let big_header_size = read_u32_at(bytes, 0, ByteOrder::Big)?;
    let big_version = read_u32_at(bytes, 4, ByteOrder::Big)?;
    if big_version == XWD_FILE_VERSION && plausible_header_size(big_header_size, bytes.len()) {
        return Ok(ByteOrder::Big);
    }

    Err("could not detect XWD header byte order".to_string())
}

fn plausible_header_size(header_size: u32, total_len: usize) -> bool {
    usize::try_from(header_size)
        .ok()
        .is_some_and(|size| size >= 100 && size <= total_len)
}

fn read_u32_at(bytes: &[u8], offset: usize, order: ByteOrder) -> Result<u32, String> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| "u32 offset overflow".to_string())?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| format!("missing u32 at XWD offset {offset}"))?;
    let array = [slice[0], slice[1], slice[2], slice[3]];
    Ok(match order {
        ByteOrder::Little => u32::from_le_bytes(array),
        ByteOrder::Big => u32::from_be_bytes(array),
    })
}

fn read_pixel_value(bytes: &[u8], byte_order: u32) -> Result<u32, String> {
    let value = match (bytes.len(), byte_order) {
        (2, BYTE_ORDER_LSB_FIRST) => u16::from_le_bytes([bytes[0], bytes[1]]) as u32,
        (2, BYTE_ORDER_MSB_FIRST) => u16::from_be_bytes([bytes[0], bytes[1]]) as u32,
        (3, BYTE_ORDER_LSB_FIRST) => {
            u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
        }
        (3, BYTE_ORDER_MSB_FIRST) => {
            (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2])
        }
        (4, BYTE_ORDER_LSB_FIRST) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        (4, BYTE_ORDER_MSB_FIRST) => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        (len, order) => {
            return Err(format!(
                "unsupported XWD pixel byte length {len} with byte order {order}"
            ))
        }
    };
    Ok(value)
}

fn extract_masked_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let raw = (pixel & mask) >> shift;
    if bits >= 8 {
        (raw >> (bits - 8)) as u8
    } else {
        let max = (1u32 << bits) - 1;
        ((raw * 255) / max) as u8
    }
}

/// Pixel layout of a ZPixmap image: an `XImage` from Xlib/MIT-SHM or the
/// body of an XWD file.
#[derive(Debug, Clone, Copy)]
struct ZPixmapLayout {
    width: u32,
    height: u32,
    /// Bytes from the start of one source row to the next (`bytes_per_line`).
    stride: usize,
    /// 16, 24 or 32.
    bits_per_pixel: u32,
    /// [`BYTE_ORDER_LSB_FIRST`] or [`BYTE_ORDER_MSB_FIRST`].
    byte_order: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
}

impl ZPixmapLayout {
    /// Validate the layout and return how many source bytes the conversion
    /// reads: `height - 1` full strides plus the pixel bytes of the last row.
    ///
    /// Rejects zero dimensions, unsupported pixel formats, a stride shorter
    /// than one row of pixels (rows would overlap and the last row would run
    /// past a `stride * height` buffer) and any size that overflows `usize`.
    fn required_len(&self) -> Result<usize, String> {
        if self.width == 0 || self.height == 0 {
            return Err(format!(
                "image dimensions must be non-zero, got {}x{}",
                self.width, self.height
            ));
        }
        if !matches!(self.bits_per_pixel, 16 | 24 | 32) {
            return Err(format!(
                "unsupported bits-per-pixel {}",
                self.bits_per_pixel
            ));
        }
        if !matches!(self.byte_order, BYTE_ORDER_LSB_FIRST | BYTE_ORDER_MSB_FIRST) {
            return Err(format!("unsupported byte order {}", self.byte_order));
        }
        let row_bytes = self.row_bytes()?;
        if self.stride < row_bytes {
            return Err(format!(
                "row stride {} is shorter than the {row_bytes} pixel bytes of a {}-pixel row",
                self.stride, self.width
            ));
        }
        let rows_before_last =
            usize::try_from(self.height - 1).map_err(|error| error.to_string())?;
        rows_before_last
            .checked_mul(self.stride)
            .and_then(|offset| offset.checked_add(row_bytes))
            .ok_or_else(|| "image size overflows usize".to_string())
    }

    fn bytes_per_pixel(&self) -> usize {
        (self.bits_per_pixel / 8) as usize
    }

    fn row_bytes(&self) -> Result<usize, String> {
        usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(self.bytes_per_pixel()))
            .ok_or_else(|| "image row size overflows usize".to_string())
    }
}

/// Convert ZPixmap pixel data to a tightly packed BGRA frame (alpha 255).
///
/// `data` must hold at least [`ZPixmapLayout::required_len`] bytes; every
/// access is bounds-checked. The output (`width * height * 4` bytes) is at
/// most twice the validated input size, so a forged header cannot trigger an
/// allocation larger than the data it came with.
fn zpixmap_to_bgra(data: &[u8], layout: &ZPixmapLayout) -> Result<CaptureFrame, String> {
    let required = layout.required_len()?;
    if data.len() < required {
        return Err(format!(
            "image data is truncated: need at least {required} bytes, got {}",
            data.len()
        ));
    }
    let width = usize::try_from(layout.width).map_err(|error| error.to_string())?;
    let height = usize::try_from(layout.height).map_err(|error| error.to_string())?;
    let dst_stride = width
        .checked_mul(4)
        .ok_or_else(|| "BGRA row size overflows usize".to_string())?;
    let dst_len = dst_stride
        .checked_mul(height)
        .ok_or_else(|| "BGRA frame size overflows usize".to_string())?;
    let bytes_per_pixel = layout.bytes_per_pixel();
    let row_bytes = layout.row_bytes()?;

    let mut pixels_bgra = vec![0u8; dst_len];
    for (row, dst_row) in pixels_bgra.chunks_exact_mut(dst_stride).enumerate() {
        // row * stride + row_bytes <= required <= data.len().
        let src_row = &data[row * layout.stride..][..row_bytes];
        for (src, dst) in src_row
            .chunks_exact(bytes_per_pixel)
            .zip(dst_row.as_chunks_mut::<4>().0)
        {
            let pixel = read_pixel_value(src, layout.byte_order)?;
            *dst = [
                extract_masked_channel(pixel, layout.blue_mask),
                extract_masked_channel(pixel, layout.green_mask),
                extract_masked_channel(pixel, layout.red_mask),
                255,
            ];
        }
    }

    Ok(CaptureFrame {
        width: layout.width,
        height: layout.height,
        stride: dst_stride,
        pixels_bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The signal wakes the waiter the moment a newer frame
    /// generation is recorded, and times out when none arrives.
    #[test]
    fn frame_signal_wakes_on_bump_and_times_out_without_one() {
        let signal = FrameSignal::new();
        let seen = signal.generation();
        assert_eq!(seen, 0);

        // No frame: the bounded wait returns None (not immediately).
        let started = Instant::now();
        assert_eq!(signal.wait_for(seen, Duration::from_millis(40)), None);
        assert!(started.elapsed() >= Duration::from_millis(40));

        // A bump from the pump thread wakes the waiter promptly.
        let waiter = signal.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            waiter.bump()
        });
        let generation = signal
            .wait_for(seen, Duration::from_secs(2))
            .unwrap_or_else(|| panic!("the bump must wake the waiter"));
        assert_eq!(generation, handle.join().unwrap_or(0));
        assert_eq!(signal.generation(), 1);
    }

    /// Overrun semantics: a zero-timeout wait polls once without
    /// blocking, and each bump is one generation.
    #[test]
    fn frame_signal_zero_timeout_polls_and_generations_advance() {
        let signal = FrameSignal::new();
        assert_eq!(signal.wait_for(0, Duration::ZERO), None);

        signal.bump();
        signal.bump();
        assert_eq!(signal.wait_for(0, Duration::ZERO), Some(2));
        assert_eq!(signal.wait_for(2, Duration::ZERO), None);
    }

    /// The in-process pointer query answers on a private Xvfb server. Xvfb
    /// centres the pointer on the root window at start-up.
    #[test]
    fn native_pointer_query_reads_the_root_pointer() {
        let server = crate::test_xvfb::Xvfb::start("640x480x24");
        let point = native_x11::query_pointer_on(server.display())
            .unwrap_or_else(|error| panic!("pointer query failed: {error}"));
        assert_eq!(point, Point { x: 320, y: 240 });
    }

    /// A capture of a window that is gone fails cleanly. The XShm attempt
    /// errors first and drops its session, and the XGetImage fallback then
    /// runs on the same display connection: the session must not have closed
    /// it (that was a use-after-free crash when the game window closed).
    #[test]
    fn capturing_a_vanished_window_fails_without_crashing() {
        let server = crate::test_xvfb::Xvfb::start("640x480x24");
        let display = server.display();
        // An id the server never handed out stands for a destroyed window.
        let result =
            native_x11::capture_with_fresh_session_on(Some(display), 0x3f0_0001, 0, 0, 64, 64);
        let concurrent = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..4).map(|_| scope.spawn(|| {
                for _ in 0..20 {
                    let failed = native_x11::capture_with_fresh_session_on(
                        Some(display), 0x3f0_0001, 0, 0, 64, 64,
                    );
                    if !matches!(failed, Err(ref error) if error.contains("XGetImage failed")) {
                        return Err("a vanished window did not report its capture failure".to_string());
                    }
                    let point = native_x11::query_pointer_on(display)?;
                    if point != (Point { x: 320, y: 240 }) {
                        return Err("concurrent capture errors changed the pointer result".to_string());
                    }
                }
                Ok::<_, String>(())
            })).collect();
            threads
                .into_iter()
                .map(|thread| thread.join())
                .collect::<Vec<_>>()
        });
        drop(server);
        let error = match result {
            Ok(_) => panic!("capturing a vanished window must fail"),
            Err(error) => error,
        };
        assert!(error.contains("XGetImage failed"), "{error}");
        for result in concurrent {
            assert!(
                matches!(result, Ok(Ok(()))),
                "concurrent Xlib request failed: {result:?}"
            );
        }
    }

    #[derive(Default)]
    struct FakeRunner {
        responses: BTreeMap<String, Result<Vec<u8>, String>>,
    }

    impl FakeRunner {
        fn with_bytes(mut self, program: &str, args: &[&str], response: Vec<u8>) -> Self {
            self.responses
                .insert(command_key(program, args), Ok(response));
            self
        }
    }

    impl X11CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<Vec<u8>, String> {
            let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
            self.responses
                .get(&command_key(program, &refs))
                .cloned()
                .unwrap_or_else(|| Err(format!("missing fake response for {program} {args:?}")))
        }
    }

    fn command_key(program: &str, args: &[&str]) -> String {
        format!("{program} {}", args.join(" "))
    }

    #[test]
    fn converts_cursor_to_window_relative_coordinates() {
        let cursor = cursor_state_from_global(
            Point { x: 110, y: 250 },
            Some(Rect {
                x: 100,
                y: 200,
                width: 800,
                height: 600,
            }),
        );
        assert_eq!(cursor.window_relative, Some(Point { x: 10, y: 50 }));
        assert_eq!(cursor.inside_reference_rect, Some(true));
        assert_eq!(cursor.scale.as_decimal_string(), "1.0");
    }

    #[test]
    fn parses_manual_region_from_common_formats() {
        assert_eq!(
            parse_manual_region("10,20,300,400"),
            Ok(Rect {
                x: 10,
                y: 20,
                width: 300,
                height: 400,
            })
        );
        assert_eq!(
            parse_manual_region("10:20:300x400"),
            Ok(Rect {
                x: 10,
                y: 20,
                width: 300,
                height: 400,
            })
        );
        assert!(parse_manual_region("10,20,0,400").is_err());
    }

    #[test]
    fn parses_capture_backend_names() {
        assert_eq!(CaptureBackend::parse("auto"), Ok(CaptureBackend::Auto));
        assert_eq!(
            CaptureBackend::parse("x11-native"),
            Ok(CaptureBackend::X11Native)
        );
        assert_eq!(
            CaptureBackend::parse("x11-command"),
            Ok(CaptureBackend::X11Command)
        );
        assert!(CaptureBackend::parse("wayland").is_err());
    }

    #[test]
    fn parses_capture_scope_names() {
        assert_eq!(CaptureScope::parse("auto"), Ok(CaptureScope::Auto));
        assert_eq!(CaptureScope::parse("full"), Ok(CaptureScope::Full));
        assert_eq!(CaptureScope::parse("roi"), Ok(CaptureScope::Roi));
        assert!(CaptureScope::parse("bad").is_err());
    }

    #[test]
    fn capture_method_display_reports_native_xshm() {
        assert_eq!(CaptureMethod::X11Command.to_string(), "x11-command");
        assert_eq!(
            CaptureMethod::X11NativeXGetImage.to_string(),
            "x11-native-xgetimage"
        );
        assert_eq!(CaptureMethod::X11NativeXShm.to_string(), "x11-native-xshm");
    }

    #[test]
    fn overlay_window_state_display_reports_excluded() {
        assert_eq!(OverlayWindowState::Excluded.to_string(), "excluded");
    }

    #[test]
    fn clamps_relative_roi_to_reference_bounds() {
        let clamped = clamp_rect_to_reference(
            Rect {
                x: -10,
                y: 20,
                width: 100,
                height: 200,
            },
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 100,
            },
        )
        .unwrap_or_else(|error| panic!("test setup failed: {error}"));

        assert_eq!(
            clamped,
            Rect {
                x: 0,
                y: 20,
                width: 80,
                height: 80,
            }
        );
    }

    #[test]
    fn native_roi_metadata_honors_request_dimensions_at_boundary() {
        let (requested, actual, origin, scope) = native_capture_metadata(
            Rect {
                x: 10,
                y: 20,
                width: 1920,
                height: 1080,
            },
            CaptureRequest::WindowRelativeRoi(Rect {
                x: 240,
                y: 0,
                width: 960,
                height: 1080,
            }),
        )
        .unwrap_or_else(|error| panic!("test setup failed: {error}"));

        assert_eq!(requested.width, 960);
        assert_eq!(requested.height, 1080);
        assert_eq!(actual, requested);
        assert_eq!(origin, RoiOrigin::WindowRelative);
        assert_eq!(scope, CaptureScopeResult::Roi);
    }

    #[test]
    fn x11_command_roi_metadata_marks_full_frame_fallback() {
        let (requested, actual, origin, scope, fallback) = command_capture_metadata(
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            CaptureRequest::WindowRelativeRoi(Rect {
                x: 240,
                y: 0,
                width: 960,
                height: 1080,
            }),
        )
        .unwrap_or_else(|error| panic!("test setup failed: {error}"));

        assert_eq!(requested.width, 960);
        assert_eq!(actual.width, 1920);
        assert_eq!(origin, RoiOrigin::WindowRelative);
        assert_eq!(scope, CaptureScopeResult::FullFrameFallback);
        assert_eq!(
            fallback.as_deref(),
            Some("x11-command ROI capture not supported")
        );
    }

    #[test]
    fn parses_little_endian_xwd_frame() {
        let bytes = synthetic_xwd_2x1();
        let frame = parse_xwd_frame(&bytes).unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.stride, 8);
        assert_eq!(frame.pixels_bgra, vec![0, 0, 255, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn captures_manual_region_by_cropping_root_xwd() {
        let runner =
            FakeRunner::default().with_bytes("xwd", &["-silent", "-root"], synthetic_xwd_2x1());

        let result = capture_manual_region_with(
            &runner,
            Rect {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
        )
        .unwrap_or_else(|error| panic!("capture failed: {error}"));

        assert_eq!(result.frame.width, 1);
        assert_eq!(result.frame.pixels_bgra, vec![0, 255, 0, 255]);
    }

    #[test]
    fn portal_stream_reference_is_refused_by_the_x11_capture_paths() {
        // The X11-internal backends cannot read a PipeWire stream: a portal
        // stream reference is refused by name without any X11 discovery or
        // capture attempt (the system runner would spawn subprocesses).
        let reference = CaptureReference {
            source: CaptureSource::PortalStream(Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            geometry: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        };
        for backend in [CaptureBackend::X11Native, CaptureBackend::X11Command] {
            let error = match capture_reference_with_backend_and_request(
                &reference,
                backend,
                CaptureRequest::Full,
            ) {
                Ok(_) => panic!("the portal stream reference must be refused ({backend})"),
                Err(error) => error,
            };
            assert!(
                error.contains("Wayland frame source"),
                "refusal must name the capturing path ({backend}): {error}"
            );
        }
    }

    fn synthetic_xwd_2x1() -> Vec<u8> {
        let mut data = Vec::new();
        let fields = [
            100u32, // header size
            XWD_FILE_VERSION,
            XWD_Z_PIXMAP,
            24,
            2,
            1,
            0,
            BYTE_ORDER_LSB_FIRST,
            32,
            BYTE_ORDER_LSB_FIRST,
            32,
            32,
            8,
            4,
            0x00ff_0000,
            0x0000_ff00,
            0x0000_00ff,
            8,
            256,
            0,
            2,
            1,
            0,
            0,
            0,
        ];
        for field in fields {
            data.extend_from_slice(&field.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0, 255, 0]);
        data.extend_from_slice(&[0, 255, 0, 0]);
        data
    }

    /// 32-bpp `0x00RRGGBB` layout with the given stride.
    fn rgb32_layout(width: u32, height: u32, stride: usize, byte_order: u32) -> ZPixmapLayout {
        ZPixmapLayout {
            width,
            height,
            stride,
            bits_per_pixel: 32,
            byte_order,
            red_mask: 0x00ff_0000,
            green_mask: 0x0000_ff00,
            blue_mask: 0x0000_00ff,
        }
    }

    /// Distinct, position-derived BGR values for pixel (x, y).
    fn sample_bgr(x: usize, y: usize) -> [u8; 3] {
        [(x * 7 + y) as u8, (y * 13 + 1) as u8, (x ^ (y * 3)) as u8]
    }

    /// LSB-first 32-bpp rows of `sample_bgr`, padded to `stride` with 0xEE
    /// so any read of padding shows up in the output.
    fn rgb32_data(width: usize, height: usize, stride: usize) -> Vec<u8> {
        let mut data = vec![0xEE; stride * height];
        for y in 0..height {
            for x in 0..width {
                let [b, g, r] = sample_bgr(x, y);
                data[y * stride + x * 4..][..4].copy_from_slice(&[b, g, r, 0]);
            }
        }
        data
    }

    fn expected_bgra(width: usize, height: usize) -> Vec<u8> {
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let [b, g, r] = sample_bgr(x, y);
                [b, g, r, 255]
            })
            .collect()
    }

    fn convert(data: &[u8], layout: ZPixmapLayout) -> Result<CaptureFrame, String> {
        zpixmap_to_bgra(data, &layout)
    }

    #[test]
    fn zpixmap_converts_odd_dimensions_and_skips_row_padding() {
        // (width, height, stride): tight rows, padded rows, odd widths.
        for (width, height, stride) in
            [(1, 1, 4), (1, 1, 8), (3, 2, 12), (3, 2, 16), (641, 3, 2568)]
        {
            let data = rgb32_data(width, height, stride);
            let frame = convert(
                &data,
                rgb32_layout(width as u32, height as u32, stride, BYTE_ORDER_LSB_FIRST),
            )
            .unwrap_or_else(|error| panic!("{width}x{height}/{stride}: {error}"));
            assert_eq!((frame.width, frame.height), (width as u32, height as u32));
            assert_eq!(frame.stride, width * 4);
            assert_eq!(
                frame.pixels_bgra,
                expected_bgra(width, height),
                "{width}x{height}/{stride}"
            );
        }
    }

    #[test]
    fn zpixmap_accepts_a_last_row_without_padding_and_rejects_one_byte_less() {
        let (width, height, stride) = (3, 2, 16);
        let data = rgb32_data(width, height, stride);
        let required = stride * (height - 1) + width * 4;
        let layout = rgb32_layout(width as u32, height as u32, stride, BYTE_ORDER_LSB_FIRST);
        assert_eq!(layout.required_len(), Ok(required));
        let frame = convert(&data[..required], layout)
            .unwrap_or_else(|error| panic!("minimal buffer must convert: {error}"));
        assert_eq!(frame.pixels_bgra, expected_bgra(width, height));
        let error = match convert(&data[..required - 1], layout) {
            Ok(_) => panic!("a buffer one byte short must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("truncated"), "{error}");
    }

    #[test]
    fn zpixmap_rejects_a_stride_shorter_than_a_row() {
        // 2 px * 4 bytes = 8 bytes per row; a 4-byte stride would make rows
        // overlap and the last row run past a stride * height buffer.
        let data = vec![0u8; 64];
        let error = match convert(&data, rgb32_layout(2, 2, 4, BYTE_ORDER_LSB_FIRST)) {
            Ok(_) => panic!("short stride must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("row stride 4"), "{error}");
    }

    #[test]
    fn zpixmap_rejects_forged_sizes_without_allocating() {
        // Zero stride with huge dimensions passes a size check without a
        // stride bound and then allocates width * height * 4 bytes.
        for layout in [
            rgb32_layout(u32::MAX, u32::MAX, 0, BYTE_ORDER_LSB_FIRST),
            rgb32_layout(1 << 20, 1 << 20, 0, BYTE_ORDER_LSB_FIRST),
            rgb32_layout(u32::MAX, u32::MAX, usize::MAX, BYTE_ORDER_LSB_FIRST),
            rgb32_layout(2, u32::MAX, usize::MAX / 2, BYTE_ORDER_LSB_FIRST),
        ] {
            assert!(convert(&[0u8; 16], layout).is_err(), "{layout:?}");
        }
    }

    #[test]
    fn zpixmap_rejects_invalid_parameters() {
        let data = [0u8; 64];
        let base = rgb32_layout(2, 2, 8, BYTE_ORDER_LSB_FIRST);
        let cases = [
            ZPixmapLayout { width: 0, ..base },
            ZPixmapLayout { height: 0, ..base },
            ZPixmapLayout {
                bits_per_pixel: 8,
                ..base
            },
            ZPixmapLayout {
                bits_per_pixel: 0,
                ..base
            },
            ZPixmapLayout {
                bits_per_pixel: 64,
                ..base
            },
            ZPixmapLayout {
                byte_order: 2,
                ..base
            },
        ];
        for layout in cases {
            assert!(convert(&data, layout).is_err(), "{layout:?}");
        }
        assert!(convert(&data, base).is_ok());
    }

    #[test]
    fn zpixmap_reads_msb_first_and_24_bit_pixels() {
        // 32 bpp MSB-first: bytes 00 RR GG BB.
        let frame = convert(
            &[0x00, 0x11, 0x22, 0x33, 0x00, 0x44, 0x55, 0x66],
            rgb32_layout(2, 1, 8, BYTE_ORDER_MSB_FIRST),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            frame.pixels_bgra,
            [0x33, 0x22, 0x11, 255, 0x66, 0x55, 0x44, 255]
        );

        // 24 bpp packed, odd width 3 with a 12-byte stride (3 pad bytes).
        let layout = ZPixmapLayout {
            bits_per_pixel: 24,
            ..rgb32_layout(3, 2, 12, BYTE_ORDER_LSB_FIRST)
        };
        let mut data = vec![0xEE; 24];
        for (index, chunk) in [
            [1, 2, 3],
            [4, 5, 6],
            [7, 8, 9],
            [10, 11, 12],
            [13, 14, 15],
            [16, 17, 18],
        ]
        .iter()
        .enumerate()
        {
            let (x, y) = (index % 3, index / 3);
            data[y * 12 + x * 3..][..3].copy_from_slice(chunk);
        }
        let frame = convert(&data, layout).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            frame.pixels_bgra,
            [
                1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, //
                10, 11, 12, 255, 13, 14, 15, 255, 16, 17, 18, 255,
            ]
        );
    }

    #[test]
    fn zpixmap_scales_16_bit_565_and_10_bit_masks() {
        let rgb565 = ZPixmapLayout {
            bits_per_pixel: 16,
            red_mask: 0xf800,
            green_mask: 0x07e0,
            blue_mask: 0x001f,
            ..rgb32_layout(2, 1, 4, BYTE_ORDER_LSB_FIRST)
        };
        // Pixel 0: full red. Pixel 1: half green (32 of 63), full blue.
        let pixels: [u16; 2] = [0xf800, (32 << 5) | 0x1f];
        let lsb: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        let msb: Vec<u8> = pixels.iter().flat_map(|p| p.to_be_bytes()).collect();
        let expected = [0, 0, 255, 255, 255, 129, 0, 255];
        let frame = convert(&lsb, rgb565).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(frame.pixels_bgra, expected);
        let frame = convert(
            &msb,
            ZPixmapLayout {
                byte_order: BYTE_ORDER_MSB_FIRST,
                ..rgb565
            },
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(frame.pixels_bgra, expected);

        // 30-bit colour (depth 30): 10 bits per channel keep the top 8.
        let deep = ZPixmapLayout {
            red_mask: 0x3ff0_0000,
            green_mask: 0x000f_fc00,
            blue_mask: 0x0000_03ff,
            ..rgb32_layout(1, 1, 4, BYTE_ORDER_LSB_FIRST)
        };
        let pixel: u32 = (0x3ff << 20) | (0x200 << 10) | 0x004;
        let frame = convert(&pixel.to_le_bytes(), deep).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(frame.pixels_bgra, [0x01, 0x80, 0xff, 255]);

        // A zero mask yields a zero channel instead of a shift panic.
        let no_blue = ZPixmapLayout {
            blue_mask: 0,
            ..rgb32_layout(1, 1, 4, BYTE_ORDER_LSB_FIRST)
        };
        let frame =
            convert(&[0xff, 0x80, 0x40, 0], no_blue).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(frame.pixels_bgra, [0, 0x80, 0x40, 255]);
    }

    #[test]
    fn xwd_with_a_stride_shorter_than_a_row_is_rejected() {
        let mut bytes = synthetic_xwd_2x1();
        // bytes_per_line (offset 48): 4 < 2 px * 4 bytes.
        bytes[48..52].copy_from_slice(&4u32.to_le_bytes());
        let error = match parse_xwd_frame(&bytes) {
            Ok(_) => panic!("short XWD stride must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("row stride"), "{error}");
    }

    #[test]
    fn xwd_with_a_color_map_past_the_end_is_rejected() {
        let mut bytes = synthetic_xwd_2x1();
        // ncolors (offset 76): a color map larger than the file.
        bytes[76..80].copy_from_slice(&1000u32.to_le_bytes());
        let error = match parse_xwd_frame(&bytes) {
            Ok(_) => panic!("image offset past the end must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("truncated"), "{error}");
    }

    #[test]
    fn xwd_with_forged_huge_dimensions_and_zero_stride_is_rejected() {
        let mut bytes = synthetic_xwd_2x1();
        // width/height (offsets 16/20) forged huge with a zero stride.
        bytes[16..20].copy_from_slice(&(1u32 << 20).to_le_bytes());
        bytes[20..24].copy_from_slice(&(1u32 << 20).to_le_bytes());
        bytes[48..52].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_xwd_frame(&bytes).is_err());
    }
}
