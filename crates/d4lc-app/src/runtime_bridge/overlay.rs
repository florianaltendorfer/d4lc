//! Overlay descriptors, overlay status helpers and the live overlay start.

use super::events::{RuntimeEvent, RuntimeEventSink};
use crate::app_state::ParagonOverlayStateBundle;
use crate::app_state::{AppState, OverlayDescriptorState, OverlayTextState};
use d4lc_core::text_measure::TextMeasure;
use d4lc_overlay_x11::{OverlayBackend, OverlayFrame, OverlayStatus};
use d4lc_rules::{self as rules, RuleOverlayDescriptor};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Core conversion: AppState → live overlay descriptors
// ---------------------------------------------------------------------------

/// [`convert_descriptors_with`] measuring text with a fixed-advance double,
/// so descriptor tests do not depend on an installed font.
#[cfg(test)]
pub fn convert_descriptors(
    overlay_descriptor_state: Option<&OverlayDescriptorState>,
) -> Vec<RuleOverlayDescriptor> {
    convert_descriptors_with(
        overlay_descriptor_state,
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
        &ParagonDescriptorCache::default(),
    )
    .to_vec()
}

/// The paragon descriptors of the last evaluated state, kept only while the
/// font can measure. The board is rebuilt
/// only when its inputs (the projected state with the live navigation applied,
/// the layout with the window geometry and font size) differ from the last
/// build; an analysis change, a repeated hover and every frame in between
/// reuse the same shared list.
#[derive(Debug, Default)]
pub struct ParagonDescriptorCache {
    entry: Mutex<Option<(ParagonOverlayStateBundle, Arc<[RuleOverlayDescriptor]>)>>,
    builds: AtomicU64,
}

impl ParagonDescriptorCache {
    fn get_or_build(
        &self,
        bundle: &ParagonOverlayStateBundle,
        measure: &dyn TextMeasure,
    ) -> Arc<[RuleOverlayDescriptor]> {
        let mut entry = self
            .entry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached, descriptors)) = entry.as_ref() {
            if cached == bundle {
                return Arc::clone(descriptors);
            }
        }
        let built: Arc<[RuleOverlayDescriptor]> =
            rules::build_paragon_overlay_descriptors(&bundle.state, &bundle.layout, measure).into();
        self.builds.fetch_add(1, Ordering::Relaxed);
        // A list built while the font could not measure lacks its text
        // panels; it must not outlive the outage in the cache.
        if measure.measure("A", bundle.layout.font_px as f32).is_some() {
            *entry = Some((bundle.clone(), Arc::clone(&built)));
        }
        built
    }

    /// How often the board was actually built.
    #[cfg(test)]
    pub fn builds(&self) -> u64 {
        self.builds.load(Ordering::Relaxed)
    }
}

/// Convert app-owned `OverlayDescriptorState` into `Vec<RuleOverlayDescriptor>`
/// via `d4lc_rules::build_live_overlay_descriptors(...)`. `measure` sizes the
/// paragon panels from the overlay font.
///
/// This is the **single conversion point** called by the live loop.  When
/// `overlay_descriptor_state` is `None`, an empty descriptor list is returned
/// (matching the pre-startup / post-shutdown semantic where no highlight state
/// is loaded).
///
/// A paragon-only state (paragon mode replaces the markers) returns the
/// cache's shared list itself.
pub fn convert_descriptors_with(
    overlay_descriptor_state: Option<&OverlayDescriptorState>,
    measure: &dyn TextMeasure,
    paragon_cache: &ParagonDescriptorCache,
) -> Arc<[RuleOverlayDescriptor]> {
    // Paragon mode replaces the markers (OverlayHandler.cs:154-163).
    let rule_state = overlay_descriptor_state
        .filter(|state| !state.paragon_mode_active)
        .and_then(|state| state.rule.as_ref())
        .map(|rule| (&rule.result, &rule.settings, &rule.layout));

    let paragon = overlay_descriptor_state.and_then(|state| state.paragon.as_ref());
    if rule_state.is_none() {
        return paragon.map_or_else(
            || Arc::from(Vec::new()),
            |bundle| paragon_cache.get_or_build(bundle, measure),
        );
    }

    let paragon_state = paragon.map(|paragon| (&paragon.state, &paragon.layout));
    rules::build_live_overlay_descriptors(rule_state, paragon_state, measure).into()
}

/// Build an `OverlayFrame` from app-owned state and runtime-derived metadata.
///
/// This mirrors `d4lc-cli/src/live.rs:build_live_overlay_frame()`.
#[cfg(test)]
pub fn build_overlay_frame(
    sequence: u64,
    tooltip_found: bool,
    hotpath_ms: u128,
    overlay_descriptor_state: Option<&OverlayDescriptorState>,
) -> OverlayFrame {
    OverlayFrame {
        sequence,
        tooltip_found,
        hotpath_ms,
        cursor_window_relative: None,
        rule_descriptors: convert_descriptors(overlay_descriptor_state).into(),
        text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
    }
}

// ---------------------------------------------------------------------------
// Overlay status helpers (for wiring into debug panel)
// ---------------------------------------------------------------------------

/// Build a headless/noop `OverlayStatus` for environments without an X11
/// display. Also the base of [`overlay_status_without_surface`], which
/// replaces the limitation with the live loop's own reason.
pub fn headless_overlay_status() -> OverlayStatus {
    OverlayStatus {
        backend: OverlayBackend::HeadlessNoop,
        enabled: false,
        window_geometry: d4lc_core::Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        interaction_zone: d4lc_core::Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        menu_locked: false,
        menu_visible: false,
        transparent: false,
        transparent_requested: false,
        transparent_confirmed: false,
        always_on_top: false,
        always_on_top_requested: false,
        always_on_top_confirmed: false,
        click_through: false,
        keyboard_interactive_requested: false,
        keyboard_interactive_confirmed: false,
        draw_error_count: 0,
        last_error: None,
        limitation: Some("No X11 display available".to_string()),
    }
}

/// Status for a live loop that could not create any overlay
/// surface.
///
/// `StartedLiveOverlay::WindowUnavailable` carries no handle, so the
/// loop synthesizes the status the Debug screen reports from the reason
/// it already logs: `headless_noop` (no surface, nothing drawn) with the
/// reason as the limitation. The vocabulary stays the handle's own
/// `OverlayStatus` — there is no second status channel and no invented
/// backend.
pub(super) fn overlay_status_without_surface(reason: &str) -> OverlayStatus {
    OverlayStatus {
        limitation: Some(reason.to_string()),
        ..headless_overlay_status()
    }
}

/// Render an `OverlayStatus` as the one-line Debug-screen "Overlay:"
/// text and the matching Logging-screen line.
///
/// The text is a rendering of the handle's own vocabulary — `backend`,
/// `enabled`, `limitation` — never a parallel status: a live backend
/// renders as `real_x11 (enabled)`, and a degraded handle
/// (`headless_noop` / `failed`) always names the limitation it carries,
/// so neither surface can silently claim an overlay that does not exist.
pub fn overlay_status_text(status: &OverlayStatus) -> String {
    let backend = status.backend.as_str();
    let state = if status.enabled {
        "enabled"
    } else {
        "disabled"
    };
    match status.limitation.as_deref() {
        Some(limitation)
            if matches!(
                status.backend,
                OverlayBackend::HeadlessNoop | OverlayBackend::Failed
            ) =>
        {
            format!("{backend} ({state}): {limitation}")
        }
        _ => format!("{backend} ({state})"),
    }
}

/// Log the live loop's overlay status once per signature change
///
/// The status itself travels inside the accepted snapshot the loop publishes
/// there is no second status channel, and the Debug screen's
/// "Overlay:" field always describes the same accepted state as the item card
/// and the frame numbers beside it. This keeps the one thing that is not
/// state: the Logging screen's line naming the surface state the Debug screen
/// shows. A status whose backend /
/// enabled / limitation signature changed — including the very first
/// observation — is logged once; a menu-lock flip alone does not repeat it.
pub(super) fn log_overlay_status_change(
    status: &OverlayStatus,
    last_logged: &mut Option<OverlayStatus>,
    event_tx: &RuntimeEventSink,
) {
    let previous = last_logged.as_ref();
    if previous == Some(status) {
        return;
    }
    let signature_changed = previous.is_none_or(|previous| {
        previous.backend != status.backend
            || previous.enabled != status.enabled
            || previous.limitation != status.limitation
    });
    if signature_changed {
        event_tx.send(RuntimeEvent::LogMessage {
            timestamp: crate::clock::hh_mm_now(),
            message: format!("overlay: {}", overlay_status_text(status)),
        });
    }
    *last_logged = Some(status.clone());
}

/// An overlay started by the app, regardless of backend. Delegates the
/// shared `OverlayHandle` surface so callers never match on the backend.
#[derive(Debug)]
pub enum StartedOverlay {
    X11(d4lc_overlay_x11::OverlayHandle),
    Wayland(d4lc_overlay_wayland::OverlayHandle),
}

impl StartedOverlay {
    pub fn status(&self) -> &OverlayStatus {
        match self {
            Self::X11(handle) => handle.status(),
            Self::Wayland(handle) => handle.status(),
        }
    }

    pub fn update(&mut self, frame: OverlayFrame) -> Result<(), String> {
        match self {
            Self::X11(handle) => handle.update(frame),
            Self::Wayland(handle) => handle.update(frame),
        }
    }

    pub fn poll_cursor_window_relative(&mut self) -> Option<d4lc_core::Point> {
        match self {
            Self::X11(_) => None,
            Self::Wayland(handle) => handle.poll_cursor_window_relative(),
        }
    }

    /// What the started handle's shared bounded text path could not draw, or
    /// why it is disabled, in the app's own vocabulary so the UI never
    /// depends on one backend's type. Both backends run the same shared text
    /// module, so the diagnostics are the same value in either case.
    pub fn text_diagnostics(&self) -> OverlayTextState {
        let diagnostics = match self {
            Self::X11(handle) => handle.text_diagnostics(),
            Self::Wayland(handle) => handle.text_diagnostics(),
        };
        OverlayTextState {
            font_unavailable: diagnostics.font_unavailable.clone(),
            unsupported_codepoints: diagnostics.unsupported_codepoints.clone(),
            clipped_chars: diagnostics.clipped_chars,
        }
    }

    /// Follow the game window. The X11 handle repositions its surface.
    /// The Wayland surface is output-anchored to the output carrying the
    /// capture stream, so its handle records the update explicitly instead
    /// of moving the surface (see
    /// `d4lc_overlay_wayland::OverlayHandle::set_window_geometry`): never
    /// a hard error, never a silent reposition.
    pub fn set_window_geometry(&mut self, geometry: d4lc_core::Rect) -> Result<(), String> {
        match self {
            Self::X11(handle) => handle.set_window_geometry(geometry),
            Self::Wayland(handle) => handle.set_window_geometry(geometry),
        }
    }

    /// Apply the overlay menu settings to the running surface (debug UI
    /// .7): icon visibility + position and optionally the stored
    /// lock value, on either backend.
    pub fn apply_menu_settings(
        &mut self,
        show_overlay_icon: bool,
        overlay_icon_pos_x: u32,
        overlay_icon_pos_y: u32,
        menu_locked: Option<bool>,
    ) {
        match self {
            Self::X11(handle) => {
                handle.apply_menu_settings(
                    show_overlay_icon,
                    overlay_icon_pos_x,
                    overlay_icon_pos_y,
                    menu_locked,
                );
            }
            Self::Wayland(handle) => {
                handle.apply_menu_settings(
                    show_overlay_icon,
                    overlay_icon_pos_x,
                    overlay_icon_pos_y,
                    menu_locked,
                );
            }
        }
    }

    /// Raster/commit costs of the last completed draw. Absent
    /// unless the backend measured them.
    pub fn draw_timings(&self) -> d4lc_overlay_x11::OverlayDrawTimings {
        match self {
            Self::X11(handle) => handle.draw_timings(),
            Self::Wayland(handle) => handle.draw_timings(),
        }
    }

    /// Whether the handle skipped the last update because nothing
    /// observable changed.
    pub fn last_update_skipped(&self) -> bool {
        match self {
            Self::X11(handle) => handle.last_update_skipped(),
            Self::Wayland(handle) => handle.last_update_skipped(),
        }
    }
}

/// The production app overlay-start outcome.
///
/// `Created` carries the display-backend-selected handle whose
/// `OverlayStatus` is the capability record (real_x11 /
/// real_wayland / headless_noop / failed + limitation). `WindowUnavailable`
/// reports that the game-window geometry could not be resolved in this
/// session: no overlay surface exists and none may be claimed — the live
/// loop logs the reason and keeps running.
#[cfg(feature = "real-runtime")]
#[derive(Debug)]
pub enum StartedLiveOverlay {
    /// Boxed: the handle enum is far larger than the reason variant and
    /// the outcome travels by value through the live-loop start path.
    Created(Box<StartedOverlay>),
    WindowUnavailable {
        reason: String,
    },
}

/// The capture target the live overlay start follows, resolved once per
/// runtime start from the prepared runtime's engine. The overlay start
/// follows the RESOLVED engine instead of re-parsing the raw
/// display-backend setting: `auto` may resolve to the Wayland portal
/// engine after detection, and a Wayland session has no X11 game-window
/// discovery to run (it is blind there).
#[cfg(feature = "real-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOverlayTarget {
    /// The X11 engine: the overlay start discovers the game window as
    /// before (the same default discovery the X11 runtime preparation
    /// performs). An unresolvable window is the no-surface outcome.
    X11Discovery,
    /// The Wayland portal engine: the prepared runtime's stream rect is
    /// the geometry — no X11 query runs. The rect is the capture stream's
    /// own space (origin (0,0)); the Wayland backend maps it 1:1 onto the
    /// advertised output whose pixel size matches it. The second field is
    /// a global logical point on the carrying output (the monitor stream's
    /// portal position, else the game window's center from KDE window
    /// management) so equally sized monitors are told apart.
    WaylandStream(d4lc_core::Rect, Option<(i32, i32)>),
}

/// Resolve the overlay start's capture target from the prepared runtime's
/// resolved engine.
#[cfg(feature = "real-runtime")]
pub(super) fn live_overlay_target(runtime: &d4lc_runtime::PreparedRuntime) -> LiveOverlayTarget {
    if runtime.captures_wayland_stream() {
        let anchor = runtime.wayland_stream_position().or_else(|| {
            crate::game_window_observer::wayland_game_window_geometry().map(|window| {
                (
                    window.x.saturating_add((window.width / 2) as i32),
                    window.y.saturating_add((window.height / 2) as i32),
                )
            })
        });
        LiveOverlayTarget::WaylandStream(runtime.capture_geometry(), anchor)
    } else {
        LiveOverlayTarget::X11Discovery
    }
}

/// The production overlay start for the app live path:
/// load the persisted settings, resolve the capture geometry for the
/// resolved engine, and create the overlay for that engine: the X11 overlay
/// for the X11 engine, the layer-shell overlay for the Wayland portal engine.
///
/// Called once per live-loop start, from the loop thread, after runtime
/// preparation succeeded — the same shape as the CLI live session's
/// `create_live_overlay`. The menu-lock state honors the persisted
/// `overlay_menu_locked` setting (the GUI owns that setting surface).
///
/// The Wayland branch is output-anchored: the stream rect maps 1:1 onto
/// the advertised output whose pixel size matches it (stream-pixel ==
/// output-pixel == surface-pixel), the layer surface fills that output,
/// and a stream no output matches is the backend's structured failed
/// state — never a guessed anchor and never a hard error here.
///
/// # Errors
///
/// A settings load failure or a hard seam error (zero overlay geometry)
/// is returned and the caller stops the live loop. An unresolvable game
/// window (X11 discovery branch) is deliberately not an error: it returns
/// [`StartedLiveOverlay::WindowUnavailable`] so the loop keeps running
/// without an overlay instead of inventing geometry or faking a surface.
#[cfg(feature = "real-runtime")]
pub fn start_live_overlay(
    config_root: &Path,
    target: LiveOverlayTarget,
) -> Result<StartedLiveOverlay, String> {
    let settings =
        d4lc_config::load_app_settings(config_root).map_err(|error| format!("overlay: {error}"))?;

    // The overlay surface covers the resolved capture target. The X11
    // branch keeps the same default window discovery the X11 runtime
    // preparation performs; the Wayland branch uses the portal stream rect
    // the prepared runtime carries — a session without a discoverable X11
    // window (the Wayland portal capture path skips window lookup by
    // design) starts without an X11 query instead of inventing geometry.
    let geometry = match target {
        LiveOverlayTarget::X11Discovery => {
            match crate::runtime_prepare::resolve_capture_source(None, None) {
                Ok(reference) => reference.geometry,
                Err(error) => {
                    return Ok(StartedLiveOverlay::WindowUnavailable {
                        reason: format!(
                            "no overlay surface: the game window geometry could not be resolved: {error}"
                        ),
                    });
                }
            }
        }
        LiveOverlayTarget::WaylandStream(rect, _) => rect,
    };

    let config = live_overlay_config(geometry, &settings);
    let started = match target {
        LiveOverlayTarget::X11Discovery => {
            d4lc_overlay_x11::create_overlay(config).map(StartedOverlay::X11)
        }
        LiveOverlayTarget::WaylandStream(_, anchor) => {
            d4lc_overlay_wayland::create_overlay_anchored(config, anchor)
                .map(StartedOverlay::Wayland)
        }
    };
    started.map(|handle| StartedLiveOverlay::Created(Box::new(handle)))
}

/// Project the resolved game-window geometry plus the persisted overlay
/// settings into the overlay create config. Pure helper of
/// [`start_live_overlay`]: the menu-lock state honors the persisted
/// `overlay_menu_locked` setting — unlike the CLI's hardcoded locked
/// menu, the GUI owns that setting surface.
#[cfg(feature = "real-runtime")]
fn live_overlay_config(
    window_geometry: d4lc_core::Rect,
    settings: &d4lc_config::AppSettings,
) -> d4lc_overlay_x11::OverlayConfig {
    d4lc_overlay_x11::OverlayConfig {
        window_geometry,
        show_overlay_icon: settings.show_overlay_icon,
        overlay_icon_pos_x: settings.overlay_icon_pos_x,
        overlay_icon_pos_y: settings.overlay_icon_pos_y,
        menu_locked: settings.overlay_menu_locked,
    }
}

/// Mirror an observed overlay menu-lock transition into the app-side
/// overlay gate — the upstream `ToggleOverlayMessage` contract: a hover
/// dwell on the overlay menu flips its lock state, and the lock change
/// turns the overlay on (locked) or off (unlocked)
/// (OverlayHandler.cs:1274-1289 → `HandleMenuItemAction` sends
/// `ToggleOverlayMessage(IsEnabled = isLocked)`; the capture handler
/// consumes it as its `IsEnabled` capture gate,
/// ScreenCaptureHandler.cs:91-96).
///
/// `last_seen` tracks the previously observed backend lock state:
/// the first observation only establishes the baseline (the persisted
/// lock value and the persisted gate may legitimately differ at
/// startup — only an observed hover transition drives the mirror), an
/// unchanged value is a no-op, and a real transition flips the gate
/// through the same headless `set_overlay_enabled` path the
/// Ctrl+F12 hotkey arm uses (persist + descriptor rebuild included).
///
/// Returns whether the gate was flipped; `Err` carries the persist
/// failure. The in-memory gate comparison happens before any state is
/// touched, so an unchanged cycle stays side-effect-free.
pub(super) fn reconcile_overlay_gate_with_menu_lock(
    menu_locked: bool,
    last_seen: &mut Option<bool>,
    state: &mut AppState,
    config_root: &Path,
) -> Result<bool, String> {
    if *last_seen == Some(menu_locked) {
        return Ok(false);
    }
    let first_observation = last_seen.is_none();
    *last_seen = Some(menu_locked);
    if first_observation {
        return Ok(false);
    }
    crate::actions::settings::set_overlay_enabled(menu_locked, state, config_root)?;
    Ok(true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[cfg(feature = "real-runtime")]
    use crate::runtime_bridge::test_support::empty_config_root;
    use crate::runtime_bridge::test_support::{
        sample_overlay_descriptor_state, sample_overlay_status, sample_paragon_state,
    };
    use std::path::PathBuf;

    // -------------------------------------------------------------------
    // convert_descriptors tests
    // -------------------------------------------------------------------

    #[test]
    fn convert_descriptors_returns_empty_when_no_state() {
        let descriptors = convert_descriptors(None);
        assert!(descriptors.is_empty());
    }

    #[test]
    fn convert_descriptors_returns_descriptors_from_real_state() {
        let overlay_state = sample_overlay_descriptor_state();
        let descriptors = convert_descriptors(Some(&overlay_state));

        // With item_power_limit_check_ok=true, a Wanted item with an affix
        // should produce at least one descriptor (affix highlight).
        assert!(
            !descriptors.is_empty(),
            "expected non-empty descriptors from real app-owned state"
        );
    }

    #[test]
    fn convert_descriptors_is_empty_when_item_power_limit_not_ok() {
        let mut state = sample_overlay_descriptor_state();
        if let Some(ref mut rule) = state.rule {
            rule.settings.item_power_limit_check_ok = false;
        }
        let descriptors = convert_descriptors(Some(&state));
        // Even with item_power check not ok, trade overlay descriptors may
        // still be pushed if trade_overlay is enabled.  Our fixture has
        // trade_overlay disabled, so result should be empty.
        assert!(
            descriptors.is_empty(),
            "with item_power check failed and no trade marker, descriptors should be empty"
        );
    }

    #[test]
    fn convert_descriptors_from_paragon_state() {
        let state = OverlayDescriptorState {
            rule: None,
            paragon: Some(sample_paragon_state()),
            paragon_mode_active: true,
        };
        let descriptors = convert_descriptors(Some(&state));
        // With a paragon route containing boards, descriptors should be non-empty.
        assert!(
            !descriptors.is_empty(),
            "expected paragon descriptors from app-owned state"
        );
    }

    #[test]
    fn convert_descriptors_carries_tuning_prism_descriptors_when_enabled() {
        // The live-loop descriptor dispatch consumes the
        // tuning-prism content — matched affixes with prisms and the
        // hints toggle produce `TuningPrism` descriptors in the frame.
        let mut state = sample_overlay_descriptor_state();
        if let Some(rule) = state.rule.as_mut() {
            rule.settings.is_tuning_prism_hints_enabled = true;
            rule.result.matched_affixes[0].tuning_prisms =
                vec!["TuningStone_1".to_string(), "TuningStone_2".to_string()];
        }
        assert!(state.rule.is_some(), "sample must carry rule state");

        let frame = build_overlay_frame(7, true, 20, Some(&state));

        let prisms: Vec<_> = frame
            .rule_descriptors
            .iter()
            .filter(|descriptor| {
                descriptor.marker_type == d4lc_rules::RuleOverlayMarkerType::TuningPrism
            })
            .collect();
        assert_eq!(prisms.len(), 2);
        assert_eq!(prisms[0].color, "Red");
        assert_eq!(prisms[1].color, "Blue");
        assert_eq!(prisms[0].shape, d4lc_rules::RuleOverlayShape::Circle);
        assert_eq!(
            prisms[0].source.upstream_method,
            "OverlayHandler.DrawGraphicsTuningPrisms"
        );

        // With the toggle off the same state emits no prism descriptors.
        let mut state = sample_overlay_descriptor_state();
        if let Some(ref mut rule) = state.rule {
            rule.result.matched_affixes[0].tuning_prisms = vec!["TuningStone_1".to_string()];
        }
        let frame = build_overlay_frame(8, true, 20, Some(&state));
        assert!(frame
            .rule_descriptors
            .iter()
            .all(|descriptor| descriptor.marker_type
                != d4lc_rules::RuleOverlayMarkerType::TuningPrism));
    }

    // -------------------------------------------------------------------
    // build_overlay_frame tests
    // -------------------------------------------------------------------

    #[test]
    fn build_overlay_frame_populates_rule_descriptors_from_state() {
        let state = sample_overlay_descriptor_state();
        let frame = build_overlay_frame(42, true, 150, Some(&state));

        assert_eq!(frame.sequence, 42);
        assert!(frame.tooltip_found);
        assert_eq!(frame.hotpath_ms, 150);
        assert!(
            !frame.rule_descriptors.is_empty(),
            "OverlayFrame.rule_descriptors must be populated from real app-owned state"
        );
    }

    #[test]
    fn build_overlay_frame_has_empty_descriptors_when_no_state() {
        let frame = build_overlay_frame(0, false, 0, None);
        assert!(frame.rule_descriptors.is_empty());
    }

    #[test]
    fn overlay_status_changes_are_logged_once_per_signature() {
        let (event_tx, event_rx) = RuntimeEventSink::new();
        let mut last_logged: Option<OverlayStatus> = None;

        let real = sample_overlay_status(OverlayBackend::RealX11, true, Some("standing note"));
        log_overlay_status_change(&real, &mut last_logged, &event_tx);
        let events: Vec<RuntimeEvent> = event_rx.try_iter().collect();
        assert!(
            matches!(events.as_slice(), [RuntimeEvent::LogMessage { message, .. }]
                if message == "overlay: real_x11 (enabled)"),
            "the first observation logs the backend state once: {events:?}"
        );

        // An unchanged status is not re-logged: the loop reports it every
        // frame through the accepted snapshot instead.
        log_overlay_status_change(&real, &mut last_logged, &event_tx);
        assert_eq!(event_rx.try_iter().count(), 0);

        // A menu-lock flip changes the status but not its backend /
        // enabled / limitation signature: the Logging screen does not
        // repeat the line (the flip reaches the UI inside the next
        // published snapshot).
        let mut flipped = real.clone();
        flipped.menu_locked = !real.menu_locked;
        log_overlay_status_change(&flipped, &mut last_logged, &event_tx);
        assert_eq!(event_rx.try_iter().count(), 0);

        // A degraded signature is logged once with its limitation named.
        let degraded = sample_overlay_status(
            OverlayBackend::Failed,
            true,
            Some("X11 init failed: boom; using headless fallback"),
        );
        log_overlay_status_change(&degraded, &mut last_logged, &event_tx);
        let events: Vec<RuntimeEvent> = event_rx.try_iter().collect();
        assert!(
            matches!(events.as_slice(), [RuntimeEvent::LogMessage { message, .. }]
                if message == "overlay: failed (enabled): X11 init failed: boom; using headless fallback"),
            "a degraded status names the limitation in the log: {events:?}"
        );
    }

    #[cfg(feature = "real-runtime")]
    #[test]
    fn live_overlay_start_outcome_is_reported_in_every_environment() {
        // The production caller's environment contract: an unresolvable
        // game window yields the window-unavailable outcome whose reason
        // names the failure (the live loop logs it and runs without a
        // surface — geometry is never invented); a resolvable window
        // yields the seam's handle, whose degraded states carry their
        // limitation. No environment hard-fails on geometry alone and no
        // outcome claims a surface that was not created.
        let config_root = empty_config_root("d4lc-test-live-overlay-outcome");
        let outcome = start_live_overlay(&config_root, LiveOverlayTarget::X11Discovery)
            .unwrap_or_else(|error| panic!("overlay start must not hard-fail: {error}"));
        let _ = std::fs::remove_dir_all(&config_root);

        match outcome {
            StartedLiveOverlay::WindowUnavailable { reason } => {
                assert!(
                    reason.contains("no overlay surface")
                        && reason.contains("geometry could not be resolved"),
                    "window-unavailable reason must name the outcome: {reason}"
                );
            }
            StartedLiveOverlay::Created(started) => {
                let status = started.status();
                if status.backend == OverlayBackend::HeadlessNoop {
                    assert!(
                        status.limitation.is_some(),
                        "degraded headless start must carry its limitation: {status:?}"
                    );
                }
            }
        }
    }

    #[cfg(feature = "real-runtime")]
    #[test]
    fn live_overlay_wayland_branch_uses_the_stream_rect_without_an_x11_query() {
        // The resolved-engine contract of the Wayland branch: the stream
        // rect from the prepared runtime is the geometry, and no X11
        // game-window discovery runs — on this display-less machine the
        // X11 discovery would fail, so a Created outcome (with the
        // backend's own degraded state) proves the branch never
        // queried X11 and never returned WindowUnavailable.
        let config_root = empty_config_root("d4lc-test-live-overlay-wayland");
        let stream_rect = d4lc_core::Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        };
        let outcome = start_live_overlay(
            &config_root,
            LiveOverlayTarget::WaylandStream(stream_rect, None),
        )
        .unwrap_or_else(|error| panic!("wayland overlay start must not hard-fail: {error}"));
        let _ = std::fs::remove_dir_all(&config_root);

        match outcome {
            StartedLiveOverlay::WindowUnavailable { reason } => {
                panic!("the Wayland branch must not run the X11 discovery: {reason}")
            }
            StartedLiveOverlay::Created(started) => {
                let status = started.status();
                if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                    assert_eq!(
                        status.backend,
                        OverlayBackend::HeadlessNoop,
                        "without a Wayland session the backend degrades to headless_noop"
                    );
                    assert!(
                        status.limitation.is_some(),
                        "degraded headless start must carry its limitation: {status:?}"
                    );
                }
            }
        }
    }

    #[cfg(feature = "real-runtime")]
    #[test]
    fn live_overlay_config_projects_persisted_overlay_settings() {
        // The GUI production start builds the create config from the
        // persisted settings — in particular the menu-lock state is the
        // stored `overlay_menu_locked` value (the GUI owns that setting
        // surface), not the CLI's hardcoded locked menu.
        let settings = d4lc_config::AppSettings {
            show_overlay_icon: false,
            overlay_icon_pos_x: 12,
            overlay_icon_pos_y: 34,
            overlay_menu_locked: false,
            ..d4lc_config::AppSettings::default()
        };

        let config = live_overlay_config(
            d4lc_core::Rect {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            },
            &settings,
        );

        assert_eq!(
            config.window_geometry,
            d4lc_core::Rect {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            },
            "the overlay surface covers the resolved game-window geometry"
        );
        assert!(
            !config.menu_locked,
            "menu lock must follow the persisted setting, not a hardcoded value"
        );
        assert!(
            !config.show_overlay_icon,
            "icon visibility must follow the persisted setting"
        );
        assert_eq!(config.overlay_icon_pos_x, 12);
        assert_eq!(config.overlay_icon_pos_y, 34);
    }

    // -------------------------------------------------------------------
    // Menu-lock → overlay-gate reconcile tests (upstream ToggleOverlayMessage)
    // -------------------------------------------------------------------

    /// Fresh empty config root for the reconcile tests: `persist_settings`
    /// writes `Config/Settings.json` under it on a real gate flip.
    fn reconcile_config_root(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|error| panic!("temp config root must be creatable: {error}"));
        dir
    }

    #[test]
    fn menu_lock_flip_works_with_out_of_range_persisted_rows() {
        // A persisted value outside the dialog ranges must not make the
        // live-loop gate flip fail (and get reverted by the coordinator).
        let config_root = reconcile_config_root("d4lc-test-menu-lock-out-of-range");
        let mut state = AppState::default_startup();
        state.shell.settings.overlay_enabled = true;
        state.shell.settings.type_area_offset_left = 30;
        state.shell.settings.overlay_update_delay = 0;
        state.shell.settings.item_power_limit = 925;

        let mut last_seen = Some(true);
        let flipped =
            reconcile_overlay_gate_with_menu_lock(false, &mut last_seen, &mut state, &config_root)
                .expect("the flip must not re-validate unrelated rows");
        assert!(flipped);
        assert!(!state.shell.settings.overlay_enabled);
        assert!(
            !d4lc_config::load_app_settings(&config_root)
                .unwrap()
                .overlay_enabled
        );
        let _ = std::fs::remove_dir_all(&config_root);
    }

    #[test]
    fn menu_lock_flip_keeps_every_other_overlay_setting() {
        // The reconcile flips only the on/off gate; the icon, item-power
        // rule, marker mode and detection gates keep their configured
        // values (the draft is seeded from the current settings).
        let config_root = reconcile_config_root("d4lc-test-menu-lock-keeps-settings");
        let mut state = AppState::default_startup();
        {
            let settings = &mut state.shell.settings;
            settings.show_overlay_icon = true;
            settings.overlay_icon_pos_x = 120;
            settings.overlay_icon_pos_y = 340;
            settings.item_power_limit_enabled = true;
            settings.item_power_limit = 750;
            settings.overlay_marker_mode = d4lc_config::OverlayMarkerModeSetting::HideUnwanted;
            settings.trade_overlay_enabled = false;
            settings.socket_detection_enabled = false;
            settings.tempered_detection_enabled = false;
        }
        let mut expected = state.shell.settings.clone();
        expected.overlay_enabled = false;

        let mut last_seen = Some(true);
        let flipped =
            reconcile_overlay_gate_with_menu_lock(false, &mut last_seen, &mut state, &config_root)
                .expect("the unlock flip must not fail");
        assert!(flipped);
        assert_eq!(state.shell.settings, expected);

        let persisted = d4lc_config::load_app_settings(&config_root).unwrap();
        assert!(persisted.show_overlay_icon);
        assert_eq!(persisted.overlay_icon_pos_x, 120);
        assert_eq!(persisted.overlay_icon_pos_y, 340);
        assert!(persisted.rule_settings.item_power.enabled);
        assert_eq!(persisted.rule_settings.item_power.limit, 750);
        assert!(!persisted.is_socket_detection_enabled);
        let _ = std::fs::remove_dir_all(&config_root);
    }

    #[test]
    fn menu_lock_flip_mirrors_the_overlay_enabled_gate() {
        // Upstream makes the hover menu lock and the overlay on/off gate
        // ONE state: the dwell flip sends
        // `ToggleOverlayMessage(IsEnabled = isLocked)`
        // (OverlayHandler.cs:1274-1289 → HandleMenuItemAction :1117-1140)
        // and the capture handler applies it as its gate
        // (ScreenCaptureHandler.cs:91-96), while the draw pass anchors no
        // descriptors while unlocked (OH:133). The reconcile must mirror
        // an observed backend flip into `overlay_enabled` (persist +
        // descriptor rebuild included), treat the first observation as a
        // baseline only, and stay side-effect-free when nothing changed.
        let config_root = reconcile_config_root("d4lc-test-menu-lock-gate");
        let mut state = AppState::default_startup();
        // A running enabled overlay: the gate on, the persisted lock
        // matching the backend seed (`live_overlay_config` seeds the menu
        // from the same setting).
        state.shell.settings.overlay_enabled = true;
        state.shell.settings.overlay_menu_locked = true;
        // Descriptor-producing state (selected preset + trade marker) so
        // the gate flip is observable in the projection itself.
        state.affix_preset_editor.create_preset("Speed").unwrap();
        state
            .affix_preset_editor
            .edit_entry(
                "Speed",
                d4lc_gui::PresetEntry {
                    kind: d4lc_gui::PresetEntryKind::Affix,
                    id: "Affix_AttackSpeed".to_string(),
                    label: Some("Attack Speed".to_string()),
                    item_type: Some("Amulet".to_string()),
                    color: "Green".to_string(),
                    is_any_type: false,
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                },
            )
            .unwrap();
        state
            .trade_imports_mut()
            .upsert_trade_item(d4lc_gui::TradeItemRow {
                trade_item_id: "Trade_Amulet".to_string(),
                item_type: "Amulet".to_string(),
                value_text: "10M".to_string(),
                affixes: vec![],
            });
        state.rebuild_overlay_descriptor_state();
        assert!(
            state.overlay_descriptor_state.is_some(),
            "the enabled overlay must project descriptors for this fixture"
        );

        let settings_file = config_root.join("Config").join("Settings.json");
        let mut last_seen: Option<bool> = None;

        // First observation: baseline only — no flip, no persist.
        let flipped =
            reconcile_overlay_gate_with_menu_lock(true, &mut last_seen, &mut state, &config_root)
                .expect("the baseline observation must not fail");
        assert!(!flipped, "the first observation only sets the baseline");
        assert!(last_seen == Some(true));
        assert!(state.shell.settings.overlay_enabled);
        assert!(
            !settings_file.exists(),
            "a baseline observation must not persist settings"
        );

        // Unchanged value: a side-effect-free no-op.
        let flipped =
            reconcile_overlay_gate_with_menu_lock(true, &mut last_seen, &mut state, &config_root)
                .expect("an unchanged observation must not fail");
        assert!(!flipped, "an unchanged lock value is a no-op");
        assert!(state.shell.settings.overlay_enabled);
        assert!(!settings_file.exists(), "a no-op must not persist settings");

        // Hover-unlock flip (OH:1284-1290 → IsEnabled = false): the gate
        // turns off, the flip persists, and the projection drops the
        // descriptors — the upstream cleared-tooltip frame.
        let flipped =
            reconcile_overlay_gate_with_menu_lock(false, &mut last_seen, &mut state, &config_root)
                .expect("the unlock flip must not fail");
        assert!(flipped, "a real lock transition must flip the gate");
        assert!(!state.shell.settings.overlay_enabled);
        let persisted = d4lc_config::load_app_settings(&config_root)
            .expect("the flip must leave a loadable settings file");
        assert!(
            !persisted.overlay_enabled,
            "the gate flip persists through the shared settings path"
        );
        assert!(
            state.overlay_descriptor_state.is_none(),
            "no descriptors may anchor while the overlay is off"
        );

        // Hover-lock flip (OH:1277-1283 → IsEnabled = true): the gate
        // turns back on, persists, and the projection is restored.
        let flipped =
            reconcile_overlay_gate_with_menu_lock(true, &mut last_seen, &mut state, &config_root)
                .expect("the lock flip must not fail");
        assert!(flipped, "a real lock transition must flip the gate");
        assert!(state.shell.settings.overlay_enabled);
        let persisted = d4lc_config::load_app_settings(&config_root)
            .expect("the flip must leave a loadable settings file");
        assert!(persisted.overlay_enabled);
        let projected = state
            .overlay_descriptor_state
            .as_ref()
            .expect("the re-enabled overlay must project descriptors again");
        let rule = projected.rule.as_ref().expect("rule state must be back");
        assert_eq!(
            rule.result
                .trade_marker
                .as_ref()
                .map(|marker| marker.trade_item_id.as_str()),
            Some("Trade_Amulet"),
            "the restored projection must carry the descriptor fixture"
        );

        let _ = std::fs::remove_dir_all(&config_root);
    }
}
