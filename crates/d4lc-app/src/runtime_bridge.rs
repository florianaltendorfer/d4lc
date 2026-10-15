//! Live-loop bridge between Slint desktop app state and the runtime/overlay pipeline.
//!
//! ## Purpose
//!
//! The runtime bridge is the narrow API boundary between the app-owned state
//! (presets, settings, paragon imports) and the live runtime loop
//! (`d4lc-runtime`, `d4lc-rules`, the overlay backends).
//!
//! ```text
//! AppState ─► RulesPublisher ─► immutable RulesSnapshot (rules revision)
//!                                   │
//! live loop: process_one_frame ─► RuntimeCoordinator ─► AnalysisSnapshot
//!                                   │ evaluate (d4lc_rules::build_live_overlay_descriptors)
//!                                   ▼
//!                         Arc<Presentation> ─► OverlayPresenter
//!                                   └──────► LiveStateMailbox ─► UI projection
//! ```
//!
//! ## Architecture invariants
//!
//! - Rule evaluation runs on the live-loop thread against an immutable rules
//!   snapshot; the Slint thread never takes part in producing a
//!   presentation. The overlay and the UI receive the same presentation.
//! - Accepted state travels only through the bounded latest-state mailbox;
//!   what is left on the notification channels is discrete and bounded
//!   ([`RuntimeEventSink`]).
//! - Descriptor generation stays in `d4lc-rules`; overlay rendering in the
//!   overlay crates; the analysis state in `d4lc-runtime`'s coordinator.
//! - Live loop runs on a background thread; neither start nor stop blocks
//!   the Slint UI thread.
//!
//! ## Upstream evidence
//!
//! - `upstream/Diablo4Companion/D4Companion.Services/ScreenProcessHandler.cs`
//!   orchestrates live capture + OCR + rule eval in a background worker.

mod debug_commands;
mod events;
mod live_core;
mod live_loop;
pub mod live_state;
mod overlay;
mod paragon;
pub mod settings_reload;
#[cfg(test)]
mod test_support;
mod ui_projection;

pub use debug_commands::{DebugCommandController, DebugCommandOutcome, OverlayLoopOutcome};
#[cfg(test)]
pub use events::LOG_EVENT_CAPACITY;
pub use events::{RuntimeEvent, RuntimeEventReceiver, RuntimeEventSink};
#[cfg(test)]
pub use live_loop::start_live_loop;
#[cfg(any(test, doc))]
pub use live_loop::LiveLoopHandle;
#[cfg(test)]
pub use live_state::{DebugStatus, Presentation};
pub use live_state::{LiveState, LiveStateMailbox, LiveStateProjection, RulesPublisher};
#[cfg(test)]
pub use overlay::convert_descriptors;
pub use overlay::convert_descriptors_with;
#[cfg(test)]
pub use overlay::headless_overlay_status;
use overlay::StartedOverlay;
pub use ui_projection::{apply_runtime_event, runtime_ui_projection};
