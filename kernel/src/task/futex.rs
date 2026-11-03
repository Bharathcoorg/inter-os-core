// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign Futex (Fast Userspace Mutex) Subsystem
//!
//! Provides zero-contention userspace synchronization and deterministic kernel wait queues.
//! Threads sleep on a user virtual address and are awakened via `sys_futex_wake`
//! or when an optional deadline expires.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::task::TaskId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FutexWaiter {
    pub task_id: TaskId,
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

    pub fn add_waiter(&mut self, task_id: TaskId, uaddr: u64, deadline_ticks: u64) {
        if let Some(w) = self.waiters.iter_mut().find(|w| w.task_id == task_id) {
            w.uaddr = uaddr;
            w.deadline_ticks = deadline_ticks;
            w.woken = false;
        } else {
            self.waiters.push(FutexWaiter {
                task_id,
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

    pub fn wake(&mut self, uaddr: u64, max_count: usize) -> Vec<TaskId> {
        let mut woken = Vec::new();
        let mut count = 0;
        for w in self.waiters.iter_mut() {
            if w.uaddr == uaddr && !w.woken {
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
}

pub static FUTEX_MANAGER: Mutex<FutexManager> = Mutex::new(FutexManager::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_futex_manager_wait_and_wake() {
        let mut mgr = FutexManager::new();
        let t1 = TaskId(101);
        let t2 = TaskId(102);
        let t3 = TaskId(103);
        let uaddr = 0x2000_1000;

        mgr.add_waiter(t1, uaddr, 0);
        mgr.add_waiter(t2, uaddr, 0);
        mgr.add_waiter(t3, 0x2000_2000, 0);

        assert!(!mgr.is_woken(t1));
        assert!(!mgr.is_woken(t2));

        // Wake 1 waiter on uaddr
        let woken = mgr.wake(uaddr, 1);
        assert_eq!(woken.len(), 1);
        assert_eq!(woken[0], t1);
        assert!(mgr.is_woken(t1));
        assert!(!mgr.is_woken(t2));

        // Wake another waiter on uaddr
        let woken2 = mgr.wake(uaddr, 1);
        assert_eq!(woken2.len(), 1);
        assert_eq!(woken2[0], t2);
        assert!(mgr.is_woken(t2));

        // Waiter on different uaddr is untouched
        assert!(!mgr.is_woken(t3));

        // Removing waiter returns woken status
        assert!(mgr.remove_waiter(t1));
        assert!(mgr.remove_waiter(t2));
        assert!(!mgr.remove_waiter(t3));
    }

    #[test]
    fn test_futex_manager_timeouts_and_cleanup() {
        let mut mgr = FutexManager::new();
        let t1 = TaskId(201);
        let t2 = TaskId(202);
        let uaddr = 0x3000_1000;

        mgr.add_waiter(t1, uaddr, 100);
        mgr.add_waiter(t2, uaddr, 200);

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
}

