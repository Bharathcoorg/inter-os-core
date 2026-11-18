// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign Futex (Fast Userspace Mutex) Subsystem
//!
//! Provides zero-contention userspace synchronization and deterministic kernel wait queues.
//! Threads sleep on a user virtual address and are awakened via `sys_futex_wake`,
//! migrated via `requeue`, or unblocked when an optional deadline expires.
//!
//! All waiters are qualified by `(process_id, uaddr)`. Cross-process wakeups and
//! requeues are strictly prevented.
//!
//! # Publication contract
//! A waiter is only allowed to block once its publication and a re-read of the
//! futex word happened inside ONE `FUTEX_MANAGER` critical section
//! ([`FutexManager::add_waiter_recheck`]). A waker that stores the new value and
//! calls `wake` while the waiter is being published either finds the published
//! waiter (and marks it woken) or its store is visible to the re-read. Reading
//! the word only *before* publication leaves a window in which the wake finds no
//! waiter and the value is never looked at again, which sleeps the thread forever
//! on a condition that is already true.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::task::TaskId;
use crate::task::process::ProcessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FutexWaiter {
    pub task_id: TaskId,
    pub process_id: ProcessId,
    pub uaddr: u64,
    pub deadline_ticks: u64,
    pub woken: bool,
}

/// What a caller must do after [`FutexManager::add_waiter_recheck`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexWaitOutcome {
    /// The waiter is published and the word still equals the expected value.
    /// The caller must block, then re-check `is_woken` after it has marked itself
    /// Blocked (a wake can still land between publication and that marking).
    Enqueued,
    /// The word no longer equals the expected value, so the wait condition is
    /// already false. The publication was rolled back and no waiter is left
    /// behind; the caller must not block and should report `EAGAIN`.
    ValueChanged,
    /// A wake raced the publication and already marked this waiter. The caller
    /// must not block. The waiter stays published so that removing it reports a
    /// successful wake rather than a timeout.
    AlreadyWoken,
}

/// Most expired waiters one timer tick drains.
///
/// WHY a fixed bound: the drain runs in the timer interrupt, where a heap
/// allocation can spin forever against the `LockedHeap` guard of the task the
/// interrupt landed on (`event::on_timer_tick` is allocation-free for exactly
/// this reason). Waiters past the bound keep their deadlines and are drained by
/// a later tick, so nothing is lost by stopping early.
pub const TIMEOUT_BATCH: usize = 32;

/// Expired waiters collected by one tick, each paired with the deadline it
/// consumed.
///
/// WHY the deadline travels with the id: collecting an expiry consumes it. If
/// the wake cannot be delivered (the scheduler lock is held elsewhere) the
/// deadline has to be handed back with [`FutexManager::restore_timeouts`], or no
/// later tick will ever time that waiter out again.
#[derive(Debug, Clone, Copy)]
pub struct FutexTimeouts {
    entries: [(TaskId, u64); TIMEOUT_BATCH],
    len: usize,
}

impl FutexTimeouts {
    pub const fn new() -> Self {
        Self {
            entries: [(TaskId(0), 0); TIMEOUT_BATCH],
            len: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn as_slice(&self) -> &[(TaskId, u64)] {
        &self.entries[..self.len]
    }

    /// Records one expiry. Returns false when the batch is full, so the caller
    /// leaves that waiter armed for the next tick instead of consuming it.
    fn push(&mut self, task_id: TaskId, deadline_ticks: u64) -> bool {
        if self.len >= self.entries.len() {
            return false;
        }
        self.entries[self.len] = (task_id, deadline_ticks);
        self.len += 1;
        true
    }
}

impl Default for FutexTimeouts {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FutexManager {
    waiters: Vec<FutexWaiter>,
}

impl FutexManager {
    pub const fn new() -> Self {
        Self {
            waiters: Vec::new(),
        }
    }

    /// Publishes a waiter without looking at the futex word.
    ///
    /// A caller that decides to block MUST use [`Self::add_waiter_recheck`]
    /// instead: publishing on the strength of a word read taken before this
    /// critical section loses a wake that raced the publication. This entry
    /// point is for callers that re-check the word themselves under the same
    /// guard, and for tests.
    pub fn add_waiter(&mut self, task_id: TaskId, process_id: ProcessId, uaddr: u64, deadline_ticks: u64) {
        if let Some(w) = self.waiters.iter_mut().find(|w| w.task_id == task_id) {
            w.process_id = process_id;
            w.uaddr = uaddr;
            w.deadline_ticks = deadline_ticks;
            w.woken = false;
        } else {
            self.waiters.push(FutexWaiter {
                task_id,
                process_id,
                uaddr,
                deadline_ticks,
                woken: false,
            });
        }
    }

    pub fn is_woken(&self, task_id: TaskId) -> bool {
        self.waiters.iter().find(|w| w.task_id == task_id).map(|w| w.woken).unwrap_or(false)
    }

    /// Publishes a waiter and re-reads the futex word in the SAME critical
    /// section, so a wake that raced the publication cannot be lost.
    ///
    /// `read_word` is called after the waiter is visible to `wake`. It MUST be a
    /// plain read of the already-validated user word (a `read_volatile` of the
    /// address the caller checked): it runs with `FUTEX_MANAGER` held and
    /// preemption deferred, so taking any other kernel lock here would invert
    /// the subsystem lock order.
    ///
    /// The caller blocks only on [`FutexWaitOutcome::Enqueued`]. On
    /// `ValueChanged` nothing is left published; on `AlreadyWoken` the waiter
    /// stays published and removing it reports the wake.
    pub fn add_waiter_recheck<F>(
        &mut self,
        task_id: TaskId,
        process_id: ProcessId,
        uaddr: u64,
        deadline_ticks: u64,
        expected_val: u32,
        read_word: F,
    ) -> FutexWaitOutcome
    where
        F: FnOnce() -> u32,
    {
        self.add_waiter(task_id, process_id, uaddr, deadline_ticks);
        self.recheck_published(task_id, expected_val, read_word)
    }

    /// Second half of [`Self::add_waiter_recheck`], split out so the two
    /// interleavings of a racing waker (store-before-publish and
    /// wake-after-publish) are each reachable from a test.
    ///
    /// MUST run under the same `FUTEX_MANAGER` guard that published the waiter.
    fn recheck_published<F>(
        &mut self,
        task_id: TaskId,
        expected_val: u32,
        read_word: F,
    ) -> FutexWaitOutcome
    where
        F: FnOnce() -> u32,
    {
        if read_word() != expected_val {
            // The condition the caller waited on is already false. Roll the
            // publication back so no stale waiter survives: a later wake would
            // otherwise mark a task that is running, and a timeout would fire
            // for a wait that never happened.
            self.remove_waiter(task_id);
            return FutexWaitOutcome::ValueChanged;
        }
        if self.is_woken(task_id) {
            return FutexWaitOutcome::AlreadyWoken;
        }
        FutexWaitOutcome::Enqueued
    }

    pub fn remove_waiter(&mut self, task_id: TaskId) -> bool {
        if let Some(pos) = self.waiters.iter().position(|w| w.task_id == task_id) {
            let w = self.waiters.remove(pos);
            w.woken
        } else {
            false
        }
    }

    pub fn wake(&mut self, process_id: ProcessId, uaddr: u64, max_count: usize) -> Vec<TaskId> {
        let mut woken = Vec::new();
        let mut count = 0;
        for w in self.waiters.iter_mut() {
            if w.process_id == process_id && w.uaddr == uaddr && !w.woken {
                w.woken = true;
                woken.push(w.task_id);
                count += 1;
                if count >= max_count {
                    break;
                }
            }
        }
        woken
    }

    /// Requeues waiters from `uaddr1` to `uaddr2` within the same process.
    ///
    /// Wakes up to `max_wake` waiters on `(process_id, uaddr1)`.
    /// Requeues up to `max_requeue` remaining waiters on `(process_id, uaddr1)` to `(process_id, uaddr2)`.
    ///
    /// # Security & Isolation
    /// Requeueing NEVER moves a thread across processes: both source and destination
    /// keys are strictly qualified by `process_id`.
    ///
    /// Returns `(woken_tasks, requeued_count)`.
    pub fn requeue(
        &mut self,
        process_id: ProcessId,
        uaddr1: u64,
        max_wake: usize,
        uaddr2: u64,
        max_requeue: usize,
    ) -> (Vec<TaskId>, usize) {
        let mut woken = Vec::new();
        let mut requeued = 0;

        for w in self.waiters.iter_mut() {
            if w.process_id == process_id && w.uaddr == uaddr1 && !w.woken {
                if woken.len() < max_wake {
                    w.woken = true;
                    woken.push(w.task_id);
                } else if requeued < max_requeue {
                    if uaddr1 != uaddr2 {
                        // Requeue waiter to uaddr2 within the SAME process.
                        // Process ID is invariant and strictly preserved.
                        w.uaddr = uaddr2;
                    }
                    requeued += 1;
                } else {
                    break;
                }
            }
        }
        (woken, requeued)
    }

    /// Collects expired waiters without allocating, consuming their deadlines.
    ///
    /// This is the timer-interrupt entry point: it never touches the heap, and
    /// it stops at [`TIMEOUT_BATCH`] so one tick cannot be flooded by a process
    /// with thousands of expired waiters. Waiters past the bound stay armed.
    ///
    /// A consumed deadline MUST either be delivered to the scheduler or handed
    /// back with [`Self::restore_timeouts`]; dropping it strands the waiter
    /// Blocked until an explicit wake or process exit.
    pub fn take_timeouts(&mut self, current_ticks: u64) -> FutexTimeouts {
        let mut expired = FutexTimeouts::new();
        for w in self.waiters.iter_mut() {
            if !w.woken && w.deadline_ticks > 0 && current_ticks >= w.deadline_ticks {
                if !expired.push(w.task_id, w.deadline_ticks) {
                    break;
                }
                w.deadline_ticks = 0;
            }
        }
        expired
    }

    /// Re-arms deadlines consumed by [`Self::take_timeouts`] whose wake could not
    /// be delivered, so a later tick retries the timeout.
    ///
    /// Only a waiter that is still parked is re-armed: one that was woken, one
    /// that re-armed itself with a new deadline, and one that is already gone
    /// (task exited) all own their state and must not be overwritten.
    pub fn restore_timeouts(&mut self, consumed: &FutexTimeouts) {
        for &(task_id, deadline_ticks) in consumed.as_slice() {
            self.restore_timeout(task_id, deadline_ticks);
        }
    }

    /// Re-arms one consumed deadline. See [`Self::restore_timeouts`].
    pub fn restore_timeout(&mut self, task_id: TaskId, deadline_ticks: u64) {
        if let Some(w) = self.waiters.iter_mut().find(|w| w.task_id == task_id) {
            if !w.woken && w.deadline_ticks == 0 {
                w.deadline_ticks = deadline_ticks;
            }
        }
    }

    /// Task-context convenience wrapper over [`Self::take_timeouts`] that keeps
    /// the historical id-list shape. Allocates, so the timer path must use
    /// `take_timeouts` directly. Bounded by [`TIMEOUT_BATCH`] like that method.
    pub fn check_timeouts(&mut self, current_ticks: u64) -> Vec<TaskId> {
        self.take_timeouts(current_ticks)
            .as_slice()
            .iter()
            .map(|&(task_id, _)| task_id)
            .collect()
    }

    pub fn cleanup_task(&mut self, task_id: TaskId) {
        self.waiters.retain(|w| w.task_id != task_id);
    }

    /// Purges all waiters belonging to `process_id` when a process terminates,
    /// returning the task IDs of all unblocked/orphaned waiters.
    pub fn cleanup_process(&mut self, process_id: ProcessId) -> Vec<TaskId> {
        let mut cleaned = Vec::new();
        self.waiters.retain(|w| {
            if w.process_id == process_id {
                cleaned.push(w.task_id);
                false
            } else {
                true
            }
        });
        cleaned
    }
}

pub static FUTEX_MANAGER: Mutex<FutexManager> = Mutex::new(FutexManager::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_futex_manager_wait_and_wake() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let p2 = ProcessId(2);
        let t1 = TaskId(101);
        let t2 = TaskId(102);
        let t3 = TaskId(103);
        let t4_other_proc = TaskId(104);
        let uaddr = 0x2000_1000;

        mgr.add_waiter(t1, p1, uaddr, 0);
        mgr.add_waiter(t2, p1, uaddr, 0);
        mgr.add_waiter(t3, p1, 0x2000_2000, 0);
        mgr.add_waiter(t4_other_proc, p2, uaddr, 0); // Same virtual address, different process

        assert!(!mgr.is_woken(t1));
        assert!(!mgr.is_woken(t2));
        assert!(!mgr.is_woken(t4_other_proc));

        // Wake 1 waiter in process p1 on uaddr
        let woken = mgr.wake(p1, uaddr, 1);
        assert_eq!(woken.len(), 1);
        assert_eq!(woken[0], t1);
        assert!(mgr.is_woken(t1));
        assert!(!mgr.is_woken(t2));
        assert!(!mgr.is_woken(t4_other_proc)); // Other process MUST NOT be woken

        // Wake another waiter in process p1 on uaddr
        let woken2 = mgr.wake(p1, uaddr, 1);
        assert_eq!(woken2.len(), 1);
        assert_eq!(woken2[0], t2);
        assert!(mgr.is_woken(t2));
        assert!(!mgr.is_woken(t4_other_proc)); // Still untouched

        // Waiter on different uaddr is untouched
        assert!(!mgr.is_woken(t3));

        // Wake waiter in process p2 on uaddr
        let woken_p2 = mgr.wake(p2, uaddr, 1);
        assert_eq!(woken_p2.len(), 1);
        assert_eq!(woken_p2[0], t4_other_proc);
        assert!(mgr.is_woken(t4_other_proc));

        // Removing waiter returns woken status
        assert!(mgr.remove_waiter(t1));
        assert!(mgr.remove_waiter(t2));
        assert!(!mgr.remove_waiter(t3));
        assert!(mgr.remove_waiter(t4_other_proc));
    }

    #[test]
    fn test_futex_manager_requeue_isolation() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let p2 = ProcessId(2);

        let t1 = TaskId(301);
        let t2 = TaskId(302);
        let t3 = TaskId(303);
        let t4_p2 = TaskId(304);

        let uaddr_cv = 0x4000_1000;
        let uaddr_mutex = 0x4000_2000;

        // Process 1 has 3 threads waiting on condition variable uaddr_cv
        mgr.add_waiter(t1, p1, uaddr_cv, 0);
        mgr.add_waiter(t2, p1, uaddr_cv, 0);
        mgr.add_waiter(t3, p1, uaddr_cv, 0);

        // Process 2 has 1 thread waiting on same virtual address uaddr_cv
        mgr.add_waiter(t4_p2, p2, uaddr_cv, 0);

        // Requeue in Process 1: wake 1, requeue up to 2 to uaddr_mutex
        let (woken, requeued) = mgr.requeue(p1, uaddr_cv, 1, uaddr_mutex, 2);
        assert_eq!(woken.len(), 1);
        assert_eq!(woken[0], t1);
        assert_eq!(requeued, 2);

        // t1 is woken
        assert!(mgr.is_woken(t1));
        // t2 and t3 are not yet woken, but requeued to uaddr_mutex
        assert!(!mgr.is_woken(t2));
        assert!(!mgr.is_woken(t3));

        // Crucial security invariant: Process 2 waiter t4_p2 was NEVER woken or requeued
        assert!(!mgr.is_woken(t4_p2));

        // Wake on original address uaddr_cv for p1 wakes nothing (all were woken or moved)
        let empty_wake = mgr.wake(p1, uaddr_cv, 10);
        assert!(empty_wake.is_empty());

        // Wake on mutex address uaddr_mutex for p1 wakes the requeued threads t2 and t3
        let woken_mutex = mgr.wake(p1, uaddr_mutex, 2);
        assert_eq!(woken_mutex.len(), 2);
        assert_eq!(woken_mutex[0], t2);
        assert_eq!(woken_mutex[1], t3);
        assert!(mgr.is_woken(t2));
        assert!(mgr.is_woken(t3));

        // Wake on uaddr_cv for p2 wakes t4_p2 (it stayed on uaddr_cv in p2)
        let woken_p2 = mgr.wake(p2, uaddr_cv, 1);
        assert_eq!(woken_p2.len(), 1);
        assert_eq!(woken_p2[0], t4_p2);
        assert!(mgr.is_woken(t4_p2));
    }

    #[test]
    fn test_futex_manager_timeouts_and_cleanup() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let t1 = TaskId(201);
        let t2 = TaskId(202);
        let uaddr = 0x3000_1000;

        mgr.add_waiter(t1, p1, uaddr, 100);
        mgr.add_waiter(t2, p1, uaddr, 200);

        let expired_at_50 = mgr.check_timeouts(50);
        assert!(expired_at_50.is_empty());

        let expired_at_150 = mgr.check_timeouts(150);
        assert_eq!(expired_at_150.len(), 1);
        assert_eq!(expired_at_150[0], t1);

        // Cleanup task removes it from manager
        mgr.cleanup_task(t2);
        let expired_at_250 = mgr.check_timeouts(250);
        assert!(expired_at_250.is_empty());
    }

    #[test]
    fn test_futex_manager_cleanup_process() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let p2 = ProcessId(2);

        let t1 = TaskId(401);
        let t2 = TaskId(402);
        let t3_p2 = TaskId(403);

        mgr.add_waiter(t1, p1, 0x1000, 0);
        mgr.add_waiter(t2, p1, 0x2000, 0);
        mgr.add_waiter(t3_p2, p2, 0x1000, 0);

        let cleaned = mgr.cleanup_process(p1);
        assert_eq!(cleaned.len(), 2);
        assert!(cleaned.contains(&t1));
        assert!(cleaned.contains(&t2));

        // p1 waiters are gone
        assert!(!mgr.is_woken(t1));
        assert!(!mgr.is_woken(t2));
        assert!(mgr.wake(p1, 0x1000, 10).is_empty());
        assert!(mgr.wake(p1, 0x2000, 10).is_empty());

        // p2 waiter is completely untouched
        let woken_p2 = mgr.wake(p2, 0x1000, 1);
        assert_eq!(woken_p2.len(), 1);
        assert_eq!(woken_p2[0], t3_p2);
    }

    // ------------------------------------------------------------------
    // Lost-wakeup regression (H7): the word must be re-read AFTER the waiter
    // is published, under the same lock that publishes it.
    // ------------------------------------------------------------------

    /// A wake that ran before publication finds no waiter. With an infinite
    /// deadline nothing else ever releases it -- this is the hang the recheck
    /// exists to prevent, pinned so the plain `add_waiter` path cannot be
    /// mistaken for a safe one.
    #[test]
    fn wake_before_publication_strands_a_plain_waiter() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let t1 = TaskId(501);
        let uaddr = 0x5000_1000;

        // Waker stores the new value and wakes: no waiter exists yet.
        assert!(mgr.wake(p1, uaddr, 1).is_empty());
        mgr.add_waiter(t1, p1, uaddr, 0); // timeout_ns == 0 -> infinite

        assert!(!mgr.is_woken(t1));
        assert!(mgr.take_timeouts(u64::MAX).is_empty(), "an infinite deadline never expires");
    }

    /// Waker's store lands after the caller's first read but before publication:
    /// the re-read inside the critical section must observe it and roll back.
    #[test]
    fn store_before_publish_is_observed_and_rolls_back() {
        use core::cell::Cell;

        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let t1 = TaskId(502);
        let uaddr = 0x5000_2000;
        let word = Cell::new(7u32);

        // The syscall's pre-check read.
        let expected = word.get();
        // The waker stores the new value in the window before publication.
        word.set(8);

        let outcome = mgr.add_waiter_recheck(t1, p1, uaddr, 0, expected, || word.get());
        assert_eq!(outcome, FutexWaitOutcome::ValueChanged);

        // Nothing may be left published: no phantom wake, no phantom timeout.
        assert!(!mgr.is_woken(t1));
        assert!(mgr.wake(p1, uaddr, 10).is_empty());
        assert!(!mgr.remove_waiter(t1));
        assert!(mgr.take_timeouts(u64::MAX).is_empty());
    }

    /// Waker's `wake` lands after publication but before the re-read: the waiter
    /// is already marked, so the caller must not block.
    #[test]
    fn wake_between_publish_and_recheck_is_observed() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let t1 = TaskId(503);
        let uaddr = 0x5000_3000;

        // Publication half of add_waiter_recheck.
        mgr.add_waiter(t1, p1, uaddr, 0);
        // Racing waker: same value still in the word, but the wake already ran.
        let woken = mgr.wake(p1, uaddr, 1);
        assert_eq!(woken, alloc::vec![t1]);

        let outcome = mgr.recheck_published(t1, 7, || 7);
        assert_eq!(outcome, FutexWaitOutcome::AlreadyWoken);

        // The waiter stays published so the syscall's removal reports a wake.
        assert!(mgr.is_woken(t1));
        assert!(mgr.remove_waiter(t1));
    }

    /// Unchanged word and no racing wake: the caller must block.
    #[test]
    fn unchanged_word_publishes_a_blocking_waiter() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let t1 = TaskId(504);
        let uaddr = 0x5000_4000;

        let outcome = mgr.add_waiter_recheck(t1, p1, uaddr, 100, 7, || 7);
        assert_eq!(outcome, FutexWaitOutcome::Enqueued);
        assert!(!mgr.is_woken(t1));

        // A wake after publication is still delivered normally.
        assert_eq!(mgr.wake(p1, uaddr, 1), alloc::vec![t1]);
        assert!(mgr.is_woken(t1));
    }

    /// A rolled-back publication must not disturb another waiter on the same
    /// address, and a requeue must not move the rolled-back task's neighbours.
    #[test]
    fn value_change_rollback_leaves_other_waiters_intact() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let p2 = ProcessId(2);
        let t1 = TaskId(505);
        let t2 = TaskId(506);
        let t3_other_proc = TaskId(507);
        let uaddr = 0x5000_5000;

        mgr.add_waiter(t2, p1, uaddr, 0);
        mgr.add_waiter(t3_other_proc, p2, uaddr, 0);

        let outcome = mgr.add_waiter_recheck(t1, p1, uaddr, 0, 7, || 9);
        assert_eq!(outcome, FutexWaitOutcome::ValueChanged);

        let woken = mgr.wake(p1, uaddr, 10);
        assert_eq!(woken, alloc::vec![t2], "only the surviving p1 waiter may be woken");
        assert!(!mgr.is_woken(t3_other_proc), "cross-process isolation must hold");
    }

    /// Re-publication for a task that already waits replaces the entry instead of
    /// duplicating it, so a rollback cannot leave two entries for one task.
    #[test]
    fn repeated_publication_keeps_one_waiter_per_task() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let t1 = TaskId(508);
        let uaddr = 0x5000_6000;

        assert_eq!(mgr.add_waiter_recheck(t1, p1, uaddr, 50, 7, || 7), FutexWaitOutcome::Enqueued);
        assert_eq!(mgr.add_waiter_recheck(t1, p1, uaddr, 0, 7, || 7), FutexWaitOutcome::Enqueued);
        assert_eq!(mgr.wake(p1, uaddr, 10), alloc::vec![t1]);
        assert!(mgr.wake(p1, uaddr, 10).is_empty());
        assert!(mgr.remove_waiter(t1));
        assert!(!mgr.remove_waiter(t1));
    }

    // ------------------------------------------------------------------
    // Consumed-timeout retry regression (H8).
    // ------------------------------------------------------------------

    /// A consumed deadline that could not be delivered must be handed back so a
    /// later tick still times the waiter out.
    #[test]
    fn consumed_timeout_is_restored_and_retried_on_a_later_tick() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let t1 = TaskId(601);
        let uaddr = 0x6000_1000;

        mgr.add_waiter(t1, p1, uaddr, 100);

        let expired = mgr.take_timeouts(150);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired.as_slice()[0], (t1, 100));
        // Deadline consumed: a bare retry without restore would find nothing.
        assert!(mgr.take_timeouts(150).is_empty());

        mgr.restore_timeouts(&expired);

        let retried = mgr.take_timeouts(151);
        assert_eq!(retried.len(), 1, "the waiter must still time out after a contended tick");
        assert_eq!(retried.as_slice()[0], (t1, 100));
    }

    /// Restore must not resurrect a waiter that was woken, re-armed by a new
    /// wait, or already removed by task exit.
    #[test]
    fn restore_skips_woken_rearmed_and_removed_waiters() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let woken_task = TaskId(602);
        let rearmed_task = TaskId(603);
        let exited_task = TaskId(604);
        let uaddr = 0x6000_2000;

        mgr.add_waiter(woken_task, p1, uaddr, 100);
        mgr.add_waiter(rearmed_task, p1, uaddr, 100);
        mgr.add_waiter(exited_task, p1, uaddr, 100);

        let expired = mgr.take_timeouts(150);
        assert_eq!(expired.len(), 3);

        // Interleavings that can happen while the scheduler lock is contended.
        assert_eq!(mgr.wake(p1, uaddr, 1), alloc::vec![woken_task]);
        mgr.add_waiter(rearmed_task, p1, uaddr, 900); // new wait, new deadline
        mgr.cleanup_task(exited_task); // task exited

        mgr.restore_timeouts(&expired);

        // The woken waiter stays woken and unarmed.
        assert!(mgr.is_woken(woken_task));
        // The re-armed waiter keeps its NEW deadline, not the stale one.
        let next = mgr.take_timeouts(200);
        assert!(next.is_empty(), "a re-armed deadline must not be pulled back to 100");
        let later = mgr.take_timeouts(900);
        assert_eq!(later.as_slice(), &[(rearmed_task, 900)][..]);
        // The exited task is gone for good: a late wake must not reach it.
        let late_wake = mgr.wake(p1, uaddr, 10);
        assert!(!late_wake.contains(&exited_task));
        mgr.cleanup_task(woken_task);
        mgr.cleanup_task(rearmed_task);
        assert!(mgr.wake(p1, uaddr, 10).is_empty());
    }

    /// One tick drains at most TIMEOUT_BATCH waiters and leaves the rest armed,
    /// so a flood of expiries neither allocates in the interrupt nor loses one.
    #[test]
    fn timeout_batch_is_bounded_and_leaves_the_rest_armed() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let total = TIMEOUT_BATCH + 5;
        for i in 0..total {
            mgr.add_waiter(TaskId(700 + i as u64), p1, 0x7000_1000, 10);
        }

        let first = mgr.take_timeouts(20);
        assert_eq!(first.len(), TIMEOUT_BATCH);
        let second = mgr.take_timeouts(20);
        assert_eq!(second.len(), 5, "the overflow must stay armed for the next tick");
        assert!(mgr.take_timeouts(20).is_empty());

        // Every waiter was reported exactly once.
        let mut seen: Vec<TaskId> = first.as_slice().iter().chain(second.as_slice().iter()).map(|&(id, _)| id).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), total);
    }

    /// A waiter with no deadline (infinite wait) is never collected, and a wake
    /// already delivered is never collected as a timeout.
    #[test]
    fn infinite_and_woken_waiters_are_never_timed_out() {
        let mut mgr = FutexManager::new();
        let p1 = ProcessId(1);
        let infinite = TaskId(801);
        let woken = TaskId(802);
        let uaddr_infinite = 0x8000_1000;
        let uaddr_woken = 0x8000_2000;

        mgr.add_waiter(infinite, p1, uaddr_infinite, 0);
        mgr.add_waiter(woken, p1, uaddr_woken, 10);
        assert_eq!(mgr.wake(p1, uaddr_woken, 1), alloc::vec![woken]);

        assert!(mgr.take_timeouts(u64::MAX).is_empty());
    }
}
