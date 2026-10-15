//! Display-backend selection seam for d4lc-app.
//!
//! The `DisplayBackend` type and its persisted-token parser live in
//! `d4lc-config` so both binaries share them; this module re-exports the
//! type and owns the app-side resolution: the build-time Wayland capture
//! support, the availability status, and the `auto` detection. It is
//! intentionally an always-compiled module (mirroring the `hotkeys`
//! backend-status module): the settings persistence path runs in every
//! build, while `runtime_prepare` is gated behind the `real-runtime`
//! feature — the seam must be reachable from both. The runtime-prep path
//! applies it in `runtime_prepare::resolve_runtime_config` /
//! `prepare_app_runtime`; the Settings screen applies it in
//! `actions::settings::handle_display_backend_changed`.
//!
//! Two resolution levels live here:
//!
//! - [`resolve_display_backend`] — the parse-level seam: validates the
//!   stored token and reports the status the setting itself selects
//!   (explicit settings, build availability). `auto` resolves to
//!   [`DisplayBackendStatus::Auto`]: the engine is genuinely unknown
//!   until it is detected against a session. The Settings path uses this
//!   level — it must never spawn an X11 discovery probe.
//! - [`resolve_display_backend_for_session`] — the once-per-runtime-start
//!   resolution: `auto` becomes a real detection from the session
//!   environment plus the result of the single X11 game-window discovery
//!   the caller ran (the same discovery the capture-source step needs, so
//!   a start probes at most once). Explicit settings stay strictly
//!   honored here — detection never replaces them.
//!
//! Distinct from `d4lc_capture::CaptureBackend`, which chooses the
//! X11-internal capture method: this is the higher-level display-server
//! choice persisted as `DisplayBackend` in `Config/Settings.json`
//! (default `auto`). The Wayland surfaces (capture, overlay, hotkeys) hang
//! their per-surface selection off the same seam.

pub use d4lc_config::DisplayBackend;

/// Build-time Wayland capture support of this binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaylandCaptureBuild {
    /// Built with the `wayland-portal` feature: ashpd and libpipewire
    /// are linked and a Wayland selection runs the real ScreenCast
    /// capture path (portal session + PipeWire stream).
    Enabled,
    /// Built without the feature: no Wayland capture code exists in
    /// this binary. A Wayland selection reports the structured
    /// unavailability naming the missing build piece — never a silent
    /// X11 fallback.
    NotBuilt,
}

/// Whether this binary contains the Wayland capture path, decided once
/// at compile time from the `wayland-portal` feature.
pub const WAYLAND_CAPTURE_BUILD: WaylandCaptureBuild = if cfg!(feature = "wayland-portal") {
    WaylandCaptureBuild::Enabled
} else {
    WaylandCaptureBuild::NotBuilt
};

/// Status of the display-backend selection seam.
///
/// The engine variants carry the resolution reason so the Debug screen's
/// "Capture engine:" row records the decision of one runtime start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayBackendStatus {
    /// The X11 capture engine was selected; `reason` records
    /// why (the explicit setting, or the auto detection's window match).
    X11 { reason: String },
    /// The Wayland ScreenCast portal engine was selected; `reason`
    /// records why (the explicit setting, or the auto detection's
    /// session facts). Runtime preparation establishes the portal
    /// session (compositor dialog or restore-token restore); any portal
    /// or PipeWire failure at session time is a structured error naming
    /// the missing piece — never a crash and never a silent X11
    /// fallback.
    Wayland { reason: String },
    /// Wayland was selected (explicitly, or by the auto detection) but
    /// this binary has no Wayland capture path (built without the
    /// `wayland-portal` feature). The reason is surfaced to the user
    /// (log + runtime status surface) and runtime preparation stops.
    WaylandUnavailable { reason: String },
    /// `auto` is selected; the engine is genuinely undetected until the
    /// once-per-runtime-start session resolution runs. The Settings
    /// path sees this level — it never probes.
    Auto,
    /// `auto` found no usable capture engine in this session; `reason`
    /// carries the capture-source failure text and preparation stops.
    /// Never a silent fallback onto a path that cannot see the game.
    NoEngine { reason: String },
}

/// The session facts the `auto` detection consumes, collected once per
/// runtime start from the running process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionEnvironment {
    /// An X11 display is reachable (`DISPLAY` is set and non-empty).
    pub x11_display: bool,
    /// The process can talk to a Wayland compositor: `WAYLAND_DISPLAY` is
    /// set and non-empty (see [`wayland_display_available`]).
    pub wayland_session: bool,
}

impl SessionEnvironment {
    /// Collect the session facts of the running process.
    pub fn from_process() -> Self {
        let x11_display = std::env::var_os("DISPLAY").is_some_and(|value| !value.is_empty());
        Self {
            x11_display,
            wayland_session: wayland_display_available(),
        }
    }
}

/// Whether this process has a Wayland display connection to use.
///
/// `XDG_SESSION_TYPE=wayland` alone is not enough: a process started
/// without `WAYLAND_DISPLAY` (an X11-only launch, or the headless test
/// harness, which strips it) must not reach for the compositor or its
/// portals.
pub(crate) fn wayland_display_available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
}

/// The outcome of the single X11 game-window discovery the `auto`
/// selection may run (executed by the caller — never inside this seam).
///
/// The probe IS the capture-source resolution: when it resolved a
/// reference (window match or manual region), the caller reuses it and
/// no second X11 discovery runs for that start. The seam receives only
/// the decision-relevant facts — this module stays free of capture-crate
/// types so it compiles in every build (the capture crate is linked only
/// behind `real-runtime`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11GameWindowProbe {
    /// `DISPLAY` is absent — the probe is not run.
    NotRun,
    /// The discovery resolved a capture reference; `matched_window`
    /// distinguishes a matched game window from a manual region.
    Resolved { matched_window: bool },
    /// The discovery ran and failed; `error` is the capture-source
    /// failure text (today's error surface).
    Failed { error: String },
}

/// The capture-engine resolution of one runtime start, produced by
/// [`resolve_display_backend_for_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureEngineResolution {
    /// The persisted setting this resolution started from.
    pub backend: DisplayBackend,
    /// The status of the resolved engine.
    pub status: DisplayBackendStatus,
}

/// The unavailability reason for a Wayland selection in a build without
/// the `wayland-portal` feature.
fn wayland_not_built_reason() -> String {
    "wayland capture is not built into this binary (the wayland-portal \
     build feature with its ScreenCast portal and PipeWire support is \
     missing); overlay and hotkey support arrive with follow-up updates"
        .to_string()
}

/// The display-backend selection seam: the single parse-level resolution
/// point that maps the persisted `DisplayBackend` setting to the parsed
/// backend plus its status — without any session probing. The
/// Settings screen uses this level; the runtime start refines `auto`
/// through [`resolve_display_backend_for_session`].
///
/// # Errors
///
/// Returns the structured [`DisplayBackend::parse`] error for any
/// stored value outside `auto|x11|wayland`.
pub fn resolve_display_backend(
    setting: &str,
) -> Result<(DisplayBackend, DisplayBackendStatus), String> {
    let backend = DisplayBackend::parse(setting)?;
    let status = match backend {
        DisplayBackend::Auto => DisplayBackendStatus::Auto,
        DisplayBackend::X11 => DisplayBackendStatus::X11 {
            reason: "explicit setting".to_string(),
        },
        DisplayBackend::Wayland => match WAYLAND_CAPTURE_BUILD {
            WaylandCaptureBuild::Enabled => DisplayBackendStatus::Wayland {
                reason: "explicit setting".to_string(),
            },
            WaylandCaptureBuild::NotBuilt => DisplayBackendStatus::WaylandUnavailable {
                reason: wayland_not_built_reason(),
            },
        },
    };
    Ok((backend, status))
}

/// The once-per-runtime-start session resolution: `auto` becomes a real
/// detection, explicit settings stay strictly honored.
///
/// Detection rules for `auto`, evaluated against [`SessionEnvironment`]
/// and the caller's single [`X11GameWindowProbe`]:
///
/// - No `DISPLAY` and a Wayland session → Wayland portal engine (when
///   this build contains the capture path; otherwise the structured
///   [`DisplayBackendStatus::WaylandUnavailable`] — never a silent X11
///   run that is blind on Wayland).
/// - `DISPLAY` present → the probe's outcome decides: a resolved
///   reference (Diablo window match or manual region) selects the X11
///   engine and is reused as the capture reference; no match with a
///   Wayland session and the portal path built selects the Wayland
///   engine; otherwise the discovery's failure text is carried as
///   [`DisplayBackendStatus::NoEngine`] — today's capture-source error.
/// - No display and no Wayland session → [`DisplayBackendStatus::NoEngine`].
pub fn resolve_display_backend_for_session(
    backend: DisplayBackend,
    build: WaylandCaptureBuild,
    env: SessionEnvironment,
    probe: &X11GameWindowProbe,
) -> CaptureEngineResolution {
    let status = match backend {
        DisplayBackend::X11 => DisplayBackendStatus::X11 {
            reason: "explicit setting".to_string(),
        },
        DisplayBackend::Wayland => match build {
            WaylandCaptureBuild::Enabled => DisplayBackendStatus::Wayland {
                reason: "explicit setting".to_string(),
            },
            WaylandCaptureBuild::NotBuilt => DisplayBackendStatus::WaylandUnavailable {
                reason: wayland_not_built_reason(),
            },
        },
        DisplayBackend::Auto => match (env.x11_display, env.wayland_session) {
            (true, _) => match probe {
                X11GameWindowProbe::Resolved { matched_window } => DisplayBackendStatus::X11 {
                    reason: if *matched_window {
                        "Diablo window found"
                    } else {
                        "manual capture region"
                    }
                    .to_string(),
                },
                X11GameWindowProbe::Failed { error } => {
                    if env.wayland_session && build == WaylandCaptureBuild::Enabled {
                        DisplayBackendStatus::Wayland {
                            reason: "no X11 game window, wayland session".to_string(),
                        }
                    } else {
                        DisplayBackendStatus::NoEngine {
                            reason: error.clone(),
                        }
                    }
                }
                // Unreachable from a caller that probes when DISPLAY is
                // present; carried instead of guessing.
                X11GameWindowProbe::NotRun => DisplayBackendStatus::NoEngine {
                    reason: "the X11 game-window discovery did not run".to_string(),
                },
            },
            (false, true) => match build {
                WaylandCaptureBuild::Enabled => DisplayBackendStatus::Wayland {
                    reason: "no X11 display, wayland session".to_string(),
                },
                WaylandCaptureBuild::NotBuilt => DisplayBackendStatus::WaylandUnavailable {
                    reason: wayland_not_built_reason(),
                },
            },
            (false, false) => DisplayBackendStatus::NoEngine {
                reason: "no X11 window matched the requested query (DISPLAY is unset and no \
                         Wayland session was detected)"
                    .to_string(),
            },
        },
    };
    CaptureEngineResolution { backend, status }
}

impl DisplayBackendStatus {
    /// The Debug screen's "Capture engine:" text for the resolved
    /// backend: `<setting> → <engine>: <reason>` — the recorded decision
    /// of one runtime start, never a live re-detection.
    pub fn capture_engine_text(&self, backend: DisplayBackend) -> String {
        let setting = backend.as_str();
        match self {
            Self::X11 { reason } => format!("{setting} → X11: {reason}"),
            Self::Wayland { reason } => format!("{setting} → Wayland portal: {reason}"),
            Self::WaylandUnavailable { reason } | Self::NoEngine { reason } => {
                format!("{setting} → unavailable: {reason}")
            }
            Self::Auto => format!("{setting} → engine detected at runtime start"),
        }
    }

    /// Whether the resolved engine is the Wayland ScreenCast portal.
    pub fn is_wayland_engine(&self) -> bool {
        matches!(self, Self::Wayland { .. })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    // ---- display-backend selection seam tests ----

    // The persisted-token parse coverage lives with the shared parser in
    // `d4lc-config`; the seam tests below cover the
    // app-side resolution on top of it.

    fn x11_session() -> SessionEnvironment {
        SessionEnvironment {
            x11_display: true,
            wayland_session: false,
        }
    }

    fn wayland_session() -> SessionEnvironment {
        SessionEnvironment {
            x11_display: false,
            wayland_session: true,
        }
    }

    fn headless_session() -> SessionEnvironment {
        SessionEnvironment {
            x11_display: false,
            wayland_session: false,
        }
    }

    // ---- parse-level seam (Settings path: never probes) ----

    #[test]
    fn display_backend_seam_resolves_x11_explicitly_and_auto_as_undetected() {
        // Explicit x11 keeps the X11 paths with the decision
        // reason; `auto` must NOT claim an engine at the parse level —
        // it is detected once per runtime start.
        let (backend, status) = resolve_display_backend("x11").expect("'x11' must parse");
        assert_eq!(backend, DisplayBackend::X11);
        assert_eq!(
            status,
            DisplayBackendStatus::X11 {
                reason: "explicit setting".to_string()
            }
        );

        let (backend, status) = resolve_display_backend("auto").expect("'auto' must parse");
        assert_eq!(backend, DisplayBackend::Auto);
        assert_eq!(status, DisplayBackendStatus::Auto);
        assert!(
            !status.is_wayland_engine(),
            "the undetected auto status must not claim the portal engine"
        );
    }

    // Build-state matrix: the two compile-time states of the seam.
    #[cfg(not(feature = "wayland-portal"))]
    #[test]
    fn wayland_selection_resolves_unavailable_in_default_builds() {
        // Default builds carry no Wayland capture code (libpipewire
        // stays out of the link path): a Wayland selection resolves to
        // the structured unavailability naming the missing build piece,
        // never a silent X11 claim.
        let (backend, status) =
            resolve_display_backend("wayland").expect("wayland parses in this build");
        assert_eq!(backend, DisplayBackend::Wayland);
        assert_eq!(WAYLAND_CAPTURE_BUILD, WaylandCaptureBuild::NotBuilt);
        match status {
            DisplayBackendStatus::WaylandUnavailable { reason } => {
                assert!(
                    reason.contains("not built into this binary"),
                    "unavailability reason must name the missing build piece: {reason}"
                );
                assert!(
                    reason.contains("wayland-portal"),
                    "unavailability reason must name the build feature: {reason}"
                );
                assert_eq!(
                    DisplayBackendStatus::WaylandUnavailable {
                        reason: reason.clone()
                    }
                    .capture_engine_text(DisplayBackend::Wayland),
                    format!("wayland → unavailable: {reason}"),
                    "the Debug row renders the unavailability with the setting prefix"
                );
            }
            other => panic!("Wayland must resolve to the structured unavailability: {other:?}"),
        }
    }

    #[cfg(feature = "wayland-portal")]
    #[test]
    fn wayland_selection_resolves_available_when_the_feature_is_built_in() {
        // With the capture feature, the seam resolves Wayland to the
        // real path; portal/PipeWire failures happen later, at session
        // time, as structured errors.
        assert_eq!(WAYLAND_CAPTURE_BUILD, WaylandCaptureBuild::Enabled);
        let (_, status) = resolve_display_backend("wayland").expect("wayland parses");
        assert_eq!(
            status,
            DisplayBackendStatus::Wayland {
                reason: "explicit setting".to_string()
            }
        );
    }

    // ---- once-per-start session resolution (auto detection matrix) ----
    // The build state is a parameter of the seam, so the full matrix is
    // exercised in feature-less builds by passing the build explicitly.

    #[test]
    fn auto_without_display_on_a_wayland_session_selects_the_portal_engine() {
        // The live-probe case that motivated the detection: no X11
        // display, native Wayland session — the X11 pipeline is blind,
        // so auto must resolve to the portal engine.
        for build in [WaylandCaptureBuild::Enabled, WaylandCaptureBuild::NotBuilt] {
            let resolution = resolve_display_backend_for_session(
                DisplayBackend::Auto,
                build,
                wayland_session(),
                &X11GameWindowProbe::NotRun,
            );
            if build == WaylandCaptureBuild::Enabled {
                assert_eq!(
                    resolution.status,
                    DisplayBackendStatus::Wayland {
                        reason: "no X11 display, wayland session".to_string()
                    }
                );
                assert!(resolution.status.is_wayland_engine());
            } else {
                // Portal build unavailable: the structured
                // unavailability — never a silent X11 run that sees
                // nothing.
                match resolution.status {
                    DisplayBackendStatus::WaylandUnavailable { ref reason } => {
                        assert!(reason.contains("wayland-portal"), "{reason}");
                    }
                    other => panic!("expected WaylandUnavailable, got {other:?}"),
                }
            }
            assert_eq!(
                resolution.backend,
                DisplayBackend::Auto,
                "the resolution must round-trip the stored setting"
            );
        }
    }

    #[test]
    fn auto_with_an_x11_diablo_window_found_selects_x11() {
        // One X11 discovery per start: the probe's reference IS the
        // capture reference (the caller reuses it), so the capture-source
        // step never probes a second time. This also covers Xwayland
        // games inside a Wayland session (DISPLAY present → the X11
        // engine wins).
        for env in [
            x11_session(),
            SessionEnvironment {
                x11_display: true,
                wayland_session: true,
            },
        ] {
            let resolution = resolve_display_backend_for_session(
                DisplayBackend::Auto,
                WaylandCaptureBuild::Enabled,
                env,
                &X11GameWindowProbe::Resolved {
                    matched_window: true,
                },
            );
            assert_eq!(
                resolution.status,
                DisplayBackendStatus::X11 {
                    reason: "Diablo window found".to_string()
                }
            );
        }
    }

    #[test]
    fn auto_with_a_manual_capture_region_selects_x11() {
        // A caller-provided manual region resolves through the same
        // discovery; the X11 engine captures it, the reason says so.
        let resolution = resolve_display_backend_for_session(
            DisplayBackend::Auto,
            WaylandCaptureBuild::Enabled,
            x11_session(),
            &X11GameWindowProbe::Resolved {
                matched_window: false,
            },
        );
        assert_eq!(
            resolution.status,
            DisplayBackendStatus::X11 {
                reason: "manual capture region".to_string()
            }
        );
    }

    #[test]
    fn auto_without_an_x11_match_on_a_wayland_session_falls_back_to_the_portal() {
        // No Diablo window matched, but the session is Wayland and the
        // portal path is built: the portal engine takes over.
        let resolution = resolve_display_backend_for_session(
            DisplayBackend::Auto,
            WaylandCaptureBuild::Enabled,
            SessionEnvironment {
                x11_display: true,
                wayland_session: true,
            },
            &X11GameWindowProbe::Failed {
                error: "no X11 window matched the requested query".to_string(),
            },
        );
        assert_eq!(
            resolution.status,
            DisplayBackendStatus::Wayland {
                reason: "no X11 game window, wayland session".to_string()
            }
        );
    }

    #[test]
    fn auto_without_an_x11_match_and_no_wayland_fails_with_todays_error_text() {
        // DISPLAY present, no match, no Wayland session: today's
        // capture-source failure — never a silent fallback onto a path
        // that cannot see the game.
        for build in [WaylandCaptureBuild::Enabled, WaylandCaptureBuild::NotBuilt] {
            let resolution = resolve_display_backend_for_session(
                DisplayBackend::Auto,
                build,
                x11_session(),
                &X11GameWindowProbe::Failed {
                    error: "no X11 window matched the requested query".to_string(),
                },
            );
            match resolution.status {
                DisplayBackendStatus::NoEngine { ref reason } => {
                    assert_eq!(reason, "no X11 window matched the requested query");
                }
                other => panic!("expected NoEngine with today's error text, got {other:?}"),
            }
        }
    }

    #[test]
    fn auto_on_a_headless_session_fails_with_a_reason() {
        // Neither display nor Wayland session: no engine can see the
        // game, and the reason names both absences.
        for build in [WaylandCaptureBuild::Enabled, WaylandCaptureBuild::NotBuilt] {
            let resolution = resolve_display_backend_for_session(
                DisplayBackend::Auto,
                build,
                headless_session(),
                &X11GameWindowProbe::NotRun,
            );
            match resolution.status {
                DisplayBackendStatus::NoEngine { ref reason } => {
                    assert!(
                        reason.contains("DISPLAY is unset") && reason.contains("Wayland"),
                        "reason must name both absences: {reason}"
                    );
                }
                other => panic!("expected NoEngine, got {other:?}"),
            }
        }
    }

    #[test]
    fn explicit_x11_and_wayland_survive_the_session_resolution_unchanged() {
        // An explicit choice is never silently replaced by the
        // detection — whatever the session looks like.
        let probe = X11GameWindowProbe::Resolved {
            matched_window: true,
        };

        let resolution = resolve_display_backend_for_session(
            DisplayBackend::X11,
            WaylandCaptureBuild::Enabled,
            wayland_session(),
            &probe,
        );
        assert_eq!(
            resolution.status,
            DisplayBackendStatus::X11 {
                reason: "explicit setting".to_string()
            }
        );

        let resolution = resolve_display_backend_for_session(
            DisplayBackend::Wayland,
            WaylandCaptureBuild::Enabled,
            x11_session(),
            &probe,
        );
        assert_eq!(
            resolution.status,
            DisplayBackendStatus::Wayland {
                reason: "explicit setting".to_string()
            }
        );

        // Explicit Wayland without the capture path stays the structured
        // unavailability in every session.
        let resolution = resolve_display_backend_for_session(
            DisplayBackend::Wayland,
            WaylandCaptureBuild::NotBuilt,
            wayland_session(),
            &X11GameWindowProbe::NotRun,
        );
        match resolution.status {
            DisplayBackendStatus::WaylandUnavailable { ref reason } => {
                assert!(reason.contains("not built into this binary"), "{reason}");
            }
            other => panic!("expected WaylandUnavailable, got {other:?}"),
        }
    }

    // ---- the Debug screen's "Capture engine:" text ----

    #[test]
    fn capture_engine_text_records_the_decision_of_one_start() {
        assert_eq!(
            DisplayBackendStatus::X11 {
                reason: "Diablo window found".to_string()
            }
            .capture_engine_text(DisplayBackend::Auto),
            "auto → X11: Diablo window found"
        );
        assert_eq!(
            DisplayBackendStatus::Wayland {
                reason: "no X11 game window, wayland session".to_string()
            }
            .capture_engine_text(DisplayBackend::Auto),
            "auto → Wayland portal: no X11 game window, wayland session"
        );
        assert_eq!(
            DisplayBackendStatus::Wayland {
                reason: "explicit setting".to_string()
            }
            .capture_engine_text(DisplayBackend::Wayland),
            "wayland → Wayland portal: explicit setting"
        );
        assert_eq!(
            DisplayBackendStatus::NoEngine {
                reason: "no X11 window matched the requested query".to_string()
            }
            .capture_engine_text(DisplayBackend::Auto),
            "auto → unavailable: no X11 window matched the requested query"
        );
    }
}
