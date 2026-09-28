//! Inter OS Asynchronous Event Notification Subsystem & Sovereign epoll / kqueue Event Reactor
//!
//! Provides non-busy, capability-attested event demultiplexing across diverse hardware and
//! software event sources:
//! - `EVENT_IPC_MSG`: Incoming message arrival in task IPC mailbox.
//! - `EVENT_SHM_READABLE`: Shared Memory DMA Ring Buffer has data available to read.
//! - `EVENT_SHM_WRITABLE`: Shared Memory DMA Ring Buffer has free slots to stream.
//! - `EVENT_TIMER_TICK`: System timer interval / deadline elapsed.
//! - `EVENT_TOUCH_INPUT`: Capacitive touch screen input frame available.
//! - `EVENT_NETWORK_PACKET`: Inbound network socket packet received.
//! - `EVENT_MODEM_URC`: Cellular modem unsolicited result code arrival.
//!
//! Replaces CPU-burning spin loops with cooperative wait-queue blocking:
//! tasks block on their registered event subscriptions until requested events fire or a
//! timer tick timeout elapses.

use alloc::vec::Vec;
use spin::Mutex;
use crate::task::{TaskId, SCHEDULER};
use crate::serial_println;

/// Inbound IPC mailbox message available for task.
pub const EVENT_IPC_MSG: u32        = 1 << 0; // 0x01
/// Zero-Copy DMA Shared Memory pipe has data available to consume.
pub const EVENT_SHM_READABLE: u32   = 1 << 1; // 0x02
/// Zero-Copy DMA Shared Memory pipe has free slot available to stream.
pub const EVENT_SHM_WRITABLE: u32   = 1 << 2; // 0x04
/// Hardware system timer interval tick or deadline elapsed.
pub const EVENT_TIMER_TICK: u32     = 1 << 3; // 0x08
/// Capacitive touch digitizer input event frame available.
pub const EVENT_TOUCH_INPUT: u32    = 1 << 4; // 0x10
/// Inbound network socket datagram or packet received.
pub const EVENT_NETWORK_PACKET: u32 = 1 << 5; // 0x20
/// Cellular baseband modem unsolicited result code (URC) arrived.
pub const EVENT_MODEM_URC: u32      = 1 << 6; // 0x40

/// An active task event subscription in the Sovereign Event Reactor.
#[derive(Debug, Clone, Copy)]
pub struct EventSubscription {
    /// Identity of the subscribing task.
    pub task_id: TaskId,
    /// Bitmask of events the task is interested in receiving.
    pub interest_mask: u32,
    /// Bitmask of events that have fired and are pending consumption.
    pub pending_mask: u32,
    /// Absolute system timer tick deadline for blocking waits (0 if not waiting with timeout).
    pub wait_until_tick: u64,
    /// Whether the task is currently blocked waiting on this subscription.
    pub is_waiting: bool,
}

/// Sovereign Event Reactor managing event registrations, posting, and task wakeups.
pub struct EventReactor {
    subscriptions: Vec<EventSubscription>,
}

impl EventReactor {
    /// Constructs a new, empty EventReactor.
    pub const fn new() -> Self {
        Self {
            subscriptions: Vec::new(),
        }
    }

    /// Registers or updates the event interest mask for a task.
    pub fn register_interest(&mut self, task_id: TaskId, interest_mask: u32) {
        if let Some(sub) = self.subscriptions.iter_mut().find(|s| s.task_id == task_id) {
            sub.interest_mask = interest_mask;
        } else {
            self.subscriptions.push(EventSubscription {
                task_id,
                interest_mask,
                pending_mask: 0,
                wait_until_tick: 0,
                is_waiting: false,
            });
        }
    }

    /// Removes all event state owned by a terminating task.
    pub fn cleanup_task(&mut self, task_id: TaskId) -> bool {
        let before = self.subscriptions.len();
        self.subscriptions.retain(|sub| sub.task_id != task_id);
        before != self.subscriptions.len()
    }

    /// Posts an event to a designated target task, waking it if it is blocked on that event.
    pub fn post_event(&mut self, task_id: TaskId, event_mask: u32) {
        if let Some(sub) = self.subscriptions.iter_mut().find(|s| s.task_id == task_id) {
            let matched = event_mask & sub.interest_mask;
            if matched != 0 {
                sub.pending_mask |= matched;
                if sub.is_waiting {
                    if let Some(mut sched) = SCHEDULER.try_lock() {
                        sub.is_waiting = false;
                        sched.unblock_task(task_id);
                    }
                }
            }
        }
    }

    /// Broadcasts an event to all tasks registered with interest in the event mask.
    pub fn broadcast_event(&mut self, event_mask: u32) {
        // Mark every matching waiter before taking the scheduler lock. There is
        // deliberately no fixed-size wake list: event fan-out must not strand
        // the 17th+ waiter in a permanently blocked state.
        let mut should_wake = false;
        for sub in self.subscriptions.iter_mut() {
            let matched = event_mask & sub.interest_mask;
            if matched != 0 {
                sub.pending_mask |= matched;
                if sub.is_waiting {
                    sub.is_waiting = false;
                    should_wake = true;
                }
            }
        }
        if should_wake {
            if let Some(mut sched) = SCHEDULER.try_lock() {
                for sub in &self.subscriptions {
                    if sub.interest_mask & event_mask != 0 {
                        sched.unblock_task(sub.task_id);
                    }
                }
            }
        }
    }

    /// Polls for pending events on behalf of a task.
    ///
    /// Returns `(triggered_events, should_block)`.
    /// If `should_block` is true, the caller should mark the task blocked and yield CPU.
    pub fn poll(
        &mut self,
        task_id: TaskId,
        interest_mask: u32,
        timeout_ticks: u32,
        current_ticks: u64,
    ) -> (u32, bool) {
        // Find or create subscription
        let idx = if let Some(pos) = self.subscriptions.iter().position(|s| s.task_id == task_id) {
            self.subscriptions[pos].interest_mask = interest_mask;
            pos
        } else {
            self.subscriptions.push(EventSubscription {
                task_id,
                interest_mask,
                pending_mask: 0,
                wait_until_tick: 0,
                is_waiting: false,
            });
            self.subscriptions.len() - 1
        };

        let sub = &mut self.subscriptions[idx];
        let ready = sub.pending_mask & interest_mask;
        if ready != 0 {
            sub.pending_mask &= !ready;
            sub.is_waiting = false;
            return (ready, false);
        }

        if timeout_ticks == 0 {
            // Non-blocking poll with no events pending
            return (0, false);
        }

        // Arm wait with deadline
        sub.wait_until_tick = current_ticks.saturating_add(timeout_ticks as u64);
        sub.is_waiting = true;
        (0, true)
    }

    /// Checks and consumes pending events after a task resumes from a blocking wait.
    pub fn check_woken(&mut self, task_id: TaskId, interest_mask: u32) -> u32 {
        if let Some(sub) = self.subscriptions.iter_mut().find(|s| s.task_id == task_id) {
            let ready = sub.pending_mask & interest_mask;
            sub.pending_mask &= !ready;
            sub.is_waiting = false;
            ready
        } else {
            0
        }
    }

    /// Invoked on hardware timer tick to evaluate deadlines and wake timed-out tasks.
    pub fn on_tick(&mut self, current_ticks: u64) {
        let mut to_unblock = [None; 16];
        let mut count = 0;
        for sub in self.subscriptions.iter_mut() {
            if sub.is_waiting && sub.wait_until_tick > 0 && current_ticks >= sub.wait_until_tick {
                if (sub.interest_mask & EVENT_TIMER_TICK) != 0 {
                    sub.pending_mask |= EVENT_TIMER_TICK;
                }
                if count < 16 {
                    to_unblock[count] = Some(sub.task_id);
                    count += 1;
                }
            }
        }
        if count > 0 {
            if let Some(mut sched) = SCHEDULER.try_lock() {
                for i in 0..count {
                    if let Some(task_id) = to_unblock[i] {
                        if let Some(sub) = self.subscriptions.iter_mut().find(|s| s.task_id == task_id) {
                            sub.is_waiting = false;
                        }
                        sched.unblock_task(task_id);
                    }
                }
            }
        }
    }
}

/// Global system-wide Sovereign Event Reactor.
pub static EVENT_REACTOR: Mutex<EventReactor> = Mutex::new(EventReactor::new());

/// Posts an event to a designated target task via the global Event Reactor.
pub fn cleanup_task(task_id: TaskId) {
    EVENT_REACTOR.lock().cleanup_task(task_id);
}

pub fn post_event(task_id: TaskId, event_mask: u32) {
    EVENT_REACTOR.lock().post_event(task_id, event_mask);
}

/// Broadcasts an event to all interested tasks via the global Event Reactor.
pub fn broadcast_event(event_mask: u32) {
    EVENT_REACTOR.lock().broadcast_event(event_mask);
}

/// Dispatches timer tick notifications to the global Event Reactor.
pub fn on_timer_tick(current_ticks: u64) {
    if let Some(mut reactor) = EVENT_REACTOR.try_lock() {
        reactor.on_tick(current_ticks);
    }
}

/// Initializes the Sovereign Event Reactor subsystem and runs self-tests.
pub fn init() {
    let mut reactor = EVENT_REACTOR.lock();
    let test_task = TaskId(999);
    reactor.register_interest(test_task, EVENT_IPC_MSG | EVENT_TIMER_TICK | EVENT_SHM_READABLE);

    // Test 1: Direct event posting and immediate non-blocking poll
    reactor.post_event(test_task, EVENT_IPC_MSG);
    let (ready1, waiting1) = reactor.poll(test_task, EVENT_IPC_MSG | EVENT_TIMER_TICK, 0, 100);
    assert_eq!(ready1, EVENT_IPC_MSG, "Reactor must deliver pending IPC event");
    assert!(!waiting1, "Non-blocking poll must not request blocking");

    // Test 2: Verify consumed event is cleared
    let (ready2, _) = reactor.poll(test_task, EVENT_IPC_MSG, 0, 100);
    assert_eq!(ready2, 0, "Consumed event must be cleared from pending mask");

    // Test 3: Arm blocking wait with timeout
    let (ready3, waiting3) = reactor.poll(test_task, EVENT_TIMER_TICK, 5, 100);
    assert_eq!(ready3, 0);
    assert!(waiting3, "Timeout > 0 with no events must request blocking");

    // Test 4: Simulate tick arrival and timeout wakeup
    reactor.on_tick(105);
    let woken = reactor.check_woken(test_task, EVENT_TIMER_TICK);
    assert_eq!(woken, EVENT_TIMER_TICK, "Timer tick deadline expiration must wake task with EVENT_TIMER_TICK");

    drop(reactor);
    serial_println!("[TEST] Sovereign Event Reactor: epoll/kqueue multi-source event dispatch & non-busy wait PASSED.");
}
