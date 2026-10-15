//! XTEST input driver for the X11 end-to-end GUI journeys.
//!
//! A thin command-line wrapper around `x11rb`'s XTEST `fake_input` plus the
//! window-lookup and capture-with-retry patterns already established by
//! `d4lc-app`'s visual smoke harness (`crates/d4lc-app/src/visual_smoke.rs`).
//! Each invocation performs exactly one primitive action — move / click /
//! type / key / focus / resize / capture / diffstat / close-request — so the
//! journey orchestration and assertions live in `scripts/xtest_journeys.sh`
//! and this binary stays a dumb, reusable remote control for a real X server.
//!
//! Window-relative coordinates (`click`, region diffs) are translated to
//! absolute root coordinates from the live window geometry, so the journeys
//! do not depend on where the window manager (or bare Xvfb) places the app.
//!
//! Run with:
//! `cargo run -p d4lc-overlay-x11 --example xtest_input --features real-x11 -- <command> ...`
//!
//! Without the `real-x11` feature the binary is a no-op that exits 2.

#![allow(clippy::too_many_lines)]

fn main() -> std::process::ExitCode {
    #[cfg(not(feature = "real-x11"))]
    {
        eprintln!("xtest_input: requires the d4lc-overlay-x11 `real-x11` feature (x11rb)");
        std::process::ExitCode::from(2)
    }

    #[cfg(feature = "real-x11")]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match real::run(&args) {
            Ok(output) => {
                if !output.is_empty() {
                    println!("{output}");
                }
                std::process::ExitCode::SUCCESS
            }
            Err(real::DriverError::WindowNotFound(message)) => {
                eprintln!("xtest_input: {message}");
                std::process::ExitCode::from(3)
            }
            Err(real::DriverError::UnsupportedKey(message)) => {
                eprintln!("xtest_input: {message}");
                std::process::ExitCode::from(4)
            }
            Err(real::DriverError::Other(message)) => {
                eprintln!("xtest_input: {message}");
                std::process::ExitCode::from(5)
            }
        }
    }
}

#[cfg(feature = "real-x11")]
mod real {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt, EventMask, ImageFormat,
        InputFocus,
    };
    use x11rb::protocol::xtest::ConnectionExt as XtestConnectionExt;
    use x11rb::rust_connection::RustConnection;

    // ── Keysym constants (only what the journeys need) ──────────────────
    const XK_ESCAPE: u32 = 0xff1b;
    const XK_RETURN: u32 = 0xff0d;
    const XK_TAB: u32 = 0xff09;
    const XK_BACKSPACE: u32 = 0xff08;
    const XK_DELETE: u32 = 0xffff;
    const XK_HOME: u32 = 0xff50;
    const XK_END: u32 = 0xff57;
    const XK_SHIFT_L: u32 = 0xffe1;
    const XK_CONTROL_L: u32 = 0xffe3;
    const XK_ALT_L: u32 = 0xffe9;
    const XK_F1: u32 = 0xffbe; // F1..F12 are consecutive: F12 = 0xffc9

    /// XTEST input goes through the server's XTEST virtual device (device id 0
    /// selects it; same as what `xdotool` produces through libXtst).
    const XTEST_DEVICE_ID: u8 = 0;

    /// Capture retry budget copied from the visual smoke harness: a slow
    /// software renderer may present the first frame late, and the same is
    /// true for frames after an XTEST click.
    const CAPTURE_RETRIES: u32 = 15;
    const CAPTURE_RETRY_SLEEP: Duration = Duration::from_millis(200);

    #[derive(Debug)]
    pub enum DriverError {
        WindowNotFound(String),
        UnsupportedKey(String),
        Other(String),
    }

    impl From<String> for DriverError {
        fn from(message: String) -> Self {
            Self::Other(message)
        }
    }

    impl From<&str> for DriverError {
        fn from(message: &str) -> Self {
            Self::Other(message.to_string())
        }
    }

    type CmdResult<T> = Result<T, DriverError>;

    struct Driver {
        conn: RustConnection,
        root: u32,
        screen_width: i16,
        screen_height: i16,
    }

    struct WindowGeometry {
        window: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        viewable: bool,
    }

    impl Driver {
        fn connect() -> CmdResult<Self> {
            let (conn, screen_num) = RustConnection::connect(None)
                .map_err(|error| format!("connect to DISPLAY: {error}"))?;
            let screen = conn
                .setup()
                .roots
                .get(screen_num)
                .ok_or_else(|| DriverError::Other(format!("screen {screen_num} missing")))?;
            Ok(Self {
                screen_width: i16::try_from(screen.width_in_pixels)
                    .map_err(|error| format!("screen width: {error}"))?,
                screen_height: i16::try_from(screen.height_in_pixels)
                    .map_err(|error| format!("screen height: {error}"))?,
                root: screen.root,
                conn,
            })
        }

        fn window_title(&self, window: u32, wm_name_atom: u32) -> CmdResult<Option<String>> {
            for atom in [wm_name_atom, u32::from(AtomEnum::WM_NAME)] {
                let reply = self
                    .conn
                    .get_property(false, window, atom, AtomEnum::ANY, 0, 1024)
                    .map_err(|error| format!("get_property {window:#x}: {error}"))?
                    .reply()
                    .map_err(|error| format!("get_property {window:#x} reply: {error}"))?;
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

        fn find_window_by_title(
            &self,
            root: u32,
            title: &str,
            wm_name_atom: u32,
        ) -> CmdResult<Option<u32>> {
            if self.window_title(root, wm_name_atom)?.as_deref() == Some(title) {
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
                if let Some(found) = self.find_window_by_title(child, title, wm_name_atom)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }

        fn intern_wm_name_atom(&self) -> CmdResult<u32> {
            Ok(self
                .conn
                .intern_atom(false, b"_NET_WM_NAME")
                .map_err(|error| format!("intern_atom _NET_WM_NAME: {error}"))?
                .reply()
                .map_err(|error| format!("intern_atom _NET_WM_NAME reply: {error}"))?
                .atom)
        }

        fn wait_for_window(&self, title: &str, timeout: Duration) -> CmdResult<WindowGeometry> {
            let wm_name_atom = self.intern_wm_name_atom()?;
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(window) = self.find_window_by_title(self.root, title, wm_name_atom)? {
                    let geometry = self.geometry_of(window)?;
                    // The X window exists before Slint painted the first
                    // frame; wait until it is actually viewable so capture
                    // callers never race the map.
                    if geometry.viewable {
                        return Ok(geometry);
                    }
                }
                if Instant::now() >= deadline {
                    return Err(DriverError::WindowNotFound(format!(
                        "no viewable X11 window titled {title:?} appeared within {timeout:?}"
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        fn resolve_window(&self, title: &str) -> CmdResult<WindowGeometry> {
            let wm_name_atom = self.intern_wm_name_atom()?;
            let window = self
                .find_window_by_title(self.root, title, wm_name_atom)?
                .ok_or_else(|| {
                    DriverError::WindowNotFound(format!("no X11 window titled {title:?}"))
                })?;
            self.geometry_of(window)
        }

        fn geometry_of(&self, window: u32) -> CmdResult<WindowGeometry> {
            let geometry = self
                .conn
                .get_geometry(window)
                .map_err(|error| format!("get_geometry {window:#x}: {error}"))?
                .reply()
                .map_err(|error| format!("get_geometry {window:#x} reply: {error}"))?;
            let position = self
                .conn
                .translate_coordinates(window, self.root, 0, 0)
                .map_err(|error| format!("translate_coordinates {window:#x}: {error}"))?
                .reply()
                .map_err(|error| format!("translate_coordinates {window:#x} reply: {error}"))?;
            let viewable = self
                .conn
                .get_window_attributes(window)
                .map_err(|error| format!("get_window_attributes {window:#x}: {error}"))?
                .reply()
                .map_err(|error| format!("get_attributes {window:#x} reply: {error}"))?
                .map_state
                == x11rb::protocol::xproto::MapState::VIEWABLE;
            Ok(WindowGeometry {
                window,
                x: position.dst_x,
                y: position.dst_y,
                width: geometry.width,
                height: geometry.height,
                viewable,
            })
        }

        fn fake_motion(&self, x: i16, y: i16) -> CmdResult<()> {
            self.conn
                .xtest_fake_input(
                    x11rb::protocol::xproto::MOTION_NOTIFY_EVENT,
                    0, // detail 0 = absolute pointer position
                    0, // CurrentTime
                    self.root,
                    x,
                    y,
                    XTEST_DEVICE_ID,
                )
                .map_err(|error| format!("fake motion: {error}"))?;
            self.conn
                .flush()
                .map_err(|error| format!("flush after motion: {error}"))?;
            Ok(())
        }

        fn fake_button(&self, press: bool) -> CmdResult<()> {
            let event_type = if press {
                x11rb::protocol::xproto::BUTTON_PRESS_EVENT
            } else {
                x11rb::protocol::xproto::BUTTON_RELEASE_EVENT
            };
            self.conn
                .xtest_fake_input(event_type, 1, 0, self.root, 0, 0, XTEST_DEVICE_ID)
                .map_err(|error| format!("fake button press={press}: {error}"))?;
            self.conn
                .flush()
                .map_err(|error| format!("flush after button: {error}"))?;
            Ok(())
        }

        fn fake_key(&self, keycode: u8, press: bool) -> CmdResult<()> {
            let event_type = if press {
                x11rb::protocol::xproto::KEY_PRESS_EVENT
            } else {
                x11rb::protocol::xproto::KEY_RELEASE_EVENT
            };
            self.conn
                .xtest_fake_input(event_type, keycode, 0, self.root, 0, 0, XTEST_DEVICE_ID)
                .map_err(|error| format!("fake key {keycode} press={press}: {error}"))?;
            self.conn
                .flush()
                .map_err(|error| format!("flush after key: {error}"))?;
            Ok(())
        }

        fn click_window_relative(&self, title: &str, x: i64, y: i64) -> CmdResult<String> {
            let geometry = self.resolve_window(title)?;
            let abs_x = i64::from(geometry.x) + x;
            let abs_y = i64::from(geometry.y) + y;
            let abs_x = clamp_i16(abs_x, "click x")?;
            let abs_y = clamp_i16(abs_y, "click y")?;
            self.fake_motion(abs_x, abs_y)?;
            std::thread::sleep(Duration::from_millis(60));
            self.fake_button(true)?;
            std::thread::sleep(Duration::from_millis(40));
            self.fake_button(false)?;
            std::thread::sleep(Duration::from_millis(60));
            Ok(format!(
                "clicked window=0x{:x} window_rel=({x},{y}) root=({abs_x},{abs_y})",
                geometry.window
            ))
        }

        fn keysym_locator(&self) -> CmdResult<HashMap<u32, (u8, u8)>> {
            let setup = self.conn.setup();
            let min = setup.min_keycode;
            let max = setup.max_keycode;
            if max <= min {
                return Err(format!("invalid keycodes: min={min} max={max}").into());
            }
            let count = max - min + 1;
            let reply = self
                .conn
                .get_keyboard_mapping(min, count)
                .map_err(|error| format!("get_keyboard_mapping: {error}"))?
                .reply()
                .map_err(|error| format!("get_keyboard_mapping reply: {error}"))?;
            let per_keycode = usize::from(reply.keysyms_per_keycode);
            if per_keycode == 0 {
                return Err("server reported zero keysyms per keycode".into());
            }
            let mut map = HashMap::new();
            for (index, chunk) in reply.keysyms.chunks(per_keycode).enumerate() {
                let keycode = min + index as u8;
                for (column, keysym) in chunk.iter().take_while(|&&keysym| keysym != 0).enumerate()
                {
                    if column > 1 {
                        // Column 0 = plain, column 1 = Shift; NumLock / AltGr
                        // columns are not needed for the journey inputs.
                        continue;
                    }
                    map.entry(*keysym).or_insert((keycode, column as u8));
                }
            }
            Ok(map)
        }

        fn keycode_for(
            &self,
            map: &HashMap<u32, (u8, u8)>,
            keysym: u32,
            what: &str,
        ) -> CmdResult<(u8, bool)> {
            map.get(&keysym)
                .copied()
                .map(|(keycode, column)| (keycode, column == 1))
                .ok_or_else(|| {
                    DriverError::UnsupportedKey(format!(
                        "keysym {keysym:#x} ({what}) not in the server keymap"
                    ))
                })
        }

        fn type_text(&self, text: &str) -> CmdResult<String> {
            let map = self.keysym_locator()?;
            let mut typed = 0_usize;
            for character in text.chars() {
                if !character.is_ascii() || !(' '..='~').contains(&character) {
                    return Err(DriverError::UnsupportedKey(format!(
                        "character {character:?} is not printable ASCII"
                    )));
                }
                let keysym = u32::from(character);
                let (keycode, needs_shift) = self.keycode_for(&map, keysym, "printable char")?;
                if needs_shift {
                    let (shift_keycode, _) = self.keycode_for(&map, XK_SHIFT_L, "Shift")?;
                    self.fake_key(shift_keycode, true)?;
                }
                self.fake_key(keycode, true)?;
                self.fake_key(keycode, false)?;
                if needs_shift {
                    let (shift_keycode, _) = self.keycode_for(&map, XK_SHIFT_L, "Shift")?;
                    self.fake_key(shift_keycode, false)?;
                }
                typed += 1;
                std::thread::sleep(Duration::from_millis(15));
            }
            Ok(format!("typed {typed} characters"))
        }

        fn named_keysym(name: &str) -> CmdResult<u32> {
            let keysym = match name {
                "escape" => XK_ESCAPE,
                "return" | "enter" => XK_RETURN,
                "tab" => XK_TAB,
                "backspace" => XK_BACKSPACE,
                "delete" => XK_DELETE,
                "home" => XK_HOME,
                "end" => XK_END,
                "space" => 0x020,
                f if f.len() == 2
                    && f.starts_with('f')
                    && f.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) =>
                {
                    let digit = u32::from(f.as_bytes()[1].saturating_sub(b'0'));
                    XK_F1 + digit - 1
                }
                f if f.len() == 3 && f.starts_with("f1") && f.ends_with(['0', '1', '2']) => {
                    let digit = u32::from(f.as_bytes()[2].saturating_sub(b'0'));
                    XK_F1 + 9 + digit
                }
                other => {
                    return Err(DriverError::UnsupportedKey(format!(
                        "unsupported key name {other:?}"
                    )))
                }
            };
            Ok(keysym)
        }

        fn send_key(&self, name: &str, modifiers: &[String], count: u32) -> CmdResult<String> {
            let map = self.keysym_locator()?;
            let keysym = Self::named_keysym(name)?;
            let (keycode, needs_shift) = self.keycode_for(&map, keysym, name)?;
            let mut mod_keycodes = Vec::new();
            for modifier in modifiers {
                let mod_keysym = match modifier.as_str() {
                    "ctrl" => XK_CONTROL_L,
                    "shift" => XK_SHIFT_L,
                    "alt" => XK_ALT_L,
                    other => {
                        return Err(DriverError::UnsupportedKey(format!(
                            "unsupported modifier {other:?}"
                        )))
                    }
                };
                let (mod_keycode, _) = self.keycode_for(&map, mod_keysym, modifier)?;
                mod_keycodes.push(mod_keycode);
            }
            let shift_in_mods = modifiers.iter().any(|m| m == "shift");
            let (shift_keycode, _) = self.keycode_for(&map, XK_SHIFT_L, "Shift")?;
            for _ in 0..count {
                for &mod_keycode in &mod_keycodes {
                    self.fake_key(mod_keycode, true)?;
                }
                if needs_shift && !shift_in_mods {
                    self.fake_key(shift_keycode, true)?;
                }
                self.fake_key(keycode, true)?;
                self.fake_key(keycode, false)?;
                if needs_shift && !shift_in_mods {
                    self.fake_key(shift_keycode, false)?;
                }
                for &mod_keycode in mod_keycodes.iter().rev() {
                    self.fake_key(mod_keycode, false)?;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Ok(format!("sent {name} x{count}"))
        }

        fn focus_window(&self, title: &str) -> CmdResult<String> {
            let geometry = self.resolve_window(title)?;
            self.conn
                .set_input_focus(InputFocus::PARENT, geometry.window, 0u32)
                .map_err(|error| format!("set_input_focus: {error}"))?;
            self.conn
                .flush()
                .map_err(|error| format!("flush after focus: {error}"))?;
            Ok(format!("focused window=0x{:x}", geometry.window))
        }

        /// Drop keyboard focus to the root (pointer-root focus with the
        /// pointer parked in the screen corner), so keys no longer reach the
        /// app window — the X11 "focus loss" state.
        fn unfocus(&self) -> CmdResult<String> {
            self.conn
                .set_input_focus(InputFocus::POINTER_ROOT, 1u32, 0u32)
                .map_err(|error| format!("set_input_focus pointer-root: {error}"))?;
            let corner_x = self.screen_width.saturating_sub(2).max(0);
            let corner_y = self.screen_height.saturating_sub(2).max(0);
            self.fake_motion(corner_x, corner_y)?;
            Ok(format!(
                "unfocused, pointer parked at ({corner_x},{corner_y})"
            ))
        }

        fn resize_window(&self, title: &str, width: u16, height: u16) -> CmdResult<String> {
            let geometry = self.resolve_window(title)?;
            let aux = x11rb::protocol::xproto::ConfigureWindowAux::new()
                .width(u32::from(width))
                .height(u32::from(height));
            self.conn
                .configure_window(geometry.window, &aux)
                .map_err(|error| format!("configure_window: {error}"))?;
            self.conn
                .flush()
                .map_err(|error| format!("flush after resize: {error}"))?;
            Ok(format!(
                "resized window=0x{:x} to {width}x{height}",
                geometry.window
            ))
        }

        fn capture(&self, title: &str, out_path: &str, retries: u32) -> CmdResult<String> {
            let geometry = self.resolve_window(title)?;
            let pixel_count = usize::from(geometry.width) * usize::from(geometry.height);
            if pixel_count == 0 {
                return Err(format!("window {title:?} has zero pixels").into());
            }
            let mut last_error = String::new();
            for attempt in 0..retries {
                let reply_result = self
                    .conn
                    .get_image(
                        ImageFormat::Z_PIXMAP,
                        geometry.window,
                        0,
                        0,
                        geometry.width,
                        geometry.height,
                        u32::MAX,
                    )
                    .map_err(|error| format!("get_image: {error}"))
                    .and_then(|cookie| {
                        cookie
                            .reply()
                            .map_err(|error| format!("get_image reply: {error}"))
                    });
                // A window racing its map can fail the request outright
                // (e.g. a transient Match/Drawable error); retry instead of
                // aborting the journey.
                let reply = match reply_result {
                    Ok(reply) => reply,
                    Err(error) => {
                        last_error = format!("capture attempt {attempt}: {error}");
                        std::thread::sleep(CAPTURE_RETRY_SLEEP);
                        continue;
                    }
                };
                let bytes_per_pixel = reply.data.len() / pixel_count;
                if bytes_per_pixel == 0
                    || bytes_per_pixel.saturating_mul(pixel_count) != reply.data.len()
                {
                    return Err(format!(
                        "unsupported image layout: {} bytes across {pixel_count} pixels",
                        reply.data.len()
                    )
                    .into());
                }
                if image_has_nonblank_pixels(bytes_per_pixel, &reply.data) {
                    let stats = ImageStats::analyze(bytes_per_pixel, &reply.data);
                    write_ppm(
                        out_path,
                        geometry.width,
                        geometry.height,
                        bytes_per_pixel,
                        &reply.data,
                    )?;
                    return Ok(format!(
                        "stats path={out_path} w={} h={} mean_luma={:.4} distinct_colors={} window=0x{:x}",
                        geometry.width, geometry.height, stats.mean_luma, stats.distinct_colors, geometry.window
                    ));
                }
                last_error = format!(
                    "capture attempt {attempt}: window {title:?} rendered only blank pixels"
                );
                std::thread::sleep(CAPTURE_RETRY_SLEEP);
            }
            Err(last_error.into())
        }

        fn send_close_request(&self, title: &str) -> CmdResult<String> {
            let geometry = self.resolve_window(title)?;
            let wm_protocols = self
                .conn
                .intern_atom(false, b"WM_PROTOCOLS")
                .map_err(|error| format!("intern_atom WM_PROTOCOLS: {error}"))?
                .reply()
                .map_err(|error| format!("intern_atom WM_PROTOCOLS reply: {error}"))?
                .atom;
            let wm_delete_window = self
                .conn
                .intern_atom(false, b"WM_DELETE_WINDOW")
                .map_err(|error| format!("intern_atom WM_DELETE_WINDOW: {error}"))?
                .reply()
                .map_err(|error| format!("intern_atom WM_DELETE_WINDOW reply: {error}"))?
                .atom;
            let event = ClientMessageEvent {
                response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
                format: 32,
                sequence: 0,
                window: geometry.window,
                type_: wm_protocols,
                data: ClientMessageData::from([wm_delete_window, 0_u32, 0, 0, 0]),
            };
            self.conn
                .send_event(false, geometry.window, EventMask::NO_EVENT, event)
                .map_err(|error| format!("send_event WM_DELETE_WINDOW: {error}"))?;
            self.conn
                .flush()
                .map_err(|error| format!("flush after close request: {error}"))?;
            Ok(format!(
                "sent WM_DELETE_WINDOW to window=0x{:x}",
                geometry.window
            ))
        }
    }

    struct ImageStats {
        mean_luma: f64,
        distinct_colors: usize,
    }

    impl ImageStats {
        fn analyze(bytes_per_pixel: usize, bytes: &[u8]) -> Self {
            let mut luma_sum = 0_f64;
            let mut pixel_count = 0_usize;
            let mut colors: Vec<[u8; 3]> = Vec::new();
            for chunk in bytes.chunks_exact(bytes_per_pixel) {
                let [r, g, b] = chunk_to_rgb(chunk, bytes_per_pixel);
                luma_sum +=
                    0.212_6 * f64::from(r) + 0.715_2 * f64::from(g) + 0.072_2 * f64::from(b);
                pixel_count += 1;
                if colors.len() < 64 && !colors.contains(&[r, g, b]) {
                    colors.push([r, g, b]);
                }
            }
            let mean_luma = if pixel_count == 0 {
                0.0
            } else {
                luma_sum / (f64::from(pixel_count as u32) * 255.0)
            };
            Self {
                mean_luma,
                distinct_colors: colors.len(),
            }
        }
    }

    fn chunk_to_rgb(chunk: &[u8], bytes_per_pixel: usize) -> [u8; 3] {
        match bytes_per_pixel {
            1 => [chunk[0], chunk[0], chunk[0]],
            2 => [chunk[1], chunk[0], chunk[0]],
            _ => [chunk[2], chunk[1], chunk[0]],
        }
    }

    fn image_has_nonblank_pixels(bytes_per_pixel: usize, bytes: &[u8]) -> bool {
        bytes
            .chunks_exact(bytes_per_pixel)
            .any(|chunk| chunk_to_rgb(chunk, bytes_per_pixel) != [0, 0, 0])
    }

    fn write_ppm(
        path: &str,
        width: u16,
        height: u16,
        bytes_per_pixel: usize,
        bytes: &[u8],
    ) -> CmdResult<()> {
        let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
        for chunk in bytes.chunks_exact(bytes_per_pixel) {
            let [r, g, b] = chunk_to_rgb(chunk, bytes_per_pixel);
            ppm.extend_from_slice(&[r, g, b]);
        }
        std::fs::write(path, ppm).map_err(|error| format!("write {path}: {error}"))?;
        Ok(())
    }

    struct Ppm {
        width: usize,
        height: usize,
        pixels: Vec<u8>,
    }

    fn parse_ppm(path: &str) -> CmdResult<Ppm> {
        let data = std::fs::read(path).map_err(|error| format!("read {path}: {error}"))?;
        if !data.starts_with(b"P6") {
            return Err(format!("{path} is not a binary PPM (P6)").into());
        }
        let mut cursor = 2_usize;
        let mut fields = [0_usize; 3];
        for field in &mut fields {
            while cursor < data.len() && data[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let start = cursor;
            while cursor < data.len() && data[cursor].is_ascii_digit() {
                cursor += 1;
            }
            if start == cursor {
                return Err(format!("{path}: truncated PPM header").into());
            }
            let text = std::str::from_utf8(&data[start..cursor])
                .map_err(|error| format!("{path}: header utf8: {error}"))?;
            *field = text
                .parse::<usize>()
                .map_err(|error| format!("{path}: header number: {error}"))?;
        }
        let [width, height, maxval] = fields;
        if maxval != 255 || width == 0 || height == 0 {
            return Err(format!(
                "{path}: unsupported PPM (expect maxval 255): {width}x{height} max={maxval}"
            )
            .into());
        }
        cursor += 1; // exactly one whitespace byte after maxval
        let expected = width * height * 3;
        let pixels = data
            .get(cursor..)
            .ok_or_else(|| DriverError::Other(format!("{path}: truncated PPM data")))?;
        if pixels.len() < expected {
            return Err(format!(
                "{path}: expected {expected} pixel bytes but got {}",
                pixels.len()
            )
            .into());
        }
        Ok(Ppm {
            width,
            height,
            pixels: pixels[..expected].to_vec(),
        })
    }

    fn diffstat(a_path: &str, b_path: &str, region: Option<[usize; 4]>) -> CmdResult<String> {
        let a = parse_ppm(a_path)?;
        let b = parse_ppm(b_path)?;
        if a.width != b.width || a.height != b.height {
            return Err(format!(
                "diffstat sizes differ: {a_path} {}x{} vs {b_path} {}x{}",
                a.width, a.height, b.width, b.height
            )
            .into());
        }
        let rx = region.map(|r| r[0]).unwrap_or(0);
        let ry = region.map(|r| r[1]).unwrap_or(0);
        let rw = region
            .map(|r| r[2].min(a.width.saturating_sub(rx)))
            .unwrap_or(a.width);
        let rh = region
            .map(|r| r[3].min(a.height.saturating_sub(ry)))
            .unwrap_or(a.height);
        if rx >= a.width || ry >= a.height || rw == 0 || rh == 0 {
            return Err(format!(
                "diffstat region {rx},{ry} {rw}x{rh} is outside {}x{}",
                a.width, a.height
            )
            .into());
        }
        let mut changed_px = 0_usize;
        let mut a_luma_sum = 0_f64;
        let mut b_luma_sum = 0_f64;
        for row in 0..rh {
            for column in 0..rw {
                let index = ((ry + row) * a.width + rx + column) * 3;
                let a_rgb = [a.pixels[index], a.pixels[index + 1], a.pixels[index + 2]];
                let b_rgb = [b.pixels[index], b.pixels[index + 1], b.pixels[index + 2]];
                a_luma_sum += luma(a_rgb);
                b_luma_sum += luma(b_rgb);
                if channel_delta(a_rgb, b_rgb) >= 8 {
                    changed_px += 1;
                }
            }
        }
        let total_px = rw * rh;
        let changed_pct = if total_px == 0 {
            0.0
        } else {
            100.0 * f64::from(changed_px as u32) / f64::from(total_px as u32)
        };
        Ok(format!(
            "diff a={a_path} b={b_path} w={} h={} region={rx},{ry},{rw}x{rh} changed_px={changed_px} total_px={total_px} changed_pct={changed_pct:.3} mean_a={:.4} mean_b={:.4}",
            a.width,
            a.height,
            a_luma_sum / f64::from(total_px as u32) / 255.0,
            b_luma_sum / f64::from(total_px as u32) / 255.0,
        ))
    }

    fn luma(rgb: [u8; 3]) -> f64 {
        0.212_6 * f64::from(rgb[0]) + 0.715_2 * f64::from(rgb[1]) + 0.072_2 * f64::from(rgb[2])
    }

    fn channel_delta(a: [u8; 3], b: [u8; 3]) -> u8 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0)
    }

    fn clamp_i16(value: i64, what: &str) -> CmdResult<i16> {
        i16::try_from(value).map_err(|error| format!("{what}={value} out of range: {error}").into())
    }

    fn parse_i64(args: &Args, key: &str) -> CmdResult<i64> {
        args.value(key)?
            .parse::<i64>()
            .map_err(|error| format!("invalid {key}: {error}").into())
    }

    struct Args<'a> {
        values: &'a [String],
    }

    impl<'a> Args<'a> {
        fn new(values: &'a [String]) -> Self {
            Self { values }
        }

        fn value(&self, key: &str) -> CmdResult<&str> {
            let mut iter = self.values.iter();
            while let Some(item) = iter.next() {
                if item == key {
                    return iter
                        .next()
                        .map(String::as_str)
                        .ok_or_else(|| format!("{key} needs a value").into());
                }
            }
            Err(format!("missing required argument {key}").into())
        }

        fn optional(&self, key: &str) -> Option<&str> {
            self.value(key).ok()
        }

        fn u64_value(&self, key: &str, default: u64) -> CmdResult<u64> {
            match self.optional(key) {
                Some(raw) => raw
                    .parse::<u64>()
                    .map_err(|error| format!("invalid {key}: {error}").into()),
                None => Ok(default),
            }
        }
    }

    /// Command dispatch. Every command prints a single evidence line on
    /// success; exit codes carry the failure class.
    pub fn run(args: &[String]) -> CmdResult<String> {
        let (command, rest) = args
            .split_first()
            .ok_or_else(|| DriverError::Other(usage()))?;
        let args = Args::new(rest);
        let driver = Driver::connect()?;
        match command.as_str() {
            "wait-window" => {
                let title = args.value("--title")?;
                let timeout_ms = args.u64_value("--timeout-ms", 30_000)?;
                let geometry = driver.wait_for_window(title, Duration::from_millis(timeout_ms))?;
                Ok(geometry_line(&geometry))
            }
            "geom" => {
                let geometry = driver.resolve_window(args.value("--title")?)?;
                Ok(geometry_line(&geometry))
            }
            "click" => driver.click_window_relative(
                args.value("--title")?,
                parse_i64(&args, "--x")?,
                parse_i64(&args, "--y")?,
            ),
            "move" => {
                let x = clamp_i16(parse_i64(&args, "--x")?, "--x")?;
                let y = clamp_i16(parse_i64(&args, "--y")?, "--y")?;
                driver.fake_motion(x, y)?;
                Ok(format!("moved pointer to ({x},{y})"))
            }
            "type-text" => driver.type_text(args.value("--text")?),
            "key" => {
                let count = args.u64_value("--count", 1)?;
                let count = u32::try_from(count).map_err(|error| format!("--count: {error}"))?;
                let modifiers: Vec<String> = args
                    .optional("--mod")
                    .map(|raw| raw.split(',').map(str::to_string).collect())
                    .unwrap_or_default();
                driver.send_key(args.value("--name")?, &modifiers, count)
            }
            "focus" => driver.focus_window(args.value("--title")?),
            "query-focus" => {
                let reply = driver
                    .conn
                    .get_input_focus()
                    .map_err(|error| format!("get_input_focus: {error}"))?
                    .reply()
                    .map_err(|error| format!("get_input_focus reply: {error}"))?;
                Ok(format!(
                    "input_focus window=0x{:x} revert_to={:?}",
                    reply.focus, reply.revert_to
                ))
            }
            "probe-grab" => {
                // Diagnose whether another client holds the keyboard grab:
                // a foreign grab makes the grab request fail with
                // AlreadyGrabbed. We immediately release our own grab.
                let reply = driver
                    .conn
                    .grab_keyboard(
                        false,
                        driver.root,
                        0u32,
                        x11rb::protocol::xproto::GrabMode::ASYNC,
                        x11rb::protocol::xproto::GrabMode::ASYNC,
                    )
                    .map_err(|error| format!("grab_keyboard: {error}"))?
                    .reply()
                    .map_err(|error| format!("grab_keyboard reply: {error}"))?;
                let status = format!("{:?}", reply.status);
                let _ = driver.conn.ungrab_keyboard(0u32);
                let _ = driver.conn.flush();
                Ok(format!("keyboard_grab_probe status={status}"))
            }
            "unfocus" => driver.unfocus(),
            "resize" => {
                let width = args.u64_value("--w", 0)?;
                let height = args.u64_value("--h", 0)?;
                let width = u16::try_from(width).map_err(|error| format!("--w: {error}"))?;
                let height = u16::try_from(height).map_err(|error| format!("--h: {error}"))?;
                if width == 0 || height == 0 {
                    return Err(
                        format!("resize needs positive --w/--h, got {width}x{height}").into(),
                    );
                }
                driver.resize_window(args.value("--title")?, width, height)
            }
            "capture" => {
                let retries = args.u64_value("--retries", u64::from(CAPTURE_RETRIES))?;
                let retries =
                    u32::try_from(retries).map_err(|error| format!("--retries: {error}"))?;
                driver.capture(args.value("--title")?, args.value("--out")?, retries.max(1))
            }
            "diffstat" => {
                let region = match args.optional("--region") {
                    Some(raw) => {
                        let parts: Vec<&str> = raw.split(',').collect();
                        if parts.len() != 4 {
                            return Err(format!("--region expects x,y,w,h; got {raw:?}").into());
                        }
                        let parsed = parts
                            .iter()
                            .map(|part| {
                                part.trim()
                                    .parse::<usize>()
                                    .map_err(|error| format!("--region: {error}"))
                            })
                            .collect::<Result<Vec<usize>, _>>()?;
                        Some([parsed[0], parsed[1], parsed[2], parsed[3]])
                    }
                    None => None,
                };
                diffstat(args.value("--a")?, args.value("--b")?, region)
            }
            "close-request" => driver.send_close_request(args.value("--title")?),
            other => Err(DriverError::Other(format!(
                "unknown command {other:?}\n{}",
                usage()
            ))),
        }
    }

    fn geometry_line(geometry: &WindowGeometry) -> String {
        format!(
            "window=0x{:x} x={} y={} w={} h={} viewable={}",
            geometry.window,
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.viewable
        )
    }

    fn usage() -> String {
        "usage: xtest_input <command> [args]\n\
         commands:\n\
         \x20 wait-window --title T [--timeout-ms N]\n\
         \x20 geom --title T\n\
         \x20 click --title T --x N --y N        (window-relative coordinates)\n\
         \x20 move --x N --y N                   (absolute root coordinates)\n\
         \x20 type-text --text S\n\
         \x20 key --name escape|return|backspace|home|end|f1..f12 [--mod ctrl,shift] [--count N]\n\
         \x20 focus --title T | unfocus\n\
         \x20 resize --title T --w N --h N\n\
         \x20 capture --title T --out P [--retries N]\n\
         \x20 diffstat --a P1 --b P2 [--region x,y,w,h]\n\
         \x20 close-request --title T"
            .to_string()
    }
}
