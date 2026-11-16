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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::event::InputEvent;

    #[test]
    fn test_queue_basic_push_pop() {
        let mut q = InputQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert_eq!(q.dropped_count(), 0);

        assert!(q.push(InputEvent::Key { char_code: 'X', pressed: true }));
        assert!(!q.is_empty());
        assert_eq!(q.len(), 1);

        let popped = q.pop();
        assert_eq!(popped, Some(InputEvent::Key { char_code: 'X', pressed: true }));
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn test_queue_capacity_overflow_backpressure() {
        let mut q = InputQueue::new();
        for i in 0..INPUT_QUEUE_CAPACITY {
            let ok = q.push(InputEvent::Key { char_code: (b'A' + (i % 26) as u8) as char, pressed: true });
            assert!(ok, "push should succeed up to capacity");
        }
        assert_eq!(q.len(), INPUT_QUEUE_CAPACITY);
        assert_eq!(q.dropped_count(), 0);

        // 65th push must be rejected and recorded
        let ok = q.push(InputEvent::Key { char_code: 'Z', pressed: false });
        assert!(!ok, "push beyond capacity must be rejected");
        assert_eq!(q.dropped_count(), 1);
        assert_eq!(q.len(), INPUT_QUEUE_CAPACITY);

        // Popping one makes room
        let first = q.pop().expect("pop must succeed");
        assert_eq!(first, InputEvent::Key { char_code: 'A', pressed: true });
        assert_eq!(q.len(), INPUT_QUEUE_CAPACITY - 1);

        // Now push succeeds
        assert!(q.push(InputEvent::Key { char_code: 'Z', pressed: false }));
        assert_eq!(q.len(), INPUT_QUEUE_CAPACITY);
    }

    #[test]
    fn test_queue_ring_buffer_wrap_preserves_order() {
        let mut q = InputQueue::new();
        // Push 40
        for i in 0..40 {
            assert!(q.push(InputEvent::Key { char_code: ((i as u8) % 100) as char, pressed: true }));
        }
        // Pop 20
        for i in 0..20 {
            assert_eq!(q.pop(), Some(InputEvent::Key { char_code: ((i as u8) % 100) as char, pressed: true }));
        }
        // Push 40 (tail wraps around 64)
        for i in 40..80 {
            assert!(q.push(InputEvent::Key { char_code: ((i as u8) % 100) as char, pressed: true }));
        }
        assert_eq!(q.len(), 60);
        // Pop remaining 60 in order
        for i in 20..80 {
            assert_eq!(q.pop(), Some(InputEvent::Key { char_code: ((i as u8) % 100) as char, pressed: true }));
        }
        assert!(q.is_empty());
    }
}
