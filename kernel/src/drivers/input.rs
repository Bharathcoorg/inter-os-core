//! Userspace-facing input event pipeline.
//!
//! Hardware-specific touch/keyboard drivers feed normalized events here.
//! The pipeline deliberately exposes events, not raw MMIO/controller state.

use alloc::collections::VecDeque;
use crate::drivers::touch_gestures::RecognizedGesture;
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InputEventKind {
    Tap = 1,
    DoubleTap = 2,
    LongPress = 3,
    Pan = 4,
    Pinch = 5,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputEvent {
    pub kind: InputEventKind,
    pub x: u32,
    pub y: u32,
    pub dx: i32,
    pub dy: i32,
    pub scale_milli: u32,
}

impl InputEvent {
    pub const EMPTY: Self = Self {
        kind: InputEventKind::Tap,
        x: 0,
        y: 0,
        dx: 0,
        dy: 0,
        scale_milli: 1000,
    };

    pub fn from_gesture(gesture: RecognizedGesture) -> Self {
        match gesture {
            RecognizedGesture::Tap { x, y } => Self { kind: InputEventKind::Tap, x, y, ..Self::EMPTY },
            RecognizedGesture::DoubleTap { x, y } => Self { kind: InputEventKind::DoubleTap, x, y, ..Self::EMPTY },
            RecognizedGesture::LongPress { x, y } => Self { kind: InputEventKind::LongPress, x, y, ..Self::EMPTY },
            RecognizedGesture::Pan { start_x, start_y, current_x, current_y, dx, dy } => Self {
                kind: InputEventKind::Pan, x: current_x, y: current_y, dx: (current_x as i32 - start_x as i32).max(dx),
                dy: (current_y as i32 - start_y as i32).max(dy), ..Self::EMPTY
            },
            RecognizedGesture::Pinch { center_x, center_y, scale_factor } => Self {
                kind: InputEventKind::Pinch, x: center_x, y: center_y, scale_milli: (scale_factor * 1000.0) as u32, ..Self::EMPTY
            },
        }
    }
}

pub struct InputPipeline {
    queue: VecDeque<InputEvent>,
}

impl InputPipeline {
    pub const fn new() -> Self { Self { queue: VecDeque::new() } }

    pub fn push(&mut self, event: InputEvent) {
        if self.queue.len() < 128 { self.queue.push_back(event); }
    }

    pub fn pop(&mut self) -> Option<InputEvent> { self.queue.pop_front() }
}

pub static INPUT_PIPELINE: Mutex<InputPipeline> = Mutex::new(InputPipeline::new());

/// Moves recognized hardware gestures into the stable userspace event queue.
pub fn pump_touch_events() {
    let mut touch = crate::drivers::touch_gestures::TOUCH_ENGINE.lock();
    let mut pipeline = INPUT_PIPELINE.lock();
    while let Some(gesture) = touch.gesture_queue.pop_front() {
        pipeline.push(InputEvent::from_gesture(gesture));
    }
}

#[cfg(test)]
mod tests {
    use super::{InputEvent, InputEventKind};
    use crate::drivers::touch_gestures::RecognizedGesture;

    #[test]
    fn gesture_is_normalized_without_raw_device_details() {
        let e = InputEvent::from_gesture(RecognizedGesture::Pan {
            start_x: 10, start_y: 20, current_x: 30, current_y: 50, dx: 20, dy: 30,
        });
        assert_eq!(e.kind, InputEventKind::Pan);
        assert_eq!((e.x, e.y, e.dx, e.dy), (30, 50, 20, 30));
    }
}
