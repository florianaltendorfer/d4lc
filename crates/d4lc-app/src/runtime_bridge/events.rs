//! Bounded runtime event channels from the live loop to the UI thread.

use d4lc_core::Rect;
use d4lc_gui::{CurrentItemAffixRowGeometryView, CurrentItemAffixView, CurrentItemNonAffixRowView};
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

// ---------------------------------------------------------------------------
// Runtime events sent back from the live loop to the UI.
// ---------------------------------------------------------------------------

/// Events emitted by the live loop thread and consumed by the Slint UI thread.
///
/// The analysis results — the accepted item, the tooltip state, the rule
/// evaluation, the overlay status and text state, the frame numbers — do not
/// travel as events: they are one [`live_state::LiveState`](super::live_state::LiveState) published into the
/// bounded latest-state mailbox. This enum carries only the
/// discrete notifications that have no latest-state form: lifecycle
/// transitions, log lines and capability reports.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeEvent {
    /// A new item has been parsed and rule-evaluated.
    ///
    /// Carries the full parsed projection the CurrentItemCard and the
    /// trade screen need (upstream `TooltipDataReadyMessage` →
    /// `TradeViewModel.HandleTooltipDataReadyMessage`,
    /// TradeViewModel.cs:148-163): item type, item power, and the
    /// parsed affix rows in source order. Without a parsed item the
    /// payload fields are `None`/empty and the consumers reset the
    /// card accordingly.
    CurrentItemUpdate {
        item_name: Option<String>,
        parse_status: String,
        /// Canonical item type id (`helm`, `chest`, ...) — the only value the
        /// rule evaluation compares; `None` when the parse did not resolve
        /// one (never substituted).
        item_type: Option<String>,
        /// Display-side base type derivation ("Helm", "Two-Handed Sword");
        /// display only, never an evaluation id.
        base_type: Option<String>,
        item_power: Option<u32>,
        affixes: Vec<CurrentItemAffixView>,
        /// Window-relative per-affix row geometry from the OCR parity path
        /// (empty when the parse carried none). The overlay joins these to
        /// the parsed rows to anchor the affix markers.
        affix_row_geometry: Vec<CurrentItemAffixRowGeometryView>,
        /// Classified non-affix tooltip rows (aspect, unique aspect, rune,
        /// sigil) with their row geometry from the OCR parity path (empty
        /// when the parse carried none). The overlay joins these to anchor
        /// the non-affix rows; evaluation happens app-side.
        non_affix_rows: Vec<CurrentItemNonAffixRowView>,
        /// Detected tooltip rect in window-relative pixels for the frame
        /// the snapshot came from; `None` when no tooltip was detected, in
        /// which case the overlay draws no affix markers.
        tooltip_window_relative: Option<Rect>,
    },
    /// Append a message to the Logging screen.
    ///
    /// Carries the human-readable message text plus a cheap HH:MM
    /// timestamp the Logging screen renders verbatim. The
    /// `apply_runtime_event` path appends to the same
    /// `LoggingViewModel` the Clear Log action clears, so the
    /// user sees a single, consistent log surface across hotkey,
    /// Debug-screen, and live-loop event sources.
    LogMessage { timestamp: String, message: String },
    /// The live loop thread exists and is preparing its runtime (loading
    /// presets, starting the OCR worker, establishing the capture
    /// session). No frame is processed yet; `detail` names what the
    /// preparation currently waits on. Stop is available.
    Preparing { detail: String },
    /// The live loop has started successfully.
    Started,
    /// The live loop has stopped.
    Stopped { reason: String },
    /// An error occurred in the live loop.
    Error { message: String },
    /// Settings hot reload succeeded: the live loop adopted a freshly
    /// prepared runtime and it applies to the next evaluated frame.
    /// Non-fatal: the loop keeps running. `revision` is the applied
    /// configuration revision.
    SettingsReloaded { detail: String, revision: u64 },
    /// Settings hot reload failed during re-preparation. Non-fatal: the
    /// loop keeps running with the previously prepared runtime.
    SettingsReloadFailed { message: String },
    /// The game-window observer's capability report (observing, focus not
    /// observable, window closed, unsupported, failed).
    GameWindowObservation { text: String },
}

// ---------------------------------------------------------------------------
// Discrete notification channels
// ---------------------------------------------------------------------------

/// Capacity of the control channel: lifecycle transitions, errors, settings
/// reload outcomes and game-window capability reports.
///
/// These are the notifications the UI must never miss — a `Stopped` that never
/// arrives leaves a dead loop looking alive — so they have a channel of their
/// own whose send waits for the UI instead of dropping. Their producers are
/// the UI's own start/stop calls, one report per real transition and one
/// reload outcome per applied configuration, so the bound is a backstop, never
/// a queue a control command waits behind.
pub const CONTROL_EVENT_CAPACITY: usize = 64;

/// Capacity of the log-line channel. Log lines are the one flood-capable
/// notification (a chatty backend, a flapping surface); a line produced while
/// the UI is this far behind is dropped and counted instead of growing the
/// channel, and the count is reported to the Logging screen.
pub const LOG_EVENT_CAPACITY: usize = 256;

/// The live loop's discrete notifications, split by what a full buffer may do
/// to them.
///
/// Analysis results do not travel here at all: they are one
/// [`live_state::LiveState`](super::live_state::LiveState) in the bounded latest-state mailbox. What is left
/// is discrete, so a status flood can neither grow memory nor delay a control
/// command's own confirmation.
#[derive(Debug, Clone)]
pub struct RuntimeEventSink {
    control: std::sync::mpsc::SyncSender<RuntimeEvent>,
    log: std::sync::mpsc::SyncSender<RuntimeEvent>,
    dropped_logs: Arc<std::sync::atomic::AtomicU64>,
}

impl RuntimeEventSink {
    /// A fresh sink and the receiver the UI thread drains.
    pub fn new() -> (Self, RuntimeEventReceiver) {
        let (control_tx, control_rx) = std::sync::mpsc::sync_channel(CONTROL_EVENT_CAPACITY);
        let (log_tx, log_rx) = std::sync::mpsc::sync_channel(LOG_EVENT_CAPACITY);
        let dropped_logs = Arc::new(std::sync::atomic::AtomicU64::new(0));
        (
            Self {
                control: control_tx,
                log: log_tx,
                dropped_logs: Arc::clone(&dropped_logs),
            },
            RuntimeEventReceiver {
                control: control_rx,
                log: log_rx,
                dropped_logs,
            },
        )
    }

    /// Send one notification.
    ///
    /// A log line is dropped (and counted, never silently) when the UI is too
    /// far behind; every other notification waits for the UI thread, which
    /// drains on its own timer.
    pub fn send(&self, event: RuntimeEvent) {
        if matches!(event, RuntimeEvent::LogMessage { .. }) {
            if self.log.try_send(event).is_err() {
                self.dropped_logs
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        // Blocking by design: the control producers are UI-driven and report
        // one real transition each, so this waits on a UI that drains every
        // 50 ms — never on a status flood, which uses the other channel.
        let _ = self.control.send(event);
    }
}

/// The UI side of [`RuntimeEventSink`]: the two bounded channels plus the
/// count of log lines the sink had to drop.
#[derive(Debug)]
pub struct RuntimeEventReceiver {
    control: std::sync::mpsc::Receiver<RuntimeEvent>,
    log: std::sync::mpsc::Receiver<RuntimeEvent>,
    dropped_logs: Arc<std::sync::atomic::AtomicU64>,
}

impl RuntimeEventReceiver {
    /// The next pending notification, control events first. Non-blocking.
    pub fn try_recv(&self) -> Option<RuntimeEvent> {
        self.control
            .try_recv()
            .ok()
            .or_else(|| self.log.try_recv().ok())
    }

    /// Every notification pending right now.
    pub fn try_iter(&self) -> std::vec::IntoIter<RuntimeEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.try_recv() {
            events.push(event);
        }
        events.into_iter()
    }

    /// Blocking read with a deadline, polling both channels.
    ///
    /// The drain timer uses [`Self::try_recv`]; this exists for the shutdown
    /// and seam tests that wait for a specific notification.
    #[cfg(test)]
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<RuntimeEvent, std::sync::mpsc::RecvTimeoutError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(event) = self.try_recv() {
                return Ok(event);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(std::sync::mpsc::RecvTimeoutError::Timeout);
            }
            // Short slices keep a log-only wait from sitting out the whole
            // deadline on the control channel.
            let slice = remaining.min(Duration::from_millis(5));
            match self.control.recv_timeout(slice) {
                Ok(event) => return Ok(event),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(disconnected) => {
                    if self.log.try_recv().is_err() {
                        return Err(disconnected);
                    }
                }
            }
        }
    }

    /// Log lines dropped because the UI could not keep up.
    pub fn dropped_logs(&self) -> u64 {
        self.dropped_logs.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The log lines an overrun UI cannot take are dropped and counted
    /// instead of growing the channel without bound, and the control channel
    /// is independent — a Stop's confirmation is never queued behind a log
    /// flood.
    #[test]
    fn a_log_flood_cannot_delay_a_control_confirmation() {
        let (event_tx, event_rx) = RuntimeEventSink::new();

        let overflow = 25usize;
        for index in 0..(LOG_EVENT_CAPACITY + overflow) {
            event_tx.send(RuntimeEvent::LogMessage {
                timestamp: "12:00".to_string(),
                message: format!("line {index}"),
            });
        }
        event_tx.send(RuntimeEvent::Stopped {
            reason: "user stop".to_string(),
        });

        assert_eq!(
            event_rx.dropped_logs(),
            u64::try_from(overflow).unwrap_or(u64::MAX),
            "the flood past the log channel's bound is counted, never unbounded"
        );
        let first = event_rx.try_recv();
        assert!(
            matches!(&first, Some(RuntimeEvent::Stopped { reason }) if reason == "user stop"),
            "the control confirmation comes out first, not behind the log flood: {first:?}"
        );
        assert_eq!(
            event_rx.try_iter().count(),
            LOG_EVENT_CAPACITY,
            "the log channel kept exactly its bound"
        );
        assert_eq!(event_rx.try_recv(), None);

        // The control channel keeps its own order and never drops.
        event_tx.send(RuntimeEvent::Started);
        event_tx.send(RuntimeEvent::Error {
            message: "boom".to_string(),
        });
        assert!(matches!(event_rx.try_recv(), Some(RuntimeEvent::Started)));
        assert!(matches!(
            event_rx.try_recv(),
            Some(RuntimeEvent::Error { message }) if message == "boom"
        ));
    }
}
