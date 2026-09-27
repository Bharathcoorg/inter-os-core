//! Userspace-facing input event pipeline.
//!
//! Hardware-specific touch/keyboard drivers feed normalized events here.
//! The pipeline deliberately exposes events, not raw MMIO/controller state.

use alloc::collections::VecDeque;
use crate::drivers::touch_gestures::RecognizedGesture;
use spin::Mutex;
use crate::task::TaskId;

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
            RecognizedGesture::Pan { start_x, start_y, current_x, current_y, dx, dy } => {
                let disp_x = current_x as i32 - start_x as i32;
                let disp_y = current_y as i32 - start_y as i32;
                Self {
                    kind: InputEventKind::Pan,
                    x: current_x,
                    y: current_y,
                    dx: if disp_x != 0 { disp_x } else { dx },
                    dy: if disp_y != 0 { disp_y } else { dy },
                    ..Self::EMPTY
                }
            }
            RecognizedGesture::Pinch { center_x, center_y, scale_factor } => Self {
                kind: InputEventKind::Pinch, x: center_x, y: center_y, scale_milli: (scale_factor * 1000.0) as u32, ..Self::EMPTY
            },
        }
    }
}

pub struct InputPipeline {
    queue: VecDeque<InputEvent>,
    service_queue: VecDeque<InputEvent>,
}

impl InputPipeline {
    pub const fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            service_queue: VecDeque::new(),
        }
    }

    pub fn push(&mut self, event: InputEvent) {
        if self.queue.len() < 128 { self.queue.push_back(event); }
        if self.service_queue.len() < 128 { self.service_queue.push_back(event); }
    }

    pub fn pop(&mut self) -> Option<InputEvent> { self.queue.pop_front() }

    pub fn pop_service(&mut self) -> Option<InputEvent> { self.service_queue.pop_front() }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.service_queue.clear();
    }
}

pub static INPUT_PIPELINE: Mutex<InputPipeline> = Mutex::new(InputPipeline::new());

/// Moves recognized hardware gestures into the stable userspace event queue.
/// Routes normalized input events to the currently focused surface owner.
pub fn dispatch_to_focused_window(event: InputEvent) {
    let target = crate::drivers::display_kms::DRM_KMS.lock().focused_surface();
    let owner = match target {
        Some(id) => crate::drivers::display_kms::DRM_KMS.lock().surfaces.iter().find(|s| s.id == id).map(|s| s.owner_task),
        None => None,
    };
    if let Some(owner) = owner {
        let mut msg = [0u8; 24];
        msg[0] = event.kind as u8;
        msg[4..8].copy_from_slice(&event.x.to_le_bytes());
        msg[8..12].copy_from_slice(&event.y.to_le_bytes());
        msg[12..16].copy_from_slice(&event.dx.to_le_bytes());
        msg[16..20].copy_from_slice(&event.dy.to_le_bytes());
        msg[20..24].copy_from_slice(&event.scale_milli.to_le_bytes());
        let _ = crate::ipc::IPC_ROUTER.lock().send_direct(TaskId(0), TaskId(owner), 0x494E_5054, &msg);
    }
}

pub fn pump_touch_events() {
    let mut touch = crate::drivers::touch_gestures::TOUCH_ENGINE.lock();
    let mut pipeline = INPUT_PIPELINE.lock();
    let mut pumped = false;
    while let Some(gesture) = touch.gesture_queue.pop_front() {
        let event = InputEvent::from_gesture(gesture);
        pipeline.push(event);
        dispatch_to_focused_window(event);
        pumped = true;
    }
    if pumped {
        crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
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

    #[test]
    fn swipe_up_displacement_preserved() {
        let e = InputEvent::from_gesture(RecognizedGesture::Pan {
            start_x: 360, start_y: 1420, current_x: 360, current_y: 1100, dx: 0, dy: -320,
        });
        assert_eq!(e.kind, InputEventKind::Pan);
        assert_eq!((e.x, e.y, e.dx, e.dy), (360, 1100, 0, -320));
    }
}
