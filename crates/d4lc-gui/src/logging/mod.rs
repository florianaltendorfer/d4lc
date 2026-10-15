//! Logging ViewModel — mirrors upstream LoggingViewModel.cs.
//!
//! ## Upstream evidence
//!
//! - `LoggingViewModel.cs:1-145` — LogMessages, BadgeCount, ClearLogMessagesCommand
//!
//! ## Bounded retention
//!
//! The message list is a bounded window: [`MAX_RETAINED_MESSAGES`] newest
//! entries are kept and the oldest ones are dropped first, with the drop count
//! exposed as [`LoggingViewModel::dropped_messages`]. Duplicate suppression
//! still applies on top of that — it never replaced a bound, it only hides
//! repeats of the newest message.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntryView {
    pub timestamp: String,
    pub message: String,
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoggingEffect {
    MessagesCleared,
}

/// Bounded retention of the Logging screen's message list. Long-running
/// sessions and error floods keep the newest entries instead of growing the
/// list without limit.
pub const MAX_RETAINED_MESSAGES: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
pub struct LoggingViewModel {
    pub messages: Vec<LogEntryView>,
    pub badge_count: usize,
    pub next_id: u64,
    /// Entries dropped by the retention bound since the last clear.
    pub dropped_messages: u64,
}

impl Default for LoggingViewModel {
    fn default() -> Self {
        Self::new()
    }
}

impl LoggingViewModel {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            badge_count: 0,
            next_id: 0,
            dropped_messages: 0,
        }
    }

    /// Add a non-duplicate entry. Returns `true` when the message was added,
    /// `false` when it was suppressed as a duplicate of the last entry.
    ///
    /// The list keeps at most [`MAX_RETAINED_MESSAGES`] entries; the oldest is
    /// dropped first and counted in `dropped_messages`.
    pub fn push_message(&mut self, message: String, timestamp: String) -> bool {
        if self.messages.last().map(|entry| entry.message.as_str()) == Some(message.as_str()) {
            return false;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.messages.push(LogEntryView {
            timestamp,
            message,
            id,
        });
        while self.messages.len() > MAX_RETAINED_MESSAGES {
            let excess = self.messages.len() - MAX_RETAINED_MESSAGES;
            self.messages.drain(..excess);
            self.dropped_messages = self.dropped_messages.saturating_add(excess as u64);
        }
        self.badge_count += 1;
        true
    }

    /// Clear all log messages and reset badge count.
    pub fn clear_log_messages(&mut self) -> Vec<LoggingEffect> {
        self.messages.clear();
        self.badge_count = 0;
        self.dropped_messages = 0;
        vec![LoggingEffect::MessagesCleared]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_empty() {
        let vm = LoggingViewModel::new();
        assert!(vm.messages.is_empty());
        assert_eq!(vm.badge_count, 0);
        assert_eq!(vm.next_id, 0);
    }

    #[test]
    fn push_message_adds_entry() {
        let mut vm = LoggingViewModel::new();
        let added = vm.push_message("test message".to_string(), "12:00".to_string());
        assert!(added);
        assert_eq!(vm.messages.len(), 1);
        assert_eq!(vm.messages[0].message, "test message");
        assert_eq!(vm.messages[0].timestamp, "12:00");
        assert_eq!(vm.messages[0].id, 0);
        assert_eq!(vm.badge_count, 1);
    }

    #[test]
    fn duplicate_message_is_suppressed() {
        let mut vm = LoggingViewModel::new();
        vm.push_message("dup".to_string(), "12:00".to_string());
        let added = vm.push_message("dup".to_string(), "12:01".to_string());
        assert!(!added);
        assert_eq!(vm.messages.len(), 1);
        assert_eq!(vm.badge_count, 1);
    }

    #[test]
    fn different_message_after_duplicate_is_allowed() {
        let mut vm = LoggingViewModel::new();
        vm.push_message("first".to_string(), "12:00".to_string());
        vm.push_message("first".to_string(), "12:01".to_string()); // suppressed
        let added = vm.push_message("second".to_string(), "12:02".to_string());
        assert!(added);
        assert_eq!(vm.messages.len(), 2);
        assert_eq!(vm.badge_count, 2);
    }

    #[test]
    fn clear_log_messages_clears_all() {
        let mut vm = LoggingViewModel::new();
        vm.push_message("msg".to_string(), "12:00".to_string());
        let effects = vm.clear_log_messages();
        assert!(vm.messages.is_empty());
        assert_eq!(vm.badge_count, 0);
        assert_eq!(effects, vec![LoggingEffect::MessagesCleared]);
    }

    #[test]
    fn clear_on_empty_is_safe() {
        let mut vm = LoggingViewModel::new();
        let effects = vm.clear_log_messages();
        assert!(vm.messages.is_empty());
        assert_eq!(vm.badge_count, 0);
        assert_eq!(effects, vec![LoggingEffect::MessagesCleared]);
    }

    /// Bounded-retention contract: a message flood keeps the
    /// newest entries up to the bound and counts every dropped one.
    #[test]
    fn message_flood_keeps_the_newest_entries_and_counts_the_drops() {
        let mut vm = LoggingViewModel::new();
        let flood = MAX_RETAINED_MESSAGES + 250;

        for index in 0..flood {
            assert!(vm.push_message(format!("message {index}"), "12:00".to_string()));
        }

        assert_eq!(vm.messages.len(), MAX_RETAINED_MESSAGES);
        assert_eq!(
            vm.dropped_messages,
            u64::try_from(flood - MAX_RETAINED_MESSAGES).unwrap_or(u64::MAX)
        );
        // Oldest first out: the list retains the newest window.
        assert_eq!(
            vm.messages.first().map(|entry| entry.message.as_str()),
            Some("message 250")
        );
        assert_eq!(
            vm.messages.last().map(|entry| entry.message.as_str()),
            Some(format!("message {}", flood - 1).as_str())
        );
        // Ids keep counting across the drop (they identify the entry, not its
        // position in the window).
        assert_eq!(
            vm.messages.last().map(|entry| entry.id),
            Some(u64::try_from(flood - 1).unwrap_or(u64::MAX))
        );
    }

    /// Clearing the list resets the retention drop count with it: the counter
    /// describes the messages currently in the window's history.
    #[test]
    fn clear_resets_the_retention_drop_count() {
        let mut vm = LoggingViewModel::new();
        for index in 0..(MAX_RETAINED_MESSAGES + 1) {
            vm.push_message(format!("message {index}"), "12:00".to_string());
        }
        assert_eq!(vm.dropped_messages, 1);

        vm.clear_log_messages();

        assert_eq!(vm.dropped_messages, 0);
        assert!(vm.messages.is_empty());
    }
}
