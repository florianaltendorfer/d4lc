//! Runtime descriptor-state ownership boundary for the d4lc Slint desktop app.
//!
//! ## State ownership model
//!
//! The Slint desktop app is the **canonical user-facing owner** of loaded state.
//! The CLI may remain a smoke/debug tool but must NOT become the canonical GUI
//! state owner.
//!
//! ```text
//! AppState
//! ├── shell: GuiShellState          (window title, tabs, panel projections)
//! ├── affix_preset_editor           (presets, entries, import state)
//! └── overlay_descriptor_state      (rule + paragon descriptors for live loop)
//! ```
//!
//! ## State lifetime
//!
//! 1. **Startup** — `AppState::default_startup()` initializes from config/fixtures.
//! 2. **User actions** — Slint callbacks update `AppState` (preset selection, import,
//!    settings changes, trade item CRUD).
//! 3. **Live-loop start** — `AppState` is passed through `runtime_bridge::convert_descriptors()`
//!    and fed to the live loop via `d4lc_rules::build_live_overlay_descriptors(...)`.
//! 4. **Live-loop stop** — App state persists via `d4lc-config` before exit.
//!
//! ## Invariant rules
//!
//! - No descriptor generation inside `.slint` files.
//! - Descriptor generation stays in `d4lc-rules`.
//! - Overlay rendering stays in `d4lc-overlay-x11`.
//! - `OverlayFrame.rule_descriptors` is populated from real app-owned state.

mod build_import;
mod dialog;
mod lifecycle;
mod overlay_projection;
mod presets;
mod rule_rows;
mod runtime_status;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod test_support;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;

pub use d4lc_gui::dialog::{ConfirmAction, ConfirmParent};
pub use d4lc_gui::logging::LoggingViewModel;
pub use d4lc_gui::{
    AffixPresetEditorState, CurrentItemPanelState, DebugPanelState, GuiShellState,
    TradeImportsPanelState,
};

// Re-export the hotkey backend types from the `hotkeys` module so
// `AppState` exposes them without forcing every consumer to import
// the module directly (see `crates/d4lc-app/src/hotkeys.rs`).
pub use crate::hotkeys::{HotkeyBackendCapability, HotkeyBackendStatus};
pub use build_import::{
    ActiveDownload, BuildImportEvent, BuildImportEventBuffer, BuildImportState, CompletedImport,
    MultiBuildRow, ParagonImportToggles, PendingMerge, PendingVariantImport,
};
pub use dialog::{DialogFamily, DialogKind, DialogState};
pub use lifecycle::{
    PersistenceOutcome, WindowLifecycleCapabilities, WindowLifecycleCloseAction,
    WindowLifecycleLaunchAction, WindowLifecycleState, WindowLifecycleStateAction,
    WindowLifecycleTrayExitAction, WindowLifecycleTrayOpenAction,
};
pub(crate) use overlay_projection::evaluate_overlay_descriptor_state;
#[cfg(test)]
pub use overlay_projection::RuleOverlayState;
pub use overlay_projection::{OverlayDescriptorState, ParagonOverlayStateBundle};
pub use runtime_status::{
    AcceptedEvaluation, AcceptedSnapshotView, DebugAction, DebugActionRecord, DebugActionStatus,
    OcrPhaseView, OverlayDrawReport, OverlayTextState, RuntimeStatus, StoredDebugRun,
};

use d4lc_core::Rect;
use d4lc_rules::ImportedParagonRoute;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// AppState — canonical state owner for the Slint desktop application.
// ---------------------------------------------------------------------------

/// Central state container owned by the Slint desktop app.
///
/// Wraps `GuiShellState` (which owns all panel projections) and adds the
/// affix-preset editor state, the overlay descriptor state that feeds the
/// live runtime loop, and the shared dialog state used by the
/// `DialogHost`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    /// Headless shell state: window title, tabs, panel projections.
    pub shell: GuiShellState,
    /// Affix-preset editor state: presets, entries, import/export pipeline.
    pub affix_preset_editor: AffixPresetEditorState,
    /// Live-overlay descriptor state consumed by the runtime bridge.
    pub overlay_descriptor_state: Option<OverlayDescriptorState>,
    /// `true` when `overlay_descriptor_state` was last rebuilt from the
    /// app-owned projection path rather than injected manually by a test.
    pub overlay_descriptor_state_from_app: bool,
    /// App-owned trade preference mirrored through the trade config dialog.
    ///
    /// The current `SettingsPanelState` does not expose this field, so the
    /// app shell owns the live copy directly and persists it through the
    /// focused `d4lc-config::AppSettings.show_current_item` adapter.
    pub trade_show_current_item: bool,
    /// Live-loop runtime status projected to the UI.
    pub runtime_status: RuntimeStatus,
    /// Shared dialog-host state.
    pub dialog: DialogState,
    /// Build-import dialog state.
    ///
    /// Holds the offline builds directory and the multi-build
    /// configuration that the build-import and multi-build
    /// config dialogs edit. Sourced from
    /// `d4lc_config::AppSettings` on startup so the dialogs
    /// receive the current app state, not static sample data.
    pub build_import: BuildImportState,
    /// Hotkey binding drafts.
    ///
    /// In-memory projection of the persisted
    /// `d4lc_config::HotkeyConfig.bindings` list. The list is
    /// hydrated at startup by
    /// `hydrate_hotkey_drafts_from_settings` (called from
    /// `main.rs` once `AppSettings` is loaded). The manager dialog's
    /// confirm writes all drafts to disk with
    /// `apply_hotkey_drafts_to_settings`; the single-binding confirm in
    /// `actions::hotkeys_dialogs` upserts only its own target. Both
    /// restart the hotkey backend (`crates/d4lc-app/src/hotkeys.rs`).
    pub hotkey_drafts: Vec<HotkeyDraft>,
    /// Hotkey registration backend status.
    ///
    /// Populated by `main.rs` immediately after
    /// `hotkeys::init_hotkeys()` returns. Stores the X11
    /// capability snapshot (available / unavailable / not yet
    /// initialised) and a per-binding registration report.
    /// Will keep this up to date as the user edits
    /// the HotkeyConfig dialog and re-registers the grabs.
    pub hotkey_backend: HotkeyBackendStatus,
    /// Most-recent hotkey action name dispatched by the
    /// runtime event loop.
    ///
    /// Set by `hotkeys::dispatch_hotkey_event` on every
    /// event, regardless of whether the action has a real
    /// handler or a stub. Pure observability field —
    /// used by tests to assert the dispatch decision and by
    /// the log surface to surface "user pressed Ctrl+F5"
    /// without the test suite needing a live Slint runtime.
    pub last_hotkey_event: Option<String>,
    /// Logging message model.
    ///
    /// Single source of truth for the Logging screen's
    /// message list. `LoggingScreen` reads the projected
    /// `Vec<LogEntryView>` and renders it; `clear_log` mutates
    /// this model in place. New entries are appended through
    /// the same path (`RuntimeEvent::LogMessage` → `apply_runtime_event`)
    /// so debug / hotkey / runtime actions share one model.
    pub logging: LoggingViewModel,
    /// Monotonic counter used to stamp `DebugActionRecord` so
    /// multiple rapid invocations remain distinguishable in tests.
    pub debug_action_seq: u64,
    /// Window lifecycle policy projection.
    ///
    /// Records the launch / close / window-state / tray decisions
    /// the policy made based on `SettingsPanelState` and the
    /// observed platform capabilities.
    pub window_lifecycle: WindowLifecycleState,
    /// Paragon board routes from the most recent provider download.
    ///
    /// A download confirms a build without creating a preset (upstream
    /// keeps paragon on the eventual import), so its boards stay
    /// live-only. `None` after a download whose build carries no
    /// paragon, and cleared by the next import — the same
    /// latest-result semantics the trade-imports tab projects. The
    /// overlay prefers these routes over the preset-keyed store,
    /// matching the previous behavior of projecting from
    /// `import_state.latest_result`.
    pub downloaded_paragon_routes: Option<Vec<ImportedParagonRoute>>,
    /// Progress events appended by the background build-import download
    /// workers. Workers never touch Slint or the
    /// `AppState` lock; the main-thread drain timer moves the events into
    /// the window properties. Cloneable so each worker owns a handle.
    pub build_import_events: BuildImportEventBuffer,
    /// Rule-input revision: bumped through
    /// [`Self::bump_rules_input_revision`] by every mutation of the rule
    /// inputs (settings, trade imports, the affix preset editor, build
    /// imports, paragon routes, the overlay geometry). The UI-side rules
    /// publisher compares only this revision instead of deep-comparing the
    /// inputs every tick; a `debug_assert` safety net in the publisher
    /// catches a mutation that forgot to bump.
    pub rules_input_revision: u64,
    /// Per-affix minimal values from `Config/MinimalAffixValues.json`
    /// (upstream `AffixManager._minimalAffixValues`,
    /// AffixManager.cs:329-351/:610-613). Drives the below-minimal
    /// affix verdict behind the minimal-value filter setting.
    pub minimal_affix_values: BTreeMap<String, f64>,
    /// Sigil dungeon tiers from `Config/DungeonTiers.json` (upstream
    /// `AffixManager._sigilDungeonTiers`, AffixManager.cs:405-424).
    /// A tier here marks a dungeon sigil and is drawn as the tier text
    /// on the sigil marker while `dungeon_tiers_enabled` is on.
    pub dungeon_tiers: BTreeMap<String, String>,
    /// The localized affix-screen catalog for
    /// [`Self::affix_catalog_language`], loaded on demand by
    /// [`crate::actions::affix::project_affix_entries`] and reused for
    /// every later projection (a filter keystroke re-projects from this
    /// cache instead of re-reading the data files).
    ///
    /// `None` until the first projection; the loader's typed error is
    /// reported once through the logging surface and the projections then
    /// render the empty catalog rather than failing.
    pub affix_catalog: Option<d4lc_rules::AffixCatalog>,
    /// The data language [`Self::affix_catalog`] was loaded for; a
    /// different language reloads it.
    pub affix_catalog_language: Option<String>,
    /// The live affix-screen keyword filter text (upstream
    /// `AffixTextFilter`): pushed by
    /// [`crate::actions::affix::handle_affix_filter_text_changed`] on every
    /// edit of the filter field, and read by every catalog projection, so
    /// `;`-separated terms AND-combine over the grids and clearing the
    /// field restores the full catalog.
    pub affix_catalog_filter_text: String,
    /// The catalog render window in rows per tab (upstream's
    /// `VirtualizingWrapPanel` materializes only the visible viewport): the
    /// catalog projection emits at most this many catalog rows per kind, so
    /// a grid instantiates a bounded number of cards. Grown one page at a
    /// time by the toolbar's "Show more" through
    /// [`crate::actions::affix::handle_affix_catalog_window_changed`].
    pub affix_catalog_window: usize,
    /// The real game-window rect the overlay surface covers, resolved
    /// from the live session (the same discovery the overlay start
    /// uses). `None` until a real window is known: the paragon
    /// projection draws nothing without real geometry, mirroring
    /// upstream `DrawGraphicsParagon`'s `if (_window == null) return;`
    /// (OverlayHandler.cs:570) — the previous hardcoded 1920x1080
    /// board never matched a real resolution.
    pub overlay_window_geometry: Option<Rect>,
    /// The resolved capture engine of the live runtime, published once
    /// per runtime preparation by the live loop: the Debug screen's
    /// "Capture engine:" row plus, on the Wayland portal engine, the
    /// stream rect the paragon geometry refresh follows (an X11 window
    /// query is blind on a Wayland session). `None` until a runtime
    /// start resolved its engine.
    pub capture_engine: Option<CaptureEnginePublication>,
    /// The last configuration revision this process actually persisted.
    ///
    /// Only a successful [`d4lc_config::save_affix_presets`] /
    /// transaction commit advances it, so a caller may report success
    /// and a new applied revision only after the bytes reached the
    /// disk; a failed write leaves the previous revision untouched.
    pub applied_config_revision: Option<d4lc_config::CommittedRevision>,
    /// The system preset catalog of this session: fetched at startup, or the
    /// bundled copy after a failed fetch. `None` until the fetch answered;
    /// the settings screen then lists from the bundled files.
    pub system_preset_catalog: Option<crate::actions::settings::SystemPresetCatalog>,
}

/// The capture-engine publication of one runtime start: the Debug row's
/// decision record and, when the resolved engine is the Wayland ScreenCast
/// portal, the stream rect the paragon geometry refresh consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureEnginePublication {
    /// The Debug screen's "Capture engine:" text:
    /// `<setting> → <engine>: <reason>`.
    pub text: String,
    /// The portal stream rect when the resolved engine is the Wayland
    /// portal; `None` for the X11 engine (the X11 geometry comes from the
    /// game-window discovery, as before).
    pub wayland_stream_geometry: Option<Rect>,
}

/// The default catalog render window in rows per tab: the affix-screen
/// projection emits at most this many catalog rows per kind until the
/// toolbar's "Show more" grows it (upstream's `VirtualizingWrapPanel`
/// materializes only the visible viewport).
pub const DEFAULT_CATALOG_WINDOW: usize = 60;

/// One in-memory hotkey binding draft.
///
/// Projection of `d4lc_config::HotkeyBinding` so the Slint
/// dialog layer does not depend on `d4lc-config` types.
/// The list is hydrated at startup from
/// `AppSettings::hotkey_config.bindings`. The manager confirm re-persists
/// every draft via `apply_hotkey_drafts_to_settings` + the existing
/// `d4lc_config::save_app_settings` round-trip.
///
/// Confirming the HotkeyConfig dialog stores a `HotkeyDraft` here, keyed
/// by the action `name` (the upstream `KeyBindingConfig*` setting names in
/// `SettingsD4.cs:93-139`), and persists only that target's binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyDraft {
    /// Stable action identifier (e.g. `"SwitchPreset"`,
    /// `"SwitchOverlay"`, `"TakeScreenshot"`,
    /// `"ToggleController"`, `"ToggleOverlay"`,
    /// `"ToggleDebugLockScreencapture"`,
    /// `"ToggleTuningPrismHints"`). Matches the
    /// `KeyBindingConfig*` setting names.
    pub name: String,
    /// Display label (e.g. `"Switch preset"`).
    pub label: String,
    /// Key combination string (e.g. `"Ctrl+F5"`). Validated for
    /// non-empty + simple modifier+key format on confirm.
    pub binding: String,
    /// Whether the draft is enabled. Even when true, no OS
    /// grab is performed — the flag is metadata only.
    pub enabled: bool,
}

impl AppState {
    /// Construct from a headless fixture, with empty editor and no overlay state.
    pub fn from_shell(shell: GuiShellState) -> Self {
        // Pre-populate the launch decision from the loaded settings so
        // the policy is known as soon as the state is constructed.
        let launch_action = if shell.settings.launch_minimized {
            WindowLifecycleLaunchAction::Minimized
        } else {
            WindowLifecycleLaunchAction::Normal
        };
        let window_lifecycle = WindowLifecycleState {
            last_launch_action: launch_action,
            ..WindowLifecycleState::default()
        };
        Self {
            shell,
            affix_preset_editor: AffixPresetEditorState::new(Vec::new()),
            overlay_descriptor_state: None,
            overlay_descriptor_state_from_app: false,
            trade_show_current_item: false,
            runtime_status: RuntimeStatus::default(),
            dialog: DialogState::default(),
            build_import: BuildImportState::default(),
            hotkey_drafts: Vec::new(),
            hotkey_backend: HotkeyBackendStatus::default(),
            last_hotkey_event: None,
            logging: LoggingViewModel::new(),
            debug_action_seq: 0,
            window_lifecycle,
            downloaded_paragon_routes: None,
            build_import_events: BuildImportEventBuffer::default(),
            rules_input_revision: 0,
            minimal_affix_values: BTreeMap::new(),
            dungeon_tiers: BTreeMap::new(),
            affix_catalog: None,
            affix_catalog_language: None,
            affix_catalog_filter_text: String::new(),
            affix_catalog_window: DEFAULT_CATALOG_WINDOW,
            overlay_window_geometry: None,
            capture_engine: None,
            applied_config_revision: None,
            system_preset_catalog: None,
        }
    }

    /// Default startup state: minimal shell + empty editor + no overlay descriptors.
    pub fn default_startup() -> Self {
        Self::from_shell(GuiShellState::default_startup())
    }

    /// Record that the rule inputs may have changed: the one
    /// helper every mutating action calls, so the rules publisher compares
    /// one `u64` instead of deep-comparing the inputs.
    pub fn bump_rules_input_revision(&mut self) {
        self.rules_input_revision = self.rules_input_revision.saturating_add(1);
    }

    /// Hydrate `build_import` from the loaded `AppSettings`.
    ///
    /// Copies the per-provider "Import paragon boards" toggles, the
    /// multi-build rotation list and the multi-build mode flag, then bumps
    /// the rules input revision. The persisted builds directory
    /// (`AppSettings::builds_dir`) hydrates the dialog's `builds_dir`
    /// override: a configured directory is used verbatim by the dialog
    /// listing and the download worker, while `None` keeps the resolution
    /// in `actions::build_import_dialogs` at the
    /// `{config_root}/Builds/{provider}` home (or the bundled fixture dir
    /// in a dev checkout — the offline default).
    pub fn hydrate_build_import_from_settings(&mut self, settings: &d4lc_config::AppSettings) {
        // The per-provider "Import paragon boards" toggles hydrate with
        // the multi-build state so the import dialog opens with the
        // persisted values (upstream reads them straight off
        // `SettingsD4`).
        self.build_import.paragon_toggles = ParagonImportToggles {
            d2core: settings.is_import_paragon_d2core_enabled,
            d4builds: settings.is_import_paragon_d4builds_enabled,
            infinity_builds: settings.is_import_paragon_infinity_builds_enabled,
            maxroll: settings.is_import_paragon_maxroll_enabled,
            mobalytics: settings.is_import_paragon_mobalytics_enabled,
        };
        self.build_import.builds_dir = settings.builds_dir.as_deref().map(std::path::PathBuf::from);
        self.build_import.multi_builds = settings
            .rule_settings
            .multi_builds
            .iter()
            .map(|entry| MultiBuildRow {
                name: entry.name.clone(),
                index: entry.index,
                color: entry.color.clone(),
            })
            .collect();
        self.bump_rules_input_revision();
    }

    /// Hydrate `hotkey_drafts` from the loaded `AppSettings`
    ///
    /// Project each `d4lc_config::HotkeyBinding` into a
    /// `HotkeyDraft` in declaration order. The drafts list is
    /// the in-memory copy the HotkeyConfig dialog edits; the
    /// persisted copy lives on `AppSettings.hotkey_config`. OS
    /// registration is intentionally NOT performed here — that
    /// is a runtime bridge concern.
    pub fn hydrate_hotkey_drafts_from_settings(&mut self, settings: &d4lc_config::AppSettings) {
        self.hotkey_drafts = settings
            .hotkey_config
            .bindings
            .iter()
            .map(|binding| HotkeyDraft {
                name: binding.name.clone(),
                label: binding.label.clone(),
                binding: binding.binding.clone(),
                enabled: binding.enabled,
            })
            .collect();
    }

    /// Write the in-memory `hotkey_drafts` back onto an
    /// `AppSettings`.
    ///
    /// Pure projection — does not touch the disk. The caller is
    /// responsible for the `save_app_settings` round-trip and
    /// for reporting persistence failures. Any binding
    /// present in `settings` but missing from the drafts list is
    /// preserved (the drafts list reflects only the user's
    /// edited subset; everything else keeps its on-disk value).
    pub fn apply_hotkey_drafts_to_settings(&self, settings: &mut d4lc_config::AppSettings) {
        for draft in &self.hotkey_drafts {
            settings.hotkey_config.upsert(d4lc_config::HotkeyBinding {
                name: draft.name.clone(),
                label: draft.label.clone(),
                binding: draft.binding.clone(),
                enabled: draft.enabled,
            });
        }
    }

    /// Record the hotkey registration backend status
    ///
    /// Called after every (re-)registration; the previous rows are
    /// replaced wholesale, so the UI always shows the newest report.
    ///
    /// Side effect: appends a one-line capability note to
    /// `WindowLifecycleState::capability_notes` when the backend reports
    /// `X11Unavailable`.
    pub fn set_hotkey_backend_status(&mut self, status: HotkeyBackendStatus) {
        if status.capability == HotkeyBackendCapability::X11Unavailable {
            if let Some(reason) = &status.unavailable_reason {
                self.window_lifecycle
                    .capability_notes
                    .push(format!("Hotkey backend reported X11 unavailable: {reason}"));
            }
        }
        self.hotkey_backend = status;
    }

    /// Publish the resolved capture engine (once per runtime preparation,
    /// from the live loop): the Debug screen's "Capture engine:" text and,
    /// on the Wayland portal engine, the stream rect the paragon geometry
    /// refresh follows instead of an X11 window query — which is blind on
    /// a Wayland session.
    pub fn set_capture_engine(&mut self, text: String, wayland_stream_geometry: Option<Rect>) {
        self.capture_engine = Some(CaptureEnginePublication {
            text,
            wayland_stream_geometry,
        });
    }

    /// Append a debug action record to `runtime_status.last_debug_action`,
    /// bumping the monotonic counter so repeated invocations remain
    /// distinguishable.
    ///
    /// Returns the freshly stamped record. Used by both the Debug-screen
    /// action handlers and the `ToggleDebugLockScreencapture` hotkey
    /// so they share one observability surface.
    pub fn record_debug_action(
        &mut self,
        action: DebugAction,
        status: DebugActionStatus,
        reason: impl Into<String>,
    ) -> DebugActionRecord {
        // saturating_add keeps the counter at u64::MAX; the field
        // is observability-only, so wrapping is not load-bearing.
        self.debug_action_seq = self.debug_action_seq.saturating_add(1);
        let record = DebugActionRecord {
            action,
            status,
            reason: reason.into(),
            monotonic: self.debug_action_seq,
        };
        self.runtime_status.last_debug_action = Some(record.clone());
        record
    }

    /// Push a log message through the `LoggingViewModel` and append a
    /// `RuntimeEvent::LogMessage` projection marker. Pure state — does
    /// not touch disk, does not touch Slint. The Slint `log-messages`
    /// property is updated by the runtime projection in
    /// `actions::runtime::apply_event_to_ui`.
    pub fn append_log_message(&mut self, message: impl Into<String>, timestamp: impl Into<String>) {
        self.logging.push_message(message.into(), timestamp.into());
    }
}

// ---------------------------------------------------------------------------
// Convenience accessors for panel states.
// ---------------------------------------------------------------------------

impl AppState {
    pub fn current_item(&self) -> &CurrentItemPanelState {
        &self.shell.current_item
    }

    pub fn trade_imports(&self) -> &TradeImportsPanelState {
        &self.shell.trade_imports
    }

    pub fn debug(&self) -> &DebugPanelState {
        &self.shell.debug
    }

    #[cfg(test)]
    pub fn settings(&self) -> &d4lc_gui::SettingsPanelState {
        &self.shell.settings
    }

    pub fn current_item_mut(&mut self) -> &mut CurrentItemPanelState {
        &mut self.shell.current_item
    }

    /// Mutable trade-import state. The trade items are a rule input, so the
    /// access itself bumps the rules input revision: no caller
    /// can change them without the rules publisher noticing.
    pub fn trade_imports_mut(&mut self) -> &mut TradeImportsPanelState {
        self.bump_rules_input_revision();
        &mut self.shell.trade_imports
    }

    pub fn debug_mut(&mut self) -> &mut DebugPanelState {
        &mut self.shell.debug
    }

    /// Mutable settings; like [`Self::trade_imports_mut`] the access bumps
    /// the rules input revision, since the settings are a rule input.
    #[cfg(test)]
    pub fn settings_mut(&mut self) -> &mut d4lc_gui::SettingsPanelState {
        self.bump_rules_input_revision();
        &mut self.shell.settings
    }
}
