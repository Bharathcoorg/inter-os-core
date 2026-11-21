// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android MessageQueue, Looper, Handler, Choreographer & Activity Back Stack
//!
//! Implements Android's event loop, message dispatch, vsync frame pacing, and
//! multi-activity back stack management without ambient authority or fake lifecycle states.
//!
//! Components:
//! - `Message` & `MessageQueue`: Timestamp-ordered asynchronous message queues.
//! - `Looper`: Event loop driving Android main thread execution.
//! - `Choreographer`: Deterministic 60 FPS vsync frame callback scheduler.
//! - `ActivityRecord` & `TaskRecord`: Android Activity back stack managing
//!   multi-activity Intent navigation and forward/back transitions:
//!   `Activity A -> Intent -> Activity B -> finish() -> Activity A resumed`.

use alloc::string::String;
use alloc::vec::Vec;
use super::framework::ActivityLifecycleState;

/// An Android event message dispatched through a Handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub what: u32,
    pub arg1: i32,
    pub arg2: i32,
    pub obj_id: u32,
    pub target_handler_id: u32,
    pub when_ticks: u64,
}

/// A priority queue of Messages sorted by scheduled execution tick.
#[derive(Debug, Clone, Default)]
pub struct MessageQueue {
    messages: Vec<Message>,
}

impl MessageQueue {
    pub const fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn enqueue_message(&mut self, msg: Message) {
        let insert_idx = self
            .messages
            .iter()
            .position(|m| m.when_ticks > msg.when_ticks)
            .unwrap_or(self.messages.len());
        self.messages.insert(insert_idx, msg);
    }

    pub fn poll_next(&mut self, current_ticks: u64) -> Option<Message> {
        if let Some(first) = self.messages.first() {
            if first.when_ticks <= current_ticks {
                return Some(self.messages.remove(0));
            }
        }
        None
    }

    pub fn remove_messages(&mut self, what: u32) -> usize {
        let before = self.messages.len();
        self.messages.retain(|m| m.what != what);
        before - self.messages.len()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Android main thread event loop driver.
#[derive(Debug, Clone, Default)]
pub struct Looper {
    pub queue: MessageQueue,
    pub is_running: bool,
    pub messages_dispatched: u64,
}

impl Looper {
    pub const fn new() -> Self {
        Self {
            queue: MessageQueue::new(),
            is_running: false,
            messages_dispatched: 0,
        }
    }

    pub fn post(&mut self, what: u32, current_ticks: u64) {
        self.queue.enqueue_message(Message {
            what,
            arg1: 0,
            arg2: 0,
            obj_id: 0,
            target_handler_id: 1,
            when_ticks: current_ticks,
        });
    }

    pub fn post_delayed(&mut self, what: u32, delay_ticks: u64, current_ticks: u64) {
        self.queue.enqueue_message(Message {
            what,
            arg1: 0,
            arg2: 0,
            obj_id: 0,
            target_handler_id: 1,
            when_ticks: current_ticks.saturating_add(delay_ticks),
        });
    }

    /// Pumps all messages ready at `current_ticks`. Returns number of messages processed.
    pub fn loop_once(&mut self, current_ticks: u64) -> usize {
        let mut count = 0;
        while let Some(_msg) = self.queue.poll_next(current_ticks) {
            count += 1;
            self.messages_dispatched += 1;
        }
        count
    }
}

/// Choreographer driving 60 FPS frame callbacks.
#[derive(Debug, Clone, Default)]
pub struct Choreographer {
    frame_callbacks: Vec<u32>, // callback IDs
    pub frames_rendered: u64,
    pub last_frame_time_nanos: u64,
}

impl Choreographer {
    pub const fn new() -> Self {
        Self {
            frame_callbacks: Vec::new(),
            frames_rendered: 0,
            last_frame_time_nanos: 0,
        }
    }

    pub fn post_frame_callback(&mut self, cb_id: u32) {
        self.frame_callbacks.push(cb_id);
    }

    pub fn do_frame(&mut self, frame_time_nanos: u64) -> usize {
        self.last_frame_time_nanos = frame_time_nanos;
        self.frames_rendered += 1;
        let cbs = self.frame_callbacks.len();
        self.frame_callbacks.clear();
        cbs
    }
}

/// Record representing one live Android Activity in the back stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRecord {
    pub token: u32,
    pub component_name: String,
    pub state: ActivityLifecycleState,
    pub launch_intent: String,
}

/// Multi-activity task back stack managing navigation and state transitions.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: u32,
    pub affinity: String,
    activity_stack: Vec<ActivityRecord>,
    next_token: u32,
}

impl TaskRecord {
    pub fn new(task_id: u32, affinity: &str) -> Self {
        Self {
            task_id,
            affinity: String::from(affinity),
            activity_stack: Vec::new(),
            next_token: 100,
        }
    }

    /// Pushes a new Activity onto the back stack, transitioning previous top to Paused/Stopped.
    pub fn start_activity(&mut self, component_name: &str, intent: &str) -> &ActivityRecord {
        // Pause previous top if exists
        if let Some(prev) = self.activity_stack.last_mut() {
            prev.state = ActivityLifecycleState::Stopped;
        }

        let token = self.next_token;
        self.next_token += 1;

        let record = ActivityRecord {
            token,
            component_name: String::from(component_name),
            state: ActivityLifecycleState::Resumed,
            launch_intent: String::from(intent),
        };

        self.activity_stack.push(record);
        self.activity_stack.last().unwrap()
    }

    /// Finishes the current top Activity and restores the underlying Activity to Resumed.
    pub fn finish_top_activity(&mut self) -> Option<ActivityRecord> {
        let mut finished = self.activity_stack.pop()?;
        finished.state = ActivityLifecycleState::Destroyed;

        // Resume underneath activity
        if let Some(underneath) = self.activity_stack.last_mut() {
            underneath.state = ActivityLifecycleState::Resumed;
        }

        Some(finished)
    }

    pub fn top_activity(&self) -> Option<&ActivityRecord> {
        self.activity_stack.last()
    }

    pub fn stack_depth(&self) -> usize {
        self.activity_stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.activity_stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_queue_and_looper() {
        let mut looper = Looper::new();
        looper.post_delayed(1, 100, 10);
        looper.post(2, 50);

        assert_eq!(looper.loop_once(40), 0);
        assert_eq!(looper.messages_dispatched, 0);

        assert_eq!(looper.loop_once(60), 1);
        assert_eq!(looper.messages_dispatched, 1);

        assert_eq!(looper.loop_once(120), 1);
        assert_eq!(looper.messages_dispatched, 2);
    }

    #[test]
    fn test_choreographer_frame_callbacks() {
        let mut choreo = Choreographer::new();
        choreo.post_frame_callback(1);
        choreo.post_frame_callback(2);
        assert_eq!(choreo.frame_callbacks.len(), 2);

        let dispatched = choreo.do_frame(16_666_666);
        assert_eq!(dispatched, 2);
        assert_eq!(choreo.frames_rendered, 1);
        assert_eq!(choreo.frame_callbacks.len(), 0);
    }

    #[test]
    fn test_task_record_back_stack() {
        let mut task = TaskRecord::new(1, "org.whispersystems.signal");
        assert_eq!(task.stack_depth(), 0);

        let act_a = task.start_activity("org.whispersystems.signal.ConversationListActivity", "MAIN");
        assert_eq!(act_a.state, ActivityLifecycleState::Resumed);
        assert_eq!(task.stack_depth(), 1);

        let act_b = task.start_activity("org.whispersystems.signal.ChatActivity", "VIEW_CHAT");
        assert_eq!(act_b.state, ActivityLifecycleState::Resumed);
        assert_eq!(task.stack_depth(), 2);

        assert_eq!(task.activity_stack[0].state, ActivityLifecycleState::Stopped);

        let finished = task.finish_top_activity().expect("finish chat");
        assert_eq!(finished.component_name, "org.whispersystems.signal.ChatActivity");
        assert_eq!(finished.state, ActivityLifecycleState::Destroyed);
        assert_eq!(task.stack_depth(), 1);

        let top = task.top_activity().expect("top activity");
        assert_eq!(top.component_name, "org.whispersystems.signal.ConversationListActivity");
        assert_eq!(top.state, ActivityLifecycleState::Resumed);
    }
}

