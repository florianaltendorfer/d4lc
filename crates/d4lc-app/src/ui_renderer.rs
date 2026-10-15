//! Slint UI renderer choice.
//!
//! The window renders with femtovg over OpenGL. When GL cannot start (VM
//! without a GL driver, broken driver), the process relaunches itself once
//! with the software renderer and names the failure in the log and on the
//! Debug panel — never a silent fallback. `SLINT_BACKEND` stays an explicit
//! override: when it is set, Slint's own selection applies and the app never
//! relaunches.
//!
//! femtovg creates its GL context only when the first window is realized
//! inside the event loop, so a GL failure surfaces as the event loop's
//! error, after every other startup step ran. `show()` before the loop
//! cannot hit it: the winit backend (i-slint-backend-winit 1.18) has no
//! winit window yet, so it only marks the window for creation, and the
//! renderer (GL context included) is created in the loop's `resumed`
//! handler, whose error ends the loop. An error from `show()` is therefore
//! not a GL startup failure and is returned as is. Slint cannot switch the
//! renderer of a running process (the platform is set once), hence the
//! relaunch instead of an in-process switch. The relaunch keeps the
//! single-instance lock held across the `exec` (handed over on standard
//! input), so no other launch can become the instance in between.

use std::cell::Cell;
use std::rc::Rc;

/// Slint's own backend/renderer override variable.
pub const OVERRIDE_ENV: &str = "SLINT_BACKEND";
/// Set on the relaunched process: why the GPU renderer did not start.
pub const FALLBACK_REASON_ENV: &str = "D4LC_UI_RENDERER_FALLBACK";

/// Upper bound for the failure text carried to the relaunched process.
const MAX_REASON_CHARS: usize = 400;

/// How this process picks its UI renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RendererPlan {
    /// winit + femtovg (OpenGL); a GL startup failure relaunches with the
    /// software renderer.
    Gpu,
    /// `SLINT_BACKEND` is set: honored as is, no fallback.
    Override { backend: String },
    /// The relaunch after the GPU renderer failed: winit + software.
    SoftwareFallback { reason: String },
}

/// The plan for this process from `SLINT_BACKEND` and the relaunch marker.
/// Empty values count as unset.
pub fn plan_renderer(slint_backend: Option<&str>, fallback_reason: Option<&str>) -> RendererPlan {
    let set = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if let Some(backend) = set(slint_backend) {
        return RendererPlan::Override { backend };
    }
    if let Some(reason) = set(fallback_reason) {
        return RendererPlan::SoftwareFallback { reason };
    }
    RendererPlan::Gpu
}

/// The plan for this process from its environment.
pub fn plan_from_env() -> RendererPlan {
    let read = |name: &str| std::env::var(name).ok();
    plan_renderer(
        read(OVERRIDE_ENV).as_deref(),
        read(FALLBACK_REASON_ENV).as_deref(),
    )
}

/// Make the plan's backend the active Slint platform. `Override` leaves the
/// selection to Slint, which reads `SLINT_BACKEND` itself.
pub fn select(plan: &RendererPlan) -> Result<(), slint::PlatformError> {
    let renderer = match plan {
        RendererPlan::Gpu => "femtovg",
        RendererPlan::SoftwareFallback { .. } => "software",
        RendererPlan::Override { .. } => return Ok(()),
    };
    slint::BackendSelector::new()
        .backend_name("winit".to_string())
        .renderer_name(renderer.to_string())
        .select()
}

/// What the window's rendering notifier reported so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererObservation {
    /// Notifier installed, no GL context yet.
    Waiting,
    /// The renderer offers no rendering notifier (the software renderer).
    NotifierUnsupported,
    /// A GL context is up; no frame rendered yet.
    GlContextUp,
    /// At least one frame was rendered through GL.
    GlFrameRendered,
}

/// What to do when the event loop ended with an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopErrorAction {
    /// Return the error (no fallback applies).
    Return,
    /// Relaunch with the software renderer, carrying this reason.
    RelaunchWithSoftware { reason: String },
}

/// Fallback decision for an event-loop error: only a GPU plan whose window
/// never rendered a GL frame relaunches. An override is never second-guessed,
/// a fallback process never relaunches again, and an error after GL frames
/// were drawn is not a GL startup failure.
pub fn on_event_loop_error(
    plan: &RendererPlan,
    observed: RendererObservation,
    error: &str,
) -> LoopErrorAction {
    if *plan != RendererPlan::Gpu || observed == RendererObservation::GlFrameRendered {
        return LoopErrorAction::Return;
    }
    let mut reason: String = error.trim().chars().take(MAX_REASON_CHARS).collect();
    if reason.is_empty() {
        reason = "the event loop failed without a message".to_string();
    }
    LoopErrorAction::RelaunchWithSoftware { reason }
}

/// The Debug-panel and log line naming the active renderer.
pub fn renderer_text(plan: &RendererPlan, observed: RendererObservation) -> String {
    match plan {
        RendererPlan::Gpu => match observed {
            RendererObservation::Waiting => {
                "femtovg (OpenGL) requested; GL context not up yet".to_string()
            }
            RendererObservation::GlContextUp | RendererObservation::GlFrameRendered => {
                "femtovg (OpenGL)".to_string()
            }
            RendererObservation::NotifierUnsupported => {
                "femtovg requested, but the window reports no OpenGL context".to_string()
            }
        },
        RendererPlan::SoftwareFallback { reason } => {
            format!("software (fallback: the femtovg GPU renderer failed to start: {reason})")
        }
        RendererPlan::Override { backend } => {
            let gl = match observed {
                RendererObservation::Waiting => "GL context not up yet",
                RendererObservation::NotifierUnsupported => "no OpenGL context",
                RendererObservation::GlContextUp | RendererObservation::GlFrameRendered => "OpenGL",
            };
            format!("{OVERRIDE_ENV}={backend} (override); {gl}")
        }
    }
}

/// Watch the window's rendering notifier. `on_change` runs on the UI thread
/// after each observation change (outside the rendering callback).
pub fn observe(
    window: &slint::Window,
    on_change: impl Fn(RendererObservation) + 'static,
) -> Rc<Cell<RendererObservation>> {
    let observed = Rc::new(Cell::new(RendererObservation::Waiting));
    let on_change = Rc::new(on_change);
    let notify = {
        let observed = Rc::clone(&observed);
        let on_change = Rc::clone(&on_change);
        move |next: RendererObservation| {
            if observed.replace(next) == next {
                return;
            }
            let on_change = Rc::clone(&on_change);
            slint::Timer::single_shot(std::time::Duration::ZERO, move || on_change(next));
        }
    };
    let result = window.set_rendering_notifier({
        let notify = notify.clone();
        let observed = Rc::clone(&observed);
        move |state, graphics_api| match state {
            slint::RenderingState::RenderingSetup
                if matches!(graphics_api, slint::GraphicsAPI::NativeOpenGL { .. }) =>
            {
                notify(RendererObservation::GlContextUp);
            }
            slint::RenderingState::AfterRendering
                if observed.get() == RendererObservation::GlContextUp =>
            {
                notify(RendererObservation::GlFrameRendered);
            }
            _ => {}
        }
    });
    if result.is_err() {
        notify(RendererObservation::NotifierUnsupported);
    }
    observed
}

/// Replace this process with a software-renderer launch of the same binary
/// and arguments. Returns only when the relaunch could not start.
///
/// `instance_lock` is the single-instance lock handed over through the
/// relaunch's standard input (`single_instance::HANDOVER_ENV`), so the lock
/// is never released in between; without it the relaunch acquires anew.
pub fn relaunch_with_software(
    reason: &str,
    instance_lock: Option<std::fs::File>,
) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return error,
    };
    let mut command = std::process::Command::new(exe);
    command
        .args(std::env::args_os().skip(1))
        .env(FALLBACK_REASON_ENV, reason);
    match instance_lock {
        Some(lock) => command
            .stdin(lock)
            .env(crate::single_instance::HANDOVER_ENV, "1"),
        None => command.env_remove(crate::single_instance::HANDOVER_ENV),
    };
    command.exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_override_and_no_marker_plans_the_gpu_renderer() {
        assert_eq!(plan_renderer(None, None), RendererPlan::Gpu);
        assert_eq!(plan_renderer(Some("  "), Some("")), RendererPlan::Gpu);
    }

    #[test]
    fn slint_backend_is_always_honored_as_an_override() {
        let plan = plan_renderer(Some("winit-software"), Some("GL failed"));
        assert_eq!(
            plan,
            RendererPlan::Override {
                backend: "winit-software".to_string()
            }
        );
        // An override never relaunches, whatever the notifier saw.
        assert_eq!(
            on_event_loop_error(&plan, RendererObservation::Waiting, "no GL"),
            LoopErrorAction::Return
        );
    }

    #[test]
    fn a_gl_startup_failure_relaunches_with_the_reason() {
        for observed in [
            RendererObservation::Waiting,
            RendererObservation::GlContextUp,
            RendererObservation::NotifierUnsupported,
        ] {
            assert_eq!(
                on_event_loop_error(&RendererPlan::Gpu, observed, " EGL: no display \n"),
                LoopErrorAction::RelaunchWithSoftware {
                    reason: "EGL: no display".to_string()
                },
                "{observed:?}"
            );
        }
        let long = "x".repeat(MAX_REASON_CHARS * 2);
        let LoopErrorAction::RelaunchWithSoftware { reason } =
            on_event_loop_error(&RendererPlan::Gpu, RendererObservation::Waiting, &long)
        else {
            panic!("expected a relaunch");
        };
        assert_eq!(reason.chars().count(), MAX_REASON_CHARS);
        assert!(matches!(
            on_event_loop_error(&RendererPlan::Gpu, RendererObservation::Waiting, ""),
            LoopErrorAction::RelaunchWithSoftware { reason } if !reason.is_empty()
        ));
    }

    #[test]
    fn errors_after_gl_frames_or_in_the_fallback_process_are_returned() {
        assert_eq!(
            on_event_loop_error(
                &RendererPlan::Gpu,
                RendererObservation::GlFrameRendered,
                "later failure"
            ),
            LoopErrorAction::Return
        );
        let fallback = plan_renderer(None, Some("EGL: no display"));
        assert_eq!(
            fallback,
            RendererPlan::SoftwareFallback {
                reason: "EGL: no display".to_string()
            }
        );
        assert_eq!(
            on_event_loop_error(&fallback, RendererObservation::NotifierUnsupported, "x"),
            LoopErrorAction::Return,
            "the fallback process never relaunches again"
        );
    }

    #[test]
    fn the_renderer_line_names_the_renderer_and_any_fallback() {
        assert_eq!(
            renderer_text(&RendererPlan::Gpu, RendererObservation::GlFrameRendered),
            "femtovg (OpenGL)"
        );
        assert!(
            renderer_text(&RendererPlan::Gpu, RendererObservation::Waiting).contains("not up yet")
        );
        let fallback = renderer_text(
            &RendererPlan::SoftwareFallback {
                reason: "EGL: no display".to_string(),
            },
            RendererObservation::NotifierUnsupported,
        );
        assert!(fallback.starts_with("software (fallback:"));
        assert!(fallback.contains("EGL: no display"));
        assert_eq!(
            renderer_text(
                &RendererPlan::Override {
                    backend: "winit-software".to_string()
                },
                RendererObservation::NotifierUnsupported
            ),
            "SLINT_BACKEND=winit-software (override); no OpenGL context"
        );
    }
}
