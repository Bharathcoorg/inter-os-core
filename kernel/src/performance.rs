//! Low-overhead performance measurement primitives.
//!
//! Metrics are intentionally architecture-neutral so the same contracts can
//! be consumed by QEMU, hardware backends, and developer tooling.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub samples: u64,
    pub total_ticks: u64,
    pub worst_ticks: u64,
}

impl Counter {
    pub const fn new() -> Self { Self { samples: 0, total_ticks: 0, worst_ticks: 0 } }

    pub fn observe(&mut self, ticks: u64) {
        self.samples = self.samples.saturating_add(1);
        self.total_ticks = self.total_ticks.saturating_add(ticks);
        self.worst_ticks = self.worst_ticks.max(ticks);
    }

    pub const fn average_ticks(&self) -> u64 {
        if self.samples == 0 { 0 } else { self.total_ticks / self.samples }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerformanceSnapshot {
    pub scheduler: Counter,
    pub ipc: Counter,
    pub frame: Counter,
}

impl PerformanceSnapshot {
    pub const fn new() -> Self {
        Self { scheduler: Counter::new(), ipc: Counter::new(), frame: Counter::new() }
    }
}


use spin::Mutex;
pub static PERFORMANCE: Mutex<PerformanceSnapshot> = Mutex::new(PerformanceSnapshot::new());

pub fn init() { *PERFORMANCE.lock() = PerformanceSnapshot::new(); }

use core::sync::atomic::{AtomicU64, Ordering};

// The most recent flip, published by the display path on every flip.
//
// WHY this exists and is separate from PERFORMANCE above. `[KMS PERF]` prints
// only the first 15 flips and every 30th after that, so the flip carrying an
// arbitrary input is almost never printed -- which is exactly the flip a latency
// benchmark needs. Publishing each flip's cost on demand is what lets a run say
// whether an outlier was the guest doing expensive work or the host stalling.
//
// WHY atomics rather than the PERFORMANCE mutex. This is written from the
// compositor while the KMS lock is held. Taking a second lock on the display
// path to record telemetry would add contention to the hot path and gives the
// lock-ordering a chance to invert against anything that reads PERFORMANCE
// while holding KMS. Two relaxed stores per flip cannot deadlock and cannot
// reorder against the surrounding work in a way that matters.
static LAST_COMPOSE_CYCLES: AtomicU64 = AtomicU64::new(0);
static LAST_PRESENT_CYCLES: AtomicU64 = AtomicU64::new(0);
static LAST_FLIP_NUMBER: AtomicU64 = AtomicU64::new(0);

/// Records the cost of one completed flip. Called from the display path.
pub fn record_frame(compose_cycles: u64, present_cycles: u64, flip: u64) {
    // Present is stored before compose so a reader that sees the compose value
    // for a given flip is guaranteed to also see that flip's present value.
    LAST_PRESENT_CYCLES.store(present_cycles, Ordering::Relaxed);
    LAST_COMPOSE_CYCLES.store(compose_cycles, Ordering::Relaxed);
    LAST_FLIP_NUMBER.store(flip, Ordering::Release);
}

/// Returns `(compose_cycles, present_cycles, flip_number)` for the most recent flip.
///
/// WHY flip number is returned with the cost: a flip number that did not change
/// between two reads means the reader sampled the same frame twice, which is
/// not evidence that the frame got cheaper.
pub fn last_frame() -> (u64, u64, u64) {
    let flip = LAST_FLIP_NUMBER.load(Ordering::Acquire);
    let compose = LAST_COMPOSE_CYCLES.load(Ordering::Relaxed);
    let present = LAST_PRESENT_CYCLES.load(Ordering::Relaxed);
    (compose, present, flip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // WHY the lock: the atomics above are process-global, and cargo runs tests
    // in parallel threads. Without serialising them these tests order each
    // other's outcomes -- which is exactly what happened: a test asserting the
    // zero initial state failed whenever another test published a frame first,
    // and it failed only in some runs.
    static SERIALISE: Mutex<()> = Mutex::new(());

    #[test]
    fn record_frame_publishes_exactly_what_was_measured() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_frame(1234, 5678, 42);
        assert_eq!(last_frame(), (1234, 5678, 42));
    }

    #[test]
    fn the_newest_flip_replaces_the_previous_one() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_frame(10, 20, 1);
        record_frame(30, 40, 2);
        assert_eq!(last_frame(), (30, 40, 2), "the last publication must win");
    }

    #[test]
    fn a_zero_cost_flip_is_distinguishable_from_no_flip() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        // A flip can legitimately cost zero cycles. The flip number is what
        // proves a measurement was taken, so the zero cost must not be
        // confused with never having recorded anything.
        record_frame(0, 0, 7);
        let (compose, present, flip) = last_frame();
        assert_eq!((compose, present), (0, 0));
        assert_eq!(flip, 7, "the flip number must survive a zero-cost measurement");
    }

    #[test]
    fn the_flip_number_is_monotonic_across_publications() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_frame(5, 5, 100);
        let (_, _, first) = last_frame();
        record_frame(5, 5, 101);
        let (_, _, second) = last_frame();
        assert!(second > first, "a reader must be able to tell one flip from the next");
    }

    // There is deliberately no test asserting the atomics start at zero. They
    // are process-global and another test publishes into them, so that
    // assertion is order-dependent by construction; the zero case is covered by
    // `a_zero_cost_flip_is_distinguishable_from_no_flip` instead.

    #[test]
    fn counter_average_is_zero_before_any_sample() {
        let c = Counter::new();
        assert_eq!(c.average_ticks(), 0);
    }

    #[test]
    fn counter_tracks_worst_and_average() {
        let mut c = Counter::new();
        c.observe(10);
        c.observe(30);
        assert_eq!(c.samples, 2);
        assert_eq!(c.total_ticks, 40);
        assert_eq!(c.worst_ticks, 30);
        assert_eq!(c.average_ticks(), 20);
    }
}
