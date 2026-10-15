//! Scoped-thread helpers shared by the detection and recognition steps.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

/// `f` over every item on up to `max_threads` (at most the host's core
/// count) scoped threads,
/// results in item order. Workers pull the next index, so the items listed
/// first start first. A panic in `f` propagates as in a sequential loop.
pub(crate) fn parallel_map<T: Sync, R: Send>(
    items: &[T],
    max_threads: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(max_threads)
        .min(items.len());
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut indexed: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            return done;
                        };
                        done.push((index, f(item)));
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(join_scoped).collect()
    });
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, result)| result).collect()
}

/// The result of a scoped thread; its panic continues in the caller.
pub(super) fn join_scoped<R>(handle: std::thread::ScopedJoinHandle<'_, R>) -> R {
    match handle.join() {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

pub(crate) fn elapsed_ms(start: Instant) -> u128 {
    start.elapsed().as_micros().div_ceil(1000)
}
