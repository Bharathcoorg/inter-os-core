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

pub struct FutexManager {
    waiters: Vec<FutexWaiter>,
}

impl FutexManager {
    pub const fn new() -> Self {
        Self {
            waiters: Vec::new(),
        }
    }

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

    pub fn check_timeouts(&mut self, current_ticks: u64) -> Vec<TaskId> {
        let mut timed_out = Vec::new();
        for w in self.waiters.iter_mut() {
            if !w.woken && w.deadline_ticks > 0 && current_ticks >= w.deadline_ticks {
                timed_out.push(w.task_id);
                w.deadline_ticks = 0;
            }
        }
        timed_out
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
}
