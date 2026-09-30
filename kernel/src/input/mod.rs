//! Mobile Touch & Hardware Input Subsystem
//!
//! Native capacitive multi-touch digitizer, mobile gesture engine, and input event queue.
//! Non-Negotiable Principle #1: The user directly owns and drives the mobile hardware.

pub mod coordinate;
pub mod event;
pub mod queue;
pub mod router;
pub mod touch;

pub use coordinate::{CoordinateSpace, CoordinateTransform, Point};
pub use event::{InputEvent, InputSource, KeyAction, KeyCode, KeyEvent, PointerEvent, PointerPhase};
pub use queue::InputQueue;
pub use router::{GestureEvent, GestureKind, InputRouter, PointerOwner, PointerState};
pub use touch::{MAX_TOUCH_SLOTS, TouchDigitizer, TouchGesture, TouchPhase, TouchPoint};

use spin::Mutex;
use crate::framebuffer::{PHONE_X, PHONE_Y};
use crate::serial_println;

/// Global thread-safe hardware input event ring buffer.
pub static INPUT_QUEUE: Mutex<InputQueue> = Mutex::new(InputQueue::new());

/// Global thread-safe capacitive touch digitizer.
pub static TOUCH_DIGITIZER: Mutex<TouchDigitizer> = Mutex::new(TouchDigitizer::new());

/// Canonical pointer/gesture router shared by hardware and Device Studio input.
pub static INPUT_ROUTER: Mutex<InputRouter> = Mutex::new(InputRouter::new());

/// Initializes the mobile touch and input subsystem, executing gesture self-tests.
pub fn init() {
    let mut q = INPUT_QUEUE.lock();

    assert!(q.is_empty());
    assert!(q.push(InputEvent::Key { char_code: 'A', pressed: true }));
    let test_touch = TouchPoint {
        slot: 0,
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 100) as u32,
        pressure: 512,
        phase: TouchPhase::Down,
    };
    assert!(q.push(InputEvent::Touch(test_touch)));
    assert_eq!(q.len(), 2);

    let ev1 = q.pop().expect("Expected key event");
    assert_eq!(ev1, InputEvent::Key { char_code: 'A', pressed: true });
    let ev2 = q.pop().expect("Expected touch event");
    assert_eq!(ev2, InputEvent::Touch(test_touch));
    assert!(q.is_empty());
    serial_println!("[TEST] Input Event Queue: Non-allocating ring buffer push/pop PASSED.");

    let mut digitizer = TOUCH_DIGITIZER.lock();
    let initial_taps = digitizer.total_taps_detected;
    let tap_x = (PHONE_X + 120) as u32;
    let tap_y = (PHONE_Y + 180) as u32;

    let g1 = digitizer.update_touch(0, tap_x, tap_y, 600, TouchPhase::Down);
    assert_eq!(g1, None);
    assert_eq!(digitizer.active_touch_count(), 1);

    let g2 = digitizer.update_touch(0, tap_x, tap_y, 0, TouchPhase::Up);
    assert_eq!(g2, Some(TouchGesture::Tap { x: tap_x, y: tap_y }));
    assert_eq!(digitizer.active_touch_count(), 0);
    assert_eq!(digitizer.total_taps_detected, initial_taps + 1);
    serial_println!("[TEST] Touch Digitizer: Single-finger capacitive Tap detected at ({}, {}).", tap_x, tap_y);

    digitizer.update_touch(0, (PHONE_X + 50) as u32, (PHONE_Y + 50) as u32, 500, TouchPhase::Down);
    digitizer.update_touch(1, (PHONE_X + 200) as u32, (PHONE_Y + 200) as u32, 500, TouchPhase::Down);
    assert_eq!(digitizer.active_touch_count(), 2);
    digitizer.update_touch(0, (PHONE_X + 50) as u32, (PHONE_Y + 50) as u32, 0, TouchPhase::Up);
    assert_eq!(digitizer.active_touch_count(), 1);
    digitizer.update_touch(1, (PHONE_X + 200) as u32, (PHONE_Y + 200) as u32, 0, TouchPhase::Up);
    assert_eq!(digitizer.active_touch_count(), 0);
    serial_println!("[TEST] Touch Digitizer: Multi-touch dual-slot tracking (2 concurrent contacts) VERIFIED.");

    digitizer.update_touch(0, 500, 300, 512, TouchPhase::Down);
    let drag = digitizer.update_touch(0, 500, 350, 512, TouchPhase::Move);
    assert_eq!(
        drag,
        Some(TouchGesture::Drag {
            start_x: 500,
            start_y: 300,
            current_x: 500,
            current_y: 350,
        })
    );
    digitizer.update_touch(0, 500, 350, 0, TouchPhase::Up);
    serial_println!("[TEST] Touch Digitizer: Vertical scroll/drag gesture (+50px) recognized.");

    {
        use crate::drivers::touch_i2c::{GoodixGt911Driver, MemoryI2cAdapter, GT911_I2C_ADDR_PRIMARY};
        let mut adapter = MemoryI2cAdapter::new();
        let mut gt911 = GoodixGt911Driver::new(GT911_I2C_ADDR_PRIMARY, 1080, 2400);
        let id = gt911.probe(&mut adapter).expect("GT911 probe failed");
        assert_eq!(&id, b"911\0", "GT911 product ID mismatch");

        let mut frame = [0u8; 1 + (2 * 8)];
        frame[0] = 0x82;
        frame[1] = 0;
        frame[2] = (360u16 & 0xFF) as u8;
        frame[3] = ((360u16 >> 8) & 0xFF) as u8;
        frame[4] = (640u16 & 0xFF) as u8;
        frame[5] = ((640u16 >> 8) & 0xFF) as u8;
        frame[6] = 50;
        frame[7] = 0;
        frame[9] = 1;
        frame[10] = (720u16 & 0xFF) as u8;
        frame[11] = ((720u16 >> 8) & 0xFF) as u8;
        frame[12] = (1200u16 & 0xFF) as u8;
        frame[13] = ((1200u16 >> 8) & 0xFF) as u8;
        frame[14] = 60;
        frame[15] = 0;

        let points = gt911.parse_frame(&frame).expect("Failed to parse GT911 frame");
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].slot, 0);
        assert_eq!(points[0].x, 360);
        assert_eq!(points[0].y, 640);
        assert_eq!(points[0].phase, TouchPhase::Down);
        assert_eq!(points[1].slot, 1);
        assert_eq!(points[1].x, 720);
        assert_eq!(points[1].y, 1200);
        assert_eq!(points[1].phase, TouchPhase::Down);
    }

    serial_println!("--- [Capacitive Touch Subsystem 100% Attested & Enforced] ---");
}

pub fn post_char_event(c: char) {
    INPUT_QUEUE.lock().push(InputEvent::Key {
        char_code: c,
        pressed: true,
    });
    post_key_event(KeyEvent {
        source: InputSource::Keyboard,
        action: KeyAction::Down,
        code: KeyCode::Character(c),
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    });
}

pub fn post_key_event(event: KeyEvent) {
    INPUT_QUEUE.lock().push(InputEvent::KeyEvent(event));
    let normalized = crate::drivers::input::InputEvent::from_key(event);
    crate::drivers::input::INPUT_PIPELINE.lock().push(normalized);
    crate::drivers::input::dispatch_to_focused_window(normalized);
    crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
}

pub fn route_touch_target(x: u32, y: u32) -> Option<(crate::gui::WindowId, u64)> {
    crate::gui::WINDOW_MANAGER.lock().hit_test(x, y)
}

pub fn post_touch_event(slot: u8, x: u32, y: u32, pressure: u16, phase: TouchPhase) {
    if phase == TouchPhase::Down {
        let _ = crate::gui::GRAVITY_COMPOSITOR.lock().trigger_force_takeover();
    }

    let ticks = crate::interrupts::get_system_ticks();
    let pointer_phase = match phase {
        TouchPhase::Down => PointerPhase::Down,
        TouchPhase::Move => PointerPhase::Move,
        TouchPhase::Up => PointerPhase::Up,
        TouchPhase::Cancel => PointerPhase::Cancel,
    };
    let pointer = PointerEvent::new(
        InputSource::DeviceStudio,
        CoordinateSpace::LogicalPortrait,
        slot,
        pointer_phase,
        x as i32,
        y as i32,
    );

    // Preserve the raw hardware event for diagnostics/low-level consumers.
    INPUT_QUEUE.lock().push(InputEvent::Touch(TouchPoint {
        slot,
        x,
        y,
        pressure,
        phase,
    }));

    let gesture = INPUT_ROUTER
        .lock()
        .dispatch_pointer_at(pointer, 720, 24, ticks);

    if let Some(gesture) = gesture {
        let event = crate::drivers::input::InputEvent::from_canonical(gesture);
        crate::drivers::input::INPUT_PIPELINE.lock().push(event);
        crate::drivers::input::dispatch_to_focused_window(event);
        crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
    } else if phase == TouchPhase::Move || phase == TouchPhase::Down {
        crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
    }
}