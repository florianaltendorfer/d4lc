//! Game-window observer — the game's focus, visibility, lifetime and
//! geometry as the window system reports them, fed into the runtime
//! coordinator.
//!
//! This module is deliberately separate from [`crate::window_observer`], which watches
//! the companion's own main window for hide-to-tray.
//!
//! ## Upstream evidence
//!
//! `ScreenCaptureHandler.cs:175-243` checks every capture cycle whether the
//! Diablo IV window is the foreground window (`GetForegroundWindow`). When
//! it is not, the handler publishes an empty screen, which clears the
//! tooltip and the overlay; while it is, `GetWindowRect` updates the window
//! offset the overlay coordinates are based on.
//!
//! ## Linux adaptation
//!
//! On X11 the same facts are window-system state, observed event-driven on a
//! thread of its own — never on the capture hot path, never waiting on OCR
//! or detection:
//!
//! - focus: the root window's EWMH `_NET_ACTIVE_WINDOW`
//!   (`PropertyNotify` on the root);
//! - minimized / hidden: `_NET_WM_STATE_HIDDEN` in the game window's
//!   `_NET_WM_STATE` and its map state (`PropertyNotify`, `MapNotify`,
//!   `UnmapNotify`);
//! - window destroyed: `DestroyNotify`, or a query failing on the window;
//! - moved / resized: `ConfigureNotify` (real, or the synthetic one ICCCM
//!   window managers send on a move), answered with the window's root
//!   position and size.
//!
//! Every change goes into the live loop's coalescing [`ControlInbox`]; the
//! loop applies it before and after each frame, so a focus loss hides the
//! overlay within one frame whatever OCR is doing.
//!
//! A window manager that does not publish `_NET_ACTIVE_WINDOW` makes focus
//! unobservable: the observer then reports that capability as unavailable
//! and never claims focus loss it cannot see.
//!
//! On a Wayland capture stream there is no X11 window id; the equivalent
//! state source is KDE's `org_kde_plasma_window_management` protocol on
//! its own connection (KWin's native window list / focus / minimize /
//! geometry protocol) — see [`self::wayland`]. A compositor without that
//! protocol (or a build without the `wayland-wm-backend` feature) reports
//! the capability as unsupported, naming what is missing.

#[cfg(feature = "wayland-wm-backend")]
mod wayland;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use d4lc_core::Rect;
use d4lc_runtime::GameWindowState;

use crate::runtime_bridge::live_state::ControlInbox;

/// One fact the window system reported about the game window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameWindowEvent {
    /// `_NET_ACTIVE_WINDOW` changed; `None` when no window is active.
    ActiveWindow(Option<u64>),
    /// The window manager does not publish `_NET_ACTIVE_WINDOW`.
    ActiveWindowUnavailable,
    /// `_NET_WM_STATE_HIDDEN` is (not) set on the game window.
    Hidden(bool),
    /// The game window is (not) mapped.
    Mapped(bool),
    /// The game window's on-screen geometry (root coordinates).
    Geometry(Rect),
    /// The game window no longer exists.
    Destroyed,
}

/// What the tracker reports to the coordinator after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameWindowUpdate {
    State(GameWindowState),
    Geometry(Rect),
}

/// Folds window-system events into the coordinator's [`GameWindowState`].
///
/// Edge-triggered: only a change of the derived state (or of the geometry)
/// is reported, so repeated identical events cost nothing downstream.
#[derive(Debug, Clone)]
pub struct GameWindowTracker {
    window: u64,
    /// `None` while focus is not observable.
    active: Option<Option<u64>>,
    hidden: bool,
    mapped: bool,
    closed: bool,
    geometry: Option<Rect>,
    reported: Option<GameWindowState>,
}

impl GameWindowTracker {
    pub fn new(window: u64) -> Self {
        Self {
            window,
            active: None,
            hidden: false,
            mapped: true,
            closed: false,
            geometry: None,
            reported: None,
        }
    }

    /// The observed window.
    #[cfg(test)]
    pub const fn window(&self) -> u64 {
        self.window
    }

    /// Whether focus is observable (the window manager publishes
    /// `_NET_ACTIVE_WINDOW`).
    pub const fn focus_observable(&self) -> bool {
        self.active.is_some()
    }

    /// The derived coordinator state. Focus that cannot be observed is
    /// never reported as lost.
    pub fn state(&self) -> GameWindowState {
        GameWindowState {
            focused: match self.active {
                Some(active) => active == Some(self.window),
                None => true,
            },
            minimized: self.hidden || !self.mapped,
            closed: self.closed,
        }
    }

    /// Apply one event; returns what changed for the coordinator.
    pub fn apply(&mut self, event: GameWindowEvent) -> Vec<GameWindowUpdate> {
        match event {
            GameWindowEvent::ActiveWindow(active) => self.active = Some(active),
            GameWindowEvent::ActiveWindowUnavailable => self.active = None,
            GameWindowEvent::Hidden(hidden) => self.hidden = hidden,
            GameWindowEvent::Mapped(mapped) => self.mapped = mapped,
            GameWindowEvent::Destroyed => self.closed = true,
            GameWindowEvent::Geometry(geometry) => {
                if geometry.width == 0 || geometry.height == 0 || self.geometry == Some(geometry) {
                    return Vec::new();
                }
                self.geometry = Some(geometry);
                return vec![GameWindowUpdate::Geometry(geometry)];
            }
        }
        let state = self.state();
        if self.reported == Some(state) {
            return Vec::new();
        }
        self.reported = Some(state);
        vec![GameWindowUpdate::State(state)]
    }
}

/// Where the observer delivers its updates.
pub trait GameWindowSink: Send + 'static {
    fn window_state(&self, state: GameWindowState);
    fn window_geometry(&self, geometry: Rect);
}

impl GameWindowSink for ControlInbox {
    fn window_state(&self, state: GameWindowState) {
        self.report_game_window(state);
    }

    fn window_geometry(&self, geometry: Rect) {
        self.report_window_geometry(geometry);
    }
}

/// A source of window-system events for one game window.
pub trait GameWindowEventSource: Send + 'static {
    /// Events that describe the window's current state (sent once at
    /// start, before any change event).
    fn initial_events(&mut self) -> Result<Vec<GameWindowEvent>, String>;
    /// The next batch of change events; may return an empty batch after a
    /// short wait so the observer can check its stop flag.
    fn next_events(&mut self) -> Result<Vec<GameWindowEvent>, String>;
}

/// The observer's capability report, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameWindowObservation {
    /// Focus, visibility, lifetime and geometry are observed.
    Observing { window: u64 },
    /// Visibility, lifetime and geometry are observed, focus is not.
    FocusUnavailable { window: u64 },
    /// The game window is gone; the observer ended.
    WindowClosed { window: u64 },
    /// This session has no game-window state source.
    Unsupported { reason: String },
    /// The observer could not start or stopped on an error.
    Failed { reason: String },
}

impl GameWindowObservation {
    /// Text for the Debug screen and the log.
    pub fn text(&self) -> String {
        match self {
            Self::Observing { window } => {
                format!("observing 0x{window:x} (focus, minimize, close, move)")
            }
            Self::FocusUnavailable { window } => format!(
                "observing 0x{window:x} (minimize, close, move); focus not observable: the window \
                 manager does not publish _NET_ACTIVE_WINDOW"
            ),
            Self::WindowClosed { window } => format!("game window 0x{window:x} closed"),
            Self::Unsupported { reason } => format!("unsupported: {reason}"),
            Self::Failed { reason } => format!("failed: {reason}"),
        }
    }
}

/// Shared, readable capability status of a running observer.
#[derive(Debug, Clone)]
pub struct ObservationCell(Arc<Mutex<GameWindowObservation>>);

impl ObservationCell {
    fn new(observation: GameWindowObservation) -> Self {
        Self(Arc::new(Mutex::new(observation)))
    }

    pub fn get(&self) -> GameWindowObservation {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set(&self, observation: GameWindowObservation) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = observation;
    }
}

/// A running observer thread for one game window. Dropping it stops the
/// thread; the thread waits at most one short poll interval.
pub struct GameWindowObserver {
    window: u64,
    stop: Arc<AtomicBool>,
    status: ObservationCell,
    thread: Option<JoinHandle<()>>,
}

impl GameWindowObserver {
    /// Observe `window` from `source` and deliver every change to `sink`.
    pub fn spawn(
        window: u64,
        mut source: impl GameWindowEventSource,
        sink: impl GameWindowSink,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = ObservationCell::new(GameWindowObservation::Observing { window });
        let thread_stop = Arc::clone(&stop);
        let thread_status = status.clone();
        let thread = thread::Builder::new()
            .name("d4lc-game-window".to_string())
            .spawn(move || {
                let mut tracker = GameWindowTracker::new(window);
                let deliver = |tracker: &mut GameWindowTracker, events: Vec<GameWindowEvent>| {
                    for event in events {
                        for update in tracker.apply(event) {
                            match update {
                                GameWindowUpdate::State(state) => sink.window_state(state),
                                GameWindowUpdate::Geometry(geometry) => {
                                    sink.window_geometry(geometry)
                                }
                            }
                        }
                    }
                    thread_status.set(if tracker.state().closed {
                        GameWindowObservation::WindowClosed { window }
                    } else if tracker.focus_observable() {
                        GameWindowObservation::Observing { window }
                    } else {
                        GameWindowObservation::FocusUnavailable { window }
                    });
                    !tracker.state().closed
                };
                let initial = match source.initial_events() {
                    Ok(events) => events,
                    Err(reason) => {
                        thread_status.set(GameWindowObservation::Failed { reason });
                        return;
                    }
                };
                if !deliver(&mut tracker, initial) {
                    return;
                }
                while !thread_stop.load(Ordering::SeqCst) {
                    match source.next_events() {
                        Ok(events) => {
                            if !deliver(&mut tracker, events) {
                                return;
                            }
                        }
                        Err(reason) => {
                            thread_status.set(GameWindowObservation::Failed { reason });
                            return;
                        }
                    }
                }
            })
            .ok();
        if thread.is_none() {
            status.set(GameWindowObservation::Failed {
                reason: "the observer thread could not be spawned".to_string(),
            });
        }
        Self {
            window,
            stop,
            status,
            thread,
        }
    }

    pub const fn window(&self) -> u64 {
        self.window
    }

    /// The observer's current capability report.
    pub fn observation(&self) -> GameWindowObservation {
        self.status.get()
    }
}

impl Drop for GameWindowObserver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why a session without a captured X11 game window has no observer.
///
/// Wayland capture streams start the KDE window-management source instead
/// (`start_wayland`); its unsupported reports name what is missing there.
pub fn unsupported_reason() -> String {
    "no captured game window to observe (manual capture region)".to_string()
}

/// Start the X11 observer for `window`, feeding `sink`.
#[cfg(feature = "x11rb-backend")]
pub fn start_x11(window: u64, sink: impl GameWindowSink) -> Result<GameWindowObserver, String> {
    let source = x11::X11GameWindowSource::connect(window)?;
    Ok(GameWindowObserver::spawn(window, source, sink))
}

/// Without the x11rb transport there is no X11 event source.
#[cfg(not(feature = "x11rb-backend"))]
pub fn start_x11(_window: u64, _sink: impl GameWindowSink) -> Result<GameWindowObserver, String> {
    Err("built without the x11rb-backend feature".to_string())
}

/// Start the Wayland observer (KDE `org_kde_plasma_window_management`) on
/// the source's own connection, feeding `sink`. The Diablo window is
/// identified by title/app-id with the same default matching semantics as
/// the X11 path.
///
/// `Err` carries the fixed capability report the live loop publishes
/// verbatim: `Unsupported` when the session or build cannot provide the
/// capability at all, `Failed` when the identified start did not succeed.
#[cfg(feature = "wayland-wm-backend")]
pub fn start_wayland(
    sink: impl GameWindowSink,
) -> Result<GameWindowObserver, GameWindowObservation> {
    let source = wayland::PlasmaWindowSource::connect_and_identify()?;
    Ok(GameWindowObserver::spawn(
        source.internal_id(),
        source,
        sink,
    ))
}

/// One-shot lookup of the Diablo window's absolute geometry through KDE
/// window management — the overlay start uses it to pick the output a
/// window stream lives on. `None` when the window or protocol is absent.
#[cfg(feature = "wayland-wm-backend")]
pub fn wayland_game_window_geometry() -> Option<d4lc_core::Rect> {
    wayland::PlasmaWindowSource::connect_and_identify()
        .ok()
        .and_then(|source| source.game_window_geometry())
}

/// Without the Wayland window-management bindings the geometry is unknown.
#[cfg(not(feature = "wayland-wm-backend"))]
pub fn wayland_game_window_geometry() -> Option<d4lc_core::Rect> {
    None
}

/// Without the Wayland window-management bindings there is no Wayland
/// game-window state source; the report names the missing build piece.
#[cfg(not(feature = "wayland-wm-backend"))]
pub fn start_wayland(
    _sink: impl GameWindowSink,
) -> Result<GameWindowObserver, GameWindowObservation> {
    Err(GameWindowObservation::Unsupported {
        reason: "built without the wayland-wm-backend feature; the game window's focus, \
                 minimize and geometry are not observable on Wayland"
            .to_string(),
    })
}

/// Pure X11 protocol interpretation, kept apart from the connection so it is
/// unit-testable without a display.
pub mod protocol {
    use d4lc_core::Rect;

    /// `_NET_ACTIVE_WINDOW` value: a single WINDOW; `0` means no active
    /// window (EWMH). An absent or malformed property is `None` (focus not
    /// observable).
    pub fn active_window(format: u8, values: &[u32]) -> Option<Option<u64>> {
        if format != 32 {
            return None;
        }
        let window = *values.first()?;
        Some((window != 0).then_some(u64::from(window)))
    }

    /// Whether `_NET_WM_STATE` lists `_NET_WM_STATE_HIDDEN`.
    pub fn is_hidden(format: u8, atoms: &[u32], hidden_atom: u32) -> bool {
        format == 32 && atoms.contains(&hidden_atom)
    }

    /// The window's root-space rectangle from `TranslateCoordinates` (its
    /// origin on the root) and `GetGeometry` (its size).
    pub fn root_rect(root_x: i16, root_y: i16, width: u16, height: u16) -> Rect {
        Rect {
            x: i32::from(root_x),
            y: i32::from(root_y),
            width: u32::from(width),
            height: u32::from(height),
        }
    }

    /// What a notification about the observed windows asks for.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Query {
        ActiveWindow,
        WmState,
        Geometry,
        Mapped(bool),
        Destroyed,
        Ignore,
    }

    /// A reduced X11 event: only the fields the observer reads.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Notify {
        Property { window: u32, atom: u32 },
        Configure { window: u32 },
        Map { window: u32 },
        Unmap { window: u32 },
        Destroy { window: u32 },
    }

    /// The observed atoms and windows.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Watch {
        pub root: u32,
        pub game: u32,
        pub net_active_window: u32,
        pub net_wm_state: u32,
    }

    pub fn classify(watch: &Watch, notify: Notify) -> Query {
        match notify {
            Notify::Property { window, atom }
                if window == watch.root && atom == watch.net_active_window =>
            {
                Query::ActiveWindow
            }
            Notify::Property { window, atom }
                if window == watch.game && atom == watch.net_wm_state =>
            {
                Query::WmState
            }
            Notify::Configure { window } if window == watch.game => Query::Geometry,
            Notify::Map { window } if window == watch.game => Query::Mapped(true),
            Notify::Unmap { window } if window == watch.game => Query::Mapped(false),
            Notify::Destroy { window } if window == watch.game => Query::Destroyed,
            _ => Query::Ignore,
        }
    }
}

#[cfg(feature = "x11rb-backend")]
mod x11 {
    use std::time::Instant;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, MapState, Window,
    };
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    use super::protocol::{self, Notify, Query, Watch};
    use super::{GameWindowEvent, GameWindowEventSource, POLL_INTERVAL, POLL_SLICE};

    /// Event source over its own X11 connection.
    pub struct X11GameWindowSource {
        conn: RustConnection,
        watch: Watch,
        net_wm_state_hidden: u32,
    }

    impl X11GameWindowSource {
        pub fn connect(window: u64) -> Result<Self, String> {
            let game = Window::try_from(window)
                .map_err(|_| format!("window id 0x{window:x} is not an X11 window"))?;
            let (conn, screen_num) =
                RustConnection::connect(None).map_err(|e| format!("x11rb connect: {e}"))?;
            let root = conn.setup().roots[screen_num].root;
            let intern = |name: &[u8]| -> Result<u32, String> {
                conn.intern_atom(false, name)
                    .map_err(|e| format!("intern_atom: {e}"))?
                    .reply()
                    .map(|reply| reply.atom)
                    .map_err(|e| format!("intern_atom reply: {e}"))
            };
            let net_active_window = intern(b"_NET_ACTIVE_WINDOW")?;
            let net_wm_state = intern(b"_NET_WM_STATE")?;
            let net_wm_state_hidden = intern(b"_NET_WM_STATE_HIDDEN")?;
            conn.change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(|e| format!("select root events: {e}"))?
            .check()
            .map_err(|e| format!("select root events: {e}"))?;
            Ok(Self {
                conn,
                watch: Watch {
                    root,
                    game,
                    net_active_window,
                    net_wm_state,
                },
                net_wm_state_hidden,
            })
        }

        fn query(&self, query: Query) -> Option<GameWindowEvent> {
            match query {
                Query::ActiveWindow => Some(self.active_window()),
                Query::WmState => self.hidden().map(GameWindowEvent::Hidden),
                Query::Geometry => Some(
                    self.geometry()
                        .map_or(GameWindowEvent::Destroyed, GameWindowEvent::Geometry),
                ),
                Query::Mapped(mapped) => Some(GameWindowEvent::Mapped(mapped)),
                Query::Destroyed => Some(GameWindowEvent::Destroyed),
                Query::Ignore => None,
            }
        }

        fn active_window(&self) -> GameWindowEvent {
            let reply = self
                .conn
                .get_property(
                    false,
                    self.watch.root,
                    self.watch.net_active_window,
                    AtomEnum::WINDOW,
                    0,
                    1,
                )
                .ok()
                .and_then(|cookie| cookie.reply().ok());
            let parsed = reply.and_then(|reply| {
                let values: Vec<u32> = reply.value32().map(Iterator::collect).unwrap_or_default();
                protocol::active_window(reply.format, &values)
            });
            match parsed {
                Some(active) => GameWindowEvent::ActiveWindow(active),
                None => GameWindowEvent::ActiveWindowUnavailable,
            }
        }

        fn hidden(&self) -> Option<bool> {
            let reply = self
                .conn
                .get_property(
                    false,
                    self.watch.game,
                    self.watch.net_wm_state,
                    AtomEnum::ATOM,
                    0,
                    64,
                )
                .ok()?
                .reply()
                .ok()?;
            let atoms: Vec<u32> = reply.value32().map(Iterator::collect).unwrap_or_default();
            Some(protocol::is_hidden(
                if reply.format == 0 { 32 } else { reply.format },
                &atoms,
                self.net_wm_state_hidden,
            ))
        }

        /// `None` when the window is gone.
        fn geometry(&self) -> Option<d4lc_core::Rect> {
            let geometry = self.conn.get_geometry(self.watch.game).ok()?.reply().ok()?;
            let translated = self
                .conn
                .translate_coordinates(self.watch.game, self.watch.root, 0, 0)
                .ok()?
                .reply()
                .ok()?;
            Some(protocol::root_rect(
                translated.dst_x,
                translated.dst_y,
                geometry.width,
                geometry.height,
            ))
        }

        fn notify(event: &Event) -> Option<Notify> {
            match event {
                Event::PropertyNotify(e) => Some(Notify::Property {
                    window: e.window,
                    atom: e.atom,
                }),
                Event::ConfigureNotify(e) => Some(Notify::Configure { window: e.window }),
                Event::MapNotify(e) => Some(Notify::Map { window: e.window }),
                Event::UnmapNotify(e) => Some(Notify::Unmap { window: e.window }),
                Event::DestroyNotify(e) => Some(Notify::Destroy { window: e.window }),
                _ => None,
            }
        }
    }

    impl GameWindowEventSource for X11GameWindowSource {
        fn initial_events(&mut self) -> Result<Vec<GameWindowEvent>, String> {
            let selected = self
                .conn
                .change_window_attributes(
                    self.watch.game,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
                )
                .map_err(|e| format!("select game window events: {e}"))?
                .check();
            if selected.is_err() {
                return Ok(vec![GameWindowEvent::Destroyed]);
            }
            let mapped = self
                .conn
                .get_window_attributes(self.watch.game)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|attributes| attributes.map_state != MapState::UNMAPPED);
            let Some(mapped) = mapped else {
                return Ok(vec![GameWindowEvent::Destroyed]);
            };
            let mut events = vec![self.active_window(), GameWindowEvent::Mapped(mapped)];
            if let Some(hidden) = self.hidden() {
                events.push(GameWindowEvent::Hidden(hidden));
            }
            events.push(
                self.geometry()
                    .map_or(GameWindowEvent::Destroyed, GameWindowEvent::Geometry),
            );
            Ok(events)
        }

        fn next_events(&mut self) -> Result<Vec<GameWindowEvent>, String> {
            let started = Instant::now();
            let mut queries = Vec::new();
            loop {
                while let Some(event) = self
                    .conn
                    .poll_for_event()
                    .map_err(|e| format!("x11 connection: {e}"))?
                {
                    if let Some(notify) = Self::notify(&event) {
                        let query = protocol::classify(&self.watch, notify);
                        if query != Query::Ignore && !queries.contains(&query) {
                            queries.push(query);
                        }
                    }
                }
                if !queries.is_empty() || started.elapsed() >= POLL_INTERVAL {
                    break;
                }
                std::thread::sleep(POLL_SLICE);
            }
            Ok(queries
                .into_iter()
                .filter_map(|query| self.query(query))
                .collect())
        }
    }
}

/// How long one `next_events` call waits for events before it returns an
/// empty batch (bounds the observer's stop latency).
#[cfg_attr(
    not(any(feature = "x11rb-backend", feature = "wayland-wm-backend")),
    allow(dead_code)
)]
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Sleep between two event polls (bounds the observer's reaction latency).
#[cfg_attr(
    not(any(feature = "x11rb-backend", feature = "wayland-wm-backend")),
    allow(dead_code)
)]
const POLL_SLICE: Duration = Duration::from_millis(5);

/// A scripted event source for tests of the observer and its consumers.
#[cfg(test)]
pub(crate) mod fake {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{GameWindowEvent, GameWindowEventSource};

    /// The test pushes event batches through a channel; an empty wait
    /// returns an empty batch.
    pub(crate) struct FakeSource {
        pub initial: Vec<GameWindowEvent>,
        pub events: mpsc::Receiver<Vec<GameWindowEvent>>,
    }

    impl GameWindowEventSource for FakeSource {
        fn initial_events(&mut self) -> Result<Vec<GameWindowEvent>, String> {
            Ok(std::mem::take(&mut self.initial))
        }

        fn next_events(&mut self) -> Result<Vec<GameWindowEvent>, String> {
            match self.events.recv_timeout(Duration::from_millis(5)) {
                Ok(events) => Ok(events),
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(Vec::new()),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err("source closed".to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSource;
    use super::protocol::{self, Notify, Query, Watch};
    use super::*;
    use std::sync::mpsc;

    const GAME: u64 = 0x0420_0007;

    fn rect(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            width: 2560,
            height: 1440,
        }
    }

    #[test]
    fn tracker_derives_focus_minimize_and_close() {
        let mut tracker = GameWindowTracker::new(GAME);
        assert_eq!(
            tracker.apply(GameWindowEvent::ActiveWindow(Some(GAME))),
            vec![GameWindowUpdate::State(GameWindowState::default())]
        );
        // Another window becomes active: focus lost.
        let lost = tracker.apply(GameWindowEvent::ActiveWindow(Some(1)));
        assert_eq!(
            lost,
            vec![GameWindowUpdate::State(GameWindowState {
                focused: false,
                ..GameWindowState::default()
            })]
        );
        // No active window at all is not the game either.
        assert!(tracker
            .apply(GameWindowEvent::ActiveWindow(None))
            .is_empty());
        tracker.apply(GameWindowEvent::ActiveWindow(Some(GAME)));
        assert!(tracker.apply(GameWindowEvent::Hidden(true)).len() == 1);
        assert!(tracker.state().minimized);
        tracker.apply(GameWindowEvent::Hidden(false));
        tracker.apply(GameWindowEvent::Mapped(false));
        assert!(tracker.state().minimized, "an unmapped window is not shown");
        tracker.apply(GameWindowEvent::Mapped(true));
        assert!(!tracker.state().minimized);
        tracker.apply(GameWindowEvent::Destroyed);
        assert!(tracker.state().closed);
    }

    #[test]
    fn unobservable_focus_is_never_reported_as_lost() {
        let mut tracker = GameWindowTracker::new(GAME);
        tracker.apply(GameWindowEvent::ActiveWindowUnavailable);
        assert!(!tracker.focus_observable());
        assert!(tracker.state().focused);
    }

    #[test]
    fn tracker_reports_each_geometry_change_once() {
        let mut tracker = GameWindowTracker::new(GAME);
        assert_eq!(
            tracker.apply(GameWindowEvent::Geometry(rect(0, 0))),
            vec![GameWindowUpdate::Geometry(rect(0, 0))]
        );
        assert!(tracker
            .apply(GameWindowEvent::Geometry(rect(0, 0)))
            .is_empty());
        assert_eq!(
            tracker.apply(GameWindowEvent::Geometry(rect(-2560, 0))),
            vec![GameWindowUpdate::Geometry(rect(-2560, 0))]
        );
        assert!(tracker
            .apply(GameWindowEvent::Geometry(Rect {
                width: 0,
                ..rect(0, 0)
            }))
            .is_empty());
    }

    #[test]
    fn active_window_property_parsing() {
        assert_eq!(protocol::active_window(32, &[0x42]), Some(Some(0x42)));
        assert_eq!(protocol::active_window(32, &[0]), Some(None));
        // Absent (format 0, no value) or malformed: not observable.
        assert_eq!(protocol::active_window(0, &[]), None);
        assert_eq!(protocol::active_window(32, &[]), None);
        assert_eq!(protocol::active_window(8, &[0x42]), None);
    }

    #[test]
    fn wm_state_hidden_parsing() {
        assert!(protocol::is_hidden(32, &[7, 9], 9));
        assert!(!protocol::is_hidden(32, &[7], 9));
        assert!(!protocol::is_hidden(32, &[], 9));
        assert!(!protocol::is_hidden(8, &[9], 9));
    }

    #[test]
    fn root_rect_keeps_negative_monitor_positions() {
        assert_eq!(
            protocol::root_rect(-1920, 24, 1920, 1056),
            Rect {
                x: -1920,
                y: 24,
                width: 1920,
                height: 1056
            }
        );
    }

    #[test]
    fn notifications_are_classified_per_observed_window() {
        let watch = Watch {
            root: 1,
            game: 2,
            net_active_window: 10,
            net_wm_state: 11,
        };
        use protocol::classify;
        assert_eq!(
            classify(
                &watch,
                Notify::Property {
                    window: 1,
                    atom: 10
                }
            ),
            Query::ActiveWindow
        );
        assert_eq!(
            classify(
                &watch,
                Notify::Property {
                    window: 2,
                    atom: 11
                }
            ),
            Query::WmState
        );
        // The active-window atom on the game window, or the state atom on
        // the root, are not what the observer watches.
        assert_eq!(
            classify(
                &watch,
                Notify::Property {
                    window: 2,
                    atom: 10
                }
            ),
            Query::Ignore
        );
        assert_eq!(
            classify(&watch, Notify::Configure { window: 2 }),
            Query::Geometry
        );
        assert_eq!(
            classify(&watch, Notify::Configure { window: 3 }),
            Query::Ignore
        );
        assert_eq!(
            classify(&watch, Notify::Unmap { window: 2 }),
            Query::Mapped(false)
        );
        assert_eq!(
            classify(&watch, Notify::Map { window: 2 }),
            Query::Mapped(true)
        );
        assert_eq!(
            classify(&watch, Notify::Destroy { window: 2 }),
            Query::Destroyed
        );
    }

    fn wait_for(inbox: &ControlInbox) -> crate::runtime_bridge::live_state::PendingControls {
        for _ in 0..2000 {
            let pending = inbox.drain();
            if pending.game_window.is_some() || pending.window_geometry.is_some() {
                return pending;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the observer delivered nothing");
    }

    #[test]
    fn observer_thread_feeds_the_control_inbox() {
        let (tx, rx) = mpsc::channel();
        let inbox = ControlInbox::default();
        let observer = GameWindowObserver::spawn(
            GAME,
            FakeSource {
                initial: vec![
                    GameWindowEvent::ActiveWindow(Some(GAME)),
                    GameWindowEvent::Geometry(rect(0, 0)),
                ],
                events: rx,
            },
            inbox.clone(),
        );
        // The initial state and geometry are two reports; collect both.
        let mut first = wait_for(&inbox);
        if first.game_window.is_none() || first.window_geometry.is_none() {
            let rest = wait_for(&inbox);
            first.game_window = first.game_window.or(rest.game_window);
            first.window_geometry = first.window_geometry.or(rest.window_geometry);
        }
        assert_eq!(first.game_window, Some(GameWindowState::default()));
        assert_eq!(first.window_geometry, Some(rect(0, 0)));

        tx.send(vec![GameWindowEvent::ActiveWindow(Some(1))])
            .unwrap_or_else(|error| panic!("send: {error}"));
        let lost = wait_for(&inbox);
        assert_eq!(
            lost.game_window,
            Some(GameWindowState {
                focused: false,
                ..GameWindowState::default()
            })
        );

        tx.send(vec![GameWindowEvent::Destroyed])
            .unwrap_or_else(|error| panic!("send: {error}"));
        let closed = wait_for(&inbox);
        assert!(closed.game_window.is_some_and(|state| state.closed));
        for _ in 0..2000 {
            if observer.observation() == (GameWindowObservation::WindowClosed { window: GAME }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            observer.observation(),
            GameWindowObservation::WindowClosed { window: GAME }
        );
        drop(observer);
    }

    #[test]
    fn unsupported_capability_is_named() {
        // A manual capture region has no game window to observe.
        assert!(GameWindowObservation::Unsupported {
            reason: unsupported_reason()
        }
        .text()
        .starts_with("unsupported: no captured game window"));
    }

    #[cfg(feature = "wayland-wm-backend")]
    #[test]
    fn start_wayland_without_a_session_reports_failed_not_unsupported() {
        // The validation environment strips WAYLAND_DISPLAY: a connection
        // attempt fails, which is a start failure — the unsupported report
        // is reserved for sessions the protocol cannot serve (missing
        // global, covered by the compositor double tests).
        let Err(error) = start_wayland(crate::runtime_bridge::live_state::ControlInbox::default())
        else {
            panic!("no Wayland session in validation runs");
        };
        assert!(
            matches!(error, GameWindowObservation::Failed { .. }),
            "a failed connection is a start failure: {error:?}"
        );
        assert!(error
            .text()
            .starts_with("failed: Wayland connection failed"));
    }

    #[cfg(not(feature = "wayland-wm-backend"))]
    #[test]
    fn start_wayland_without_the_feature_names_the_build_piece() {
        let error = start_wayland(crate::runtime_bridge::live_state::ControlInbox::default())
            .expect_err("without the feature there is no Wayland source");
        assert!(matches!(error, GameWindowObservation::Unsupported { .. }));
        assert!(
            error.text().contains("wayland-wm-backend"),
            "the report must name the missing build piece: {error:?}"
        );
    }
}
