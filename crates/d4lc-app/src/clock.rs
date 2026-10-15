//! Wall-clock text for log lines and status labels, in the local time zone.

use std::time::SystemTime;

use chrono::{DateTime, Local};

/// The current local time as `HH:MM`, the Logging screen's timestamp.
pub(crate) fn hh_mm_now() -> String {
    Local::now().format("%H:%M").to_string()
}

/// `at` in local time as `HH:MM:SS`.
pub(crate) fn hh_mm_ss(at: SystemTime) -> String {
    DateTime::<Local>::from(at).format("%H:%M:%S").to_string()
}
