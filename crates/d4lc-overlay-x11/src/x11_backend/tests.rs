use super::{arc_from_rect, triangle_points_from_rect};
use crate::test_support::*;
use crate::*;
use d4lc_core::{Point, Rect};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[test]
fn tuning_prism_colors_and_circle_arc_use_upstream_values() {
    // Upstream outline brush for every prism circle is `Colors.Black`;
    // `LightBlue` and `Pink` mirror `System.Windows.Media.Colors`
    // exactly.
    assert_eq!(color_pixel("black"), 0xFF000000);
    assert_eq!(color_pixel("LightBlue"), 0xFFADD8E6);
    assert_eq!(color_pixel("pink"), 0xFFFFC0CB);

    // Full-circle arc from the radius-10 bounding square.
    let arc = arc_from_rect(Rect {
        x: 390,
        y: 240,
        width: 20,
        height: 20,
    });
    assert_eq!(arc.x, 390);
    assert_eq!(arc.y, 240);
    assert_eq!(arc.width, 20);
    assert_eq!(arc.height, 20);
    assert_eq!(arc.angle1, 0);
    assert_eq!(arc.angle2, 360 * 64);

    // Greater-affix triangle vertices: base on the bottom edge, apex
    // at the top-center (OverlayHandler.cs 307).
    let triangle = triangle_points_from_rect(Rect {
        x: 120,
        y: 300,
        width: 20,
        height: 20,
    });
    assert_eq!(triangle[0].x, 120);
    assert_eq!(triangle[0].y, 320);
    assert_eq!(triangle[1].x, 140);
    assert_eq!(triangle[1].y, 320);
    assert_eq!(triangle[2].x, 130);
    assert_eq!(triangle[2].y, 300);
}

/// Start an Xvfb on a display number the server chooses (`-displayfd`) and
/// wait until it reports that display, which it does once it is listening.
fn spawn_xvfb_on_free_display(screen: &str) -> Result<(std::process::Child, String), String> {
    use std::io::BufRead as _;

    let mut child = match std::process::Command::new("Xvfb")
        .args([
            "-displayfd",
            "1",
            "-screen",
            "0",
            screen,
            "-nolisten",
            "tcp",
            // Without it the server resets whenever its last client
            // disconnects (the harness probe), and a connection made during
            // that reset is dropped.
            "-noreset",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                "DISPLAY is unavailable and Xvfb is not installed or not on PATH".to_string(),
            );
        }
        Err(error) => return Err(format!("could not start Xvfb: {error}")),
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("could not read the Xvfb display number".to_string());
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(line);
    });
    let display_number = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .ok()
        .and_then(|line| line.trim().parse::<u16>().ok());
    match display_number {
        Some(number) => Ok((child, format!(":{number}"))),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err("DISPLAY is unavailable and Xvfb did not report a display".to_string())
        }
    }
}

struct DisplayHarness {
    saved_display: Option<String>,
    active_display: String,
    xvfb: Option<std::process::Child>,
}

impl DisplayHarness {
    fn acquire() -> Result<Self, String> {
        let saved_display = std::env::var("DISPLAY").ok();
        if let Some(display) = saved_display.clone() {
            if x11rb::rust_connection::RustConnection::connect(Some(display.as_str())).is_ok() {
                return Ok(Self {
                    saved_display,
                    active_display: display,
                    xvfb: None,
                });
            }
        }

        Self::spawn_xvfb(saved_display)
    }

    fn spawn_xvfb(saved_display: Option<String>) -> Result<Self, String> {
        // Xvfb picks a free display number itself (`-displayfd`) and reports
        // it once it accepts connections, so parallel test processes never
        // share a display or connect to a server that is still starting.
        let (mut child, active_display) = spawn_xvfb_on_free_display("1280x720x24")?;
        std::env::set_var("DISPLAY", &active_display);
        for _ in 0..20 {
            if x11rb::rust_connection::RustConnection::connect(Some(active_display.as_str()))
                .is_ok()
            {
                return Ok(Self {
                    saved_display,
                    active_display,
                    xvfb: Some(child),
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let _ = child.kill();
        let _ = child.wait();
        restore_display(saved_display);
        Err("DISPLAY is unavailable and the Xvfb session did not accept connections".to_string())
    }

    fn active_display(&self) -> &str {
        &self.active_display
    }
}

impl Drop for DisplayHarness {
    fn drop(&mut self) {
        restore_display(self.saved_display.clone());
        if let Some(child) = self.xvfb.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct SyntheticTargetWindow {
    conn: x11rb::rust_connection::RustConnection,
    window: u32,
    width: u16,
    height: u16,
}

impl SyntheticTargetWindow {
    fn create(geometry: Rect) -> Result<Self, String> {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{
            ConnectionExt, CreateWindowAux, EventMask, ImageFormat, WindowClass,
        };

        // The Xvfb the DisplayHarness spawned may still be initializing
        // while parallel test binaries run their own harnesses on the
        // same display-number range; a connect that fails with a
        // partial read is a race, not a broken setup, so retry with a
        // short delay before giving up.
        let (conn, screen_num) = (0..5)
            .find_map(|attempt| {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                x11rb::rust_connection::RustConnection::connect(None).ok()
            })
            .ok_or_else(|| "connect synthetic target: no usable X11 display".to_string())?;
        let screen = &conn.setup().roots[screen_num];
        let window = conn
            .generate_id()
            .map_err(|error| format!("generate synthetic target id: {error}"))?;
        let width = geometry.width.min(u16::MAX as u32) as u16;
        let height = geometry.height.min(u16::MAX as u32) as u16;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            geometry.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            geometry.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .background_pixel(0x00202020)
                .event_mask(EventMask::EXPOSURE),
        )
        .map_err(|error| format!("create synthetic target: {error}"))?
        .check()
        .map_err(|error| format!("create synthetic target check: {error}"))?;
        conn.map_window(window)
            .map_err(|error| format!("map synthetic target: {error}"))?
            .check()
            .map_err(|error| format!("map synthetic target check: {error}"))?;
        conn.clear_area(false, window, 0, 0, width, height)
            .map_err(|error| format!("clear synthetic target: {error}"))?
            .check()
            .map_err(|error| format!("clear synthetic target check: {error}"))?;
        conn.flush()
            .map_err(|error| format!("flush synthetic target: {error}"))?;
        std::thread::sleep(std::time::Duration::from_millis(50));

        let _ = conn
            .get_image(ImageFormat::Z_PIXMAP, window, 0, 0, width, height, u32::MAX)
            .map_err(|error| format!("probe synthetic target image: {error}"))?
            .reply()
            .map_err(|error| format!("probe synthetic target image reply: {error}"))?;

        Ok(Self {
            conn,
            window,
            width,
            height,
        })
    }

    fn capture_pixels(&self) -> Result<(u32, u32, usize, Vec<u8>), String> {
        use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};

        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                0,
                0,
                self.width,
                self.height,
                u32::MAX,
            )
            .map_err(|error| format!("capture synthetic target: {error}"))?
            .reply()
            .map_err(|error| format!("capture synthetic target reply: {error}"))?;
        let pixel_count = usize::from(self.width).saturating_mul(usize::from(self.height));
        if pixel_count == 0 {
            return Err("synthetic target capture has zero pixels".to_string());
        }
        let bytes_per_pixel = reply.data.len() / pixel_count;
        if bytes_per_pixel == 0 || bytes_per_pixel.saturating_mul(pixel_count) != reply.data.len() {
            return Err(format!(
                "unsupported synthetic target layout: {} bytes for {}x{}",
                reply.data.len(),
                self.width,
                self.height,
            ));
        }
        Ok((
            u32::from(self.width),
            u32::from(self.height),
            bytes_per_pixel,
            reply.data,
        ))
    }
}

impl Drop for SyntheticTargetWindow {
    fn drop(&mut self) {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::ConnectionExt;

        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

fn smoke_artifact_dir() -> PathBuf {
    workspace_root().join("debug-output/x11-overlay-smoke")
}

fn reset_smoke_artifacts() -> PathBuf {
    let dir = smoke_artifact_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    for file_name in [
        "capability-report.json",
        "render-plan.json",
        "timing-summary.json",
        "overlay-window.ppm",
        "synthetic-target.ppm",
    ] {
        let path = dir.join(file_name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("failed to reset {}: {error}", path.display()),
        }
    }
    dir
}

fn write_json_artifact(path: &Path, value: &serde_json::Value) {
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
}

fn write_ppm_artifact(path: &Path, width: u32, height: u32, bytes_per_pixel: usize, bytes: &[u8]) {
    let expected_len = width as usize * height as usize * bytes_per_pixel;
    assert_eq!(
        bytes.len(),
        expected_len,
        "ppm artifact expected {expected_len} bytes but got {}",
        bytes.len()
    );

    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for chunk in bytes.chunks_exact(bytes_per_pixel) {
        let (r, g, b) = match bytes_per_pixel {
            1 => (chunk[0], chunk[0], chunk[0]),
            2 => (chunk[1], chunk[0], chunk[0]),
            _ => (chunk[2], chunk[1], chunk[0]),
        };
        ppm.extend_from_slice(&[r, g, b]);
    }

    std::fs::write(path, ppm)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
}

fn image_has_nonblank_pixels(bytes_per_pixel: usize, bytes: &[u8]) -> bool {
    bytes
        .chunks_exact(bytes_per_pixel)
        .any(|chunk| chunk.iter().any(|byte| *byte != 0))
}

/// Count the bright pixels of `rect`: the client-side glyph ink the text
/// path uploads (`put_image`) on a dark overlay panel, as the window
/// capture reports it (little-endian 32bpp ZPixmap, so the first three
/// bytes are the colour channels).
fn bright_pixels_in(bytes_per_pixel: usize, bytes: &[u8], width: u32, rect: Rect) -> usize {
    if bytes_per_pixel < 3 {
        return 0;
    }
    let mut count = 0;
    for y in rect.y.max(0)..(rect.y + rect.height as i32) {
        for x in rect.x.max(0)..(rect.x + rect.width as i32) {
            let index = ((y as usize) * (width as usize) + x as usize) * bytes_per_pixel;
            let Some(pixel) = bytes.get(index..index + 3) else {
                continue;
            };
            if pixel[0] > 0x80 && pixel[1] > 0x80 && pixel[2] > 0x80 {
                count += 1;
            }
        }
    }
    count
}

fn rect_is_within_window(rect: Rect, width: u32, height: u32) -> bool {
    rect.x >= 0
        && rect.y >= 0
        && rect.x.saturating_add(rect.width as i32) <= width as i32
        && rect.y.saturating_add(rect.height as i32) <= height as i32
}

/// Synthetic descriptor set of the overlay smoke coverage: affix,
/// aspect, unique, rune, sigil, trade panel and paragon panels.
///
/// `window_geometry` only sizes and centers the paragon layout around the
/// window (see `ParagonOverlayLayout`); every descriptor rect stays
/// window-relative, exactly like the live producers emit it.
fn build_synthetic_smoke_descriptors(
    window_geometry: Rect,
) -> Vec<d4lc_rules::RuleOverlayDescriptor> {
    let mut descriptors = vec![
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Affix {
                affix_id: "Affix_AttackSpeed".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
            shape: d4lc_rules::RuleOverlayShape::Rectangle,
            geometry: Rect {
                x: 40,
                y: 50,
                width: 24,
                height: 24,
            },
            color: "Gold".into(),
            text: Some("AS".to_string()),
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "synthetic_affix_marker".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAffixes",
                "synthetic affix smoke marker",
            ),
        },
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Power {
                power_id: "Aspect_BerserkRipping".to_string(),
                power_kind: d4lc_rules::RulePowerKind::Aspect,
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Aspect,
            shape: d4lc_rules::RuleOverlayShape::Circle,
            geometry: Rect {
                x: 100,
                y: 50,
                width: 24,
                height: 24,
            },
            color: "Orange".into(),
            text: Some("Aspect".to_string()),
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "synthetic_aspect_marker".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAspects",
                "synthetic aspect smoke marker",
            ),
        },
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Power {
                power_id: "Unique_HarlequinCrest".to_string(),
                power_kind: d4lc_rules::RulePowerKind::Unique,
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Unique,
            shape: d4lc_rules::RuleOverlayShape::Circle,
            geometry: Rect {
                x: 160,
                y: 50,
                width: 24,
                height: 24,
            },
            color: "Green".into(),
            text: Some("Unique".to_string()),
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "synthetic_unique_marker".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAspects",
                "synthetic unique smoke marker",
            ),
        },
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Rune {
                rune_id: "Rune_Tam".to_string(),
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Rune,
            shape: d4lc_rules::RuleOverlayShape::Triangle,
            geometry: Rect {
                x: 220,
                y: 50,
                width: 24,
                height: 24,
            },
            color: "Purple".into(),
            text: Some("Rune".to_string()),
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "synthetic_rune_marker".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAspects",
                "synthetic rune smoke marker",
            ),
        },
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Sigil {
                sigil_id: "Dungeon_Ghoa_Ruins".to_string(),
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Sigil,
            shape: d4lc_rules::RuleOverlayShape::Rectangle,
            geometry: Rect {
                x: 280,
                y: 50,
                width: 24,
                height: 24,
            },
            color: "Blue".into(),
            text: Some("Sigil".to_string()),
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "synthetic_sigil_marker".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAspects",
                "synthetic sigil smoke marker",
            ),
        },
        d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Trade,
            marker_type: d4lc_rules::RuleOverlayMarkerType::Trade,
            shape: d4lc_rules::RuleOverlayShape::TextPanel,
            geometry: Rect {
                x: 40,
                y: 420,
                width: 220,
                height: 40,
            },
            color: "background".into(),
            text: Some("5b".to_string()),
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::TextPanel,
            geometry_source: "synthetic_trade_panel".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsTrading",
                "synthetic trade smoke panel",
            ),
        },
    ];

    let paragon_layout = d4lc_rules::ParagonOverlayLayout {
        window_geometry,
        panel_left: 540,
        panel_top: 70,
        panel_height: 40,
        panel_gap: 4,
        panel_text_offset: 16,
        font_px: 16,
        tile_gap: 4,
        tile_count: 21,
        node_size_full: 16,
        node_size_collapsed: 10,
        node_border_size: 2,
        collapsed_left_offset: -40,
        collapsed_top_offset: 20,
    };
    descriptors.extend(d4lc_rules::build_paragon_overlay_descriptors(
        &d4lc_rules::ParagonOverlayState {
            build_name: "Synthetic Smoke".to_string(),
            routes: vec![d4lc_rules::ImportedParagonRoute {
                step_name: Some("Route A".to_string()),
                boards: vec![d4lc_rules::ImportedParagonBoard {
                    index: 0,
                    name: "Decimator".to_string(),
                    glyph: "Marshal".to_string(),
                    rotation: "0°".to_string(),
                    active_nodes: vec![0, 1, 2, 20],
                }],
            }],
            current_route_index: 0,
            current_board_index: Some(0),
            current_board_name: None,
            mode: d4lc_rules::ParagonOverlayMode::Full,
        },
        &paragon_layout,
        &d4lc_core::text_measure::FixedTextMeasure::HALF_EM,
    ));

    descriptors
}

/// Identical frames skip the X11 draw, a changed descriptor
/// list redraws, and an empty frame after content redraws immediately
/// (the hide stays immediate). A skip is observable through
/// `last_update_skipped` and never looks like a draw.
#[test]
fn x11_skips_identical_frames_and_redraws_on_change() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _harness = DisplayHarness::acquire()
        .unwrap_or_else(|reason| panic!("the skip-draw test needs an X server: {reason}"));
    let overlay_geometry = Rect {
        x: 40,
        y: 60,
        width: 960,
        height: 540,
    };
    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: overlay_geometry,
        show_overlay_icon: true,
        overlay_icon_pos_x: 920,
        overlay_icon_pos_y: 24,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("failed to create real X11 overlay: {error}"));

    let frame =
        |sequence: u64, rule_descriptors: Vec<d4lc_rules::RuleOverlayDescriptor>| OverlayFrame {
            sequence,
            tooltip_found: !rule_descriptors.is_empty(),
            hotpath_ms: 1,
            cursor_window_relative: None,
            rule_descriptors: rule_descriptors.into(),
            text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
        };

    overlay
        .update(frame(
            1,
            build_synthetic_smoke_descriptors(overlay_geometry),
        ))
        .unwrap_or_else(|error| panic!("first update: {error}"));
    assert!(!overlay.last_update_skipped(), "the first update draws");

    overlay
        .update(frame(
            2,
            build_synthetic_smoke_descriptors(overlay_geometry),
        ))
        .unwrap_or_else(|error| panic!("identical update: {error}"));
    assert!(
        overlay.last_update_skipped(),
        "an identical frame must skip the draw"
    );

    let mut changed = build_synthetic_smoke_descriptors(overlay_geometry);
    changed.pop();
    overlay
        .update(frame(3, changed))
        .unwrap_or_else(|error| panic!("changed update: {error}"));
    assert!(
        !overlay.last_update_skipped(),
        "a changed frame must redraw"
    );

    overlay
        .update(frame(4, Vec::new()))
        .unwrap_or_else(|error| panic!("empty update: {error}"));
    assert!(
        !overlay.last_update_skipped(),
        "an empty frame after content must redraw (the hide stays immediate)"
    );

    overlay
        .update(frame(5, Vec::new()))
        .unwrap_or_else(|error| panic!("empty update: {error}"));
    assert!(
        overlay.last_update_skipped(),
        "an identical empty frame skips"
    );

    // A window geometry change invalidates the pixels and forces a redraw.
    overlay
        .set_window_geometry(Rect {
            x: 60,
            y: 80,
            width: 960,
            height: 540,
        })
        .unwrap_or_else(|error| panic!("geometry update: {error}"));
    overlay
        .update(frame(6, Vec::new()))
        .unwrap_or_else(|error| panic!("moved update: {error}"));
    assert!(
        !overlay.last_update_skipped(),
        "a geometry change must force a redraw"
    );
}

#[test]
fn paragon_active_node_stroke_renders_and_resets_between_rectangles() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _harness =
        DisplayHarness::acquire().unwrap_or_else(|error| panic!("X11 test display: {error}"));
    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    })
    .unwrap_or_else(|error| panic!("create overlay: {error}"));
    let descriptor = |x, shape| d4lc_rules::RuleOverlayDescriptor {
        target: d4lc_rules::RuleOverlayTarget::Paragon {
            kind: d4lc_rules::ParagonOverlayElementKind::Node,
            route_index: Some(0),
            board_index: Some(0),
            board_name: None,
            node_index: Some(0),
            panel_hit_width: None,
        },
        marker_type: d4lc_rules::RuleOverlayMarkerType::Paragon,
        shape,
        geometry: Rect {
            x,
            y: 16,
            width: 16,
            height: 24,
        },
        color: "#14DC50".into(),
        text: None,
        text_origin: None,
        z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
        geometry_source: "stroke regression".into(),
        source: d4lc_rules::RuleSourceNote::new(
            "OverlayHandler.cs",
            "DrawGraphicsParagon",
            "active node",
        ),
    };
    collect_render_ops(
        &mut overlay,
        1,
        vec![
            descriptor(
                16,
                d4lc_rules::RuleOverlayShape::StrokedRectangle { stroke: 4 },
            ),
            descriptor(48, d4lc_rules::RuleOverlayShape::Rectangle),
        ],
    );
    let (width, height, bpp, pixels) = overlay
        .capture_real_x11_window_for_test()
        .unwrap_or_else(|error| panic!("capture overlay: {error}"));
    let pixel = |x: usize, y: usize| &pixels[(y * width as usize + x) * bpp..][..3];
    assert_eq!(pixel(17, 28), [80, 220, 20]);
    assert_eq!(pixel(24, 28), [0, 0, 0]);
    assert_eq!(pixel(48, 28), [80, 220, 20]);
    assert_eq!(
        pixel(49, 28),
        [0, 0, 0],
        "the next rectangle must use stroke 1"
    );
    let output = workspace_root().join("debug-output/paragon-overlay");
    std::fs::create_dir_all(&output).unwrap_or_else(|error| panic!("create artifacts: {error}"));
    write_ppm_artifact(
        &output.join("active-node-stroke.ppm"),
        width,
        height,
        bpp,
        &pixels,
    );
    collect_render_ops(
        &mut overlay,
        2,
        vec![descriptor(
            16,
            d4lc_rules::RuleOverlayShape::StrokedRectangle { stroke: 1 },
        )],
    );
    let (width, _, bpp, pixels) = overlay
        .capture_real_x11_window_for_test()
        .unwrap_or_else(|error| panic!("capture overlay: {error}"));
    assert_eq!(
        &pixels[(28 * width as usize + 17) * bpp..][..3],
        [0, 0, 0],
        "redrawing a thinner border must clear the previous stroke"
    );
}

/// Panel text blends over the panel fill: the pixels between and around
/// the glyphs keep the panel colour instead of being replaced by the line
/// box's transparent background (which showed the game through the paragon
/// panels on a composited desktop).
#[test]
fn panel_text_composites_over_the_panel_fill() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _harness =
        DisplayHarness::acquire().unwrap_or_else(|error| panic!("X11 test display: {error}"));
    let geometry = Rect {
        x: 0,
        y: 0,
        width: 960,
        height: 540,
    };
    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: geometry,
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    })
    .unwrap_or_else(|error| panic!("create overlay: {error}"));
    let descriptors = build_synthetic_smoke_descriptors(geometry);
    let build_panel = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.target,
                d4lc_rules::RuleOverlayTarget::Paragon {
                    kind: d4lc_rules::ParagonOverlayElementKind::BuildPanel,
                    ..
                }
            )
        })
        .map(|descriptor| descriptor.geometry)
        .unwrap_or_else(|| panic!("synthetic descriptors have no paragon build panel"));
    collect_render_ops(&mut overlay, 1, descriptors);
    let (width, _, bpp, pixels) = overlay
        .capture_real_x11_window_for_test()
        .unwrap_or_else(|error| panic!("capture overlay: {error}"));

    let interior = Rect {
        x: build_panel.x + 2,
        y: build_panel.y + 2,
        width: build_panel.width.saturating_sub(4),
        height: build_panel.height.saturating_sub(4),
    };
    let mut cleared = 0;
    for y in interior.y..interior.y + interior.height as i32 {
        for x in interior.x..interior.x + interior.width as i32 {
            let pixel = &pixels[((y as usize) * (width as usize) + x as usize) * bpp..][..3];
            if pixel == [0, 0, 0] {
                cleared += 1;
            }
        }
    }
    assert_eq!(
        cleared, 0,
        "the build panel text must not clear the panel fill beneath its line box"
    );
    assert!(
        bright_pixels_in(bpp, &pixels, width, interior) > 0,
        "the build panel caption must reach the window as glyph ink"
    );
}

/// Affix circles are antialiased like upstream's Direct2D markers: the edge
/// carries intermediate shades, the centre is the marker colour and the
/// left edge has no nub where a core arc stroke starts and ends.
#[test]
fn affix_circles_are_antialiased_without_a_stroke_nub() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _harness =
        DisplayHarness::acquire().unwrap_or_else(|error| panic!("X11 test display: {error}"));
    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    })
    .unwrap_or_else(|error| panic!("create overlay: {error}"));
    let circle = Rect {
        x: 12,
        y: 12,
        width: 20,
        height: 20,
    };
    collect_render_ops(
        &mut overlay,
        1,
        vec![d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Affix {
                affix_id: "Affix_AttackSpeed".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
            shape: d4lc_rules::RuleOverlayShape::Circle,
            geometry: circle,
            color: "Green".into(),
            text: None,
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "antialias regression".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "OverlayHandler.cs",
                "DrawGraphicsAffixes",
                "affix circle",
            ),
        }],
    );
    let (width, _, bpp, pixels) = overlay
        .capture_real_x11_window_for_test()
        .unwrap_or_else(|error| panic!("capture overlay: {error}"));
    let pixel = |x: i32, y: i32| {
        let index = ((y as usize) * (width as usize) + x as usize) * bpp;
        [pixels[index], pixels[index + 1], pixels[index + 2]]
    };
    // Little-endian BGR: the centre is the opaque marker green.
    assert_eq!(pixel(22, 22), [0, 0xFF, 0]);
    // Left of the circle on its centre row: untouched (no stroke nub).
    assert_eq!(pixel(10, 22), [0, 0, 0]);
    assert_eq!(pixel(11, 22), [0, 0, 0]);
    let expected = crate::shape::outline_fill_circle(circle, 0xFF00_FF00)
        .unwrap_or_else(|| panic!("circle raster"));
    let partial = expected
        .pixels
        .iter()
        .filter(|pixel| {
            let alpha = *pixel >> 24;
            alpha > 0 && alpha < 255
        })
        .count();
    assert!(partial > 0, "the raster has an antialiased edge");
    // The window carries the raster's colours: every fully covered pixel
    // matches it exactly.
    for row in 0..expected.height as i32 {
        for column in 0..expected.width as i32 {
            let source = expected.pixels[(row as u32 * expected.width + column as u32) as usize];
            if source >> 24 != 0xFF {
                continue;
            }
            let bytes = source.to_le_bytes();
            assert_eq!(
                pixel(circle.x + column, circle.y + row),
                [bytes[0], bytes[1], bytes[2]],
                "pixel ({column}, {row})"
            );
        }
    }
}

#[test]
fn synthetic_real_x11_smoke_records_render_evidence() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let artifact_dir = reset_smoke_artifacts();

    let display = DisplayHarness::acquire()
        .unwrap_or_else(|reason| panic!("the synthetic X11 smoke needs an X server: {reason}"));

    let overlay_geometry = Rect {
        x: 40,
        y: 60,
        width: 960,
        height: 540,
    };
    // The synthetic target extends 64px past the overlay footprint: where
    // the overlay occludes the target, XGetImage content is undefined on
    // compositing-free X servers (bare Xvfb on CI reads zeros), so the
    // unoccluded band is what makes the target capture deterministic.
    let target_geometry = Rect {
        x: 40,
        y: 60,
        width: 1024,
        height: 540,
    };
    let target_window = SyntheticTargetWindow::create(target_geometry)
        .unwrap_or_else(|error| panic!("failed to create synthetic target window: {error}"));

    let create_started_at = Instant::now();
    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: overlay_geometry,
        show_overlay_icon: true,
        overlay_icon_pos_x: 920,
        overlay_icon_pos_y: 24,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("failed to create real X11 overlay: {error}"));
    let create_ms = create_started_at.elapsed().as_millis();

    assert_eq!(
        overlay.status().backend,
        OverlayBackend::RealX11,
        "synthetic X11 smoke expected real_x11 backend, got {:?} ({:?})",
        overlay.status().backend,
        overlay.status().limitation,
    );

    let descriptors = build_synthetic_smoke_descriptors(overlay_geometry);
    let update_started_at = Instant::now();
    overlay
        .update(OverlayFrame {
            sequence: 172,
            tooltip_found: true,
            hotpath_ms: 7,
            cursor_window_relative: Some(Point { x: 880, y: 18 }),
            rule_descriptors: descriptors.clone().into(),
            text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
        })
        .unwrap_or_else(|error| panic!("synthetic X11 smoke update failed: {error}"));
    let update_ms = update_started_at.elapsed().as_millis();

    std::thread::sleep(std::time::Duration::from_millis(100));

    let capture_started_at = Instant::now();
    let (overlay_width, overlay_height, overlay_bpp, overlay_pixels) = overlay
        .capture_real_x11_window_for_test()
        .unwrap_or_else(|error| panic!("failed to capture overlay window: {error}"));
    let (target_width, target_height, target_bpp, target_pixels) = target_window
        .capture_pixels()
        .unwrap_or_else(|error| panic!("failed to capture synthetic target: {error}"));
    let capture_ms = capture_started_at.elapsed().as_millis();

    assert!(
        image_has_nonblank_pixels(overlay_bpp, &overlay_pixels),
        "synthetic X11 smoke produced a blank overlay capture"
    );
    assert!(
        image_has_nonblank_pixels(target_bpp, &target_pixels),
        "synthetic target capture is blank"
    );

    let render_ops = overlay.last_render_ops().to_vec();
    let marker_types = render_ops
        .iter()
        .map(|operation| format!("{:?}", operation.marker_type))
        .collect::<std::collections::BTreeSet<_>>();
    for required_marker in [
        "Affix", "Aspect", "Unique", "Rune", "Sigil", "Trade", "Paragon", "Menu",
    ] {
        assert!(
            marker_types.contains(required_marker),
            "synthetic X11 smoke missing marker type {required_marker}; saw {marker_types:?}"
        );
    }

    // Shape parity on the real draw path: circle and triangle marker
    // ops must reach the renderer, not only rect approximations.
    let render_kinds = render_ops
        .iter()
        .map(|operation| format!("{:?}", operation.kind))
        .collect::<std::collections::BTreeSet<_>>();
    for required_kind in ["Circle", "Triangle", "OutlineFillRect"] {
        let present = render_kinds
            .iter()
            .any(|kind| kind.starts_with(required_kind));
        assert!(
            present,
            "synthetic X11 smoke missing render kind {required_kind}; saw {render_kinds:?}"
        );
    }

    // Marker form parity with upstream `OverlayHandler.cs` 251-318: the
    // sigil dungeon marker is an outline+fill rectangle with stroke 2,
    // an any-type affix marker the same form with stroke 1, and the
    // paragon tiles stay outline-only rectangles.
    let stroke_of = |marker: d4lc_rules::RuleOverlayMarkerType| {
        render_ops
            .iter()
            .filter(|operation| operation.marker_type == marker)
            .filter_map(|operation| match operation.kind {
                OverlayRenderKind::OutlineFillRect { stroke } => Some(stroke),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        stroke_of(d4lc_rules::RuleOverlayMarkerType::Sigil),
        std::collections::BTreeSet::from([2]),
        "the sigil dungeon marker must use the upstream stroke 2"
    );
    assert_eq!(
        stroke_of(d4lc_rules::RuleOverlayMarkerType::Affix),
        std::collections::BTreeSet::from([1]),
        "an any-type affix marker must use the upstream stroke 1"
    );
    let paragon_kinds = render_ops
        .iter()
        .filter(|operation| operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Paragon)
        .map(|operation| operation.kind)
        .collect::<Vec<_>>();
    assert!(
        paragon_kinds.contains(&OverlayRenderKind::OutlineRect),
        "paragon tiles must stay outline-only rectangles; saw {paragon_kinds:?}"
    );
    assert!(
        !paragon_kinds
            .iter()
            .any(|kind| matches!(kind, OverlayRenderKind::OutlineFillRect { .. })),
        "paragon tiles must not take the affix marker's outline+fill form; saw \
         {paragon_kinds:?}"
    );

    // The bounded text path reaches the server: the trade panel's caption
    // is laid out and rasterized client-side (one `put_image` per line)
    // and its ink is visible in the captured window, brighter than the
    // dark panel fill it sits on. This is the end-to-end proof for the
    // path that replaced the server-side `image_text8`, on a real X
    // server (the harness' private Xvfb display).
    let trade_panel = descriptors
        .iter()
        .find(|descriptor| descriptor.marker_type == d4lc_rules::RuleOverlayMarkerType::Trade)
        .map(|descriptor| descriptor.geometry)
        .unwrap_or_else(|| panic!("synthetic smoke has no trade panel descriptor"));
    assert!(
        bright_pixels_in(overlay_bpp, &overlay_pixels, overlay_width, trade_panel) > 0,
        "the trade panel caption must reach the X11 window as rasterized glyph ink"
    );

    assert!(
        render_ops.iter().all(|operation| rect_is_within_window(
            operation.geometry,
            overlay_width,
            overlay_height
        )),
        "synthetic X11 smoke produced geometry outside the target window"
    );
    assert!(
        rect_is_within_window(
            overlay.status().interaction_zone,
            overlay.status().window_geometry.width,
            overlay.status().window_geometry.height,
        ),
        "synthetic X11 smoke interaction zone escapes the target window"
    );

    write_ppm_artifact(
        &artifact_dir.join("overlay-window.ppm"),
        overlay_width,
        overlay_height,
        overlay_bpp,
        &overlay_pixels,
    );
    write_ppm_artifact(
        &artifact_dir.join("synthetic-target.ppm"),
        target_width,
        target_height,
        target_bpp,
        &target_pixels,
    );

    write_json_artifact(
        &artifact_dir.join("render-plan.json"),
        &serde_json::json!({
            "display": display.active_display(),
            "frame": {
                "sequence": 172,
                "tooltip_found": true,
                "hotpath_ms": 7,
            },
            "descriptors": descriptors.iter().map(rule_overlay_descriptor_json).collect::<Vec<_>>(),
            "render_ops": render_ops.iter().map(render_op_json).collect::<Vec<_>>(),
        }),
    );
    write_json_artifact(
        &artifact_dir.join("capability-report.json"),
        &serde_json::json!({
            "status": "rendered",
            "display": display.active_display(),
            "backend": overlay.status().backend.as_str(),
            "window_geometry": rect_json(overlay.status().window_geometry),
            "interaction_zone": rect_json(overlay.status().interaction_zone),
            "menu_locked": overlay.status().menu_locked,
            "menu_visible": overlay.status().menu_visible,
            "transparent_requested": overlay.status().transparent_requested,
            "transparent_confirmed": overlay.status().transparent_confirmed,
            "always_on_top_requested": overlay.status().always_on_top_requested,
            "always_on_top_confirmed": overlay.status().always_on_top_confirmed,
            "click_through": overlay.status().click_through,
            "limitation": overlay.status().limitation,
            "draw_error_count": overlay.status().draw_error_count,
            "last_error": overlay.status().last_error,
            "render_op_count": render_ops.len(),
            "marker_types": marker_types.into_iter().collect::<Vec<_>>(),
            "render_kinds": render_kinds.into_iter().collect::<Vec<_>>(),
            "overlay_capture_nonblank": true,
            "target_capture_nonblank": true,
            "geometry_within_target_window": true,
        }),
    );
    write_json_artifact(
        &artifact_dir.join("timing-summary.json"),
        &serde_json::json!({
            "create_overlay_ms": create_ms,
            "update_ms": update_ms,
            "capture_ms": capture_ms,
            "artifact_dir": artifact_dir.display().to_string(),
        }),
    );
}

/// When DISPLAY is set and the `real-x11` feature is active, create_overlay
/// should attempt X11. If X11 succeeds the backend is RealX11; if it fails
/// (e.g. in a CI environment with a fake DISPLAY) it degrades to Failed.
/// Either way the live loop must not panic.
#[test]
fn real_x11_feature_with_display_does_not_panic() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _harness = DisplayHarness::acquire()
        .unwrap_or_else(|reason| panic!("this test needs an X server: {reason}"));

    let result = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    });

    // Must not panic. Backend is either RealX11 or Failed.
    let handle = result.unwrap_or_else(|e| panic!("create_overlay must not Err: {e}"));
    let backend = handle.status().backend;
    assert!(
        backend == OverlayBackend::RealX11 || backend == OverlayBackend::Failed,
        "unexpected backend: {backend}"
    );
    if backend == OverlayBackend::Failed {
        assert!(!handle.status().click_through);
    }
}
