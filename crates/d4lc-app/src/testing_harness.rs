//! Real-input GUI test harness (headless).
//!
//! Replaces the windowing system with the pinned Slint testing backend and
//! embeds its MCP server, so tests drive the UI the way a user does:
//!
//! - **Clicks / drags** go through real pointer events
//!   (`PointerMoved`/`Pressed`/`Released`) dispatched at element centers via
//!   hit-testing — either synchronously through `ElementHandle`
//!   (`mock_single_click`, mock-time paced) or through the embedded MCP
//!   server (`click_element`, `drag_element`, including double clicks).
//! - **Keyboard** goes through real `KeyPressed`/`KeyReleased` window events
//!   via the MCP server's `dispatch_key_event` (the Rust `ElementHandle` API
//!   does not offer key dispatch).
//!
//! Everything runs without a display server: the testing backend renders
//! through the software rasterizer, and the mock clock
//! (`mock_elapsed_time`, pumped on the event-loop thread) drives timers —
//! including the app's 50 ms runtime drain timer — deterministically.
//! Tests never skip: if the testing backend cannot be installed (e.g. a
//! windowing backend was initialized first in this process) or an element
//! cannot be found (e.g. the UI was built without debug info), the test
//! fails loudly.
//!
//! The process session is forced headless for the suite's duration
//! (`HeadlessSessionGuard`): `DISPLAY` / `XDG_SESSION_TYPE` /
//! `WAYLAND_DISPLAY` are removed before the wired app is built, so the
//! global-hotkey backend stays on its documented no-thread path instead of
//! taking real keyboard grabs from a test process and blocking the UI thread
//! in a join on its X11 event loop (see the guard for the reproduced stall).
//!
//! Every UI-thread round trip is bounded (`UI_ROUNDTRIP_TIMEOUT`): a stalled
//! event loop fails the test in seconds naming the helper that never came
//! back, instead of hanging it for hours.
//!
//! Every pilot is its own `#[test] #[ignore]` function and owns its process:
//! the testing backend installs once per process, so a scenario family cannot
//! share a window with another family. `scripts/run_pilot_suite.py` starts one
//! process per pilot and runs them in parallel, and
//! `scripts/verify_local.sh` is the gate that calls it. The suites below are
//! therefore independent: each seeds the state it asserts on and never
//! relies on what an earlier family left in the shared window.
//!
//! Element lookup needs the Slint compiler's debug info: `build.rs` enables
//! `CompilerConfiguration::with_debug_info` for non-release profiles.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use i_slint_backend_testing::ElementHandle;
use serde::Deserialize;
use serde_json::{json, Value};
use slint::{ComponentHandle, Weak};

use crate::actions::affix::CATALOG_WINDOW_PAGE;
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

mod evidence;
mod launch;
mod mcp_client;
mod widgets;

use evidence::{
    assert_png_and_dimensions, take_evidence_screenshot, workspace_root, write_screenshot,
};
use launch::{
    on_ui, on_ui_ctx, reserve_free_port, seed_standard_editor_presets, start_harness,
    stop_mock_clock_pump, SEED_ATTACK_SPEED_TEXT, SEED_LIFE_TEXT,
};
use mcp_client::{
    accessible_enabled, descendants_with_role, parse_handle, recorded_events, str_property,
    McpClient, McpHandle,
};
use widgets::{
    assert_dialog_closed, assert_label_findable, click_control_by_label, click_on_label,
    click_on_ui, dialog_state, find_button_by_label, find_by_element_id, find_text_input,
    input_value, label_count, read_ui, type_text,
};

// ── Process-wide harness state ────────────────────────────────────────────

/// Scenarios reach the wired window through this weak handle; the strong
/// `AppWindow` is leaked on the UI thread so it lives for the process.
static APP_WINDOW: OnceLock<Weak<AppWindow>> = OnceLock::new();

/// Scenarios reach the wired `AppState` through this handle — the same
/// `Arc` the wiring closures hold. It exists so a scenario can SEED
/// state that has no UI create path (non-Affix preset entries); every
/// interaction under test is still driven by real input.
static HARNESS_APP_STATE: OnceLock<std::sync::Arc<std::sync::Mutex<AppState>>> = OnceLock::new();

/// Signals that the UI thread installed the platform, created the wired app
/// and is about to enter the event loop (or failed trying).
static UI_START: Mutex<Option<std::sync::mpsc::Sender<Result<(), String>>>> = Mutex::new(None);

/// Stops the mock-clock pump when the scenario suite ends.
static PUMP_SHUTDOWN: Mutex<bool> = Mutex::new(false);

const PUMP_TICK_MS: u64 = 10;

/// Errors that must surface as loud test failures, never skips.
type HarnessResult<T> = Result<T, String>;

fn harness_panic(context: &str, error: &str) -> ! {
    panic!("real-input GUI harness: {context}: {error}")
}

// ── Pilot suite ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
