//! Mobile Touch & Hardware Input Subsystem
//!
//! Native capacitive multi-touch digitizer, mobile gesture engine, and input event queue.
//! Non-Negotiable Principle #1: The user directly owns and drives the mobile hardware.

pub mod event;
pub mod queue;
pub mod touch;

pub use event::InputEvent;
pub use queue::InputQueue;
pub use touch::{MAX_TOUCH_SLOTS, TouchDigitizer, TouchGesture, TouchPhase, TouchPoint};

use spin::Mutex;
use crate::framebuffer::{PHONE_X, PHONE_Y};
use crate::serial_println;

/// Global thread-safe hardware input event ring buffer.
pub static INPUT_QUEUE: Mutex<InputQueue> = Mutex::new(InputQueue::new());

/// Global thread-safe capacitive touch digitizer.
pub static TOUCH_DIGITIZER: Mutex<TouchDigitizer> = Mutex::new(TouchDigitizer::new());

/// Initializes the mobile touch and input subsystem, executing gesture self-tests.
pub fn init() {
    let mut q = INPUT_QUEUE.lock();

    // 1. Queue push and pop self-test
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

    // 2. Capacitive Touch Screen Tap Gesture Self-Test
    let mut digitizer = TOUCH_DIGITIZER.lock();
    let initial_taps = digitizer.total_taps_detected;
    let tap_x = (PHONE_X + 120) as u32;
    let tap_y = (PHONE_Y + 180) as u32;

    // Contact down
    let g1 = digitizer.update_touch(0, tap_x, tap_y, 600, TouchPhase::Down);
    assert_eq!(g1, None);
    assert_eq!(digitizer.active_touch_count(), 1);

    // Contact lift within tap tolerance
    let g2 = digitizer.update_touch(0, tap_x, tap_y, 0, TouchPhase::Up);
    assert_eq!(g2, Some(TouchGesture::Tap { x: tap_x, y: tap_y }));
    assert_eq!(digitizer.active_touch_count(), 0);
    assert_eq!(digitizer.total_taps_detected, initial_taps + 1);
    serial_println!("[TEST] Touch Digitizer: Single-finger capacitive Tap detected at ({}, {}).", tap_x, tap_y);

    // 3. Multi-Touch Slot Tracking Self-Test
    // Finger 0 Down
    digitizer.update_touch(0, (PHONE_X + 50) as u32, (PHONE_Y + 50) as u32, 500, TouchPhase::Down);
    // Finger 1 Down
    digitizer.update_touch(1, (PHONE_X + 200) as u32, (PHONE_Y + 200) as u32, 500, TouchPhase::Down);
    assert_eq!(digitizer.active_touch_count(), 2);

    // Finger 0 lifts
    digitizer.update_touch(0, (PHONE_X + 50) as u32, (PHONE_Y + 50) as u32, 0, TouchPhase::Up);
    assert_eq!(digitizer.active_touch_count(), 1);

    // Finger 1 lifts
    digitizer.update_touch(1, (PHONE_X + 200) as u32, (PHONE_Y + 200) as u32, 0, TouchPhase::Up);
    assert_eq!(digitizer.active_touch_count(), 0);
    serial_println!("[TEST] Touch Digitizer: Multi-touch dual-slot tracking (2 concurrent contacts) VERIFIED.");

    // 4. Mobile Drag / Scroll Gesture Self-Test
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

    // 5. Mobile I2C Capacitive Touchscreen Hardware Controller Driver Test (Goodix GT911)
    {
        use crate::drivers::touch_i2c::{GoodixGt911Driver, MemoryI2cAdapter, GT911_I2C_ADDR_PRIMARY};
        let mut adapter = MemoryI2cAdapter::new();
        let mut gt911 = GoodixGt911Driver::new(GT911_I2C_ADDR_PRIMARY, 1080, 2400);

        let id = gt911.probe(&mut adapter).expect("GT911 probe failed");
        assert_eq!(&id, b"911\0", "GT911 product ID mismatch");

        // Synthesize raw multi-touch frame: 2 contacts (Point 0: 360,640; Point 1: 720,1200)
        let mut frame = [0u8; 1 + (2 * 8)];
        frame[0] = 0x82; // Ready bit (0x80) | 2 touch points (0x02)

        // Point 0 (slot 0)
        frame[1] = 0; // Track ID 0
        frame[2] = (360u16 & 0xFF) as u8;
        frame[3] = ((360u16 >> 8) & 0xFF) as u8;
        frame[4] = (640u16 & 0xFF) as u8;
        frame[5] = ((640u16 >> 8) & 0xFF) as u8;
        frame[6] = 50; // Size
        frame[7] = 0;

        // Point 1 (slot 1)
        frame[9] = 1; // Track ID 1
        frame[10] = (720u16 & 0xFF) as u8;
        frame[11] = ((720u16 >> 8) & 0xFF) as u8;
        frame[12] = (1200u16 & 0xFF) as u8;
        frame[13] = ((1200u16 >> 8) & 0xFF) as u8;
        frame[14] = 60; // Size
        frame[15] = 0;

        let points = gt911.parse_frame(&frame).expect("Failed to parse GT911 frame");
        assert_eq!(points.len(), 2, "Expected 2 decoded touch points");
        assert_eq!(points[0].slot, 0);
        assert_eq!(points[0].x, 360);
        assert_eq!(points[0].y, 640);
        assert_eq!(points[0].phase, TouchPhase::Down);

        assert_eq!(points[1].slot, 1);
        assert_eq!(points[1].x, 720);
        assert_eq!(points[1].y, 1200);
        assert_eq!(points[1].phase, TouchPhase::Down);

        serial_println!(
            "[TEST] Mobile I2C Touch Driver: Goodix GT911 product ID '911' verified, dual-touch frame decoded at ({}, {}) & ({}, {}).",
            points[0].x, points[0].y, points[1].x, points[1].y
        );
    }

    serial_println!("--- [Capacitive Touch Subsystem 100% Attested & Enforced] ---");
}

/// Enqueues a character pressed event from virtual/physical keyboard.
pub fn post_char_event(c: char) {
    INPUT_QUEUE.lock().push(InputEvent::Key {
        char_code: c,
        pressed: true,
    });
    crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
}

/// Enqueues a touch screen contact event from the digitizer controller.
/// Resolves a screen coordinate to the authoritative application window before
/// delivering the event. System overlays and modal windows take precedence.
pub fn route_touch_target(x: u32, y: u32) -> Option<(crate::gui::WindowId, u64)> {
    crate::gui::WINDOW_MANAGER.lock().hit_test(x, y)
}

pub fn post_touch_event(slot: u8, x: u32, y: u32, pressure: u16, phase: TouchPhase) {
    // Dismiss transient modal overlay if screen is touched
    if phase == TouchPhase::Down {
        let _ = crate::gui::GRAVITY_COMPOSITOR.lock().trigger_force_takeover();
    }

    let ticks = crate::interrupts::get_system_ticks();
    {
        let mut engine = crate::drivers::touch_gestures::TOUCH_ENGINE.lock();
        match phase {
            TouchPhase::Down => engine.handle_down(slot as usize, slot, x, y, ticks),
            TouchPhase::Move => engine.handle_move(slot as usize, x, y, ticks),
            TouchPhase::Up | TouchPhase::Cancel => engine.handle_up(slot as usize, ticks),
        }
    }
    crate::drivers::input::pump_touch_events();

    let gesture_opt = TOUCH_DIGITIZER.lock().update_touch(slot, x, y, pressure, phase);
    let mut q = INPUT_QUEUE.lock();
    q.push(InputEvent::Touch(TouchPoint {
        slot,
        x,
        y,
        pressure,
        phase,
    }));
    if let Some(gesture) = gesture_opt {
        q.push(InputEvent::Gesture(gesture));
    }
    crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
}
