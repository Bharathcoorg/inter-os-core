// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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

// Touch-to-scanout latency, measured between two points inside the guest.
//
// WHY measured here rather than from the host: a host clock can only see when a
// serial line arrived, and these lines share COM1 with everything else the
// kernel prints, so delivery jitter would be read as guest latency. Differencing
// two `SYSTEM_TICKS` readings inside the guest is immune to that, and the delta
// is what a person waiting for the screen to answer their finger feels.
//
// WHY one stamp and not a queue: the event a person cares about is the most
// recent touch the display has not answered yet, so a newer touch replaces an
// older unanswered one. A touch that no repaint ever answers is reported by the
// next flip that comes along, which is why `max` is a bound and not a promise.
static INPUT_ARRIVAL_MS: AtomicU64 = AtomicU64::new(0);
static INPUT_UNANSWERED: AtomicBool = AtomicBool::new(false);
static TAP_TO_FLIP_LAST_MS: AtomicU64 = AtomicU64::new(0);
static TAP_TO_FLIP_MAX_MS: AtomicU64 = AtomicU64::new(0);

// The same split one stage earlier: the first surface write after a touch is
// where the app stops waiting for its wake and starts drawing. Without it, a
// slow tap cannot be told apart from an app that woke instantly and then painted
// for half a second -- which are different bugs fixed in different places.
//
// WHY the owner is part of the stamp: every running task writes into its own
// surface, and the shell repaints on its own timer. A stamp that accepted the
// first write from any task would credit the touched app with the shell's
// write, and report a paint that never started -- it did, in one run, because
// the dead `sys_surface_write` this stamp first sat in was replaced by the live
// syscall path and every neighbouring task's write began answering the tap.
static PAINT_STAMPED: AtomicBool = AtomicBool::new(false);
static TAP_TO_PAINT_LAST_MS: AtomicU64 = AtomicU64::new(0);
static INPUT_OWNER_TASK: AtomicU64 = AtomicU64::new(u64::MAX);

// The two remaining boundaries of the same wait. `first write` says the app
// woke and began drawing; `flip request` says the app finished and asked the
// kernel to show it. Without the second one, an app that rasterizes for three
// quarters of a second and a kernel that takes three quarters of a second to
// present are the same number, and they are bugs in different files.
static FLIP_REQ_STAMPED: AtomicBool = AtomicBool::new(false);
static TAP_TO_FLIPREQ_LAST_MS: AtomicU64 = AtomicU64::new(0);
static WRITES_SINCE_TOUCH: AtomicU64 = AtomicU64::new(0);

/// What the guest measured while it answered one touch.
///
/// WHY a struct rather than four getters: the flip that answers a touch clears
/// which writes and which request belonged to it. Reading the stages after the
/// clear returns nothing, and a caller that forgets collapses every stage into
/// the total -- which is what an earlier version of this instrument did, and it
/// reported the app's whole wait as paint time.
pub struct TapLatency {
    /// Dispatch to the flip that reached the scanout.
    pub total_ms: u64,
    /// Dispatch to the touched app's first pixel write. `None` if it never drew.
    pub paint_ms: Option<u64>,
    /// Dispatch to the touched app's request to present. `None` if it never asked.
    pub flip_request_ms: Option<u64>,
    /// How many surface writes the touched app charged to this one touch.
    pub writes: u64,
}

/// Marks a dispatched touch as awaiting the flip that will show its effect.
pub fn record_touch_dispatched(now_ms: u64, owner_task: u64) {
    INPUT_ARRIVAL_MS.store(now_ms, Ordering::Relaxed);
    INPUT_OWNER_TASK.store(owner_task, Ordering::Relaxed);
    PAINT_STAMPED.store(false, Ordering::Release);
    FLIP_REQ_STAMPED.store(false, Ordering::Release);
    WRITES_SINCE_TOUCH.store(0, Ordering::Relaxed);
    INPUT_UNANSWERED.store(true, Ordering::Release);
}

/// Stamps the first surface write by the touched task that follows a dispatch.
///
/// A write from any other task is the wrong task's paint and is ignored, so the
/// gap that is reported is the one the person's finger is actually waiting on.
pub fn record_paint_after_touch(now_ms: u64, writing_task: u64) {
    if !INPUT_UNANSWERED.load(Ordering::Acquire) {
        return;
    }
    if writing_task != INPUT_OWNER_TASK.load(Ordering::Relaxed) {
        return;
    }
    WRITES_SINCE_TOUCH.fetch_add(1, Ordering::Relaxed);
    if PAINT_STAMPED.swap(true, Ordering::AcqRel) {
        return;
    }
    let gap = now_ms.saturating_sub(INPUT_ARRIVAL_MS.load(Ordering::Relaxed));
    TAP_TO_PAINT_LAST_MS.store(gap, Ordering::Relaxed);
}

/// Stamps the moment the touched task asks the kernel to show its frame.
pub fn record_flip_request(now_ms: u64, requesting_task: u64) {
    if !INPUT_UNANSWERED.load(Ordering::Acquire) {
        return;
    }
    if requesting_task != INPUT_OWNER_TASK.load(Ordering::Relaxed) {
        return;
    }
    if FLIP_REQ_STAMPED.swap(true, Ordering::AcqRel) {
        return;
    }
    let gap = now_ms.saturating_sub(INPUT_ARRIVAL_MS.load(Ordering::Relaxed));
    TAP_TO_FLIPREQ_LAST_MS.store(gap, Ordering::Relaxed);
}

/// Returns the milliseconds from the touch the current frame answers to the
/// first write of that frame, or `None` if no write followed the touch at all.
pub fn tap_to_paint_ms() -> Option<u64> {
    if PAINT_STAMPED.load(Ordering::Acquire) {
        Some(TAP_TO_PAINT_LAST_MS.load(Ordering::Relaxed))
    } else {
        None
    }
}

/// Called once by a flip that has reached the scanout. Returns the stages of
/// the wait for the last dispatched touch, only for the first flip that
/// follows that touch.
pub fn take_tap_to_flip(now_ms: u64) -> Option<TapLatency> {
    if !INPUT_UNANSWERED.swap(false, Ordering::AcqRel) {
        return None;
    }
    let delta = now_ms.saturating_sub(INPUT_ARRIVAL_MS.load(Ordering::Relaxed));
    // Snapshot the stages before clearing them: after this point they belong to
    // no touch, and a reader that asks afterwards gets `None` for all of them.
    let paint = if PAINT_STAMPED.load(Ordering::Acquire) {
        Some(TAP_TO_PAINT_LAST_MS.load(Ordering::Relaxed))
    } else {
        None
    };
    let request = if FLIP_REQ_STAMPED.load(Ordering::Acquire) {
        Some(TAP_TO_FLIPREQ_LAST_MS.load(Ordering::Relaxed))
    } else {
        None
    };
    let writes = WRITES_SINCE_TOUCH.load(Ordering::Relaxed);
    TAP_TO_FLIP_LAST_MS.store(delta, Ordering::Relaxed);
    if delta > TAP_TO_FLIP_MAX_MS.load(Ordering::Relaxed) {
        TAP_TO_FLIP_MAX_MS.store(delta, Ordering::Relaxed);
    }
    // This frame has been published, so writes belonging to the next one must
    // not be charged to the touch that is now answered.
    PAINT_STAMPED.store(false, Ordering::Release);
    FLIP_REQ_STAMPED.store(false, Ordering::Release);
    WRITES_SINCE_TOUCH.store(0, Ordering::Relaxed);
    Some(TapLatency {
        total_ms: delta,
        paint_ms: paint,
        flip_request_ms: request,
        writes,
    })
}

/// Returns `(last, max)` tap-to-flip milliseconds.
pub fn tap_to_flip_stats() -> (u64, u64) {
    (
        TAP_TO_FLIP_LAST_MS.load(Ordering::Relaxed),
        TAP_TO_FLIP_MAX_MS.load(Ordering::Relaxed),
    )
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

    // A task id that is nobody else's: the shell is 11, the services are 1..10,
    // so the touched task here cannot be mistaken for one of them.
    const TOUCHED: u64 = 900;

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

    #[test]
    fn a_dispatched_touch_is_answered_once_by_the_next_flip() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_touch_dispatched(1_000, TOUCHED);
        let answered = take_tap_to_flip(1_040).expect("the flip that answers the touch reports the gap");
        assert_eq!(answered.total_ms, 40);
        assert!(take_tap_to_flip(1_060).is_none(), "one touch must not be counted by two flips");
    }

    #[test]
    fn the_newest_touch_is_the_one_the_screen_answers() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_touch_dispatched(2_000, TOUCHED);
        record_touch_dispatched(2_010, TOUCHED);
        let answered = take_tap_to_flip(2_050).expect("the waiting touch is the newest one");
        assert_eq!(answered.total_ms, 40);
        assert!(take_tap_to_flip(2_051).is_none());
    }

    #[test]
    fn the_first_write_after_a_touch_is_the_one_that_started_the_paint() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_touch_dispatched(6_000, TOUCHED);
        assert_eq!(tap_to_paint_ms(), None, "no write has followed the touch yet");
        record_paint_after_touch(6_030, TOUCHED);
        assert_eq!(tap_to_paint_ms(), Some(30), "the paint is measured from the dispatch");
        record_paint_after_touch(6_090, TOUCHED);
        assert_eq!(tap_to_paint_ms(), Some(30), "only the first write starts the paint");
        take_tap_to_flip(6_100);
        assert_eq!(tap_to_paint_ms(), None, "a published frame stops charging writes to that touch");
    }

    #[test]
    fn the_stages_come_out_of_the_flip_that_consumes_the_touch() {
        // WHY this test exists: an earlier version returned only the total and
        // left the reader to ask the earlier stages afterwards, but consuming
        // the touch had already cleared them, so every stage printed as the
        // total and a three hundred millisecond paint looked like a three
        // hundred millisecond wake. The stages must travel with the answer.
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_touch_dispatched(8_000, TOUCHED);
        record_paint_after_touch(8_020, TOUCHED);
        record_paint_after_touch(8_040, TOUCHED);
        record_flip_request(8_070, TOUCHED);
        let answered = take_tap_to_flip(8_100).expect("this flip answers the touch");
        assert_eq!(answered.total_ms, 100);
        assert_eq!(answered.paint_ms, Some(20), "the first write, not the last");
        assert_eq!(answered.flip_request_ms, Some(70), "when the app stopped drawing");
        assert_eq!(answered.writes, 2, "every write the touched app pushed for this frame");
        assert!(tap_to_paint_ms().is_none(), "and having answered it, the stages are released");
    }

    #[test]
    fn another_tasks_flip_is_not_the_touched_apps_request() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_touch_dispatched(9_000, TOUCHED);
        record_flip_request(9_005, 11);
        let answered = take_tap_to_flip(9_050).expect("the next flip still answers the touch");
        assert_eq!(
            answered.flip_request_ms,
            None,
            "the shell presenting its own frame is not the touched app finishing its paint"
        );
        assert_eq!(answered.writes, 0);
    }

    #[test]
    fn another_tasks_write_does_not_start_the_touched_apps_paint() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_touch_dispatched(7_000, TOUCHED);
        record_paint_after_touch(7_004, 2);
        record_paint_after_touch(7_008, 10);
        assert_eq!(
            tap_to_paint_ms(),
            None,
            "the shell repainting on its own timer must not be credited to a tap in another app"
        );
        record_paint_after_touch(7_090, TOUCHED);
        assert_eq!(tap_to_paint_ms(), Some(90), "the touched task's own write is the paint");
        take_tap_to_flip(7_100);
    }

    #[test]
    fn a_flip_that_no_touch_prompted_reports_nothing() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        take_tap_to_flip(3_000);
        assert!(take_tap_to_flip(3_001).is_none(), "an idle flip is not evidence that a tap landed");
    }

    #[test]
    fn the_worst_gap_survives_as_a_bound() {
        let _guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        record_touch_dispatched(4_000, TOUCHED);
        take_tap_to_flip(4_030);
        record_touch_dispatched(5_000, TOUCHED);
        take_tap_to_flip(5_500);
        let (last, max) = tap_to_flip_stats();
        assert_eq!(last, 500, "the last measurement is the one the reader just asked for");
        assert!(max >= 500, "the bound must not forget the worst gap it has seen");
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
