//! Per-import job control: cancellation, the commit gate, and phase timings.
//!
//! One [`ImportControl`] belongs to one import job. The UI thread cancels
//! it; the worker (and the browser session it drives) polls it between
//! steps and inside every bounded wait, and stores the downloaded build
//! only through [`ImportControl::commit`]. Cancel and commit take the same
//! lock, so exactly one of them wins: a cancelled job never stores a build,
//! and a cancel that arrives after the commit reports that it was too late.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a cancellation-aware wait sleeps between cancel checks.
const CANCEL_POLL_STEP: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    Running,
    Cancelled,
    Committed,
}

/// One measured phase of an import job (monotonic clock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseTiming {
    pub phase: &'static str,
    pub duration: Duration,
}

#[derive(Debug)]
struct ControlInner {
    state: Mutex<JobState>,
    phases: Mutex<Vec<PhaseTiming>>,
    /// Set by the worker once it released every resource (browser
    /// process, staging files).
    finished: std::sync::atomic::AtomicBool,
}

/// Shared handle to one import job's control block (cheap to clone).
#[derive(Debug, Clone)]
pub struct ImportControl {
    inner: Arc<ControlInner>,
}

impl Default for ImportControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of [`ImportControl::cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The job is cancelled; it will store nothing.
    Cancelled,
    /// The job was already cancelled (repeated click).
    AlreadyCancelled,
    /// The job had already stored its build; cancelling came too late.
    AlreadyCommitted,
}

impl ImportControl {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ControlInner {
                state: Mutex::new(JobState::Running),
                phases: Mutex::new(Vec::new()),
                finished: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, JobState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Request cancellation. Safe to call any number of times.
    pub fn cancel(&self) -> CancelOutcome {
        let mut state = self.state();
        match *state {
            JobState::Running => {
                *state = JobState::Cancelled;
                CancelOutcome::Cancelled
            }
            JobState::Cancelled => CancelOutcome::AlreadyCancelled,
            JobState::Committed => CancelOutcome::AlreadyCommitted,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        *self.state() == JobState::Cancelled
    }

    /// `Err(Cancelled)` once the job is cancelled.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// Sleep `duration`, waking every [`CANCEL_POLL_STEP`] to honor a cancel.
    pub fn sleep(&self, duration: Duration) -> Result<(), Cancelled> {
        let deadline = Instant::now() + duration;
        loop {
            self.check()?;
            let now = Instant::now();
            if now >= deadline {
                return Ok(());
            }
            std::thread::sleep(CANCEL_POLL_STEP.min(deadline - now));
        }
    }

    /// Run the storing step `store` unless the job is cancelled; the job is
    /// committed when `store` succeeds. A concurrent [`cancel`](Self::cancel)
    /// waits for the store to finish and then reports `AlreadyCommitted`.
    pub fn commit<T, E>(
        &self,
        store: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, Cancelled> {
        let mut state = self.state();
        if *state == JobState::Cancelled {
            return Err(Cancelled);
        }
        let result = store();
        if result.is_ok() {
            *state = JobState::Committed;
        }
        Ok(result)
    }

    /// Record one measured phase.
    pub fn record(&self, phase: &'static str, duration: Duration) {
        self.inner
            .phases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(PhaseTiming { phase, duration });
    }

    /// Run `body` and record its duration as `phase`.
    pub fn timed<T>(&self, phase: &'static str, body: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = body();
        self.record(phase, started.elapsed());
        result
    }

    /// Every phase recorded so far, in recording order.
    pub fn phases(&self) -> Vec<PhaseTiming> {
        self.inner
            .phases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl ImportControl {
    /// The worker released every resource of the job.
    pub fn mark_finished(&self) {
        self.inner
            .finished
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_finished(&self) -> bool {
        self.inner
            .finished
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Wait up to `timeout` for the worker to finish; `true` when it did.
    pub fn wait_finished(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}

/// Two handles are equal when they control the same job.
impl PartialEq for ImportControl {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for ImportControl {}

/// Marker error: the job was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn cancel_wins_before_commit_and_blocks_the_store() {
        let control = ImportControl::new();
        assert_eq!(control.cancel(), CancelOutcome::Cancelled);
        assert_eq!(control.cancel(), CancelOutcome::AlreadyCancelled);
        let mut stored = false;
        let result = control.commit(|| {
            stored = true;
            Ok::<(), ()>(())
        });
        assert_eq!(result, Err(Cancelled));
        assert!(!stored, "a cancelled job must never run its store step");
    }

    #[test]
    fn commit_wins_before_cancel_and_cancel_reports_too_late() {
        let control = ImportControl::new();
        assert_eq!(control.commit(|| Ok::<u8, ()>(7)), Ok(Ok(7)));
        assert_eq!(control.cancel(), CancelOutcome::AlreadyCommitted);
        assert!(!control.is_cancelled());
    }

    #[test]
    fn a_failed_store_leaves_the_job_cancellable() {
        let control = ImportControl::new();
        assert_eq!(
            control.commit(|| Err::<(), _>("disk full")),
            Ok(Err("disk full"))
        );
        assert_eq!(control.cancel(), CancelOutcome::Cancelled);
    }

    #[test]
    fn cancel_from_another_thread_ends_a_long_sleep_promptly() {
        let control = ImportControl::new();
        let remote = control.clone();
        let started = Instant::now();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            remote.cancel()
        });
        assert_eq!(control.sleep(Duration::from_secs(30)), Err(Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(canceller.join().unwrap(), CancelOutcome::Cancelled);
    }

    #[test]
    fn concurrent_cancel_waits_for_a_running_store() {
        // The store holds the lock: a cancel issued meanwhile must observe
        // the committed result, never a half-written state.
        let control = ImportControl::new();
        let remote = control.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let canceller = std::thread::spawn(move || {
            entered_rx.recv().unwrap();
            remote.cancel()
        });
        let result = control.commit(|| {
            entered_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            Ok::<(), ()>(())
        });
        assert_eq!(result, Ok(Ok(())));
        assert_eq!(canceller.join().unwrap(), CancelOutcome::AlreadyCommitted);
    }

    #[test]
    fn phases_are_recorded_in_order() {
        let control = ImportControl::new();
        control.record("browser", Duration::from_millis(5));
        let value = control.timed("parse", || 3);
        assert_eq!(value, 3);
        let phases: Vec<_> = control.phases().iter().map(|p| p.phase).collect();
        assert_eq!(phases, ["browser", "parse"]);
    }
}
