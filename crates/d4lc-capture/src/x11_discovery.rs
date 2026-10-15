//! X11 window discovery over a native X11 connection (x11rb, no helper
//! programs).
//!
//! The candidate list is the union, in this order and without duplicates,
//! of the window manager's EWMH client lists on the root window
//! (`_NET_CLIENT_LIST_STACKING`, `_NET_CLIENT_LIST`) and the whole window
//! tree below the root (depth-first, topmost sibling first). The tree keeps
//! discovery working without an EWMH window manager; without an active
//! window the first match wins, so the topmost matching window is chosen. Every window then
//! gets its title (`_NET_WM_NAME`, else `WM_NAME`), its `WM_CLASS` class,
//! its `_NET_WM_PID` and its absolute geometry; a window that vanished or
//! has no valid geometry is left out, a window without properties is kept
//! with empty values.
//!
//! All requests of one phase are sent before the first reply is read, so a
//! discovery costs a handful of round trips regardless of the window count.

use d4lc_core::{Rect, WindowInfo};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::cookie::Cookie;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, GetPropertyReply, Window};

use crate::{validate_rect, WindowCandidate, WindowQuery, WindowSelection};

/// Discover the X11 windows on the display named by `DISPLAY` and select
/// the one `query` asks for (the active matching window first, else the
/// first matching one).
///
/// # Errors
///
/// When the X server cannot be reached, the root window cannot be read, or
/// no window exists at all.
pub fn discover_x11_windows(query: &WindowQuery) -> Result<WindowSelection, String> {
    discover_on_display(None, query)
}

/// [`discover_x11_windows`] on the named display (`None`: `DISPLAY`).
fn discover_on_display(
    display: Option<&str>,
    query: &WindowQuery,
) -> Result<WindowSelection, String> {
    let (connection, screen) = x11rb::connect(display)
        .map_err(|error| format!("cannot connect to the X server: {error}"))?;
    let root = connection
        .setup()
        .roots
        .get(screen)
        .ok_or_else(|| format!("the X server has no screen {screen}"))?
        .root;
    discover_on(&connection, root, query)
}

/// [`discover_x11_windows`] on an open connection.
pub(crate) fn discover_on(
    connection: &impl Connection,
    root: Window,
    query: &WindowQuery,
) -> Result<WindowSelection, String> {
    let snapshot = read_windows(connection, root)?;
    Ok(select_windows(snapshot.windows, snapshot.active, query))
}

/// Whether one window satisfies the query (title/class case-insensitive
/// containment, exact-title mode, pid pin). Public so the Wayland
/// game-window observer reuses the exact X11 matching semantics instead of
/// a second matcher.
pub fn window_matches_query(info: &WindowInfo, pid: Option<u32>, query: &WindowQuery) -> bool {
    if let Some(expected_pid) = query.pid {
        if pid != Some(expected_pid) {
            return false;
        }
    }
    if let Some(expected_class) = &query.class_name {
        let Some(class_name) = &info.class_name else {
            return false;
        };
        if !contains_case_insensitive(class_name, expected_class) {
            return false;
        }
    }
    if let Some(expected_title) = &query.title {
        if query.exact_title {
            if info.title.as_str() != expected_title.as_str() {
                return false;
            }
        } else if !contains_case_insensitive(&info.title, expected_title) {
            return false;
        }
    }
    true
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

/// One window as read from the X server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct X11Window {
    pub(crate) id: u64,
    pub(crate) title: Option<String>,
    pub(crate) class_name: Option<String>,
    pub(crate) pid: Option<u32>,
    pub(crate) rect: Rect,
}

/// The windows of one discovery pass and the active window, if any.
#[derive(Debug)]
pub(crate) struct WindowSnapshot {
    pub(crate) windows: Vec<X11Window>,
    pub(crate) active: Option<u64>,
}

/// Turn the windows of one pass into candidates and pick the selection.
pub(crate) fn select_windows(
    windows: Vec<X11Window>,
    active: Option<u64>,
    query: &WindowQuery,
) -> WindowSelection {
    let candidates = windows
        .into_iter()
        .map(|window| {
            let info = WindowInfo {
                id: window.id,
                title: window.title.unwrap_or_default(),
                class_name: window.class_name,
                rect: window.rect,
            };
            let matches_query = window_matches_query(&info, window.pid, query);
            WindowCandidate {
                info,
                pid: window.pid,
                is_active: active == Some(window.id),
                matches_query,
            }
        })
        .collect::<Vec<_>>();
    let selected = candidates
        .iter()
        .find(|candidate| candidate.matches_query && candidate.is_active)
        .or_else(|| candidates.iter().find(|candidate| candidate.matches_query))
        .cloned();
    WindowSelection {
        candidates,
        selected,
    }
}

/// The atoms discovery reads.
struct Atoms {
    client_list_stacking: u32,
    client_list: u32,
    active_window: u32,
    wm_name_utf8: u32,
    wm_pid: u32,
}

fn intern_atoms(connection: &impl Connection) -> Result<Atoms, String> {
    let names: [&[u8]; 5] = [
        b"_NET_CLIENT_LIST_STACKING",
        b"_NET_CLIENT_LIST",
        b"_NET_ACTIVE_WINDOW",
        b"_NET_WM_NAME",
        b"_NET_WM_PID",
    ];
    let cookies = names
        .iter()
        .map(|name| connection.intern_atom(false, name).map_err(x11_error))
        .collect::<Result<Vec<_>, _>>()?;
    let atoms = cookies
        .into_iter()
        .map(|cookie| cookie.reply().map(|reply| reply.atom).map_err(x11_error))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Atoms {
        client_list_stacking: atoms[0],
        client_list: atoms[1],
        active_window: atoms[2],
        wm_name_utf8: atoms[3],
        wm_pid: atoms[4],
    })
}

/// Read every candidate window of the display (module docs).
pub(crate) fn read_windows(
    connection: &impl Connection,
    root: Window,
) -> Result<WindowSnapshot, String> {
    let atoms = intern_atoms(connection)?;

    let mut ids: Vec<Window> = Vec::new();
    let push_unique = |id: Window, ids: &mut Vec<Window>| {
        if id != 0 && id != root && !ids.contains(&id) {
            ids.push(id);
        }
    };
    for list in [atoms.client_list_stacking, atoms.client_list] {
        if let Some(reply) = root_property(connection, root, list)? {
            for id in reply.value32().into_iter().flatten() {
                push_unique(id, &mut ids);
            }
        }
    }
    for id in window_tree(connection, root)? {
        push_unique(id, &mut ids);
    }
    if ids.is_empty() {
        return Err("no X11 window ids were discovered (empty client list and window tree)".into());
    }

    let active = root_property(connection, root, atoms.active_window)?
        .and_then(|reply| reply.value32().and_then(|mut values| values.next()))
        .filter(|id| *id != 0)
        .map(u64::from);

    // Phase 1: properties and geometry of every window.
    let mut pending = Vec::with_capacity(ids.len());
    for &id in &ids {
        pending.push((
            id,
            property_request(connection, id, atoms.wm_name_utf8)?,
            property_request(connection, id, AtomEnum::WM_NAME.into())?,
            property_request(connection, id, AtomEnum::WM_CLASS.into())?,
            property_request(connection, id, atoms.wm_pid)?,
            connection.get_geometry(id).map_err(x11_error)?,
        ));
    }
    let mut described = Vec::with_capacity(pending.len());
    for (id, net_name, wm_name, wm_class, wm_pid, geometry) in pending {
        // A window destroyed since it was listed answers BadWindow here;
        // that window is simply no longer a candidate.
        let Ok(geometry) = geometry.reply() else {
            continue;
        };
        let title = net_name
            .reply()
            .ok()
            .and_then(|reply| text_property(&reply))
            .or_else(|| wm_name.reply().ok().and_then(|reply| text_property(&reply)));
        let class_name = wm_class
            .reply()
            .ok()
            .and_then(|reply| class_property(&reply));
        let pid = wm_pid
            .reply()
            .ok()
            .and_then(|reply| reply.value32().and_then(|mut values| values.next()));
        described.push((id, title, class_name, pid, geometry));
    }

    // Phase 2: absolute position. Like `xwininfo`, the upper-left corner
    // is the outer corner of the window border.
    let mut positioned = Vec::with_capacity(described.len());
    for (id, title, class_name, pid, geometry) in described {
        let border = i16::try_from(geometry.border_width).unwrap_or(i16::MAX);
        let cookie = connection
            .translate_coordinates(id, root, -border, -border)
            .map_err(x11_error)?;
        positioned.push((id, title, class_name, pid, geometry, cookie));
    }
    let mut windows = Vec::with_capacity(positioned.len());
    for (id, title, class_name, pid, geometry, cookie) in positioned {
        let Ok(origin) = cookie.reply() else {
            continue;
        };
        let rect = Rect {
            x: i32::from(origin.dst_x),
            y: i32::from(origin.dst_y),
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
        };
        if validate_rect(rect).is_err() {
            continue;
        }
        windows.push(X11Window {
            id: u64::from(id),
            title,
            class_name,
            pid,
            rect,
        });
    }
    Ok(WindowSnapshot { windows, active })
}

/// Every window below `root`, depth-first, topmost sibling first.
fn window_tree(connection: &impl Connection, root: Window) -> Result<Vec<Window>, String> {
    let mut order = Vec::new();
    let mut level = vec![root];
    // Breadth of one tree level is queried together; the order is then
    // rebuilt depth-first from the per-window child lists.
    let mut children_of = std::collections::HashMap::new();
    while !level.is_empty() {
        let cookies = level
            .iter()
            .map(|&window| {
                connection
                    .query_tree(window)
                    .map(|cookie| (window, cookie))
                    .map_err(x11_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = Vec::new();
        for (window, cookie) in cookies {
            match cookie.reply() {
                Ok(reply) => {
                    next.extend(reply.children.iter().copied());
                    children_of.insert(window, reply.children);
                }
                Err(error) if window == root => {
                    return Err(format!("cannot read the X11 window tree: {error}"));
                }
                // A child destroyed while the tree was read has no subtree.
                Err(_) => {}
            }
        }
        level = next;
    }
    let mut stack = vec![root];
    while let Some(window) = stack.pop() {
        if window != root {
            order.push(window);
        }
        // QueryTree lists children bottom-to-top; pushing them in that order
        // pops the topmost first.
        if let Some(children) = children_of.get(&window) {
            stack.extend(children.iter().copied());
        }
    }
    Ok(order)
}

fn root_property(
    connection: &impl Connection,
    root: Window,
    property: u32,
) -> Result<Option<GetPropertyReply>, String> {
    let reply = property_request(connection, root, property)?
        .reply()
        .map_err(x11_error)?;
    Ok((reply.type_ != u32::from(AtomEnum::NONE)).then_some(reply))
}

/// Upper bound of one property read in 32-bit units (4 MiB): far above any
/// title, class or client list, and a hard cap on what a client can make
/// discovery read.
const MAX_PROPERTY_LONGS: u32 = 1 << 20;

fn property_request<C: RequestConnection>(
    connection: &C,
    window: Window,
    property: u32,
) -> Result<Cookie<'_, C, GetPropertyReply>, String> {
    connection
        .get_property(
            false,
            window,
            property,
            AtomEnum::ANY,
            0,
            MAX_PROPERTY_LONGS,
        )
        .map_err(x11_error)
}

/// A text property: UTF-8 when it decodes as such (`UTF8_STRING`, and the
/// ASCII subset of `STRING`), else Latin-1 (`STRING` is ISO 8859-1).
fn text_property(reply: &GetPropertyReply) -> Option<String> {
    if reply.type_ == u32::from(AtomEnum::NONE) || reply.format != 8 {
        return None;
    }
    let bytes = reply.value.split(|byte| *byte == 0).next().unwrap_or(&[]);
    Some(match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
    })
}

/// `WM_CLASS` holds `instance\0class\0`; the class is the second string
/// (the instance when the class is missing).
fn class_property(reply: &GetPropertyReply) -> Option<String> {
    if reply.type_ == u32::from(AtomEnum::NONE) || reply.format != 8 {
        return None;
    }
    let mut parts = reply
        .value
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned());
    let instance = parts.next();
    parts.next().or(instance)
}

fn x11_error(error: impl std::fmt::Display) -> String {
    format!("X11 window discovery failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_xvfb::Xvfb;
    use x11rb::protocol::xproto::{CreateWindowAux, PropMode, WindowClass};
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    fn window(id: u64, title: &str, pid: Option<u32>) -> X11Window {
        X11Window {
            id,
            title: Some(title.to_string()),
            class_name: Some("Diablo IV".to_string()),
            pid,
            rect: Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        }
    }

    #[test]
    fn selection_prefers_the_active_matching_window() {
        let query = WindowQuery {
            title: Some("Diablo IV".to_string()),
            exact_title: true,
            ..WindowQuery::default()
        };
        let windows = vec![
            window(1, "Diablo IV", Some(1)),
            window(2, "Diablo IV", Some(2)),
            window(3, "Terminal", None),
        ];
        let selection = select_windows(windows.clone(), Some(2), &query);
        assert_eq!(selection.candidates.len(), 3);
        assert_eq!(selection.selected.map(|c| c.info.id), Some(2));

        // Without an active match the first match in discovery order wins.
        let selection = select_windows(windows.clone(), Some(3), &query);
        assert_eq!(selection.selected.map(|c| c.info.id), Some(1));

        // A pid pin overrides the active window.
        let pinned = WindowQuery {
            pid: Some(1),
            ..query.clone()
        };
        let selection = select_windows(windows, Some(2), &pinned);
        assert_eq!(selection.selected.map(|c| c.info.id), Some(1));
    }

    #[test]
    fn a_window_without_properties_stays_a_candidate() {
        let windows = vec![X11Window {
            title: None,
            class_name: None,
            pid: None,
            ..window(7, "", None)
        }];
        let selection = select_windows(windows, None, &WindowQuery::default());
        assert_eq!(selection.candidates.len(), 1);
        assert_eq!(selection.candidates[0].info.title, "");
        assert_eq!(selection.selected.map(|c| c.info.id), Some(7));
    }

    /// A display with test windows; the server runs without a window
    /// manager, so the tests set the EWMH root properties themselves.
    struct Desk {
        _server: Xvfb,
        connection: RustConnection,
        root: Window,
    }

    impl Desk {
        fn new() -> Self {
            let server = Xvfb::start("640x480x24");
            let (connection, screen) = x11rb::connect(Some(server.display()))
                .unwrap_or_else(|error| panic!("connect to Xvfb: {error}"));
            let root = connection.setup().roots[screen].root;
            Self {
                _server: server,
                connection,
                root,
            }
        }

        fn atom(&self, name: &str) -> u32 {
            self.connection
                .intern_atom(false, name.as_bytes())
                .unwrap_or_else(|error| panic!("intern {name}: {error}"))
                .reply()
                .unwrap_or_else(|error| panic!("intern {name}: {error}"))
                .atom
        }

        /// A mapped top-level window at `rect` with `border` pixels of border.
        fn window(&self, parent: Window, rect: Rect, border: u16) -> Window {
            let id = self
                .connection
                .generate_id()
                .unwrap_or_else(|error| panic!("window id: {error}"));
            self.connection
                .create_window(
                    0,
                    id,
                    parent,
                    i16::try_from(rect.x).unwrap_or_default(),
                    i16::try_from(rect.y).unwrap_or_default(),
                    u16::try_from(rect.width).unwrap_or_default(),
                    u16::try_from(rect.height).unwrap_or_default(),
                    border,
                    WindowClass::INPUT_OUTPUT,
                    0,
                    &CreateWindowAux::new(),
                )
                .unwrap_or_else(|error| panic!("create window: {error}"));
            self.connection
                .map_window(id)
                .unwrap_or_else(|error| panic!("map window: {error}"));
            id
        }

        fn set_text(&self, window: Window, property: &str, kind: &str, value: &[u8]) {
            let (property, kind) = (self.atom(property), self.atom(kind));
            self.connection
                .change_property8(PropMode::REPLACE, window, property, kind, value)
                .unwrap_or_else(|error| panic!("set text property: {error}"));
        }

        fn set_windows(&self, window: Window, property: &str, ids: &[Window]) {
            let property = self.atom(property);
            self.connection
                .change_property32(PropMode::REPLACE, window, property, AtomEnum::WINDOW, ids)
                .unwrap_or_else(|error| panic!("set window list: {error}"));
        }

        fn set_pid(&self, window: Window, pid: u32) {
            let property = self.atom("_NET_WM_PID");
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    property,
                    AtomEnum::CARDINAL,
                    &[pid],
                )
                .unwrap_or_else(|error| panic!("set pid: {error}"));
        }

        fn discover(&self, query: &WindowQuery) -> WindowSelection {
            self.connection
                .sync()
                .unwrap_or_else(|error| panic!("sync: {error}"));
            discover_on(&self.connection, self.root, query)
                .unwrap_or_else(|error| panic!("discovery failed: {error}"))
        }
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn diablo_query() -> WindowQuery {
        WindowQuery::default().with_default_diablo_title()
    }

    #[test]
    fn discovers_titles_classes_pids_and_geometry_on_a_real_server() {
        let desk = Desk::new();
        let game = desk.window(desk.root, rect(40, 60, 320, 200), 0);
        desk.set_text(game, "_NET_WM_NAME", "UTF8_STRING", "Diablo IV".as_bytes());
        desk.set_text(game, "WM_NAME", "STRING", b"fallback");
        desk.set_text(
            game,
            "WM_CLASS",
            "STRING",
            b"steam_app_2344520\0Diablo IV\0",
        );
        desk.set_pid(game, 4242);
        let bordered = desk.window(desk.root, rect(100, 120, 50, 40), 3);
        desk.set_text(bordered, "WM_NAME", "STRING", b"Diablo IV \xfc");

        let selection = desk.discover(&diablo_query());
        let by_id = |id: Window| {
            selection
                .candidates
                .iter()
                .find(|candidate| candidate.info.id == u64::from(id))
                .unwrap_or_else(|| panic!("window {id:#x} missing: {selection:?}"))
        };
        let game_candidate = by_id(game);
        assert_eq!(game_candidate.info.title, "Diablo IV");
        assert_eq!(game_candidate.info.class_name.as_deref(), Some("Diablo IV"));
        assert_eq!(game_candidate.pid, Some(4242));
        assert_eq!(game_candidate.info.rect, rect(40, 60, 320, 200));
        // WM_NAME fallback, Latin-1 decoded; the rect starts at the outer
        // border corner like `xwininfo` reports it.
        let bordered_candidate = by_id(bordered);
        assert_eq!(bordered_candidate.info.title, "Diablo IV ü");
        assert_eq!(bordered_candidate.info.rect, rect(100, 120, 50, 40));
        assert_eq!(bordered_candidate.pid, None);
        // Both titles contain "Diablo IV"; without an active window the
        // topmost one (created last) is selected.
        assert_eq!(
            selection.selected.map(|c| c.info.id),
            Some(u64::from(bordered))
        );
    }

    #[test]
    fn follows_the_ewmh_lists_the_active_window_and_renames() {
        let desk = Desk::new();
        let first = desk.window(desk.root, rect(0, 0, 200, 100), 0);
        let second = desk.window(desk.root, rect(10, 10, 200, 100), 0);
        let terminal = desk.window(desk.root, rect(20, 20, 200, 100), 0);
        for (window, pid) in [(first, 1), (second, 2)] {
            desk.set_text(window, "_NET_WM_NAME", "UTF8_STRING", b"Diablo IV");
            desk.set_pid(window, pid);
        }
        desk.set_text(terminal, "_NET_WM_NAME", "UTF8_STRING", b"Terminal");
        // A window manager lists clients bottom-to-top and marks the
        // focused one; the stale id 0x7fffff of a closed window must not
        // break discovery.
        desk.set_windows(
            desk.root,
            "_NET_CLIENT_LIST_STACKING",
            &[terminal, second, 0x7f_ffff, first],
        );
        desk.set_windows(desk.root, "_NET_ACTIVE_WINDOW", &[second]);

        let selection = desk.discover(&diablo_query());
        let order = selection
            .candidates
            .iter()
            .map(|candidate| candidate.info.id)
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            vec![u64::from(terminal), u64::from(second), u64::from(first)],
            "client-list order first, the stale id dropped"
        );
        assert_eq!(
            selection.selected.map(|c| c.info.id),
            Some(u64::from(second))
        );
        assert!(selection.candidates[1].is_active);

        // A pid pin selects the inactive instance.
        let pinned = WindowQuery {
            pid: Some(1),
            ..diablo_query()
        };
        assert_eq!(
            desk.discover(&pinned).selected.map(|c| c.info.id),
            Some(u64::from(first))
        );

        // Renaming the active window away moves the selection.
        desk.set_text(second, "_NET_WM_NAME", "UTF8_STRING", b"Battle.net");
        assert_eq!(
            desk.discover(&diablo_query()).selected.map(|c| c.info.id),
            Some(u64::from(first))
        );

        // A window that disappears is no longer a candidate.
        desk.connection
            .destroy_window(first)
            .unwrap_or_else(|error| panic!("destroy: {error}"));
        let selection = desk.discover(&diablo_query());
        assert!(selection
            .candidates
            .iter()
            .all(|candidate| candidate.info.id != u64::from(first)));
        assert_eq!(selection.selected, None);
    }

    #[test]
    fn reparented_client_windows_are_found_below_their_frames() {
        let desk = Desk::new();
        let frame = desk.window(desk.root, rect(30, 40, 300, 220), 0);
        let client = desk.window(frame, rect(5, 20, 290, 195), 0);
        desk.set_text(client, "_NET_WM_NAME", "UTF8_STRING", b"Diablo IV");

        let selection = desk.discover(&diablo_query());
        let selected = selection
            .selected
            .clone()
            .unwrap_or_else(|| panic!("client below the frame not found: {selection:?}"));
        assert_eq!(selected.info.id, u64::from(client));
        assert_eq!(selected.info.rect, rect(35, 60, 290, 195));
    }

    #[test]
    fn an_unreachable_display_is_a_structured_error() {
        let Err(error) = discover_on_display(Some(":999"), &diablo_query()) else {
            panic!("no X server listens on :999");
        };
        assert!(
            error.starts_with("cannot connect to the X server"),
            "unexpected error: {error}"
        );
    }
}
