// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Fixed-Capacity Hardware Input Event Ring Buffer
//!
//! Provides lock-protected, non-allocating buffering for interrupt-driven
//! keyboard and pointer events.

use super::event::InputEvent;

pub const INPUT_QUEUE_CAPACITY: usize = 64;

/// Circular ring buffer for hardware input events.
pub struct InputQueue {
    buffer: [Option<InputEvent>; INPUT_QUEUE_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    dropped: u64,
}

impl InputQueue {
    /// Creates an empty input queue.
    pub const fn new() -> Self {
        Self {
            buffer: [None; INPUT_QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            dropped: 0,
        }
    }

    /// Pushes an event into the queue. Returns true if queued, false if dropped (overflow).
    pub fn push(&mut self, event: InputEvent) -> bool {
        if self.count >= INPUT_QUEUE_CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }

        self.buffer[self.tail] = Some(event);
        self.tail = (self.tail + 1) % INPUT_QUEUE_CAPACITY;
        self.count += 1;
        true
    }

    /// Pops the oldest event from the queue.
    pub fn pop(&mut self) -> Option<InputEvent> {
        if self.count == 0 {
            return None;
        }

        let event = self.buffer[self.head].take();
        self.head = (self.head + 1) % INPUT_QUEUE_CAPACITY;
        self.count -= 1;
        event
    }

    /// Returns the number of events waiting in the queue.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns the cumulative number of events rejected because the queue was full.
    pub fn dropped_count(&self) -> u64 { self.dropped }

    /// Returns true if the queue contains no events.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}
