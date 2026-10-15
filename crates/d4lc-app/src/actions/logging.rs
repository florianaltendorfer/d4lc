//! Logging action handlers.
//!
//! ## Upstream evidence
//!
//! - `LoggingViewModel.cs:101-104` — ClearLogMessagesCommand
//!
//! `handle_clear_log` clears `AppState.logging` (the in-app
//! `LoggingViewModel`) and projects the empty list to the
//! `LoggingScreen.log-messages` property.
//!
//! Entries reach the same `LoggingViewModel` from `apply_runtime_event` and
//! from the Debug-screen action handlers (see `actions::debug`).

use std::rc::Rc;

use d4lc_gui::logging::{LogEntryView, LoggingViewModel};
use slint::Model as _;

use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

/// The Slint row of one log entry.
fn log_entry_row(entry: &LogEntryView) -> crate::slint_generated::LogEntry {
    crate::slint_generated::LogEntry {
        timestamp: slint::SharedString::from(entry.timestamp.as_str()),
        message: slint::SharedString::from(entry.message.as_str()),
    }
}

/// Handle clear-log callback.
///
/// Mutates the real `LoggingViewModel` on `AppState` and refreshes
/// the Slint `log-messages` projection so the Logging screen is
/// empty after the click.
pub fn handle_clear_log(app: &AppWindow, state: &mut AppState) {
    // Clear the real, app-owned message model. The duplicate is
    // intentional: the `AppState` field already exists; the
    // `LoggingViewModel::clear_log_messages` path is the only one
    // that resets the `badge_count` / `next_id` so subsequent
    // pushes resume from a clean slate.
    let effects = state.logging.clear_log_messages();
    // Sanity check: we expect at least one `MessagesCleared` effect
    // even on an already-empty model (the upstream `LoggingViewModel`
    // guarantees it; assert so the dispatch contract is enforced
    // during refactors).
    debug_assert!(
        effects
            .iter()
            .any(|e| matches!(e, d4lc_gui::logging::LoggingEffect::MessagesCleared)),
        "clear_log_messages must emit MessagesCleared even on empty model"
    );
    // Project the now-empty list into the Slint `log-messages`
    // property so the visible Logging screen updates immediately.
    let model_entries: Vec<LogEntryView> = state.logging.messages.clone();
    project_log_messages(app, &model_entries);
}

/// Project the `LoggingViewModel::messages` into the Slint
/// `log-messages` array. Pure projection — no IO, no state
/// mutation. Called by `handle_clear_log` and by one-off writers
/// (`update_check`) that do not own a [`LogProjection`]; the runtime
/// drain uses [`LogProjection::sync`] so a message flood does not
/// rebuild the model.
pub fn project_log_messages(app: &AppWindow, entries: &[LogEntryView]) {
    let converted: Vec<crate::slint_generated::LogEntry> =
        entries.iter().map(log_entry_row).collect();
    let model = Rc::new(slint::VecModel::from(converted));
    app.set_log_messages(model.into());
}

/// The drain loop's incremental projection of the bounded log store into the
/// Slint `log-messages` model.
///
/// The store is append-only apart from its retention window and Clear Log, and
/// it reports how many entries the window dropped
/// ([`LoggingViewModel::dropped_messages`]). This keeps the *same* model in
/// step with O(changed rows) work: rows the window dropped leave the front,
/// new entries are appended. The model is rebuilt only when it is no longer
/// the one this projection owns — a fresh window, or a one-off writer that
/// replaced it — which is a rare, bounded path, never a per-event cost.
#[derive(Default)]
pub struct LogProjection {
    model: Option<Rc<slint::VecModel<crate::slint_generated::LogEntry>>>,
    /// `dropped_messages` of the store the model was last in step with.
    synced_dropped: u64,
}

impl std::fmt::Debug for LogProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogProjection")
            .field(
                "rows",
                &self.model.as_ref().map_or(0, |model| model.row_count()),
            )
            .field("synced_dropped", &self.synced_dropped)
            .finish_non_exhaustive()
    }
}

impl LogProjection {
    /// Bring the Slint model in step with `store`.
    pub fn sync(&mut self, app: &AppWindow, store: &LoggingViewModel) {
        let Some(model) = self.model_for(app, store) else {
            return;
        };
        let rows = model.row_count();
        // A clear resets the store's drop counter and can leave the model
        // holding rows the store no longer has at all; a model ahead of the
        // store is rebuilt from it rather than guessed at.
        let resync = store.dropped_messages < self.synced_dropped || rows > store.messages.len();
        if resync {
            model.clear();
        } else {
            let dropped = u64::try_from(rows)
                .unwrap_or(u64::MAX)
                .min(store.dropped_messages - self.synced_dropped);
            for _ in 0..dropped {
                model.remove(0);
            }
        }
        for entry in store.messages.iter().skip(model.row_count()) {
            model.push(log_entry_row(entry));
        }
        self.synced_dropped = store.dropped_messages;
    }

    /// The model this projection owns, taking over the app's property with a
    /// one-time rebuild when it is not ours (yet). `None` after a rebuild —
    /// the model is then already in step.
    fn model_for(
        &mut self,
        app: &AppWindow,
        store: &LoggingViewModel,
    ) -> Option<Rc<slint::VecModel<crate::slint_generated::LogEntry>>> {
        let ours = self.model.clone().filter(|ours| {
            app.get_log_messages()
                .as_any()
                .downcast_ref::<slint::VecModel<crate::slint_generated::LogEntry>>()
                .is_some_and(|existing| std::ptr::eq(existing, Rc::as_ref(ours)))
        });
        if let Some(ours) = ours {
            return Some(ours);
        }
        let model = Rc::new(slint::VecModel::from(
            store.messages.iter().map(log_entry_row).collect::<Vec<_>>(),
        ));
        app.set_log_messages(Rc::clone(&model).into());
        self.model = Some(model);
        self.synced_dropped = store.dropped_messages;
        None
    }

    /// Identity of the model this projection owns. Tests use it to prove an
    /// update kept the same model instead of replacing it.
    #[cfg(test)]
    pub fn model_identity(
        &self,
    ) -> Option<*const slint::VecModel<crate::slint_generated::LogEntry>> {
        self.model.as_ref().map(Rc::as_ptr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use d4lc_gui::logging::MAX_RETAINED_MESSAGES;

    fn model_row_count(model: slint::ModelRc<crate::slint_generated::LogEntry>) -> usize {
        use slint::Model as _;
        model.row_count()
    }

    /// One row of the app's current `log-messages` model.
    fn model_row(
        app: &crate::slint_generated::AppWindow,
        row: usize,
    ) -> crate::slint_generated::LogEntry {
        app.get_log_messages()
            .row_data(row)
            .unwrap_or_else(|| panic!("row {row}"))
    }

    /// The runtime drain's projection keeps the same model and appends
    /// the new rows instead of rebuilding the list on every message.
    #[test]
    fn log_projection_updates_the_model_in_place() {
        let app = crate::test_window::new_app_window();
        let mut store = LoggingViewModel::new();
        store.push_message("first".to_string(), "01:00".to_string());
        store.push_message("second".to_string(), "01:01".to_string());
        let mut projection = LogProjection::default();

        projection.sync(&app, &store);
        assert_eq!(model_row_count(app.get_log_messages()), 2);
        let identity = projection.model_identity();
        assert!(identity.is_some(), "the projection owns a model");

        store.push_message("third".to_string(), "01:02".to_string());
        projection.sync(&app, &store);

        assert_eq!(model_row_count(app.get_log_messages()), 3);
        assert_eq!(
            projection.model_identity(),
            identity,
            "the same model instance is kept: rows are appended, not rebuilt"
        );
        assert_eq!(model_row(&app, 2).message.as_str(), "third");

        // A message that is suppressed as a duplicate of the newest one
        // changes neither the store nor the model.
        assert!(!store.push_message("third".to_string(), "01:03".to_string()));
        projection.sync(&app, &store);
        assert_eq!(model_row_count(app.get_log_messages()), 3);
    }

    /// The retention window drops from the front, and the model follows
    /// it in step — it never keeps a row the store no longer has, and a Clear
    /// Log (which resets the store's counter) empties it.
    #[test]
    fn log_projection_follows_the_retention_window_and_clear() {
        let app = crate::test_window::new_app_window();
        let mut store = LoggingViewModel::new();
        for index in 0..(MAX_RETAINED_MESSAGES + 2) {
            store.push_message(format!("message {index}"), "12:00".to_string());
        }
        let mut projection = LogProjection::default();
        projection.sync(&app, &store);

        assert_eq!(store.messages.len(), MAX_RETAINED_MESSAGES);
        assert_eq!(
            model_row_count(app.get_log_messages()),
            MAX_RETAINED_MESSAGES
        );
        assert_eq!(
            model_row(&app, 0).message.as_str(),
            store.messages[0].message.as_str(),
            "the model's oldest row is the store's oldest row, not a dropped one"
        );
        assert_eq!(
            model_row(&app, MAX_RETAINED_MESSAGES - 1).message.as_str(),
            store.messages[MAX_RETAINED_MESSAGES - 1].message.as_str()
        );

        // Clear Log resets the store's drop counter with the list; the model
        // must empty with it.
        store.clear_log_messages();
        projection.sync(&app, &store);
        assert_eq!(model_row_count(app.get_log_messages()), 0);

        // A one-off writer that replaced the property (the update check) is
        // taken over with a single rebuild, and stays in step afterwards.
        store.push_message("after clear".to_string(), "12:03".to_string());
        project_log_messages(&app, &store.messages);
        projection.sync(&app, &store);
        assert_eq!(model_row_count(app.get_log_messages()), 1);
        store.push_message("and another".to_string(), "12:04".to_string());
        let identity = projection.model_identity();
        projection.sync(&app, &store);
        assert_eq!(model_row_count(app.get_log_messages()), 2);
        assert_eq!(projection.model_identity(), identity);
    }

    #[test]
    fn clear_log_empties_model_and_projection() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.append_log_message("hello".to_string(), "12:00".to_string());
        state.append_log_message("world".to_string(), "12:01".to_string());
        assert_eq!(state.logging.messages.len(), 2);

        handle_clear_log(&app, &mut state);

        assert!(state.logging.messages.is_empty());
        assert_eq!(state.logging.badge_count, 0);
        // The Slint-side `log-messages` model is reset to empty.
        let model = app.get_log_messages();
        assert_eq!(model_row_count(model), 0);
    }

    #[test]
    fn clear_log_on_empty_model_is_safe_and_projects_empty() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        handle_clear_log(&app, &mut state);
        assert!(state.logging.messages.is_empty());
        assert_eq!(model_row_count(app.get_log_messages()), 0);
    }

    #[test]
    fn project_log_messages_reflects_appended_entries() {
        let app = crate::test_window::new_app_window();
        let mut state = AppState::default_startup();
        state.append_log_message("a".to_string(), "01:00".to_string());
        state.append_log_message("b".to_string(), "01:01".to_string());
        project_log_messages(&app, &state.logging.messages);
        let model = app.get_log_messages();
        assert_eq!(model_row_count(model), 2);
    }
}
