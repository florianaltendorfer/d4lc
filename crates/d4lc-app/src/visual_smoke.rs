#[cfg(feature = "x11rb-backend")]
use std::cell::RefCell;
#[cfg(feature = "x11rb-backend")]
use std::path::{Path, PathBuf};
#[cfg(feature = "x11rb-backend")]
use std::rc::Rc;
use std::sync::Mutex;
#[cfg(feature = "x11rb-backend")]
use std::time::Duration;

#[cfg(feature = "x11rb-backend")]
use slint::{ComponentHandle, SharedString, VecModel};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const VISUAL_SMOKE_ENV: &str = "D4LC_VISUAL_SMOKE";
const SKIP_HINT: &str =
    "visual smoke skipped: set D4LC_VISUAL_SMOKE=1 to run the X11 screenshot harness";
const REPORT_FILE: &str = "visual-smoke-report.txt";
const SKIP_FILE: &str = "skip-report.txt";
const FAILURE_FILE: &str = "failure-report.txt";
const DISALLOWED_VISIBLE_MARKERS: &[&str] = &["content pending", "coming soon"];

#[cfg(feature = "x11rb-backend")]
#[test]
#[ignore = "desktop visual smoke: run through scripts/verify_visual_smoke.sh"]
fn visual_smoke_generates_artifacts_or_skip_report() {
    assert_eq!(
        std::env::var(VISUAL_SMOKE_ENV).ok().as_deref(),
        Some("1"),
        "{SKIP_HINT}"
    );

    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let artifact_dir = reset_artifacts();

    match run_visual_smoke(&artifact_dir) {
        Ok(report) => write_text_file(&artifact_dir.join(REPORT_FILE), &report.to_text())
            .unwrap_or_else(|error| panic!("failed to write visual smoke report: {error}")),
        Err(SmokeError::Skipped(reason)) => write_text_file(&artifact_dir.join(SKIP_FILE), &reason)
            .unwrap_or_else(|error| panic!("failed to write visual smoke skip report: {error}")),
        Err(SmokeError::Failed(reason)) => {
            write_text_file(&artifact_dir.join(FAILURE_FILE), &reason).unwrap_or_else(|error| {
                panic!("failed to write visual smoke failure report: {error}")
            });
            panic!("{reason}");
        }
    }
}

#[cfg(feature = "x11rb-backend")]
const CAPTURE_SPECS: [CaptureSpec; 10] = [
    CaptureSpec {
        label: "affix",
        file_name: "01-affix.ppm",
        expected_tab: 0,
        expected_dialog_id: None,
        source_path: "crates/d4lc-app/ui/screens/affix_screen.slint",
        source_contents: include_str!("../ui/screens/affix_screen.slint"),
        after_capture: Some(UiAction::ResizeWindow {
            width: 1150.0,
            height: 440.0,
            tab: 0,
        }),
    },
    // The affix screen at a short window: the gear-slot rail must keep its
    // own bounded viewport (the slot sections scroll inside it, nothing is
    // laid out past the window bottom) while the presets block stays put.
    // The default 1150x625 window leaves the rail enough room that a
    // regression in the height chain can hide; this capture pins the short
    // case in the gate.
    CaptureSpec {
        label: "affix-short-window",
        file_name: "10-affix-short-window.ppm",
        expected_tab: 0,
        expected_dialog_id: None,
        source_path: "crates/d4lc-app/ui/screens/affix_screen.slint",
        source_contents: include_str!("../ui/screens/affix_screen.slint"),
        after_capture: Some(UiAction::ResizeWindow {
            width: 1150.0,
            height: 625.0,
            tab: 1,
        }),
    },
    CaptureSpec {
        label: "trade",
        file_name: "02-trade.ppm",
        expected_tab: 1,
        expected_dialog_id: None,
        source_path: "crates/d4lc-app/ui/screens/trade_screen.slint",
        source_contents: include_str!("../ui/screens/trade_screen.slint"),
        after_capture: Some(UiAction::SetActiveTab(2)),
    },
    CaptureSpec {
        label: "logging",
        file_name: "03-logging.ppm",
        expected_tab: 2,
        expected_dialog_id: None,
        source_path: "crates/d4lc-app/ui/screens/logging_screen.slint",
        source_contents: include_str!("../ui/screens/logging_screen.slint"),
        after_capture: Some(UiAction::SetActiveTab(3)),
    },
    CaptureSpec {
        label: "debug",
        file_name: "04-debug.ppm",
        expected_tab: 3,
        expected_dialog_id: None,
        source_path: "crates/d4lc-app/ui/screens/debug_screen.slint",
        source_contents: include_str!("../ui/screens/debug_screen.slint"),
        after_capture: Some(UiAction::SetActiveTab(4)),
    },
    CaptureSpec {
        label: "settings",
        file_name: "05-settings.ppm",
        expected_tab: 4,
        expected_dialog_id: None,
        source_path: "crates/d4lc-app/ui/screens/settings_screen.slint",
        source_contents: include_str!("../ui/screens/settings_screen.slint"),
        after_capture: Some(UiAction::OpenOverlayDialog),
    },
    CaptureSpec {
        label: "overlay-config-dialog",
        file_name: "06-overlay-config-dialog.ppm",
        expected_tab: 4,
        expected_dialog_id: Some("overlay_config"),
        source_path: "crates/d4lc-app/ui/dialogs/overlay_config_dialog.slint",
        source_contents: include_str!("../ui/dialogs/overlay_config_dialog.slint"),
        after_capture: Some(UiAction::OpenImportPresetWithText),
    },
    CaptureSpec {
        label: "import-preset-dialog",
        file_name: "07-import-preset-dialog.ppm",
        expected_tab: 4,
        expected_dialog_id: Some("import_preset"),
        source_path: "crates/d4lc-app/ui/dialogs/import_preset_dialog.slint",
        source_contents: include_str!("../ui/dialogs/import_preset_dialog.slint"),
        after_capture: Some(UiAction::OpenImportLocalTab),
    },
    CaptureSpec {
        label: "import-local-tab",
        file_name: "08-import-local-tab.ppm",
        expected_tab: 4,
        expected_dialog_id: Some("import_preset"),
        source_path: "crates/d4lc-app/ui/dialogs/import_preset_dialog.slint",
        source_contents: include_str!("../ui/dialogs/import_preset_dialog.slint"),
        after_capture: Some(UiAction::OpenImportMergeTab),
    },
    CaptureSpec {
        label: "import-merge-tab",
        file_name: "09-import-merge-tab.ppm",
        expected_tab: 4,
        expected_dialog_id: Some("import_preset"),
        source_path: "crates/d4lc-app/ui/dialogs/import_preset_dialog.slint",
        source_contents: include_str!("../ui/dialogs/import_preset_dialog.slint"),
        after_capture: None,
    },
];

#[cfg(feature = "x11rb-backend")]
#[derive(Clone, Copy)]
struct CaptureSpec {
    label: &'static str,
    file_name: &'static str,
    expected_tab: i32,
    expected_dialog_id: Option<&'static str>,
    source_path: &'static str,
    source_contents: &'static str,
    after_capture: Option<UiAction>,
}

#[cfg(feature = "x11rb-backend")]
#[derive(Clone, Copy)]
enum UiAction {
    SetActiveTab(i32),
    // Resize the live window and continue the walk on `tab`. The short-window
    // affix capture needs a window that leaves the rail far less room than
    // its content, and the captures that follow need the default size back.
    ResizeWindow { width: f32, height: f32, tab: i32 },
    OpenOverlayDialog,
    OpenImportPresetWithText,
    // The import dialog stays open; the capture walks to the Local and
    // Merge tabs through the same in-out tab property the tab strip
    // writes (the Rust-side switch the title clicks produce).
    OpenImportLocalTab,
    OpenImportMergeTab,
}

#[cfg(feature = "x11rb-backend")]
#[derive(Clone)]
struct CaptureRecord {
    label: &'static str,
    file_name: &'static str,
    source_path: &'static str,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    distinct_colors: usize,
    window_id_hex: String,
}

#[cfg(feature = "x11rb-backend")]
#[derive(Clone)]
struct SmokeReport {
    display: String,
    xvfb_spawned: bool,
    window_title: String,
    placeholder_check: Vec<(&'static str, Vec<&'static str>)>,
    captures: Vec<CaptureRecord>,
}

#[cfg(feature = "x11rb-backend")]
#[derive(Clone)]
struct ScenarioContext {
    app_weak: slint::Weak<crate::slint_generated::AppWindow>,
    state: Rc<RefCell<crate::app_state::AppState>>,
    x11_capture: Rc<X11WindowCapture>,
    artifact_dir: PathBuf,
    window_title: String,
    report: Rc<RefCell<SmokeReport>>,
    error_message: Rc<RefCell<Option<String>>>,
}

#[cfg(feature = "x11rb-backend")]
impl SmokeReport {
    fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("status=passed\n");
        out.push_str(&format!("display={}\n", self.display));
        out.push_str(&format!("xvfb_spawned={}\n", self.xvfb_spawned));
        out.push_str(&format!("window_title={}\n", self.window_title));
        out.push_str("placeholder_check=passed\n");
        for (source_path, markers) in &self.placeholder_check {
            out.push_str(&format!(
                "placeholder_source={} markers={}\n",
                source_path,
                if markers.is_empty() {
                    "none".to_string()
                } else {
                    markers.join(",")
                }
            ));
        }
        out.push_str(&format!("capture_count={}\n", self.captures.len()));
        for capture in &self.captures {
            out.push_str(&format!(
                "capture={} file={} size={}x{} bytes_per_pixel={} distinct_colors={} window_id={} source={}\n",
                capture.label,
                capture.file_name,
                capture.width,
                capture.height,
                capture.bytes_per_pixel,
                capture.distinct_colors,
                capture.window_id_hex,
                capture.source_path,
            ));
        }
        out
    }
}

#[cfg(feature = "x11rb-backend")]
enum SmokeError {
    Skipped(String),
    Failed(String),
}

#[cfg(feature = "x11rb-backend")]
#[derive(Clone)]
struct EnvGuard {
    display: Option<String>,
    xdg_session_type: Option<String>,
    wayland_display: Option<String>,
    winit_unix_backend: Option<String>,
}

#[cfg(feature = "x11rb-backend")]
impl EnvGuard {
    fn capture() -> Self {
        Self {
            display: std::env::var("DISPLAY").ok(),
            xdg_session_type: std::env::var("XDG_SESSION_TYPE").ok(),
            wayland_display: std::env::var("WAYLAND_DISPLAY").ok(),
            winit_unix_backend: std::env::var("WINIT_UNIX_BACKEND").ok(),
        }
    }

    fn restore(&self) {
        restore_env_var("DISPLAY", self.display.as_deref());
        restore_env_var("XDG_SESSION_TYPE", self.xdg_session_type.as_deref());
        restore_env_var("WAYLAND_DISPLAY", self.wayland_display.as_deref());
        restore_env_var("WINIT_UNIX_BACKEND", self.winit_unix_backend.as_deref());
    }
}

#[cfg(feature = "x11rb-backend")]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(feature = "x11rb-backend")]
struct DisplayHarness {
    _env_guard: EnvGuard,
    active_display: String,
    xvfb: Option<std::process::Child>,
}

/// Whether the harness may capture the process's existing `DISPLAY`.
///
/// Only a real X11 session's display is reusable. A Wayland session's
/// `DISPLAY` belongs to XWayland, which does not keep a client window's
/// pixels readable through `GetImage`; reusing it makes the gate capture a
/// blank window. A Wayland session therefore always gets the private Xvfb.
#[cfg(feature = "x11rb-backend")]
fn may_reuse_host_display(xdg_session_type: Option<&str>, wayland_display: Option<&str>) -> bool {
    xdg_session_type != Some("wayland") && !wayland_display.is_some_and(|value| !value.is_empty())
}

#[cfg(feature = "x11rb-backend")]
impl DisplayHarness {
    fn acquire() -> Result<Self, SmokeError> {
        let env_guard = EnvGuard::capture();
        if may_reuse_host_display(
            env_guard.xdg_session_type.as_deref(),
            env_guard.wayland_display.as_deref(),
        ) {
            if let Some(existing_display) = env_guard.display.clone() {
                if x11rb::rust_connection::RustConnection::connect(Some(existing_display.as_str()))
                    .is_ok()
                {
                    force_x11_env(&existing_display);
                    return Ok(Self {
                        _env_guard: env_guard,
                        active_display: existing_display,
                        xvfb: None,
                    });
                }
            }
        }

        Self::spawn_xvfb(env_guard)
    }

    fn spawn_xvfb(env_guard: EnvGuard) -> Result<Self, SmokeError> {
        use std::io::BufRead as _;

        // Xvfb picks a free display number itself (`-displayfd`) and reports
        // it once it accepts connections, so parallel runs never share a
        // display or connect to a server that is still starting.
        let mut child = match std::process::Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "1280x720x24",
                "-nolisten",
                "tcp",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SmokeError::Skipped(
                    "DISPLAY is unavailable and Xvfb is not installed or not on PATH".to_string(),
                ));
            }
            Err(error) => {
                return Err(SmokeError::Skipped(format!(
                    "DISPLAY is unavailable and Xvfb could not be started: {error}"
                )));
            }
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SmokeError::Skipped(
                "DISPLAY is unavailable and the Xvfb display number could not be read".to_string(),
            ));
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send(line);
        });
        let number = receiver
            .recv_timeout(Duration::from_secs(10))
            .ok()
            .and_then(|line| line.trim().parse::<u16>().ok());
        let Some(number) = number else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SmokeError::Skipped(
                "DISPLAY is unavailable and Xvfb did not report a display".to_string(),
            ));
        };

        let active_display = format!(":{number}");
        force_x11_env(&active_display);
        for _ in 0..20 {
            if x11rb::rust_connection::RustConnection::connect(Some(active_display.as_str()))
                .is_ok()
            {
                return Ok(Self {
                    _env_guard: env_guard,
                    active_display,
                    xvfb: Some(child),
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = child.kill();
        let _ = child.wait();
        env_guard.restore();
        Err(SmokeError::Skipped(
            "DISPLAY is unavailable and the Xvfb session did not accept connections".to_string(),
        ))
    }

    fn display(&self) -> &str {
        &self.active_display
    }

    fn xvfb_spawned(&self) -> bool {
        self.xvfb.is_some()
    }
}

#[cfg(feature = "x11rb-backend")]
impl Drop for DisplayHarness {
    fn drop(&mut self) {
        if let Some(child) = self.xvfb.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(feature = "x11rb-backend")]
fn run_visual_smoke(artifact_dir: &Path) -> Result<SmokeReport, SmokeError> {
    let display_harness = DisplayHarness::acquire()?;
    let placeholder_check = verify_placeholder_sources()?;
    let window_title = format!("d4lc visual smoke {}", std::process::id());
    let (app, state) = build_smoke_app(&window_title)?;

    let x11_capture = Rc::new(
        X11WindowCapture::connect().map_err(|error| SmokeError::Failed(error.to_string()))?,
    );
    let report = Rc::new(RefCell::new(SmokeReport {
        display: display_harness.display().to_string(),
        xvfb_spawned: display_harness.xvfb_spawned(),
        window_title: window_title.clone(),
        placeholder_check,
        captures: Vec::new(),
    }));
    let error_message = Rc::new(RefCell::new(None::<String>));

    app.show()
        .map_err(|error| SmokeError::Failed(format!("AppWindow::show failed: {error}")))?;
    schedule_capture_step(
        ScenarioContext {
            app_weak: app.as_weak(),
            state,
            x11_capture,
            artifact_dir: artifact_dir.to_path_buf(),
            window_title,
            report: report.clone(),
            error_message: error_message.clone(),
        },
        0,
    );

    slint::run_event_loop()
        .map_err(|error| SmokeError::Failed(format!("slint::run_event_loop failed: {error}")))?;

    if let Some(error) = error_message.borrow_mut().take() {
        return Err(SmokeError::Failed(error));
    }

    let final_report = report.borrow().clone();
    Ok(final_report)
}

#[cfg(feature = "x11rb-backend")]
fn schedule_capture_step(context: ScenarioContext, step_index: usize) {
    slint::Timer::single_shot(
        if step_index == 0 {
            Duration::from_millis(300)
        } else {
            Duration::from_millis(150)
        },
        move || {
            let Some(app) = context.app_weak.upgrade() else {
                *context.error_message.borrow_mut() =
                    Some("visual smoke AppWindow was dropped".to_string());
                let _ = slint::quit_event_loop();
                return;
            };
            let spec = CAPTURE_SPECS[step_index];

            let result = (|| -> Result<(), String> {
                verify_ui_state(&app, spec)?;
                let capture = context.x11_capture.capture_window(
                    &context.window_title,
                    &context.artifact_dir.join(spec.file_name),
                )?;
                context.report.borrow_mut().captures.push(CaptureRecord {
                    label: spec.label,
                    file_name: spec.file_name,
                    source_path: spec.source_path,
                    width: capture.width,
                    height: capture.height,
                    bytes_per_pixel: capture.bytes_per_pixel,
                    distinct_colors: capture.distinct_colors,
                    window_id_hex: format!("0x{:x}", capture.window_id),
                });

                match spec.after_capture {
                    Some(UiAction::SetActiveTab(tab)) => {
                        app.set_dialog_visible(false);
                        app.set_active_tab(tab);
                    }
                    Some(UiAction::ResizeWindow { width, height, tab }) => {
                        app.set_dialog_visible(false);
                        app.set_active_tab(tab);
                        // 1150x440 keeps every fixed row of the affix screen
                        // (control row, presets block) inside the window while
                        // leaving the rail far less room than its content:
                        // the viewport must clamp and scroll, not grow.
                        app.window()
                            .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
                                width, height,
                            )));
                    }
                    Some(UiAction::OpenOverlayDialog) => {
                        crate::actions::settings::handle_open_overlay_config(
                            &app,
                            &mut context.state.borrow_mut(),
                        );
                    }
                    Some(UiAction::OpenImportPresetWithText) => {
                        crate::actions::build_import_dialogs::handle_open_import_preset(
                            &app,
                            &mut context.state.borrow_mut(),
                        );
                        // Type into the Maxroll URL field after the open
                        // reset so the capture shows real input text, not
                        // the placeholder: the vertical centering of the
                        // shared DarkLineEdit input is what this capture
                        // pins down. The default website selection is
                        // Maxroll.gg, so the dialog opens on tab 5.
                        app.set_import_maxroll_url("t3jx0916".into());
                    }
                    Some(UiAction::OpenImportLocalTab) => {
                        app.set_import_tab_index(0);
                    }
                    Some(UiAction::OpenImportMergeTab) => {
                        app.set_import_tab_index(1);
                    }
                    None => {}
                }

                Ok(())
            })();

            if let Err(error) = result {
                *context.error_message.borrow_mut() = Some(error);
                let _ = app.hide();
                let _ = slint::quit_event_loop();
                return;
            }

            if step_index + 1 < CAPTURE_SPECS.len() {
                schedule_capture_step(context.clone(), step_index + 1);
            } else {
                let _ = app.hide();
                let _ = slint::quit_event_loop();
            }
        },
    );
}

#[cfg(feature = "x11rb-backend")]
fn verify_ui_state(
    app: &crate::slint_generated::AppWindow,
    spec: CaptureSpec,
) -> Result<(), String> {
    if app.get_active_tab() != spec.expected_tab {
        return Err(format!(
            "capture {} expected active tab {} but saw {}",
            spec.label,
            spec.expected_tab,
            app.get_active_tab()
        ));
    }

    match spec.expected_dialog_id {
        Some(expected_dialog_id) => {
            if !app.get_dialog_visible() {
                return Err(format!(
                    "capture {} expected dialog-visible=true",
                    spec.label
                ));
            }
            if app.get_dialog_id().as_str() != expected_dialog_id {
                return Err(format!(
                    "capture {} expected dialog-id {} but saw {}",
                    spec.label,
                    expected_dialog_id,
                    app.get_dialog_id()
                ));
            }
        }
        None => {
            if app.get_dialog_visible() {
                return Err(format!("capture {} expected no visible dialog", spec.label));
            }
        }
    }

    Ok(())
}

/// The smoke app's deterministic wanted preset (see `build_smoke_app`).
#[cfg(feature = "x11rb-backend")]
const SMOKE_PRESET_NAME: &str = "Visual Smoke";

/// Seed the wanted entries the affix capture renders.
///
/// The ids and the stored labels mirror the shipped preset shape so the
/// capture pins the resolution behavior the live screen shows: ids are
/// individual `IdName` members (`Life`, `X2_DamageType_Fire`), the stored
/// labels are the preset's own `Label` text (`Maximum Life`, `x Fire
/// Damage Multiplier`, which must not be what the screen shows), and the last entry has no data row at all (the orphan
/// fallback keeps its own label).
#[cfg(feature = "x11rb-backend")]
fn seed_smoke_preset(state: &mut crate::app_state::AppState) {
    use d4lc_gui::{AffixPresetEditorPreset, PresetEntry, PresetEntryKind};

    let affix = |id: &str, label: Option<&str>, item_type: &str| PresetEntry {
        kind: PresetEntryKind::Affix,
        id: id.to_string(),
        label: label.map(str::to_string),
        item_type: Some(item_type.to_string()),
        color: "#008000".to_string(),
        is_any_type: false,
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    };
    let entries = vec![
        affix("CoreStat_Willpower", None, "helm"),
        affix("Life", None, "chest"),
        // The same id wanted for a second slot keeps its own rail chip.
        affix("Life", None, "weapon"),
        affix(
            "X2_DamageType_Fire",
            Some("x Fire Damage Multiplier"),
            "weapon",
        ),
        affix(
            "Lucky_Hit_Resource",
            Some("Lucky Hit: Up to a 15 Chance to Restore Primary Resource"),
            "ring",
        ),
        affix("AttackSpeed", None, "gloves"),
        PresetEntry::non_affix(
            PresetEntryKind::Aspect,
            "Legendary_Generic_110",
            None,
            "#3B82F6",
        ),
        PresetEntry::non_affix(PresetEntryKind::Sigil, "Torture", None, "#F59E0B"),
        affix("NotInTheLocalizedData", Some("Unknown Affix"), "boots"),
    ];
    state.affix_preset_editor =
        d4lc_gui::AffixPresetEditorState::new(vec![AffixPresetEditorPreset {
            name: SMOKE_PRESET_NAME.to_string(),
            entries,
        }]);
    state.affix_preset_editor.selected_preset_name = Some(SMOKE_PRESET_NAME.to_string());
}

#[cfg(feature = "x11rb-backend")]
fn build_smoke_app(
    window_title: &str,
) -> Result<
    (
        crate::slint_generated::AppWindow,
        Rc<RefCell<crate::app_state::AppState>>,
    ),
    SmokeError,
> {
    let state = Rc::new(RefCell::new(crate::app_state::AppState::default_startup()));
    let app = crate::slint_generated::AppWindow::new()
        .map_err(|error| SmokeError::Failed(format!("AppWindow::new failed: {error}")))?;
    // A window picks a language from the desktop locale when it is created;
    // the captures are English like the default setting.
    crate::i18n::select("en-US")
        .map_err(|error| SmokeError::Failed(format!("UI language not applied: {error}")))?;

    app.set_window_title(SharedString::from(window_title));
    app.set_window_topmost(false);

    let tab_labels = state
        .borrow()
        .shell
        .tabs
        .iter()
        .map(crate::map_tab_label)
        .map(SharedString::from)
        .collect::<Vec<_>>();
    app.set_tab_labels(Rc::new(VecModel::from(tab_labels)).into());
    app.set_active_tab(0);

    // The affix capture must exercise the wanted-entry rail and the wanted
    // cards, not only the empty catalog: every visible affix defect so far was
    // in the wanted rows (chip geometry, the resolved description text). The
    // seed mirrors the shape of the shipped upstream preset — one entry per
    // gear slot, data ids stored one `IdName` list member at a time the way
    // the live `AffixPresets-v2.json` does (`Life`, `X2_DamageType_Fire`, ...
    // while the index row carries the `;`-joined list), the stored preset
    // labels (`Maximum Life`, `x Fire Damage Multiplier`), one family entry
    // and one id the data does not carry (the orphan path).
    seed_smoke_preset(&mut state.borrow_mut());

    {
        let state = state.borrow();
        app.set_debug_mode(state.shell.settings.debug_mode);
        app.set_dev_mode(state.shell.settings.dev_mode);
        app.set_check_updates(state.shell.settings.check_for_updates);
        app.set_launch_minimized(state.shell.settings.launch_minimized);
        app.set_minimize_to_tray(state.shell.settings.minimize_to_tray);
        app.set_controller_mode(state.shell.settings.controller_mode);
        app.set_paragon_mode(state.shell.settings.paragon_mode_active);

        let names: Vec<SharedString> = state
            .affix_preset_editor
            .presets
            .iter()
            .map(|preset| SharedString::from(preset.name.as_str()))
            .collect();
        app.set_preset_names(Rc::new(VecModel::from(names)).into());

        app.set_overlay_status_text("Offline".into());
        app.set_runtime_status_text(state.runtime_status.status_message.clone().into());
        app.set_current_item_name(
            state
                .runtime_status
                .current_item_name
                .clone()
                .unwrap_or_default()
                .into(),
        );
        app.set_current_item_parse_status(
            state
                .runtime_status
                .current_item_parse_status
                .clone()
                .into(),
        );
        app.set_debug_frame_sequence(state.runtime_status.debug_frame_sequence as i32);
        app.set_debug_hotpath_ms(state.runtime_status.debug_hotpath_ms as i32);
        app.set_debug_tooltip_found(state.runtime_status.debug_tooltip_found);
    }

    // The preset ComboBox selection is set after `preset-names` is published
    // (setting it first makes the std ComboBox drop the unknown selection).
    app.set_selected_preset(SharedString::from(SMOKE_PRESET_NAME));

    // The filter-icon strip shows the saved toggles, as at startup.
    crate::actions::affix::set_filter_toggle_models(&app, &state.borrow().shell.settings.toggles);

    // The affix capture must exercise the wanted-entry rail and the wanted
    // cards, not only the empty catalog: the visible defects were all in
    // the wanted rows (chip geometry, the resolved description text). The
    // seed mirrors the shape of the shipped upstream preset — one entry per
    // gear slot, data ids stored one list-member at a time (the live
    // `AffixPresets-v2.json` stores `Life`, `X2_DamageType_Fire`, ... while
    // the index row carries the `;`-joined `IdName`), the stored preset
    // label text (`Maximum Life`, `x Fire Damage Multiplier`), one family
    // entry and one id the data does not carry (the orphan path).
    seed_smoke_preset(&mut state.borrow_mut());

    // Project the affix screen's catalog models through the live seam so the
    // affix capture shows the real wrap grid (config cell + fixed-size cards
    // with their markers and class badges), not an empty catalog. The loader
    // is the same one the running app uses; a missing localized data set
    // leaves the grid with the preset's own rows only.
    crate::actions::affix::project_affix_entries(&app, &mut state.borrow_mut());

    // Settings description panel: the 05-settings capture must show the
    // rendered label/value grid and the language-ready dimming, not an
    // empty panel. The values mirror the vendored
    // `assets/original/systempresets/systempresets.json` entry for
    // `1080p_SMF.zip` — the same catalog `project_preset_details` reads —
    // pinned here so the capture stays deterministic and independent of
    // the config root on the machine running the gate.
    app.set_preset_details(crate::slint_generated::PresetDetail {
        filename: "1080p_SMF.zip".into(),
        resolution: "e.g. 1920x1080".into(),
        config: "SDR (HDR off) with medium font".into(),
        tooltip_width: "375".into(),
        tooltip_height_type: "200".into(),
        brightness_threshold: "70 / 255".into(),
        affix_area_height_offset_top: "8".into(),
        affix_area_height_offset_bottom: "8".into(),
        affix_aspect_area_width_offset: "18".into(),
        aspect_area_height_offset_top: "8".into(),
        paragon_node_size: "29".into(),
        paragon_node_size_collapsed: "16".into(),
        language_ready_de_de: true,
        language_ready_en_us: true,
        language_ready_es_es: true,
        language_ready_es_mx: true,
        language_ready_fr_fr: true,
        language_ready_it_it: false,
        language_ready_ja_jp: false,
        language_ready_ko_kr: false,
        language_ready_pl_pl: true,
        language_ready_pt_br: true,
        language_ready_ru_ru: true,
        language_ready_tr_tr: false,
        language_ready_zh_cn: false,
        language_ready_zh_tw: false,
    });

    Ok((app, state))
}

#[cfg(feature = "x11rb-backend")]
fn verify_placeholder_sources() -> Result<Vec<(&'static str, Vec<&'static str>)>, SmokeError> {
    let mut results = Vec::new();
    for spec in CAPTURE_SPECS {
        if results.iter().any(|(path, _)| *path == spec.source_path) {
            continue;
        }
        let markers = find_disallowed_markers(spec.source_contents);
        if !markers.is_empty() {
            return Err(SmokeError::Failed(format!(
                "{} still contains disallowed visible placeholder markers: {}",
                spec.source_path,
                markers.join(", ")
            )));
        }
        results.push((spec.source_path, markers));
    }
    Ok(results)
}

#[cfg(feature = "x11rb-backend")]
fn find_disallowed_markers(source_contents: &str) -> Vec<&'static str> {
    let lowered = source_contents.to_ascii_lowercase();
    DISALLOWED_VISIBLE_MARKERS
        .iter()
        .copied()
        .filter(|marker| lowered.contains(marker))
        .collect()
}

#[cfg(feature = "x11rb-backend")]
fn reset_artifacts() -> PathBuf {
    let dir = artifact_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", dir.display()));
    for file_name in [
        REPORT_FILE,
        SKIP_FILE,
        FAILURE_FILE,
        "01-affix.ppm",
        "10-affix-short-window.ppm",
        "02-trade.ppm",
        "03-logging.ppm",
        "04-debug.ppm",
        "05-settings.ppm",
        "06-overlay-config-dialog.ppm",
        "07-import-preset-dialog.ppm",
        "08-import-local-tab.ppm",
        "09-import-merge-tab.ppm",
    ] {
        let path = dir.join(file_name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("failed to remove {}: {error}", path.display()),
        }
    }
    dir
}

#[cfg(feature = "x11rb-backend")]
fn artifact_dir() -> PathBuf {
    // A run may redirect the artifact tree (e.g. to keep drift captures for a
    // Slint version bump next to the default gate output). Relative overrides
    // resolve against the workspace root, exactly like the default below.
    match std::env::var_os("D4LC_VISUAL_SMOKE_DIR") {
        Some(dir) if !dir.is_empty() => {
            let path = PathBuf::from(dir);
            if path.is_absolute() {
                path
            } else {
                workspace_root().join(path)
            }
        }
        _ => workspace_root().join("debug-output/visual-smoke"),
    }
}

#[cfg(feature = "x11rb-backend")]
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("workspace root missing from {}", env!("CARGO_MANIFEST_DIR")))
        .to_path_buf()
}

#[cfg(feature = "x11rb-backend")]
fn write_text_file(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(feature = "x11rb-backend")]
fn force_x11_env(display: &str) {
    std::env::set_var("DISPLAY", display);
    std::env::set_var("XDG_SESSION_TYPE", "x11");
    std::env::set_var("WINIT_UNIX_BACKEND", "x11");
    // Screenshot determinism: pin the CPU software renderer so the gate does
    // not depend on a GL/EGL provider being present on headless X servers
    // (requires the slint "renderer-software" feature, enabled on d4lc-app).
    std::env::set_var("SLINT_BACKEND", "winit-software");
    std::env::remove_var("WAYLAND_DISPLAY");
}

#[cfg(feature = "x11rb-backend")]
fn restore_env_var(name: &str, value: Option<&str>) {
    if let Some(value) = value {
        std::env::set_var(name, value);
    } else {
        std::env::remove_var(name);
    }
}

/// Bounds for waiting on the window manager: the polls (50 ms apart) for the
/// window to become viewable and the attempts (100 ms apart) at `GetImage`
/// while it answers `BadMatch`.
#[cfg(feature = "x11rb-backend")]
const VIEWABLE_WAIT_POLLS: u32 = 100;
#[cfg(feature = "x11rb-backend")]
const GET_IMAGE_MATCH_ATTEMPTS: u32 = 50;

#[cfg(feature = "x11rb-backend")]
struct X11WindowCapture {
    conn: x11rb::rust_connection::RustConnection,
    root_window: u32,
    wm_name_atom: u32,
    net_wm_name_atom: u32,
}

#[cfg(feature = "x11rb-backend")]
impl X11WindowCapture {
    fn connect() -> Result<Self, String> {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::ConnectionExt as _;

        let (conn, screen_num) = x11rb::rust_connection::RustConnection::connect(None)
            .map_err(|error| format!("connect visual smoke X11 capture: {error}"))?;
        let root_window = conn.setup().roots[screen_num].root;
        let wm_name_atom = conn
            .intern_atom(false, b"WM_NAME")
            .map_err(|error| format!("intern_atom WM_NAME: {error}"))?
            .reply()
            .map_err(|error| format!("intern_atom WM_NAME reply: {error}"))?
            .atom;
        let net_wm_name_atom = conn
            .intern_atom(false, b"_NET_WM_NAME")
            .map_err(|error| format!("intern_atom _NET_WM_NAME: {error}"))?
            .reply()
            .map_err(|error| format!("intern_atom _NET_WM_NAME reply: {error}"))?
            .atom;
        Ok(Self {
            conn,
            root_window,
            wm_name_atom,
            net_wm_name_atom,
        })
    }

    fn capture_window(&self, title: &str, path: &Path) -> Result<CaptureObservation, String> {
        use x11rb::errors::ReplyError;
        use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

        let window_id = self.find_window_by_title_with_retry(title)?;
        self.wait_until_viewable(window_id)?;

        // A slow runner (software GL on headless Xvfb) can present the first
        // Slint frame well after the capture timer fires, so a uniform
        // capture is retried instead of failing the gate immediately.
        let mut blank_attempts = 0u32;
        let mut match_attempts = 0u32;
        loop {
            // Read the geometry on every attempt: the window manager can
            // still be resizing or moving a window that just mapped.
            let geometry = self
                .conn
                .get_geometry(window_id)
                .map_err(|error| format!("get_geometry {window_id:#x}: {error}"))?
                .reply()
                .map_err(|error| format!("get_geometry {window_id:#x} reply: {error}"))?;
            if geometry.width < 800 || geometry.height < 400 {
                return Err(format!(
                    "captured window {window_id:#x} is too small for a desktop smoke gate: {}x{}",
                    geometry.width, geometry.height
                ));
            }
            let pixel_count =
                usize::from(geometry.width).saturating_mul(usize::from(geometry.height));

            let reply = match self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    window_id,
                    0,
                    0,
                    geometry.width,
                    geometry.height,
                    u32::MAX,
                )
                .map_err(|error| format!("get_image {window_id:#x}: {error}"))?
                .reply()
            {
                Ok(reply) => reply,
                // GetImage answers Match while the window is not viewable or
                // its rectangle is not wholly on the screen, both of which
                // are transient right after a map, resize or move. A window
                // that stays that way is a real failure and is reported with
                // the state the server saw.
                Err(ReplyError::X11Error(error)) if is_bad_match(&error) => {
                    match_attempts += 1;
                    if match_attempts >= GET_IMAGE_MATCH_ATTEMPTS {
                        return Err(format!(
                            "get_image {window_id:#x} kept failing with BadMatch after \
                             {match_attempts} attempts: {}",
                            self.describe_window(window_id)
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(error) => return Err(format!("get_image {window_id:#x} reply: {error}")),
            };

            let bytes_per_pixel = reply.data.len() / pixel_count.max(1);
            if bytes_per_pixel == 0
                || bytes_per_pixel.saturating_mul(pixel_count) != reply.data.len()
            {
                return Err(format!(
                    "unsupported image layout for window {window_id:#x}: {} bytes across {} pixels",
                    reply.data.len(),
                    pixel_count
                ));
            }

            let distinct_colors = count_distinct_colors(bytes_per_pixel, &reply.data, 32);
            if image_has_nonblank_pixels(bytes_per_pixel, &reply.data) && distinct_colors >= 8 {
                write_ppm_artifact(
                    path,
                    u32::from(geometry.width),
                    u32::from(geometry.height),
                    bytes_per_pixel,
                    &reply.data,
                )?;

                return Ok(CaptureObservation {
                    window_id,
                    width: u32::from(geometry.width),
                    height: u32::from(geometry.height),
                    bytes_per_pixel,
                    distinct_colors,
                });
            }

            blank_attempts += 1;
            if blank_attempts >= 15 {
                return Err(format!(
                    "captured window {window_id:#x} did not render meaningful pixels (distinct_colors={distinct_colors})"
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Wait until the server reports the window viewable (mapped with every
    /// ancestor mapped): the state `GetImage` requires, reached a moment after
    /// the window manager maps the window.
    fn wait_until_viewable(&self, window_id: u32) -> Result<(), String> {
        use x11rb::protocol::xproto::{ConnectionExt as _, MapState};

        let mut last = None;
        for _ in 0..VIEWABLE_WAIT_POLLS {
            let attributes = self
                .conn
                .get_window_attributes(window_id)
                .map_err(|error| format!("get_window_attributes {window_id:#x}: {error}"))?
                .reply()
                .map_err(|error| format!("get_window_attributes {window_id:#x} reply: {error}"))?;
            if attributes.map_state == MapState::VIEWABLE {
                return Ok(());
            }
            last = Some(attributes.map_state);
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "window {window_id:#x} never became viewable (last map state {last:?})"
        ))
    }

    /// What the server reports about a window: for a `GetImage` failure
    /// message that says why the window could not be read.
    fn describe_window(&self, window_id: u32) -> String {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;

        let map_state = self
            .conn
            .get_window_attributes(window_id)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|attributes| format!("{:?}", attributes.map_state));
        let geometry = self
            .conn
            .get_geometry(window_id)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let origin = self
            .conn
            .translate_coordinates(window_id, self.root_window, 0, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let screen = self
            .conn
            .setup()
            .roots
            .iter()
            .find(|screen| screen.root == self.root_window)
            .map(|screen| (screen.width_in_pixels, screen.height_in_pixels));
        format!(
            "map state {map_state:?}, size {:?}, position on screen {:?}, screen {screen:?}; \
             the window is not viewable or not wholly inside the screen",
            geometry.map(|geometry| (geometry.width, geometry.height)),
            origin.map(|origin| (origin.dst_x, origin.dst_y)),
        )
    }

    fn find_window_by_title_with_retry(&self, title: &str) -> Result<u32, String> {
        for _ in 0..20 {
            if let Some(window_id) = self.find_window_by_title(self.root_window, title)? {
                return Ok(window_id);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!("failed to find X11 window with title {title:?}"))
    }

    fn find_window_by_title(&self, root: u32, title: &str) -> Result<Option<u32>, String> {
        use x11rb::protocol::xproto::ConnectionExt as _;

        if self.window_title(root)?.as_deref() == Some(title) {
            return Ok(Some(root));
        }

        let children = self
            .conn
            .query_tree(root)
            .map_err(|error| format!("query_tree {root:#x}: {error}"))?
            .reply()
            .map_err(|error| format!("query_tree {root:#x} reply: {error}"))?
            .children;

        for child in children {
            if let Some(window_id) = self.find_window_by_title(child, title)? {
                return Ok(Some(window_id));
            }
        }

        Ok(None)
    }

    fn window_title(&self, window_id: u32) -> Result<Option<String>, String> {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

        for atom in [self.net_wm_name_atom, self.wm_name_atom] {
            let reply = self
                .conn
                .get_property(false, window_id, atom, AtomEnum::ANY, 0, 1024)
                .map_err(|error| format!("get_property {window_id:#x}: {error}"))?
                .reply()
                .map_err(|error| format!("get_property {window_id:#x} reply: {error}"))?;
            if reply.value.is_empty() {
                continue;
            }

            let title = String::from_utf8_lossy(&reply.value)
                .trim_matches(char::from(0))
                .trim()
                .to_string();
            if !title.is_empty() {
                return Ok(Some(title));
            }
        }

        Ok(None)
    }
}

/// Whether the X11 error is the protocol's `Match` error (code 8).
#[cfg(feature = "x11rb-backend")]
fn is_bad_match(error: &x11rb::x11_utils::X11Error) -> bool {
    error.error_code == x11rb::protocol::xproto::MATCH_ERROR
}

#[cfg(feature = "x11rb-backend")]
struct CaptureObservation {
    window_id: u32,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    distinct_colors: usize,
}

#[cfg(feature = "x11rb-backend")]
fn write_ppm_artifact(
    path: &Path,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    bytes: &[u8],
) -> Result<(), String> {
    let expected_len = width as usize * height as usize * bytes_per_pixel;
    if bytes.len() != expected_len {
        return Err(format!(
            "ppm artifact {} expected {expected_len} bytes but got {}",
            path.display(),
            bytes.len()
        ));
    }

    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for chunk in bytes.chunks_exact(bytes_per_pixel) {
        let [r, g, b] = chunk_to_rgb(chunk, bytes_per_pixel);
        ppm.extend_from_slice(&[r, g, b]);
    }

    std::fs::write(path, ppm).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(feature = "x11rb-backend")]
fn image_has_nonblank_pixels(bytes_per_pixel: usize, bytes: &[u8]) -> bool {
    bytes.chunks_exact(bytes_per_pixel).any(|chunk| {
        let [r, g, b] = chunk_to_rgb(chunk, bytes_per_pixel);
        r != 0 || g != 0 || b != 0
    })
}

#[cfg(feature = "x11rb-backend")]
fn count_distinct_colors(bytes_per_pixel: usize, bytes: &[u8], limit: usize) -> usize {
    let mut colors: Vec<[u8; 3]> = Vec::new();
    for chunk in bytes.chunks_exact(bytes_per_pixel) {
        let color = chunk_to_rgb(chunk, bytes_per_pixel);
        if !colors.contains(&color) {
            colors.push(color);
            if colors.len() >= limit {
                return colors.len();
            }
        }
    }
    colors.len()
}

#[cfg(feature = "x11rb-backend")]
fn chunk_to_rgb(chunk: &[u8], bytes_per_pixel: usize) -> [u8; 3] {
    match bytes_per_pixel {
        1 => [chunk[0], chunk[0], chunk[0]],
        2 => [chunk[1], chunk[0], chunk[0]],
        _ => [chunk[2], chunk[1], chunk[0]],
    }
}

#[cfg(all(test, feature = "x11rb-backend"))]
mod tests {
    use super::may_reuse_host_display;

    #[test]
    fn a_wayland_session_never_reuses_the_xwayland_display() {
        assert!(!may_reuse_host_display(Some("wayland"), None));
        assert!(!may_reuse_host_display(None, Some("wayland-0")));
        assert!(!may_reuse_host_display(Some("x11"), Some("wayland-0")));
        assert!(may_reuse_host_display(Some("x11"), None));
        assert!(may_reuse_host_display(None, None));
        assert!(may_reuse_host_display(None, Some("")));
    }
}
