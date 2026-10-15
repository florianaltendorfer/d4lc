use std::sync::{Arc, Mutex};

/// Build-import dialog state.
///
/// Owned by `AppState` and projected to the Slint UI through
/// the build-import / multi-build root in-out properties on
/// `main_window.slint`. The state is intentionally small: the
/// import results themselves live in the existing
/// `trade_imports.import_state`; this struct only holds the
/// offline builds directory (user-configurable) and the
/// multi-build list, which `SettingsPanelState` does not carry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildImportState {
    /// User-configured offline builds directory. `None` means
    /// "use the bundled workspace fixture if present" (the
    /// offline fallback in dev / `cargo test` runs;
    /// production installs that lack the fixtures surface a
    /// clear empty state).
    pub builds_dir: Option<std::path::PathBuf>,
    /// Multi-build rotation list. Mirrors
    /// `AppSettings::rule_settings::multi_builds`.
    pub multi_builds: Vec<MultiBuildRow>,
    /// Per-provider "Import paragon boards" toggles of the tabbed
    /// import dialog (upstream
    /// `IsImportParagon{Provider}Enabled`, SettingsD4.cs:30-34).
    /// Hydrated from `AppSettings` at startup; a toggle in the dialog
    /// writes through immediately.
    pub paragon_toggles: ParagonImportToggles,
    /// The stored builds of each provider, cached when the dialog lists
    /// are projected (the Slint rows carry `{id, name, date}`; the cache
    /// keeps the full descriptors incl. variant names and URLs).
    pub provider_builds: std::collections::BTreeMap<String, Vec<d4lc_rules::ImportBuildDescriptor>>,
    /// The stored profiles of each provider, cached when the Mobalytics
    /// Profiles sub-tab is projected (the Slint rows carry the profile
    /// names; the cache keeps the full entities incl. variant URLs).
    pub provider_profiles:
        std::collections::BTreeMap<String, Vec<d4lc_rules::MobalyticsProfileFile>>,
    /// The running download job, if any (the single-download guard).
    /// Events of any other job id are stale and ignored by the drain.
    pub active_download: Option<ActiveDownload>,
    /// Id of the next download job (monotonic per process).
    pub next_download_id: u64,
    /// Jobs that were cancelled or abandoned but whose worker may still be
    /// shutting its browser down; checked at exit so no Chromium outlives
    /// the app.
    pub detached_downloads: Vec<crate::actions::build_download::ImportControl>,
    /// The build variant chosen for the pending add-as-preset flow
    /// (seeded by the variant-row Plus click, consumed by the
    /// Confirm-name overlay's Done).
    pub pending_import: Option<PendingVariantImport>,
    /// Marker for the Merge tab's pending Confirm-name flow (seeded by
    /// the Merge click, consumed by the Confirm-name overlay's Done; the
    /// overlay is shared with the add-as-preset flow). The two selected
    /// preset names stay in the dialog's combo properties while the
    /// overlay is open.
    pub pending_merge: Option<PendingMerge>,
}

/// The running build-import download job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveDownload {
    pub id: u64,
    pub provider: String,
    /// What is being downloaded: the build id or page URL.
    pub subject: String,
    /// The requested Mobalytics URL: the completion drain applies the
    /// upstream nested-tab auto-switch from it (`AddMobalyticsBuildExecute`:
    /// profile+builds → Builds sub-tab, profile-only → Profiles sub-tab).
    pub mobalytics_url: Option<String>,
    pub control: crate::actions::build_download::ImportControl,
    /// The Add / Update click that started the job (monotonic clock).
    pub started: std::time::Instant,
}

/// The variant chosen for the pending add-as-preset flow of the tabbed
/// import dialog (upstream `Add{Provider}BuildAsPresetExecute` captures
/// the clicked variant and the Confirm-name dialog decides the name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingVariantImport {
    pub provider: String,
    pub build_id: String,
    pub build_name: String,
    pub variant_name: String,
}

/// Marker for the Merge tab's pending Confirm-name flow of the tabbed
/// import dialog (upstream `MergeBuildsExecute` opens the same
/// SetPresetName dialog the add-as-preset flow uses). The two selected
/// preset names stay in the dialog's combo properties while the overlay
/// is open; the marker only routes the overlay's Done into the merge flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingMerge;

/// The canonical build-import provider ids (the upstream
/// `BuildsManager*.Provider` strings; the `PROVIDER_*` constants in
/// `actions::build_import_dialogs` carry the same values — the state
/// layer duplicates the literals to keep the actions -> state dependency
/// direction one-way).
pub const PROVIDER_ID_D2CORE: &str = "D2Core";
pub const PROVIDER_ID_D4BUILDS: &str = "D4Builds";
pub const PROVIDER_ID_INFINITY_BUILDS: &str = "InfinityBuilds";
pub const PROVIDER_ID_MAXROLL: &str = "Maxroll";
pub const PROVIDER_ID_MOBALYTICS: &str = "Mobalytics";

/// Per-provider "Import paragon boards" toggles of the tabbed build-import
/// dialog (upstream `IsImportParagonD2CoreEnabled` …
/// `IsImportParagonMobalyticsEnabled`, SettingsD4.cs:30-34, all default
/// true).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParagonImportToggles {
    pub d2core: bool,
    pub d4builds: bool,
    pub infinity_builds: bool,
    pub maxroll: bool,
    pub mobalytics: bool,
}

impl Default for ParagonImportToggles {
    fn default() -> Self {
        Self {
            d2core: true,
            d4builds: true,
            infinity_builds: true,
            maxroll: true,
            mobalytics: true,
        }
    }
}

impl ParagonImportToggles {
    /// Read the toggle of `provider` (a build-import provider id). The
    /// string literals are the canonical provider ids of
    /// `actions::build_import_dialogs::PROVIDER_*` (kept inline here so
    /// the state layer does not depend on the action layer). Unknown
    /// providers read `false`; the UI never offers them.
    pub fn get(&self, provider: &str) -> bool {
        match provider {
            PROVIDER_ID_D2CORE => self.d2core,
            PROVIDER_ID_D4BUILDS => self.d4builds,
            PROVIDER_ID_INFINITY_BUILDS => self.infinity_builds,
            PROVIDER_ID_MAXROLL => self.maxroll,
            PROVIDER_ID_MOBALYTICS => self.mobalytics,
            _ => false,
        }
    }

    /// Write the toggle of `provider`. Unknown providers are ignored.
    pub fn set(&mut self, provider: &str, enabled: bool) {
        match provider {
            PROVIDER_ID_D2CORE => self.d2core = enabled,
            PROVIDER_ID_D4BUILDS => self.d4builds = enabled,
            PROVIDER_ID_INFINITY_BUILDS => self.infinity_builds = enabled,
            PROVIDER_ID_MAXROLL => self.maxroll = enabled,
            PROVIDER_ID_MOBALYTICS => self.mobalytics = enabled,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Build-import progress events.
//
// Upstream reports a download's progress through
// `{Provider}StatusUpdateMessage` / `{Provider}CompletedMessage` messenger
// messages that the nested download dialog receives. The port runs
// downloads on `std::thread` workers that must NOT touch Slint APIs, so the
// workers append events into this shared buffer instead and the
// main-thread 50 ms drain timer (see `wiring::wire_build_import_actions`)
// moves them into the window properties.
// ---------------------------------------------------------------------------

/// One progress event appended by a build-import download worker. Every
/// event carries its job id; the drain ignores events of any job but the
/// active one (a cancelled or replaced job can never touch the dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildImportEvent {
    /// The import step the dialog headline shows ("Starting browser",
    /// "Loading page", "Saving", …).
    Step { job_id: u64, step: String },
    /// A provider status line for the progress detail (upstream
    /// `{Provider}StatusUpdateMessage`). `build_name` / `variant_names` are
    /// the build the flow currently sees (empty until known).
    Status {
        job_id: u64,
        provider: String,
        status: String,
        build_name: String,
        variant_names: Vec<String>,
    },
    /// A line for the Logging screen (upstream `ErrorOccurredMessage` /
    /// `WarningOccurredMessage` sent while a flow runs).
    Log { job_id: u64, line: String },
    /// The flow finished (upstream `{Provider}CompletedMessage`): the stored
    /// build, or the classified failure. `phases` are the measured worker
    /// phases; `finished_at` is when the worker reported completion.
    Completed {
        job_id: u64,
        provider: String,
        result: Result<CompletedImport, crate::actions::build_download::ImportError>,
        phases: Vec<crate::actions::build_download::PhaseTiming>,
        finished_at: std::time::Instant,
    },
}

/// A build (or Mobalytics profile) the worker stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedImport {
    pub name: String,
    /// The stored file's stem: the id the build list shows.
    pub build_id: String,
    pub variant_names: Vec<String>,
}

/// Cloneable handle to the shared build-import progress-event buffer.
///
/// Workers own clones and append; only the main-thread drain removes.
/// Locks follow the poison-recovering convention of the wiring helpers.
#[derive(Debug, Clone, Default)]
pub struct BuildImportEventBuffer {
    inner: Arc<Mutex<Vec<BuildImportEvent>>>,
}

impl BuildImportEventBuffer {
    /// Append an event (worker-thread safe; no Slint, no `AppState`).
    pub fn push(&self, event: BuildImportEvent) {
        match self.inner.lock() {
            Ok(mut guard) => guard.push(event),
            Err(poisoned) => poisoned.into_inner().push(event),
        }
    }

    /// Take every buffered event (main-thread drain).
    pub fn drain(&self) -> Vec<BuildImportEvent> {
        match self.inner.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }
}

/// Structural equality for the `AppState` derive: two buffers are equal
/// when their buffered events are equal (the test suite constructs full
/// `AppState` values and compares them).
impl PartialEq for BuildImportEventBuffer {
    fn eq(&self, other: &Self) -> bool {
        let left = self.inner.lock().map(|g| g.clone());
        let right = other.inner.lock().map(|g| g.clone());
        match (left, right) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
}

/// One row of the multi-build rotation list. A flat projection of
/// `d4lc_config::MultiBuildRuleConfig` so the dialog does not
/// need to know about `d4lc_config` types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiBuildRow {
    pub name: String,
    pub index: usize,
    pub color: String,
}
