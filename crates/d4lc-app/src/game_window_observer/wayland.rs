//! Wayland game-window state source over KDE's plasma-window-management.
//!
//! On a Wayland capture stream there is no X11 window id to observe, so
//! this source connects to the compositor on its **own** `wl_display`
//! connection (never the overlay's) on the observer thread and talks
//! `org_kde_plasma_window_management` — KWin's native protocol for exactly
//! the facts this observer needs: the window list, the per-window active
//! flag, the minimized flag and the window geometry. KWin implements
//! neither `zwlr_foreign_toplevel_management` (no focus state there
//! anyway) nor focus in `ext_foreign_toplevel_list`, which is why the KDE
//! protocol and not a generic one is used.
//!
//! ## Provenance and licensing
//!
//! The client bindings come from the maintained `wayland-protocols-plasma`
//! crate, which vendors KDE's `plasma-wayland-protocols` XML tree verbatim
//! (`plasma-window-management.xml`, LGPL-2.1-or-later; the crate itself is
//! MIT). No protocol XML is vendored into this repository and no binding
//! code is generated at build time; the crate version in `Cargo.lock`
//! pins the protocol definition. See `licenses/README.md`.
//!
//! ## Protocol constraints, reported
//!
//! - `org_kde_plasma_window_management` may be bound by **one client at a
//!   time** (protocol description). If another client (a panel, for
//!   example) holds it, KWin refuses the bind and the observer reports
//!   `Failed` with the compositor's answer.
//! - A compositor that does not advertise the global (GNOME/Mutter, for
//!   example) yields the structured `Unsupported` report naming the
//!   missing protocol.
//! - Focus is only observable as this window's own `active` flag: "the
//!   game window is not active" is reported the only way the
//!   [`GameWindowEvent`] vocabulary expresses focus loss (`ActiveWindow`
//!   with no active window), which folds into the same
//!   unfocused-but-present coordinator state the X11 path produces.
//! - Window identification happens once at observer start (the game must
//!   be running for a portal capture stream to exist); a game restarted
//!   afterwards is a new window and needs a new capture selection, exactly
//!   like the X11 path where `GameWindowWatch::follow` restarts on a new
//!   captured window id.
//! - Title changes of the tracked window and windows mapped later are not
//!   re-matched, mirroring the X11 path where the observed window is fixed
//!   until the user re-selects.
//!
//! ## Upstream evidence
//!
//! The parent module documents the upstream focus handling
//! (`ScreenCaptureHandler.cs:175-243`): a non-foreground game window
//! publishes an empty screen (the overlay is hidden), a foreground one
//! updates the overlay offset from `GetWindowRect`. This source feeds
//! exactly those facts into the same [`GameWindowTracker`](super::GameWindowTracker) pipeline.

use std::collections::HashMap;
use std::time::Instant;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window::OrgKdePlasmaWindow;
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::OrgKdePlasmaWindowManagement;

use super::{GameWindowEvent, GameWindowEventSource, GameWindowObservation};

/// Why the observer could not start, as the fixed capability report the
/// live loop publishes verbatim: `Unsupported` when the session cannot
/// provide the capability at all, `Failed` when a working setup did not
/// start.
type StartError = GameWindowObservation;

/// Lowest protocol version the observer can work with: `initial_state`
/// (v4) must exist and `geometry` (v6) is what the overlay follows.
const REQUIRED_VERSION: u32 = 6;
/// The version from which a window object can be released again.
const DESTRUCTOR_VERSION: u32 = 4;
/// `window_with_uuid` announcements start here, so uuid-based window
/// objects are only attempted once the negotiated version has it.
const UUID_VERSION: u32 = 13;
/// Roundtrip budget for the enumeration handshake before identification
/// runs on whatever facts arrived (a compositor that never answers
/// `initial_state` must not hang the live loop's start path).
const IDENTIFY_MAX_ROUNDS: u32 = 32;

/// A window the compositor announced (`window` / `window_with_uuid`) that
/// has no `org_kde_plasma_window` object yet.
#[derive(Debug, Clone)]
struct AnnouncedWindow {
    internal_id: u64,
    uuid: Option<String>,
}

/// User data attached to every `org_kde_plasma_window` object we create.
#[derive(Debug)]
struct ObjectData {
    internal_id: u64,
}

/// The dispatch state of the observer's private Wayland connection: it
/// collects the announced windows and their facts during identification
/// and, afterwards, the change events of the tracked game window.
#[derive(Debug, Default)]
struct PlasmaEvents {
    /// Facts-gathering phase (from connect until the game window is
    /// identified).
    identifying: bool,
    /// Windows announced by the compositor that have no object yet.
    announced: Vec<AnnouncedWindow>,
    /// Facts per known window, keyed by the compositor's internal id.
    facts: HashMap<u64, protocol::WindowFacts>,
    /// Negotiated `org_kde_plasma_window_management` version.
    version: u32,
    /// The identified game window's compositor-internal id.
    tracked_internal_id: u64,
    /// The tracked window's protocol object, retained so the compositor
    /// keeps sending its events for the observer's lifetime (protocol
    /// objects are reference-counted by proxy).
    _game: Option<OrgKdePlasmaWindow>,
    /// Change events of the tracked window since the last drain.
    pending: Vec<GameWindowEvent>,
}

impl PlasmaEvents {
    fn note_announced(&mut self, window: AnnouncedWindow) {
        if let Some(known) = self
            .announced
            .iter_mut()
            .find(|w| w.internal_id == window.internal_id)
        {
            // A v13+ compositor announces every window both plain and with
            // a uuid; keep the richer form.
            if known.uuid.is_none() {
                known.uuid = window.uuid;
            }
            return;
        }
        self.announced.push(window);
    }
}

/// The running Wayland game-window event source: a private connection to
/// the compositor, identified once, then polled like the X11 source.
pub(crate) struct PlasmaWindowSource {
    /// Keeps the wl_display connection alive for the observer's lifetime.
    _conn: Connection,
    queue: EventQueue<PlasmaEvents>,
    state: PlasmaEvents,
}

impl PlasmaWindowSource {
    /// Connect to the session compositor and identify the Diablo window.
    ///
    /// `Err` is the fixed capability report (`Unsupported` or `Failed`).
    pub(crate) fn connect_and_identify() -> Result<Self, StartError> {
        let conn = Connection::connect_to_env().map_err(|error| StartError::Failed {
            reason: format!("Wayland connection failed: {error}"),
        })?;
        Self::identify_on_connection(conn)
    }

    /// The identified game window's compositor-internal id (the value the
    /// observer thread and the Debug screen report).
    pub(crate) const fn internal_id(&self) -> u64 {
        self.state.tracked_internal_id
    }

    /// The identified window's absolute geometry, when KWin reported one.
    pub(crate) fn game_window_geometry(&self) -> Option<d4lc_core::Rect> {
        self.tracked_facts().geometry
    }

    /// The identified window's facts snapshot, for `initial_events`.
    fn tracked_facts(&self) -> protocol::WindowFacts {
        self.state
            .facts
            .get(&self.state.tracked_internal_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Identification on an explicit connection — the seam the in-process
    /// compositor double tests use.
    fn identify_on_connection(conn: Connection) -> Result<Self, StartError> {
        let (globals, mut queue) =
            registry_queue_init(&conn).map_err(|error| StartError::Failed {
                reason: format!("Wayland registry init failed: {error}"),
            })?;
        let qh = queue.handle();

        let interface = OrgKdePlasmaWindowManagement::interface();
        let advertised = globals
            .contents()
            .clone_list()
            .into_iter()
            .find(|global| global.interface == interface.name);
        let Some(advertised) = advertised else {
            return Err(unsupported_missing_protocol());
        };
        if advertised.version < REQUIRED_VERSION {
            return Err(StartError::Unsupported {
                reason: format!(
                    "the compositor implements org_kde_plasma_window_management version {}, \
                     but version {REQUIRED_VERSION} or newer is required to observe the game \
                     window",
                    advertised.version
                ),
            });
        }

        let wm: OrgKdePlasmaWindowManagement = globals
            .bind(&qh, REQUIRED_VERSION..=interface.version, ())
            .map_err(|error| StartError::Failed {
                reason: format!("binding org_kde_plasma_window_management failed: {error}"),
            })?;
        let mut state = PlasmaEvents {
            identifying: true,
            version: wm.version(),
            ..PlasmaEvents::default()
        };

        // The bind is answered with the windows the compositor already
        // mapped; every announced window is turned into an
        // org_kde_plasma_window object whose facts arrive before
        // initial_state. Bounded roundtrips: a compositor that never
        // finishes the handshake must not hang the live loop's start.
        let mut created: Vec<(u64, OrgKdePlasmaWindow)> = Vec::new();
        let mut rounds = 0;
        loop {
            rounds += 1;
            queue
                .roundtrip(&mut state)
                .map_err(|error| StartError::Failed {
                    reason: format!("Wayland roundtrip failed: {error}"),
                })?;
            for announced in std::mem::take(&mut state.announced) {
                if state.facts.contains_key(&announced.internal_id) {
                    continue;
                }
                state.facts.insert(
                    announced.internal_id,
                    protocol::WindowFacts {
                        uuid: announced.uuid.clone(),
                        ..protocol::WindowFacts::new(announced.internal_id)
                    },
                );
                let data = ObjectData {
                    internal_id: announced.internal_id,
                };
                let object = if announced.uuid.is_some() && state.version >= UUID_VERSION {
                    wm.get_window_by_uuid(announced.uuid.unwrap_or_default(), &qh, data)
                } else if let Ok(id) = u32::try_from(announced.internal_id) {
                    wm.get_window(id, &qh, data)
                } else {
                    continue;
                };
                created.push((announced.internal_id, object));
            }
            let settled = state.announced.is_empty()
                && state
                    .facts
                    .values()
                    .all(protocol::WindowFacts::initial_received);
            if settled || rounds >= IDENTIFY_MAX_ROUNDS {
                break;
            }
        }

        let query = protocol::default_diablo_query();
        let Some(game) = protocol::identify(state.facts.values(), &query) else {
            return Err(StartError::Failed {
                reason: format!(
                    "no window matching the game-window query (title contains {:?}, \
                     case-insensitive) on the Wayland session; the game-window observer has \
                     nothing to observe",
                    query.title.as_deref().unwrap_or_default()
                ),
            });
        };
        let tracked_internal_id = game.internal_id;

        // Release the objects of every other window: their facts were only
        // needed for identification.
        for (internal_id, object) in &created {
            if *internal_id != tracked_internal_id && state.version >= DESTRUCTOR_VERSION {
                object.destroy();
            }
        }
        state.identifying = false;
        state.tracked_internal_id = tracked_internal_id;
        state._game = created
            .into_iter()
            .find(|(internal_id, _)| *internal_id == tracked_internal_id)
            .map(|(_, object)| object);

        Ok(Self {
            _conn: conn,
            queue,
            state,
        })
    }
}

fn unsupported_missing_protocol() -> StartError {
    StartError::Unsupported {
        reason: "the compositor does not advertise org_kde_plasma_window_management (KDE KWin); \
                 the game window's focus, minimize and geometry are not observable on Wayland"
            .to_string(),
    }
}

impl GameWindowEventSource for PlasmaWindowSource {
    fn initial_events(&mut self) -> Result<Vec<GameWindowEvent>, String> {
        Ok(protocol::initial_events(&self.tracked_facts()))
    }

    fn next_events(&mut self) -> Result<Vec<GameWindowEvent>, String> {
        let started = Instant::now();
        loop {
            self.queue
                .roundtrip(&mut self.state)
                .map_err(|error| format!("wayland connection: {error}"))?;
            let events = std::mem::take(&mut self.state.pending);
            if !events.is_empty() || started.elapsed() >= super::POLL_INTERVAL {
                return Ok(events);
            }
            std::thread::sleep(super::POLL_SLICE);
        }
    }
}

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for PlasmaEvents {
    fn event(
        state: &mut Self,
        _wm: &OrgKdePlasmaWindowManagement,
        event: <OrgKdePlasmaWindowManagement as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::Event as WmEvent;
        match event {
            WmEvent::Window { id } if state.identifying => {
                state.note_announced(AnnouncedWindow {
                    internal_id: u64::from(id),
                    uuid: None,
                });
            }
            WmEvent::WindowWithUuid { id, uuid } if state.identifying => {
                state.note_announced(AnnouncedWindow {
                    internal_id: u64::from(id),
                    uuid: Some(uuid),
                });
            }
            // show_desktop and the stacking-order events carry nothing the
            // observer needs.
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PlasmaEvents {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Globals appearing (or disappearing) after the identification
        // handshake carry nothing this observer needs: it binds the
        // window-management global once, before identifying.
    }
}

impl Dispatch<OrgKdePlasmaWindow, ObjectData> for PlasmaEvents {
    fn event(
        state: &mut Self,
        _object: &OrgKdePlasmaWindow,
        event: <OrgKdePlasmaWindow as wayland_client::Proxy>::Event,
        data: &ObjectData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window::Event as WindowEvent;
        let internal_id = data.internal_id;
        if state.identifying {
            let facts = state
                .facts
                .entry(internal_id)
                .or_insert_with(|| protocol::WindowFacts::new(internal_id));
            match event {
                WindowEvent::TitleChanged { title } => facts.title = Some(title),
                WindowEvent::AppIdChanged { app_id } => facts.app_id = Some(app_id),
                WindowEvent::PidChanged { pid } => facts.pid = Some(pid),
                WindowEvent::StateChanged { flags } => facts.state = Some(flags),
                WindowEvent::Geometry {
                    x,
                    y,
                    width,
                    height,
                } => facts.geometry = Some(protocol::geometry(x, y, width, height)),
                WindowEvent::Unmapped => facts.unmapped = true,
                WindowEvent::InitialState => facts.initial_state_received = true,
                _ => {}
            }
            return;
        }
        if internal_id != state.tracked_internal_id {
            return;
        }
        match event {
            WindowEvent::StateChanged { flags } => state
                .pending
                .extend(protocol::state_events(internal_id, flags)),
            WindowEvent::Geometry {
                x,
                y,
                width,
                height,
            } => state
                .pending
                .push(GameWindowEvent::Geometry(protocol::geometry(
                    x, y, width, height,
                ))),
            WindowEvent::Unmapped => state.pending.push(GameWindowEvent::Destroyed),
            // Title changes of the tracked window are not re-matched (see
            // the module docs); nothing else the window reports is observed.
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Pure protocol interpretation (mirrors the X11 `protocol` module): every
// fact the generated events carry is folded here, without a connection, so
// the mapping is unit-testable and the oracle is the protocol XML.
// ---------------------------------------------------------------------------

mod protocol {
    use d4lc_capture::{window_matches_query, WindowQuery};
    use d4lc_core::{Rect, WindowInfo};

    use super::super::GameWindowEvent;

    /// `org_kde_plasma_window_management.state`: the window is the active
    /// (focused) one (plasma-window-management.xml, `state` entry 0x1).
    pub const STATE_ACTIVE: u32 = 0x1;
    /// `org_kde_plasma_window_management.state`: the window is minimized
    /// (plasma-window-management.xml, `state` entry 0x2).
    pub const STATE_MINIMIZED: u32 = 0x2;

    /// The facts the observer tracks about one plasma window.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct WindowFacts {
        pub internal_id: u64,
        pub uuid: Option<String>,
        pub title: Option<String>,
        pub app_id: Option<String>,
        pub pid: Option<u32>,
        /// The last `state_changed` bitfield.
        pub state: Option<u32>,
        pub geometry: Option<Rect>,
        pub unmapped: bool,
        pub initial_state_received: bool,
    }

    impl WindowFacts {
        pub fn new(internal_id: u64) -> Self {
            Self {
                internal_id,
                ..Self::default()
            }
        }

        pub fn initial_received(&self) -> bool {
            self.initial_state_received
        }

        pub fn active(&self) -> bool {
            self.state.is_some_and(|state| state & STATE_ACTIVE != 0)
        }

        pub fn minimized(&self) -> bool {
            self.state.is_some_and(|state| state & STATE_MINIMIZED != 0)
        }

        /// The window shape the shared capture matcher consumes: the
        /// plasma `app_id` stands in for the X11 `WM_CLASS`.
        pub fn window_info(&self) -> WindowInfo {
            WindowInfo {
                id: self.internal_id,
                title: self.title.clone().unwrap_or_default(),
                class_name: self.app_id.clone(),
                rect: self.geometry.unwrap_or_default(),
            }
        }
    }

    /// The default Diablo query — exactly the default the X11 capture path
    /// applies (`WindowQuery::with_default_diablo_title` on an empty
    /// query): title containment, case-insensitive.
    pub fn default_diablo_query() -> WindowQuery {
        WindowQuery::default().with_default_diablo_title()
    }

    /// Pick the game window among the enumerated facts, in the X11
    /// selection order: an active matching window first, then the first
    /// matching one. Unmapped windows are not candidates.
    pub fn identify<'a>(
        windows: impl Iterator<Item = &'a WindowFacts>,
        query: &WindowQuery,
    ) -> Option<&'a WindowFacts> {
        let matching: Vec<&WindowFacts> = windows
            .filter(|window| !window.unmapped)
            .filter(|window| window_matches_query(&window.window_info(), window.pid, query))
            .collect();
        matching
            .iter()
            .find(|window| window.active())
            .copied()
            .or_else(|| matching.first().copied())
    }

    /// A plasma geometry event is the window's absolute (root-space)
    /// rectangle — the same space the X11 path answers with.
    pub fn geometry(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// One `state_changed` bitfield folded into observer events.
    ///
    /// The active flag is reported through `ActiveWindow`: `Some` when
    /// this window is active, `None` when it is not — the Wayland protocol
    /// exposes only this window's own flag, and "not active" is exactly
    /// the focus loss the X11 `_NET_ACTIVE_WINDOW` path reports.
    pub fn state_events(internal_id: u64, flags: u32) -> Vec<GameWindowEvent> {
        vec![
            GameWindowEvent::ActiveWindow((flags & STATE_ACTIVE != 0).then_some(internal_id)),
            GameWindowEvent::Hidden(flags & STATE_MINIMIZED != 0),
        ]
    }

    /// The identified window's start state. A window that unmapped before
    /// `initial_state` is already gone (`Destroyed`); otherwise an
    /// announced window is mapped. Geometry is only reported when the
    /// compositor sent one — there is no query whose failure could mean
    /// "destroyed" (the X11 mapping), so absence reports nothing.
    pub fn initial_events(facts: &WindowFacts) -> Vec<GameWindowEvent> {
        if facts.unmapped {
            return vec![GameWindowEvent::Destroyed];
        }
        let mut events = vec![
            GameWindowEvent::ActiveWindow(facts.active().then_some(facts.internal_id)),
            GameWindowEvent::Mapped(true),
            GameWindowEvent::Hidden(facts.minimized()),
        ];
        if let Some(rect) = facts.geometry {
            events.push(GameWindowEvent::Geometry(rect));
        }
        events
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use d4lc_runtime::GameWindowState;

    use super::super::{GameWindowEvent, GameWindowTracker, GameWindowUpdate};
    use super::protocol::{
        default_diablo_query, geometry, identify, initial_events, state_events, WindowFacts,
        STATE_ACTIVE, STATE_MINIMIZED,
    };

    const GAME: u64 = 7;

    fn game_facts(state: u32) -> WindowFacts {
        WindowFacts {
            internal_id: GAME,
            uuid: Some("u-7".to_string()),
            title: Some("Diablo IV".to_string()),
            app_id: Some("steam_app_2344520".to_string()),
            pid: Some(1234),
            state: Some(state),
            geometry: Some(geometry(1920, 0, 2560, 1440)),
            ..WindowFacts::default()
        }
    }

    fn dolphin_facts(state: u32) -> WindowFacts {
        WindowFacts {
            internal_id: 100,
            uuid: Some("u-100".to_string()),
            title: Some("Files — Home".to_string()),
            app_id: Some("org.kde.dolphin".to_string()),
            state: Some(state),
            ..WindowFacts::default()
        }
    }

    #[test]
    fn state_bits_map_to_active_and_hidden_events() {
        // Active + minimized + unrelated flags: the observed bits map, the
        // rest (maximized, keep-above, …) is ignored.
        assert_eq!(
            state_events(GAME, STATE_ACTIVE | STATE_MINIMIZED | 0x4 | 0x8000),
            vec![
                GameWindowEvent::ActiveWindow(Some(GAME)),
                GameWindowEvent::Hidden(true),
            ]
        );
        // No active bit: focus loss, exactly the state the X11 path folds
        // into the coordinator (unfocused-but-present).
        assert_eq!(
            state_events(GAME, STATE_MINIMIZED),
            vec![
                GameWindowEvent::ActiveWindow(None),
                GameWindowEvent::Hidden(true),
            ]
        );
    }

    #[test]
    fn state_sequences_reach_the_coordinator_visible_outcomes() {
        // The full plasma → GameWindowEvent → GameWindowTracker →
        // GameWindowUpdate chain, asserted on the coordinator state.
        let mut tracker = GameWindowTracker::new(GAME);
        let mut updates = Vec::new();
        for event in state_events(GAME, STATE_ACTIVE) {
            updates.extend(tracker.apply(event));
        }
        assert_eq!(
            updates,
            vec![GameWindowUpdate::State(GameWindowState::default())]
        );

        updates.clear();
        for event in state_events(GAME, STATE_MINIMIZED) {
            updates.extend(tracker.apply(event));
        }
        // The tracker folds per event (as for an X11 event batch): the
        // focus loss reports first, the minimize second. The coalescing
        // ControlInbox collapses the pair before the live loop applies it
        // (proven end to end by the compositor-double test).
        assert_eq!(
            updates,
            vec![
                GameWindowUpdate::State(GameWindowState {
                    focused: false,
                    ..GameWindowState::default()
                }),
                GameWindowUpdate::State(GameWindowState {
                    focused: false,
                    minimized: true,
                    closed: false,
                }),
            ]
        );

        // Restored and focused again.
        updates.clear();
        for event in state_events(GAME, STATE_ACTIVE) {
            updates.extend(tracker.apply(event));
        }
        assert_eq!(
            updates,
            vec![
                GameWindowUpdate::State(GameWindowState {
                    minimized: true,
                    ..GameWindowState::default()
                }),
                GameWindowUpdate::State(GameWindowState::default()),
            ]
        );
    }

    #[test]
    fn initial_events_carry_the_identified_window_state() {
        let events = initial_events(&game_facts(STATE_ACTIVE));
        assert_eq!(
            events,
            vec![
                GameWindowEvent::ActiveWindow(Some(GAME)),
                GameWindowEvent::Mapped(true),
                GameWindowEvent::Hidden(false),
                GameWindowEvent::Geometry(geometry(1920, 0, 2560, 1440)),
            ]
        );

        // A window without a reported geometry reports no geometry rather
        // than inventing one.
        let mut no_geometry = game_facts(0);
        no_geometry.geometry = None;
        assert!(!initial_events(&no_geometry)
            .iter()
            .any(|event| matches!(event, GameWindowEvent::Geometry(_))));

        // Unmapped before initial_state: already gone.
        let mut gone = game_facts(0);
        gone.unmapped = true;
        assert_eq!(initial_events(&gone), vec![GameWindowEvent::Destroyed]);
    }

    #[test]
    fn initial_events_feed_the_tracker_end_to_end() {
        let mut tracker = GameWindowTracker::new(GAME);
        let mut updates = Vec::new();
        for event in initial_events(&game_facts(STATE_ACTIVE)) {
            updates.extend(tracker.apply(event));
        }
        // State (focused, mapped, not hidden) once, geometry once.
        assert_eq!(
            updates,
            vec![
                GameWindowUpdate::State(GameWindowState::default()),
                GameWindowUpdate::Geometry(geometry(1920, 0, 2560, 1440)),
            ]
        );
        assert!(tracker.focus_observable());
        assert_eq!(tracker.window(), GAME);
    }

    #[test]
    fn identify_uses_the_default_diablo_title_match() {
        let query = default_diablo_query();
        assert_eq!(
            query.title.as_deref(),
            Some("Diablo IV"),
            "the default query must be the default Diablo title"
        );
        assert!(!query.exact_title);

        // The game is picked among unrelated windows, case-insensitively,
        // by title containment (the same matcher the X11 path uses).
        let windows = [dolphin_facts(STATE_ACTIVE), game_facts(STATE_ACTIVE)];
        let identified = identify(windows.iter(), &query).expect("game identified");
        assert_eq!(identified.internal_id, GAME);

        // A changed title no longer matches — no invented fallback.
        let mut renamed = game_facts(0);
        renamed.title = Some("Character Selection".to_string());
        assert!(identify([dolphin_facts(0), renamed].iter(), &query).is_none());

        // An empty session has no game window.
        assert!(identify([dolphin_facts(0)].iter(), &query).is_none());
    }

    #[test]
    fn identify_prefers_the_active_matching_window() {
        // Two matching windows (the game and a launcher carrying the game
        // title): the active one wins, mirroring the X11 selection order.
        let query = default_diablo_query();
        let mut launcher = game_facts(0);
        launcher.internal_id = 99;
        launcher.title = Some("Battle.net — Diablo IV".to_string());
        let windows = [launcher, game_facts(STATE_ACTIVE)];
        let identified = identify(windows.iter(), &query).expect("identified");
        assert_eq!(identified.internal_id, GAME);

        // Neither active: the first matching window is taken.
        let mut second = game_facts(0);
        second.internal_id = 8;
        let windows = [game_facts(0), second];
        assert_eq!(
            identify(windows.iter(), &query)
                .expect("first matching window is taken")
                .internal_id,
            GAME
        );
    }

    #[test]
    fn unmapped_windows_are_not_identified() {
        let query = default_diablo_query();
        let mut gone = game_facts(STATE_ACTIVE);
        gone.unmapped = true;
        assert!(identify([gone, dolphin_facts(0)].iter(), &query).is_none());
    }

    #[test]
    fn app_id_stands_in_for_the_x11_class_name() {
        // The plasma app_id reaches the shared matcher as the class name,
        // so a class-based query behaves exactly as on X11.
        let facts = game_facts(0);
        assert_eq!(
            facts.window_info().class_name.as_deref(),
            Some("steam_app_2344520")
        );
        assert_eq!(facts.window_info().title, "Diablo IV");
    }
}

// ---------------------------------------------------------------------------
// In-process compositor double: the handshake smoke proves the real client
// bindings against real protocol traffic — bind, window enumeration,
// get_window_by_uuid, initial state, and the change-event mapping — the
// same way the overlay crate's double proves its backend
// (crates/d4lc-overlay-wayland/tests/wayland_protocol.rs pattern). This is
// synthetic evidence only; it never claims a real desktop session. The
// bounded waits keep each test well under a second of wall time.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod double_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use d4lc_runtime::GameWindowState;
    use wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window as server_window;
    use wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window_management as server_wm;
    use wayland_server::{
        Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    };

    use super::super::{GameWindowObservation, GameWindowObserver};
    use super::protocol::{geometry, WindowFacts, STATE_ACTIVE, STATE_MINIMIZED};

    const GAME_ID: u64 = 7;
    const DOLPHIN_ID: u64 = 100;
    const WAIT_TICKS: usize = 5000;

    type ServerWindow = server_window::OrgKdePlasmaWindow;
    type ServerWm = server_wm::OrgKdePlasmaWindowManagement;

    #[derive(Clone)]
    struct ServerGame {
        internal_id: u64,
        uuid: String,
        title: String,
        app_id: String,
        pid: u32,
        flags: u32,
        geometry: (i32, i32, u32, u32),
    }

    /// What the test drives: the double's window registry and the state
    /// changes to apply on the next dispatch tick.
    #[derive(Default)]
    struct DoubleState {
        windows: Vec<ServerGame>,
        bound_windows: Vec<(ServerWindow, String)>,
        commands: Vec<Command>,
    }

    #[derive(Clone, Copy)]
    enum Command {
        SetGameFlags(u32),
        UnmapGame,
    }

    fn send_initial_state(window: &ServerWindow, game: &ServerGame) {
        window.title_changed(game.title.clone());
        window.app_id_changed(game.app_id.clone());
        window.pid_changed(game.pid);
        window.state_changed(game.flags);
        window.geometry(
            game.geometry.0,
            game.geometry.1,
            game.geometry.2,
            game.geometry.3,
        );
        window.initial_state();
    }

    struct TestKWin {
        state: Arc<Mutex<DoubleState>>,
    }

    impl TestKWin {
        fn apply_commands(&self) {
            let commands = std::mem::take(
                &mut self
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .commands,
            );
            if commands.is_empty() {
                return;
            }
            let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for command in commands {
                for (window, uuid) in &guard.bound_windows {
                    if uuid != "u-7" {
                        continue;
                    }
                    match command {
                        Command::SetGameFlags(flags) => window.state_changed(flags),
                        Command::UnmapGame => window.unmapped(),
                    }
                }
            }
        }
    }

    impl GlobalDispatch<server_wm::OrgKdePlasmaWindowManagement, ()> for TestKWin {
        fn bind(
            state: &mut Self,
            _handle: &DisplayHandle,
            _client: &Client,
            resource: New<server_wm::OrgKdePlasmaWindowManagement>,
            _global_data: &(),
            data_init: &mut DataInit<'_, Self>,
        ) {
            let wm = data_init.init(resource, ());
            // KWin answers the bind with every window it already mapped.
            let guard = state.state.lock().unwrap_or_else(|e| e.into_inner());
            for game in &guard.windows {
                wm.window_with_uuid(game.internal_id as u32, game.uuid.clone());
            }
        }
    }

    impl Dispatch<server_wm::OrgKdePlasmaWindowManagement, ()> for TestKWin {
        fn request(
            state: &mut Self,
            _client: &Client,
            _resource: &server_wm::OrgKdePlasmaWindowManagement,
            request: server_wm::Request,
            _data: &(),
            _dhandle: &DisplayHandle,
            data_init: &mut DataInit<'_, Self>,
        ) {
            let server_wm::Request::GetWindowByUuid {
                id,
                internal_window_uuid,
            } = request
            else {
                return;
            };
            let guard = state.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(game) = guard
                .windows
                .iter()
                .find(|game| game.uuid == internal_window_uuid)
            else {
                return;
            };
            let game = game.clone();
            let window = data_init.init(id, internal_window_uuid.clone());
            send_initial_state(&window, &game);
            drop(guard);
            state
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .bound_windows
                .push((window, internal_window_uuid));
        }
    }

    impl Dispatch<server_window::OrgKdePlasmaWindow, String> for TestKWin {
        fn request(
            _state: &mut Self,
            _client: &Client,
            _resource: &server_window::OrgKdePlasmaWindow,
            _request: server_window::Request,
            _data: &String,
            _dhandle: &DisplayHandle,
            _data_init: &mut DataInit<'_, Self>,
        ) {
            // set_state / close / … are never sent by the observer.
        }
    }

    struct GameWmDouble {
        /// Holds the socket and its lock file; removed with the double.
        _socket_dir: tempfile::TempDir,
        socket_path: std::path::PathBuf,
        state: Arc<Mutex<DoubleState>>,
    }

    impl GameWmDouble {
        fn spawn(with_wm_global: bool, global_version: u32) -> Self {
            let display = Display::new().expect("wayland double display");
            let mut dhandle = display.handle();
            let state = Arc::new(Mutex::new(DoubleState {
                windows: vec![
                    ServerGame {
                        internal_id: DOLPHIN_ID,
                        uuid: "u-100".to_string(),
                        title: "Files — Home".to_string(),
                        app_id: "org.kde.dolphin".to_string(),
                        pid: 4242,
                        flags: STATE_ACTIVE,
                        geometry: (0, 0, 800, 600),
                    },
                    ServerGame {
                        internal_id: GAME_ID,
                        uuid: "u-7".to_string(),
                        title: "Diablo IV".to_string(),
                        app_id: "steam_app_2344520".to_string(),
                        pid: 1234,
                        flags: STATE_ACTIVE | 0x100 | 0x200,
                        geometry: (1920, 0, 2560, 1440),
                    },
                ],
                ..DoubleState::default()
            }));
            if with_wm_global {
                dhandle.create_global::<TestKWin, ServerWm, _>(global_version, ());
            }
            let socket_dir = tempfile::Builder::new()
                .prefix("d4lc-gamewm-")
                .tempdir()
                .expect("socket dir");
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let socket_path = socket_dir.path().join(format!(
                "d4lc-gamewm-{}-{}-{}.sock",
                std::process::id(),
                nanos,
                u64::from(with_wm_global),
            ));
            let socket =
                ListeningSocket::bind_absolute(socket_path.clone()).expect("bind double socket");

            let thread_state = Arc::clone(&state);
            std::thread::spawn(move || {
                // Runs for the process lifetime, like the overlay double:
                // the double has no shutdown protocol; its socket directory
                // is removed in Drop so parallel runs cannot collide.
                let mut display = display;
                let mut kwin = TestKWin {
                    state: thread_state,
                };
                loop {
                    if let Ok(Some(stream)) = socket.accept() {
                        let _ = dhandle.insert_client(stream, Arc::new(()));
                    }
                    kwin.apply_commands();
                    let _ = display.dispatch_clients(&mut kwin);
                    let _ = display.flush_clients();
                    std::thread::sleep(Duration::from_micros(100));
                }
            });

            Self {
                _socket_dir: socket_dir,
                socket_path,
                state,
            }
        }

        fn connect(&self) -> wayland_client::Connection {
            let stream = UnixStream::connect(&self.socket_path).expect("connect to double");
            wayland_client::Connection::from_socket(stream).expect("client connection")
        }

        fn set_game_flags(&self, flags: u32) {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .commands
                .push(Command::SetGameFlags(flags));
        }

        fn unmap_game(&self) {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .commands
                .push(Command::UnmapGame);
        }
    }

    /// Waits until the inbox carries a game-window report.
    fn wait_for_state(inbox: &crate::runtime_bridge::live_state::ControlInbox) -> GameWindowState {
        for _ in 0..WAIT_TICKS {
            let pending = inbox.drain();
            if let Some(state) = pending.game_window {
                return state;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the observer delivered no game-window state");
    }

    #[test]
    fn double_handshake_identifies_the_game_and_maps_state_changes() {
        let server = GameWmDouble::spawn(true, 18);
        let conn = server.connect();

        let source =
            super::PlasmaWindowSource::identify_on_connection(conn).expect("identify on double");
        assert_eq!(source.internal_id(), GAME_ID);

        // Full pipeline: the real observer thread with a real ControlInbox,
        // so the coordinator-visible outcomes are asserted, not internals.
        let inbox = crate::runtime_bridge::live_state::ControlInbox::default();
        let observer = GameWindowObserver::spawn(source.internal_id(), source, inbox.clone());
        assert_eq!(
            wait_for_state(&inbox),
            GameWindowState {
                focused: true,
                minimized: false,
                closed: false
            },
            "the identified window is active at start"
        );
        assert_eq!(
            observer.observation(),
            GameWindowObservation::Observing { window: GAME_ID }
        );

        // Another window takes focus and the game minimizes: one
        // state_changed bitfield, both facts.
        server.set_game_flags(STATE_MINIMIZED);
        assert_eq!(
            wait_for_state(&inbox),
            GameWindowState {
                focused: false,
                minimized: true,
                closed: false
            },
            "focus loss and minimize must both reach the coordinator"
        );

        // The game regains focus.
        server.set_game_flags(STATE_ACTIVE);
        assert_eq!(
            wait_for_state(&inbox),
            GameWindowState::default(),
            "restored and focused again"
        );

        // The window is unmapped: closed, and the observer ends.
        server.unmap_game();
        let mut closed = false;
        for _ in 0..WAIT_TICKS {
            if inbox.drain().game_window.is_some_and(|state| state.closed) {
                closed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(closed, "the unmapped window must report closed");
        let mut ended = None;
        for _ in 0..WAIT_TICKS {
            ended = Some(observer.observation());
            if ended == Some(GameWindowObservation::WindowClosed { window: GAME_ID }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            ended,
            Some(GameWindowObservation::WindowClosed { window: GAME_ID })
        );
        drop(observer);
    }

    #[test]
    fn double_without_the_protocol_global_reports_structured_unsupported() {
        let server = GameWmDouble::spawn(false, 18);
        let conn = server.connect();
        let Err(error) = super::PlasmaWindowSource::identify_on_connection(conn) else {
            panic!("missing global must fail the start");
        };
        assert!(
            matches!(error, GameWindowObservation::Unsupported { .. }),
            "a missing global is an unsupported session, not a failure: {error:?}"
        );
        let reason = error.text();
        assert!(
            reason.contains("org_kde_plasma_window_management"),
            "the report must name the missing protocol: {reason}"
        );
    }

    #[test]
    fn double_with_a_too_old_protocol_version_reports_unsupported() {
        let server = GameWmDouble::spawn(true, 5);
        let conn = server.connect();
        let Err(error) = super::PlasmaWindowSource::identify_on_connection(conn) else {
            panic!("a version below 6 must fail the start");
        };
        assert!(matches!(error, GameWindowObservation::Unsupported { .. }));
        assert!(
            error.text().contains("version 5"),
            "the report must name the negotiated version: {error:?}"
        );
    }

    #[test]
    fn identified_facts_carry_title_app_id_and_geometry() {
        let server = GameWmDouble::spawn(true, 18);
        let conn = server.connect();
        let source =
            super::PlasmaWindowSource::identify_on_connection(conn).expect("identify on double");
        let facts: WindowFacts = source.tracked_facts();
        assert_eq!(facts.internal_id, GAME_ID);
        assert_eq!(facts.title.as_deref(), Some("Diablo IV"));
        assert_eq!(facts.app_id.as_deref(), Some("steam_app_2344520"));
        assert_eq!(facts.pid, Some(1234));
        assert!(facts.active(), "the double marks the game active");
        assert_eq!(facts.geometry, Some(geometry(1920, 0, 2560, 1440)));
    }
}
