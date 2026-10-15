//! In-process Wayland compositor double for the d4lc overlay backend.
//!
//! The double (`wayland-server` + `wayland-protocols-wlr` server bindings)
//! advertises `wl_compositor`, `wl_shm`, one `wl_output`, and — for the
//! full run — `zwlr_layer_shell_v1` over connected Unix socket pairs. It records
//! the layer surface's protocol requests so the overlay backend's status
//! model is asserted against real protocol traffic, not against the
//! backend's own bookkeeping:
//!
//! - full run: layer surface on the `Overlay` layer anchored to the
//!   advertised output with all edges and zero margins, click-through
//!   input region covering only the interaction zone, ARGB8888 buffers at
//!   the configured (output) size, ack_configure flow;
//! - no-matching-output run: an output of a different size than the
//!   capture stream — the backend must yield the structured `failed`
//!   state naming the mismatch instead of anchoring to a guessed output;
//! - absent-global run (the GNOME/Mutter analog): no `zwlr_layer_shell_v1`
//!   global at all — the backend must yield the structured `failed` state
//!   with the layer-shell `limitation` string and must not crash.
//!
//! The smoke run additionally writes an evidence bundle under
//! `debug-output/wayland-overlay-smoke/`. This is synthetic evidence only; it never
//! claims a real desktop session.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::io::Write as IoWrite;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use d4lc_core::Rect;
use d4lc_overlay_wayland::{
    create_overlay_on_connection, premultiply_argb, OverlayBackend, OverlayConfig, OverlayFrame,
    OverlayRenderKind,
};
use d4lc_rules::ParagonOverlayElementKind;
use wayland_protocols_wlr::layer_shell::v1::server::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_server::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_region, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
};

// ---------------------------------------------------------------------------
// Protocol recorder.
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Recorder {
    layer: Option<u32>,
    namespace: String,
    anchor: Option<u32>,
    keyboard_interactivity: Option<u32>,
    size: Option<(u32, u32)>,
    margins: Option<(i32, i32, i32, i32)>,
    /// Protocol object id of the `wl_output` the client bound.
    output_id: Option<u32>,
    /// Protocol object id of the `wl_output` passed to `get_layer_surface`
    /// (`None` = the null output).
    layer_surface_output: Option<u32>,
    input_region_set: bool,
    input_region_updates: usize,
    input_region_rects: Vec<(i32, i32, i32, i32)>,
    acked_configures: u32,
    attached_buffers: Vec<(u32, i32, i32, i32)>, // format, width, height, stride
    commits: u32,
    /// Largest size any `wl_shm_pool` reached (create or resize), in bytes.
    max_pool_size: i32,
    /// `wl_buffer.release` events the double sent.
    releases: u32,
}

/// The synthetic output the double advertises: name, current-mode pixel
/// size, and scale factor.
#[derive(Debug, Clone)]
struct OutputSpec {
    name: String,
    width: i32,
    height: i32,
    scale: i32,
}

impl OutputSpec {
    /// The baseline output: 960x540 at scale 1 — the same pixel size as
    /// the synthetic capture stream, so the size match succeeds.
    fn matching_stream() -> Self {
        Self {
            name: "DOUBLE-1".to_string(),
            width: 960,
            height: 540,
            scale: 1,
        }
    }
}

/// Server-side user data attached to each `wl_buffer` so an attach can be
/// traced back to its shm pool creation parameters.
#[derive(Debug, Clone)]
struct BufferInfo {
    format: u32,
    width: i32,
    height: i32,
    stride: i32,
}

struct TestCompositor {
    recorder: Arc<Mutex<Recorder>>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    configured: bool,
    next_serial: u32,
    surface_has_buffer: bool,
    /// The buffer attached since the last commit.
    pending_buffer: Option<wl_buffer::WlBuffer>,
    /// The buffer the last commit made current. A real compositor releases
    /// it once a later commit replaces it; the double does the same.
    shown_buffer: Option<wl_buffer::WlBuffer>,
    /// The advertised output's pixel size: the size the double assigns in
    /// its configure events.
    output_size: (i32, i32),
}

impl TestCompositor {
    fn record<T>(&self, f: impl FnOnce(&mut Recorder) -> T) {
        let mut guard = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard);
    }
}

// ---------------------------------------------------------------------------
// Globals.
// ---------------------------------------------------------------------------

macro_rules! impl_pass_through_global {
    ($resource:ty) => {
        impl GlobalDispatch<$resource, (), TestCompositor> for TestCompositor {
            fn bind(
                _state: &mut Self,
                _handle: &DisplayHandle,
                _client: &Client,
                resource: New<$resource>,
                _global_data: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                data_init.init(resource, ());
            }
        }
    };
}

impl_pass_through_global!(wl_compositor::WlCompositor);
impl_pass_through_global!(wl_shm::WlShm);
impl_pass_through_global!(zwlr_layer_shell_v1::ZwlrLayerShellV1);

impl GlobalDispatch<wl_output::WlOutput, OutputSpec, TestCompositor> for TestCompositor {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wl_output::WlOutput>,
        spec: &OutputSpec,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let output = data_init.init(resource, ());
        let version = output.version();
        output.geometry(
            0,
            0,
            700,
            400,
            wl_output::Subpixel::Unknown,
            "d4lc".to_string(),
            "double".to_string(),
            wl_output::Transform::Normal,
        );
        output.mode(
            wl_output::Mode::Current | wl_output::Mode::Preferred,
            spec.width,
            spec.height,
            60_000,
        );
        if version >= 2 {
            output.scale(spec.scale);
        }
        if version >= 4 {
            output.name(spec.name.clone());
            output.description("d4lc synthetic output".to_string());
        }
        if version >= 2 {
            output.done();
        }
        state.record(|recorder| {
            recorder.output_id = Some(output.id().protocol_id());
        });
    }
}

impl Dispatch<wl_output::WlOutput, (), TestCompositor> for TestCompositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_output::WlOutput,
        _request: wl_output::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<wl_compositor::WlCompositor, (), TestCompositor> for TestCompositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_compositor::WlCompositor,
        request: wl_compositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                data_init.init(id, ());
            }
            wl_compositor::Request::CreateRegion { id } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_region::WlRegion, (), TestCompositor> for TestCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wl_region::WlRegion,
        request: wl_region::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_region::Request::Add {
            x,
            y,
            width,
            height,
        } = request
        {
            state.record(|recorder| {
                recorder.input_region_rects.push((x, y, width, height));
            });
        }
    }
}

impl Dispatch<wl_shm::WlShm, (), TestCompositor> for TestCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wl_shm::WlShm,
        request: wl_shm::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, size } = request {
            state.record(|recorder| recorder.max_pool_size = recorder.max_pool_size.max(size));
            // The pool fd belongs to the client's shared memory; the double
            // does not read pixels server-side, so drop it (the kernel
            // closes the descriptor once both ends drop it).
            drop(fd);
            data_init.init(id, ());
        }
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, (), TestCompositor> for TestCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wl_shm_pool::WlShmPool,
        request: wl_shm_pool::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::Resize { size } = request {
            state.record(|recorder| recorder.max_pool_size = recorder.max_pool_size.max(size));
            return;
        }
        if let wl_shm_pool::Request::CreateBuffer {
            id,
            width,
            height,
            stride,
            format,
            ..
        } = request
        {
            let format = format.into_result().unwrap_or(wl_shm::Format::Argb8888) as u32;
            let info = BufferInfo {
                format,
                width,
                height,
                stride,
            };
            state.record(|recorder| {
                recorder
                    .attached_buffers
                    .push((info.format, info.width, info.height, info.stride));
            });
            data_init.init(id, info);
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferInfo, TestCompositor> for TestCompositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_buffer::WlBuffer,
        _request: wl_buffer::Request,
        _data: &BufferInfo,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, (), TestCompositor> for TestCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wl_surface::WlSurface,
        request: wl_surface::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_surface::Request::Attach { buffer, .. } => {
                state.surface_has_buffer = buffer.is_some();
                state.pending_buffer = buffer;
            }
            wl_surface::Request::SetInputRegion { region: _ } => {
                state.record(|recorder| {
                    recorder.input_region_set = true;
                    recorder.input_region_updates += 1;
                });
            }
            wl_surface::Request::Commit => {
                state.record(|recorder| recorder.commits += 1);
                if let Some(buffer) = state.pending_buffer.take() {
                    let current = buffer.id();
                    if let Some(previous) = state.shown_buffer.replace(buffer) {
                        if previous.id() != current && previous.is_alive() {
                            previous.release();
                            state.record(|recorder| recorder.releases += 1);
                        }
                    }
                }
                if let Some(layer_surface) = state.layer_surface.clone() {
                    if !state.configured && !state.surface_has_buffer {
                        // Initial commit without a buffer: the layer-shell
                        // contract says the compositor answers with the
                        // first configure. The client requested the 0x0
                        // fill with all edges anchored, so the double
                        // assigns the advertised output's size.
                        let serial = state.next_serial;
                        state.next_serial += 1;
                        layer_surface.configure(
                            serial,
                            state.output_size.0 as u32,
                            state.output_size.1 as u32,
                        );
                        state.configured = true;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, (), TestCompositor> for TestCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_shell_v1::Request::GetLayerSurface {
            id,
            layer,
            namespace,
            output,
            ..
        } = request
        {
            let layer_surface = data_init.init(id, ());
            state.record(|recorder| {
                recorder.layer = Some(u32::from(layer));
                recorder.namespace = namespace;
                recorder.layer_surface_output =
                    output.as_ref().map(|output| output.id().protocol_id());
            });
            state.layer_surface = Some(layer_surface);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, (), TestCompositor> for TestCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_layer_surface_v1::Request::SetSize { width, height } => {
                state.record(|recorder| recorder.size = Some((width, height)));
            }
            zwlr_layer_surface_v1::Request::SetAnchor { anchor } => {
                state.record(|recorder| recorder.anchor = Some(u32::from(anchor)));
            }
            zwlr_layer_surface_v1::Request::SetMargin {
                top,
                right,
                bottom,
                left,
            } => {
                state.record(|recorder| recorder.margins = Some((top, right, bottom, left)));
            }
            zwlr_layer_surface_v1::Request::SetKeyboardInteractivity {
                keyboard_interactivity,
            } => {
                state.record(|recorder| {
                    recorder.keyboard_interactivity = Some(u32::from(keyboard_interactivity))
                });
            }
            zwlr_layer_surface_v1::Request::AckConfigure { serial: _ } => {
                state.record(|recorder| recorder.acked_configures += 1);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Double server lifecycle.
// ---------------------------------------------------------------------------

struct DoubleServer {
    display_handle: DisplayHandle,
    recorder: Arc<Mutex<Recorder>>,
    /// The server thread intentionally runs for the process lifetime (the
    /// double has no shutdown protocol); the handle is retained here so the
    /// join token is not silently discarded at struct construction.
    _handle: Option<JoinHandle<()>>,
}

impl DoubleServer {
    fn spawn(with_layer_shell: bool, output: OutputSpec) -> Self {
        let display = Display::new().expect("wayland double display");
        let dhandle = display.handle();
        let recorder = Arc::new(Mutex::new(Recorder::default()));

        let state = TestCompositor {
            recorder: Arc::clone(&recorder),
            layer_surface: None,
            configured: false,
            next_serial: 1,
            surface_has_buffer: false,
            pending_buffer: None,
            shown_buffer: None,
            output_size: (output.width, output.height),
        };

        dhandle.create_global::<TestCompositor, wl_compositor::WlCompositor, _>(6, ());
        dhandle.create_global::<TestCompositor, wl_shm::WlShm, _>(1, ());
        dhandle.create_global::<TestCompositor, wl_output::WlOutput, _>(4, output);
        if with_layer_shell {
            dhandle
                .create_global::<TestCompositor, zwlr_layer_shell_v1::ZwlrLayerShellV1, _>(4, ());
        }

        let handle = std::thread::spawn(move || {
            // Keep the display alive for the thread's lifetime.
            let mut display = display;
            let mut state = state;
            loop {
                let _ = display.dispatch_clients(&mut state);
                let _ = display.flush_clients();
                std::thread::sleep(Duration::from_micros(100));
            }
        });

        Self {
            display_handle: dhandle,
            recorder,
            _handle: Some(handle),
        }
    }

    /// Connect a client to the double. Returns the client `Connection` for
    /// `create_overlay_on_connection`.
    fn connect(&self) -> wayland_client::Connection {
        // Both ends live in this process; no filesystem name or lockfile is
        // needed, so parallel tests cannot collide during socket binding.
        let (client, server) = UnixStream::pair().expect("double socket pair");
        self.display_handle
            .clone()
            .insert_client(server, Arc::new(()))
            .expect("insert double client");
        wayland_client::Connection::from_socket(client).expect("client connection")
    }

    fn wait_for(&self, condition: impl Fn(&Recorder) -> bool, timeout: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < timeout {
            {
                let guard = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                if condition(&guard) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }
}

#[test]
fn double_accepts_independent_in_process_connections() {
    let server = DoubleServer::spawn(true, OutputSpec::matching_stream());
    let first = server.connect();
    let second = server.connect();
    first.roundtrip().expect("first client roundtrip");
    second.roundtrip().expect("second client roundtrip");
}

// ---------------------------------------------------------------------------
// Synthetic descriptor coverage (mirrors the X11 smoke descriptor set).
// ---------------------------------------------------------------------------

fn synthetic_descriptors() -> Vec<d4lc_rules::RuleOverlayDescriptor> {
    use d4lc_rules::{
        RuleOverlayDescriptor, RuleOverlayMarkerType, RuleOverlayShape, RuleOverlayTarget,
        RuleOverlayZOrderGroup, RulePowerKind, RuleSourceNote,
    };

    let source = |method: &'static str, note: &'static str| {
        RuleSourceNote::new("D4Companion.Services/OverlayHandler.cs", method, note)
    };

    // Marker geometry is window-relative, like every real producer emits it:
    // the render planner consumes it verbatim as surface-local coordinates.
    let marker = |target: RuleOverlayTarget,
                  marker_type: RuleOverlayMarkerType,
                  shape: RuleOverlayShape,
                  offset_x: i32,
                  color: &str,
                  text: &str,
                  method: &'static str| {
        RuleOverlayDescriptor {
            target,
            marker_type,
            shape,
            geometry: Rect {
                x: offset_x,
                y: 60,
                width: 24,
                height: 24,
            },
            color: color.into(),
            text: Some(text.to_string()),
            text_origin: None,
            z_order_group: RuleOverlayZOrderGroup::Marker,
            geometry_source: format!("synthetic_{text}_marker").into(),
            source: source(method, "synthetic wayland smoke marker"),
        }
    };

    vec![
        marker(
            RuleOverlayTarget::Affix {
                affix_id: "Affix_AttackSpeed".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
            },
            RuleOverlayMarkerType::Affix,
            RuleOverlayShape::Rectangle,
            40,
            "Gold",
            "AS",
            "OverlayHandler.DrawGraphicsAffixes",
        ),
        marker(
            RuleOverlayTarget::Power {
                power_id: "Aspect_BerserkRipping".to_string(),
                power_kind: RulePowerKind::Aspect,
            },
            RuleOverlayMarkerType::Aspect,
            RuleOverlayShape::Circle,
            100,
            "Orange",
            "Aspect",
            "OverlayHandler.DrawGraphicsAspects",
        ),
        marker(
            RuleOverlayTarget::Power {
                power_id: "Unique_HarlequinCrest".to_string(),
                power_kind: RulePowerKind::Unique,
            },
            RuleOverlayMarkerType::Unique,
            RuleOverlayShape::Circle,
            160,
            "Green",
            "Unique",
            "OverlayHandler.DrawGraphicsAspects",
        ),
        marker(
            RuleOverlayTarget::Rune {
                rune_id: "Rune_Tam".to_string(),
            },
            RuleOverlayMarkerType::Rune,
            RuleOverlayShape::Triangle,
            220,
            "Purple",
            "Rune",
            "OverlayHandler.DrawGraphicsAspects",
        ),
        marker(
            RuleOverlayTarget::Sigil {
                sigil_id: "Dungeon_Ghoa_Ruins".to_string(),
            },
            RuleOverlayMarkerType::Sigil,
            RuleOverlayShape::Rectangle,
            280,
            "Blue",
            "Sigil",
            "OverlayHandler.DrawGraphicsAffixes",
        ),
        RuleOverlayDescriptor {
            target: RuleOverlayTarget::Trade,
            marker_type: RuleOverlayMarkerType::Trade,
            shape: RuleOverlayShape::TextPanel,
            geometry: Rect {
                x: 40,
                y: 420,
                width: 220,
                height: 40,
            },
            color: "Teal".into(),
            text: Some("Trade: 2x Match".to_string()),
            text_origin: None,
            z_order_group: RuleOverlayZOrderGroup::TextPanel,
            geometry_source: "synthetic_trade_panel".into(),
            source: source(
                "OverlayHandler.DrawGraphicsTrading",
                "synthetic trade panel",
            ),
        },
        RuleOverlayDescriptor {
            target: RuleOverlayTarget::Paragon {
                kind: ParagonOverlayElementKind::BuildPanel,
                route_index: Some(0),
                board_index: Some(0),
                board_name: Some("Starting Board".into()),
                node_index: None,
                panel_hit_width: None,
            },
            marker_type: RuleOverlayMarkerType::Paragon,
            shape: RuleOverlayShape::TextPanel,
            geometry: Rect {
                x: 40,
                y: 480,
                width: 220,
                height: 40,
            },
            color: "Cyan".into(),
            text: Some("Board: Starting".to_string()),
            text_origin: None,
            z_order_group: RuleOverlayZOrderGroup::TextPanel,
            geometry_source: "synthetic_paragon_panel".into(),
            source: source(
                "OverlayHandler.DrawGraphicsParagon",
                "synthetic paragon panel",
            ),
        },
    ]
}

fn overlay_geometry() -> Rect {
    Rect {
        x: 40,
        y: 60,
        width: 960,
        height: 540,
    }
}

/// The headless-path tests need a process without a Wayland session. When
/// this one has `WAYLAND_DISPLAY`, run `test_name` again in a child test
/// process without it and fail unless that child passes; returns whether
/// the child ran, in which case the caller has nothing left to do.
fn rerun_without_wayland_session(test_name: &str) -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    let exe =
        std::env::current_exe().unwrap_or_else(|error| panic!("current test binary: {error}"));
    let output = std::process::Command::new(exe)
        .args([test_name, "--exact", "--test-threads=1"])
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap_or_else(|error| panic!("rerun {test_name} without WAYLAND_DISPLAY: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{test_name} without WAYLAND_DISPLAY failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn overlay_config() -> OverlayConfig {
    OverlayConfig {
        window_geometry: overlay_geometry(),
        show_overlay_icon: true,
        overlay_icon_pos_x: 920,
        overlay_icon_pos_y: 24,
        menu_locked: true,
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[test]
fn layer_shell_double_renders_descriptors_with_protocol_requests() {
    // When D4LC_WAYLAND_SMOKE_EXTERNAL is set, the smoke run connects to
    // whatever compositor WAYLAND_DISPLAY advertises (a synthetic nested
    // session, never assumed to be a real desktop) instead of the
    // in-process double. Protocol-request assertions are double-only.
    let external = std::env::var_os("D4LC_WAYLAND_SMOKE_EXTERNAL").is_some();
    let server = (!external).then(|| DoubleServer::spawn(true, OutputSpec::matching_stream()));
    let (conn, compositor_label) = if external {
        (
            wayland_client::Connection::connect_to_env()
                .expect("D4LC_WAYLAND_SMOKE_EXTERNAL set but WAYLAND_DISPLAY unusable"),
            "nested compositor via WAYLAND_DISPLAY (synthetic session, no real desktop)",
        )
    } else {
        let server = server.as_ref().expect("double spawned");
        (server.connect(), "in-process wayland-server double")
    };

    let mut handle = create_overlay_on_connection(conn, compositor_label, overlay_config())
        .expect("create_overlay_on_connection must not hard-fail");

    let status = handle.status();
    assert_eq!(
        status.backend,
        OverlayBackend::RealWayland,
        "expected the real Wayland backend on a layer-shell compositor, got {:?} ({:?})",
        status.backend,
        status.limitation,
    );
    assert!(status.enabled);
    // Requested-vs-confirmed discipline: the ARGB buffer and the overlay
    // layer are requests the client cannot prove are honored.
    assert!(
        status.transparent_requested && !status.transparent_confirmed && !status.transparent,
        "transparency must stay requested-unconfirmed: {status:?}"
    );
    assert!(
        status.always_on_top_requested && !status.always_on_top_confirmed && !status.always_on_top,
        "z-order must stay requested-unconfirmed: {status:?}"
    );
    assert!(
        status.click_through,
        "click-through must be confirmed once the input region survived the first frame"
    );
    assert!(
        !status.keyboard_interactive_requested && !status.keyboard_interactive_confirmed,
        "the overlay requests no keyboard interactivity (layer-shell None mode)"
    );
    assert!(
        status
            .limitation
            .as_deref()
            .unwrap_or_default()
            .contains("compositor-managed z-order"),
        "real-wayland limitation must name the z-order terms: {:?}",
        status.limitation,
    );
    assert!(
        status
            .limitation
            .as_deref()
            .unwrap_or_default()
            .contains("output-anchored placement")
            && status
                .limitation
                .as_deref()
                .unwrap_or_default()
                .contains("identity transform"),
        "real-wayland limitation must state the output-anchored identity chain: {:?}",
        status.limitation,
    );
    assert!(
        !status
            .limitation
            .as_deref()
            .unwrap_or_default()
            .contains("not identity"),
        "a matched scale-1 output must not surface a scale deviation: {:?}",
        status.limitation,
    );

    // Protocol: the layer surface went to the Overlay layer anchored to
    // the advertised output with all edges, zero margins, and the 0x0
    // fill request whose size the configure assigns. (Recorder assertions
    // exist only against the in-process double.)
    if let Some(server) = server.as_ref() {
        assert!(server.wait_for(
            |recorder| recorder.layer == Some(3) && recorder.input_region_set,
            Duration::from_secs(5),
        ));
        let guard = server.recorder.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(guard.layer, Some(3), "layer must be Overlay (3)");
        assert_eq!(guard.namespace, "d4lc-overlay");
        assert_eq!(
            guard.size,
            Some((0, 0)),
            "fill request: the compositor assigns the output size via configure"
        );
        assert_eq!(
            guard.margins,
            Some((0, 0, 0, 0)),
            "output-anchored placement uses zero margins"
        );
        assert_eq!(
            guard.anchor,
            Some(15),
            "anchor must be ALL edges (TOP|BOTTOM|LEFT|RIGHT = 1|2|4|8)"
        );
        assert_eq!(
            guard.layer_surface_output, guard.output_id,
            "the surface must be anchored to the advertised output, not the null output"
        );
        assert_eq!(
            guard.keyboard_interactivity,
            Some(0),
            "keyboard interactivity must be None (0)"
        );
        assert_eq!(
            guard.input_region_rects,
            vec![(883, 12, 50, 50)],
            "input region must cover only the interaction zone (920/1000*960, 24/1000*540)"
        );
        assert!(guard.acked_configures >= 1, "client must ack configures");
    }

    // Render one full frame from synthetic descriptors.
    handle.enable_buffer_snapshot();
    let descriptors = synthetic_descriptors();
    let update_started = Instant::now();
    handle
        .update(OverlayFrame {
            sequence: 243,
            tooltip_found: true,
            hotpath_ms: 7,
            cursor_window_relative: None,
            rule_descriptors: descriptors.clone().into(),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        })
        .expect("wayland overlay update must succeed");
    let update_ms = update_started.elapsed().as_millis();

    // Render-op expansion: same shared pipeline as X11 — z-ordered,
    // window-relative, covering every product marker type.
    let render_ops = handle.last_render_ops().to_vec();
    assert!(!render_ops.is_empty());
    let marker_types = render_ops
        .iter()
        .map(|op| format!("{:?}", op.marker_type))
        .collect::<BTreeSet<_>>();
    for required in [
        "Affix", "Aspect", "Unique", "Rune", "Sigil", "Trade", "Paragon", "Menu",
    ] {
        assert!(
            marker_types.contains(required),
            "synthetic smoke missing marker type {required}; saw {marker_types:?}"
        );
    }
    assert!(
        render_ops
            .iter()
            .all(|op| rect_within(op.geometry, 960, 540)),
        "render ops must stay inside the overlay surface"
    );
    assert!(
        render_ops
            .iter()
            .any(|op| op.kind == OverlayRenderKind::Text && op.text.is_some()),
        "text ops must reach the draw path (rect-only rendering is not parity-equivalent)"
    );

    // The committed buffer: non-blank, in-bounds, with real glyph pixels.
    let (width, height, pixels) = handle
        .committed_buffer_snapshot()
        .expect("buffer snapshot after update");
    assert_eq!((width, height), (960, 540));
    assert!(
        nonblank_pixel_count(&pixels) > 0,
        "committed buffer is blank"
    );
    let text_ops: Vec<_> = render_ops
        .iter()
        .filter(|op| op.kind == OverlayRenderKind::Text)
        .collect();
    assert!(!text_ops.is_empty());
    let glyph_pixels: usize = text_ops
        .iter()
        .map(|op| nonzero_pixels_in_rect(&pixels, width, op.geometry))
        .sum();
    assert!(
        glyph_pixels > 0,
        "text ops must rasterize glyphs client-side; rect-only rendering would leave zero"
    );

    // Protocol: the committed frame attached a premultiplied ARGB8888
    // buffer of the requested geometry (double-recorded).
    if let Some(server) = server.as_ref() {
        let guard = server.recorder.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            guard.attached_buffers.iter().any(|(format, w, h, stride)| {
                *format == wl_shm::Format::Argb8888 as u32
                    && *w == 960
                    && *h == 540
                    && *stride == 960 * 4
            }),
            "expected an ARGB8888 buffer commit at the overlay geometry; saw {:?}",
            guard.attached_buffers,
        );
    }

    write_smoke_bundle(
        &handle,
        &render_ops,
        &descriptors,
        update_ms,
        "in-process wayland-server double (synthetic; no real desktop session)",
    );
}

/// On the real protocol path the first frame attaches and
/// commits, an identical frame skips the draw (no second attach), a changed
/// frame redraws, and an empty frame after content redraws (the hide stays
/// immediate). A skip is reported through `last_update_skipped` and never
/// looks like a draw.
#[test]
fn paragon_panels_update_the_real_wayland_input_region_without_trapping_the_board() {
    use d4lc_rules::{ParagonOverlayElementKind as Kind, RuleOverlayShape, RuleOverlayTarget};
    let server = DoubleServer::spawn(true, OutputSpec::matching_stream());
    let mut config = overlay_config();
    config.show_overlay_icon = false;
    config.menu_locked = false;
    let mut handle =
        create_overlay_on_connection(server.connect(), "in-process double", config).unwrap();
    assert_eq!(handle.status().backend, OverlayBackend::RealWayland);
    let mut step = synthetic_descriptors().remove(0);
    step.target = RuleOverlayTarget::Paragon {
        kind: Kind::StepPanel,
        route_index: Some(0),
        board_index: None,
        board_name: None,
        node_index: None,
        panel_hit_width: None,
    };
    step.shape = RuleOverlayShape::TextPanel;
    step.geometry = Rect {
        x: 0,
        y: 152,
        width: 240,
        height: 50,
    };
    step.text = Some("Step 1 / 2".to_string());
    let mut board = step.clone();
    board.target = RuleOverlayTarget::Paragon {
        kind: Kind::BoardEntry,
        route_index: Some(0),
        board_index: Some(0),
        board_name: None,
        node_index: None,
        panel_hit_width: None,
    };
    board.geometry.y = 204;
    let frame = |sequence, rule_descriptors| OverlayFrame {
        sequence,
        rule_descriptors,
        text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        tooltip_found: false,
        hotpath_ms: 0,
        cursor_window_relative: None,
    };
    let descriptors = vec![step, board];
    handle.update(frame(1, descriptors.clone().into())).unwrap();
    assert!(server.wait_for(|r| r.input_region_updates == 2, Duration::from_secs(5)));
    assert_eq!(
        server.recorder.lock().unwrap().input_region_rects,
        vec![(0, 152, 240, 50), (0, 204, 240, 50)]
    );
    handle.update(frame(2, descriptors.into())).unwrap();
    assert!(handle.last_update_skipped());
    assert_eq!(server.recorder.lock().unwrap().input_region_updates, 2);
    handle.update(frame(3, Vec::new().into())).unwrap();
    assert!(
        server.wait_for(|r| r.input_region_updates == 3, Duration::from_secs(5)),
        "leaving paragon mode must restore an empty input region"
    );
    assert_eq!(server.recorder.lock().unwrap().input_region_rects.len(), 2);
}

#[test]
fn layer_shell_skips_identical_frames_and_redraws_on_change() {
    let server = DoubleServer::spawn(true, OutputSpec::matching_stream());
    let mut handle = create_overlay_on_connection(
        server.connect(),
        "in-process wayland-server double",
        overlay_config(),
    )
    .expect("create_overlay_on_connection must not hard-fail");

    let frame =
        |sequence: u64, rule_descriptors: Vec<d4lc_rules::RuleOverlayDescriptor>| OverlayFrame {
            sequence,
            tooltip_found: !rule_descriptors.is_empty(),
            hotpath_ms: 1,
            cursor_window_relative: None,
            rule_descriptors: rule_descriptors.into(),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        };
    let attach_count = || {
        server
            .recorder
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .attached_buffers
            .len()
    };

    // Warm-up: the first frame of a surface always draws. Creation already
    // round-tripped the transparent initial commit, so this draw is only
    // flushed; wait for its attach before taking the baseline.
    let attaches_after_create = attach_count();
    handle
        .update(frame(1, synthetic_descriptors()))
        .expect("first update");
    assert!(!handle.last_update_skipped());
    assert!(
        server.wait_for(
            |recorder| recorder.attached_buffers.len() > attaches_after_create,
            Duration::from_secs(5),
        ),
        "the first frame must attach a buffer"
    );
    let attaches_after_first = attach_count();

    // The identical frame (same descriptors, same surface) skips: no new
    // attach reaches the compositor.
    handle
        .update(frame(2, synthetic_descriptors()))
        .expect("identical update");
    assert!(
        handle.last_update_skipped(),
        "an identical frame must skip the draw"
    );
    assert_eq!(
        attach_count(),
        attaches_after_first,
        "a skipped frame must not attach a new buffer"
    );

    // A changed descriptor list redraws. The commit is flushed, not awaited
    // so the double's record is polled instead of read back
    // synchronously.
    let mut changed = synthetic_descriptors();
    changed.remove(0);
    handle.update(frame(3, changed)).expect("changed update");
    assert!(!handle.last_update_skipped(), "a changed frame must redraw");
    assert!(
        server.wait_for(
            |recorder| recorder.attached_buffers.len() > attaches_after_first,
            Duration::from_secs(5),
        ),
        "the redrawn frame must attach a buffer"
    );

    // An empty frame after content redraws: the hide is immediate.
    handle.update(frame(4, Vec::new())).expect("empty update");
    assert!(
        !handle.last_update_skipped(),
        "an empty frame after content must redraw"
    );

    // And an identical empty frame skips again.
    handle.update(frame(5, Vec::new())).expect("empty update");
    assert!(handle.last_update_skipped());
}

/// Buffer releases reach the backend between updates, so a long run of
/// changing frames reuses released slots and the SHM pool stays bounded. The
/// live overlay never read the socket after setup: every changed frame grew
/// the pool by a full output buffer.
#[test]
fn changing_frames_reuse_released_buffers_and_keep_the_pool_bounded() {
    let server = DoubleServer::spawn(true, OutputSpec::matching_stream());
    let mut handle = create_overlay_on_connection(
        server.connect(),
        "in-process wayland-server double",
        overlay_config(),
    )
    .expect("create_overlay_on_connection must not hard-fail");
    let recorder = || {
        server
            .recorder
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    };

    for sequence in 0..30u64 {
        let rule_descriptors = if sequence % 2 == 0 {
            synthetic_descriptors()
        } else {
            Vec::new()
        };
        let commits = recorder().commits;
        handle
            .update(OverlayFrame {
                sequence,
                tooltip_found: !rule_descriptors.is_empty(),
                hotpath_ms: 1,
                cursor_window_relative: None,
                rule_descriptors: rule_descriptors.into(),
                text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
            })
            .expect("update");
        assert!(!handle.last_update_skipped(), "every frame changes");
        assert!(
            server.wait_for(
                |recorder| recorder.commits > commits,
                Duration::from_secs(5)
            ),
            "frame {sequence} must be committed"
        );
        // Let the release of the replaced buffer reach the client socket.
        std::thread::sleep(Duration::from_millis(5));
    }

    let recorded = recorder();
    assert!(recorded.releases >= 25, "the double released buffers");
    let buffer_bytes = 960 * 540 * 4;
    assert!(
        recorded.max_pool_size <= 3 * buffer_bytes,
        "the pool must reuse released buffers: {} bytes for {}-byte buffers",
        recorded.max_pool_size,
        buffer_bytes
    );
    assert_eq!(handle.status().draw_error_count, 0);
}

#[test]
fn absent_layer_shell_global_reports_structured_unavailable_without_crash() {
    let server = DoubleServer::spawn(false, OutputSpec::matching_stream());
    let conn = server.connect();

    let mut handle = create_overlay_on_connection(
        conn,
        "in-process wayland-server double without zwlr_layer_shell_v1",
        overlay_config(),
    )
    .expect("create must not hard-fail without the layer-shell global");

    let status = handle.status();
    assert_eq!(
        status.backend,
        OverlayBackend::Failed,
        "absent layer-shell global must yield the structured failed state"
    );
    let limitation = status.limitation.clone().expect("limitation required");
    assert!(
        limitation.contains("zwlr_layer_shell_v1"),
        "limitation must name the missing global: {limitation}"
    );
    assert!(
        limitation.contains("GNOME"),
        "limitation must name the GNOME/Mutter family: {limitation}"
    );
    // Nothing was requested, so nothing may be claimed confirmed.
    assert!(!status.transparent_requested && !status.transparent_confirmed);
    assert!(!status.always_on_top_requested && !status.always_on_top_confirmed);
    assert!(!status.click_through);
    assert!(!status.keyboard_interactive_requested && !status.keyboard_interactive_confirmed);

    // The live loop must keep running: updates on the failed handle build
    // the shared render ops (the descriptor pipeline stays functional) but
    // never draw, and the metadata is preserved.
    handle
        .update(OverlayFrame {
            sequence: 1,
            tooltip_found: false,
            hotpath_ms: 0,
            cursor_window_relative: None,
            rule_descriptors: synthetic_descriptors().into(),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        })
        .expect("update on a failed handle must not error the live loop");
    assert_eq!(handle.status().backend, OverlayBackend::Failed);
    assert!(
        !handle.last_render_ops().is_empty(),
        "the shared render-op expansion must keep working on the failed backend"
    );
}

#[test]
fn no_matching_output_is_a_failure() {
    // The capture stream (960x540) covers exactly one output; the double
    // advertises a differently-sized one. The surface is never anchored to
    // a guessed output: the structured failed state names the mismatch.
    let server = DoubleServer::spawn(
        true,
        OutputSpec {
            name: "OTHER-1".to_string(),
            width: 1280,
            height: 720,
            scale: 1,
        },
    );
    let conn = server.connect();

    let mut handle = create_overlay_on_connection(
        conn,
        "in-process double whose output does not match the stream",
        overlay_config(),
    )
    .expect("create must not hard-fail without a matching output");

    let status = handle.status();
    assert_eq!(
        status.backend,
        OverlayBackend::Failed,
        "a stream no output matches must yield the structured failed state"
    );
    let limitation = status.limitation.clone().expect("limitation required");
    assert!(
        limitation.contains("no advertised output matches the capture stream size 960x540"),
        "the limitation must name the unmatched stream size: {limitation}"
    );
    assert!(
        limitation.contains("OTHER-1 1280x720"),
        "the limitation must name the observed output for diagnosis: {limitation}"
    );
    assert!(
        limitation.contains("mispositioned on a guessed output"),
        "the limitation must state why nothing is created: {limitation}"
    );
    // Nothing was requested, so nothing may be claimed confirmed.
    assert!(!status.transparent_requested && !status.click_through);

    // The live loop keeps running: the descriptor pipeline stays
    // functional on the failed handle.
    handle
        .update(OverlayFrame {
            sequence: 2,
            tooltip_found: false,
            hotpath_ms: 0,
            cursor_window_relative: None,
            rule_descriptors: synthetic_descriptors().into(),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        })
        .expect("update on the no-match failed handle must not error the live loop");
    assert!(!handle.last_render_ops().is_empty());
}

#[test]
fn scale_two_output_surfaces_the_non_identity_mapping_limitation() {
    // A matched output whose scale is not 1 breaks the identity mapping
    // assumption: the backend still anchors (the pixel size matches the
    // stream) but the status must say the placement is not guaranteed.
    let server = DoubleServer::spawn(
        true,
        OutputSpec {
            name: "HI-DPI".to_string(),
            width: 960,
            height: 540,
            scale: 2,
        },
    );
    let conn = server.connect();

    let mut handle = create_overlay_on_connection(
        conn,
        "in-process double with a scale-2 output matching the stream size",
        overlay_config(),
    )
    .expect("create must not hard-fail on a scale-2 output");

    let status = handle.status();
    assert_eq!(
        status.backend,
        OverlayBackend::RealWayland,
        "the stream size still matches the output's mode pixel size"
    );
    let limitation = status.limitation.clone().expect("limitation required");
    assert!(
        limitation.contains("output-anchored placement") && limitation.contains("HI-DPI"),
        "the identity chain line must name the anchored output: {limitation}"
    );
    assert!(
        limitation.contains("output scale 2")
            && limitation.contains("not identity")
            && limitation.contains("placement is not guaranteed"),
        "scale != 1 must surface the non-identity mapping: {limitation}"
    );

    handle
        .update(OverlayFrame {
            sequence: 3,
            tooltip_found: false,
            hotpath_ms: 0,
            cursor_window_relative: None,
            rule_descriptors: synthetic_descriptors().into(),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        })
        .expect("update on the scale-2 handle must not error the live loop");
    assert!(!handle.last_render_ops().is_empty());
}

#[test]
fn set_window_geometry_on_the_real_backend_is_an_explicit_noop_with_status() {
    let server = DoubleServer::spawn(true, OutputSpec::matching_stream());
    let conn = server.connect();

    let mut handle = create_overlay_on_connection(
        conn,
        "in-process double for set_window_geometry behavior",
        overlay_config(),
    )
    .expect("create must not hard-fail");
    assert_eq!(handle.status().backend, OverlayBackend::RealWayland);

    // Zero geometry stays a hard validation error.
    assert!(handle
        .set_window_geometry(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 600,
        })
        .is_err());

    // The identical stream rect is a clean no-op.
    handle
        .set_window_geometry(overlay_geometry())
        .expect("an identical geometry update must succeed");

    let commits_before = {
        let guard = server.recorder.lock().unwrap_or_else(|e| e.into_inner());
        guard.commits
    };

    // A different game-window geometry: the output-anchored surface is
    // untouched, the update is recorded in the limitation, and the
    // geometry record stays the stream rect the surface maps 1:1.
    handle
        .set_window_geometry(Rect {
            x: 100,
            y: 50,
            width: 800,
            height: 600,
        })
        .expect("a changed geometry update must not hard-fail on the wayland backend");
    let status = handle.status();
    assert_eq!(
        status.window_geometry,
        overlay_geometry(),
        "the record must stay the capture stream rect"
    );
    let limitation = status.limitation.clone().expect("limitation required");
    assert!(
        limitation.contains("geometry update to 800x600 at (100, 50) not applied")
            && limitation.contains("output-anchored to the capture stream"),
        "the limitation must record the not-applied update: {limitation}"
    );

    let commits_after = {
        let guard = server.recorder.lock().unwrap_or_else(|e| e.into_inner());
        guard.commits
    };
    assert_eq!(
        commits_before, commits_after,
        "the output-anchored surface must not be moved or resized"
    );

    // The live loop keeps drawing over the untouched surface.
    handle
        .update(OverlayFrame {
            sequence: 4,
            tooltip_found: false,
            hotpath_ms: 0,
            cursor_window_relative: None,
            rule_descriptors: synthetic_descriptors().into(),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        })
        .expect("update after a geometry update must not error the live loop");
    assert!(!handle.last_render_ops().is_empty());
}

#[test]
fn set_window_geometry_on_headless_follows_the_reported_geometry() {
    // Without a Wayland session there is no surface and nothing can be
    // mispositioned: the record and the per-mille menu zone follow the
    // reported geometry like the X11 headless bookkeeping.
    if rerun_without_wayland_session(
        "set_window_geometry_on_headless_follows_the_reported_geometry",
    ) {
        return;
    }
    let mut handle = d4lc_overlay_wayland::create_overlay(overlay_config())
        .expect("headless create must not fail");

    assert!(handle
        .set_window_geometry(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        })
        .is_err());
    handle
        .set_window_geometry(Rect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("a headless geometry update must succeed");
    let status = handle.status();
    assert_eq!(
        status.window_geometry,
        Rect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        },
        "the headless record follows the reported geometry"
    );
    assert_eq!(
        status.interaction_zone,
        d4lc_overlay_x11::menu_interaction_zone(
            status.window_geometry,
            overlay_config().overlay_icon_pos_x,
            overlay_config().overlay_icon_pos_y,
        ),
        "the per-mille menu zone follows the reported geometry"
    );
}

#[test]
fn headless_status_records_no_wayland_session() {
    // The always-available baseline: the same status fields the X11 crate
    // models, with a limitation naming the missing Wayland session.
    if rerun_without_wayland_session("headless_status_records_no_wayland_session") {
        return;
    }
    let handle = d4lc_overlay_wayland::create_overlay(overlay_config())
        .expect("headless create must not fail");
    let status = handle.status();
    assert_eq!(status.backend, OverlayBackend::HeadlessNoop);
    let limitation = status.limitation.clone().expect("limitation required");
    assert!(
        limitation.contains("WAYLAND_DISPLAY"),
        "headless limitation must name the missing session: {limitation}"
    );
    assert!(!status.transparent_requested && !status.click_through);
}

// ---------------------------------------------------------------------------
// Evidence helpers.
// ---------------------------------------------------------------------------

fn rect_within(rect: Rect, width: u32, height: u32) -> bool {
    rect.x >= 0
        && rect.y >= 0
        && rect.x + rect.width as i32 <= width as i32
        && rect.y + rect.height as i32 <= height as i32
}

fn nonblank_pixel_count(argb: &[u8]) -> usize {
    argb.as_chunks::<4>()
        .0
        .iter()
        .any(|px| px.iter().any(|b| *b != 0)) as usize
}

fn nonzero_pixels_in_rect(argb: &[u8], width: u32, rect: Rect) -> usize {
    let mut count = 0;
    for y in rect.y.max(0)..(rect.y + rect.height as i32) {
        for x in rect.x.max(0)..(rect.x + rect.width as i32) {
            if x < 0 || y < 0 || x >= width as i32 {
                continue;
            }
            let idx = ((y as usize) * (width as usize) + x as usize) * 4;
            if idx + 4 <= argb.len() && argb[idx..idx + 4].iter().any(|b| *b != 0) {
                count += 1;
            }
        }
    }
    count
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn write_json(path: &PathBuf, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("artifact dir");
    }
    let mut file = std::fs::File::create(path).expect("artifact file");
    file.write_all(
        serde_json::to_string_pretty(value)
            .unwrap_or_default()
            .as_bytes(),
    )
    .expect("artifact write");
}

fn write_ppm(path: &PathBuf, width: u32, height: u32, argb: &[u8]) {
    // Premultiplied ARGB8888 (b,g,r,a little-endian) to binary RGB PPM.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("artifact dir");
    }
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    for px in argb.as_chunks::<4>().0 {
        // Undo premultiplication for display; fully transparent pixels
        // render as white so the PPM shows the surface shape.
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        if a == 0 {
            out.extend_from_slice(&[255, 255, 255]);
        } else {
            let unpremultiply =
                |c: u8| u8::try_from(u16::from(c) * 255 / u16::from(a)).unwrap_or(255);
            out.extend_from_slice(&[unpremultiply(r), unpremultiply(g), unpremultiply(b)]);
        }
    }
    std::fs::write(path, out).expect("ppm write");
}

fn descriptor_json(descriptor: &d4lc_rules::RuleOverlayDescriptor) -> serde_json::Value {
    serde_json::json!({
        "marker_type": format!("{:?}", descriptor.marker_type),
        "shape": format!("{:?}", descriptor.shape),
        "z_order_group": format!("{:?}", descriptor.z_order_group),
        "geometry": {
            "x": descriptor.geometry.x,
            "y": descriptor.geometry.y,
            "width": descriptor.geometry.width,
            "height": descriptor.geometry.height,
        },
        "color": descriptor.color,
        "text": descriptor.text,
        "geometry_source": descriptor.geometry_source,
        "upstream_method": descriptor.source.upstream_method,
    })
}

fn render_op_json(op: &d4lc_overlay_wayland::OverlayRenderOp) -> serde_json::Value {
    serde_json::json!({
        "kind": format!("{:?}", op.kind),
        "marker_type": format!("{:?}", op.marker_type),
        "z_order_group": format!("{:?}", op.z_order_group),
        "geometry": {
            "x": op.geometry.x,
            "y": op.geometry.y,
            "width": op.geometry.width,
            "height": op.geometry.height,
        },
        "color": op.color,
        "text": op.text,
        "geometry_source": op.geometry_source,
        "upstream_method": op.source.as_ref().map(|s| s.upstream_method.clone()),
    })
}

fn write_smoke_bundle(
    handle: &d4lc_overlay_wayland::OverlayHandle,
    render_ops: &[d4lc_overlay_wayland::OverlayRenderOp],
    descriptors: &[d4lc_rules::RuleOverlayDescriptor],
    update_ms: u128,
    compositor_label: &str,
) {
    let artifact_dir = workspace_root().join("debug-output/wayland-overlay-smoke");
    let status = handle.status();
    let marker_types = render_ops
        .iter()
        .map(|op| format!("{:?}", op.marker_type))
        .collect::<BTreeSet<_>>();

    if let Some((width, height, pixels)) = handle.committed_buffer_snapshot() {
        write_ppm(
            &artifact_dir.join("rendered-overlay.ppm"),
            width,
            height,
            &pixels,
        );
    }

    write_json(
        &artifact_dir.join("render-plan.json"),
        &serde_json::json!({
            "compositor_source": handle.compositor_source().unwrap_or(compositor_label),
            "frame": {
                "sequence": 243,
                "tooltip_found": true,
                "hotpath_ms": 7,
            },
            "descriptors": descriptors.iter().map(descriptor_json).collect::<Vec<_>>(),
            "render_ops": render_ops.iter().map(render_op_json).collect::<Vec<_>>(),
        }),
    );

    write_json(
        &artifact_dir.join("capability-report.json"),
        &serde_json::json!({
            "status": "rendered",
            "evidence_kind": "synthetic",
            "compositor_source": handle.compositor_source().unwrap_or(compositor_label),
            "backend": status.backend.as_str(),
            "window_geometry": {
                "x": status.window_geometry.x,
                "y": status.window_geometry.y,
                "width": status.window_geometry.width,
                "height": status.window_geometry.height,
            },
            "interaction_zone": {
                "x": status.interaction_zone.x,
                "y": status.interaction_zone.y,
                "width": status.interaction_zone.width,
                "height": status.interaction_zone.height,
            },
            "menu_locked": status.menu_locked,
            "menu_visible": status.menu_visible,
            "transparent_requested": status.transparent_requested,
            "transparent_confirmed": status.transparent_confirmed,
            "always_on_top_requested": status.always_on_top_requested,
            "always_on_top_confirmed": status.always_on_top_confirmed,
            "click_through": status.click_through,
            "keyboard_interactive_requested": status.keyboard_interactive_requested,
            "keyboard_interactive_confirmed": status.keyboard_interactive_confirmed,
            "limitation": status.limitation,
            "draw_error_count": status.draw_error_count,
            "last_error": status.last_error,
            "render_op_count": render_ops.len(),
            "marker_types": marker_types.into_iter().collect::<Vec<_>>(),
            "overlay_buffer_nonblank": true,
            "geometry_within_surface": true,
            "text_rasterization": {
                "font": "assigned family resolved through fontdb (see licenses/README.md)",
                "license": "swash Apache-2.0 OR MIT / fontdb MIT; no font data is redistributed",
                "rasterizer": "shared d4lc-overlay-x11 text path (swash glyph masks), \
                               same coverage-to-pixel rule as the X11 put_image payload",
            },
            "color_pipeline": {
                "palette": "shared d4lc-overlay-x11 color_pixel (straight alpha)",
                "shm_format": "wl_shm ARGB8888 premultiplied",
                "premultiply_sample": premultiply_argb(0xCC1A1A1A),
            },
        }),
    );

    write_json(
        &artifact_dir.join("timing-summary.json"),
        &serde_json::json!({
            "update_ms": update_ms,
            "artifact_dir": artifact_dir.display().to_string(),
            "note": "synthetic in-process double; not a real desktop session measurement",
        }),
    );

    let readme = artifact_dir.join("README.txt");
    std::fs::write(
        &readme,
        "Synthetic Wayland overlay smoke evidence.\n\
         Produced against an in-process wayland-server compositor double —\n\
         or, when D4LC_WAYLAND_SMOKE_CONNECT=1, a nested compositor exposed\n\
         via WAYLAND_DISPLAY. This is NOT a real desktop session: no claim\n\
         of live Diablo IV overlay behavior may be derived from it.\n\
         The layer-surface protocol requests were recorded by the double and\n\
         asserted in crates/d4lc-overlay-wayland/tests/wayland_protocol.rs.\n",
    )
    .expect("readme write");
}
