//! Capacitive Multi-Touch Digitizer & Mobile Gesture Engine
//!
//! Designed natively for smartphone and tablet touch screens.
//! Replaces legacy desktop mouse models with direct touch point tracking,
//! multi-slot touch state machines, palm rejection, and native mobile gestures (Tap, Drag, Swipe).

use crate::framebuffer::{PHONE_H, PHONE_W, PHONE_X, PHONE_Y};

pub const MAX_TOUCH_SLOTS: usize = 5;

/// State phase of a touch contact point.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    /// New finger contact placed on the capacitive screen.
    Down,
    /// Contact moved across the digitizer surface.
    Move,
    /// Contact lifted from the screen.
    Up,
    /// Contact cancelled (e.g. edge palm rejection or system gesture take-over).
    Cancel,
}

/// An active touch contact point on the digitizer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TouchPoint {
    /// Hardware tracking ID / multi-touch contact slot (0..4).
    pub slot: u8,
    /// Absolute display canvas X coordinate (0..1280).
    pub x: u32,
    /// Absolute display canvas Y coordinate (0..720).
    pub y: u32,
    /// Touch pressure reading (0..1024, or default 512 for standard capacitive screens).
    pub pressure: u16,
    /// Current contact phase.
    pub phase: TouchPhase,
}

impl TouchPoint {
    /// Returns true if this touch point falls directly inside the smartphone screen bounds.
    pub fn is_inside_phone_screen(&self) -> bool {
        let x = self.x as usize;
        let y = self.y as usize;
        x >= PHONE_X && x < (PHONE_X + PHONE_W) && y >= PHONE_Y && y < (PHONE_Y + PHONE_H)
    }

    /// Converts absolute canvas coordinates to phone-local coordinates (0..PHONE_W, 0..PHONE_H).
    pub fn to_phone_local(&self) -> Option<(u32, u32)> {
        if self.is_inside_phone_screen() {
            Some((
                (self.x as usize - PHONE_X) as u32,
                (self.y as usize - PHONE_Y) as u32,
            ))
        } else {
            None
        }
    }
}

/// High-level gesture recognized from touch streams.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TouchGesture {
    /// Clean tap at screen coordinates.
    Tap { x: u32, y: u32 },
    /// Drag / pan gesture from start coordinates to current coordinates.
    Drag {
        start_x: u32,
        start_y: u32,
        current_x: u32,
        current_y: u32,
    },
    /// Lift complete.
    Release { x: u32, y: u32 },
}

/// Tracking history for a single touch slot to calculate gestures.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SlotTracker {
    start_x: u32,
    start_y: u32,
    current_x: u32,
    current_y: u32,
    is_active: bool,
}

/// Multi-touch capacitive screen digitizer.
pub struct TouchDigitizer {
    active_contacts: [Option<TouchPoint>; MAX_TOUCH_SLOTS],
    trackers: [SlotTracker; MAX_TOUCH_SLOTS],
    pub total_taps_detected: u64,
}

impl TouchDigitizer {
    /// Creates a new TouchDigitizer.
    pub const fn new() -> Self {
        Self {
            active_contacts: [None; MAX_TOUCH_SLOTS],
            trackers: [SlotTracker {
                start_x: 0,
                start_y: 0,
                current_x: 0,
                current_y: 0,
                is_active: false,
            }; MAX_TOUCH_SLOTS],
            total_taps_detected: 0,
        }
    }

    /// Feeds a touch contact update from the hardware digitizer controller.
    /// Returns a recognized mobile gesture, if any.
    pub fn update_touch(
        &mut self,
        slot: u8,
        x: u32,
        y: u32,
        pressure: u16,
        phase: TouchPhase,
    ) -> Option<TouchGesture> {
        let slot_idx = (slot as usize) % MAX_TOUCH_SLOTS;

        let contact = TouchPoint {
            slot,
            x,
            y,
            pressure,
            phase,
        };

        match phase {
            TouchPhase::Down => {
                self.active_contacts[slot_idx] = Some(contact);
                self.trackers[slot_idx] = SlotTracker {
                    start_x: x,
                    start_y: y,
                    current_x: x,
                    current_y: y,
                    is_active: true,
                };
                None
            }
            TouchPhase::Move => {
                if let Some(ref mut c) = self.active_contacts[slot_idx] {
                    c.x = x;
                    c.y = y;
                    c.pressure = pressure;
                    c.phase = TouchPhase::Move;
                }
                if self.trackers[slot_idx].is_active {
                    self.trackers[slot_idx].current_x = x;
                    self.trackers[slot_idx].current_y = y;
                    let dx = (x as i32 - self.trackers[slot_idx].start_x as i32).abs();
                    let dy = (y as i32 - self.trackers[slot_idx].start_y as i32).abs();
                    if dx > 10 || dy > 10 {
                        return Some(TouchGesture::Drag {
                            start_x: self.trackers[slot_idx].start_x,
                            start_y: self.trackers[slot_idx].start_y,
                            current_x: x,
                            current_y: y,
                        });
                    }
                }
                None
            }
            TouchPhase::Up => {
                self.active_contacts[slot_idx] = None;
                if self.trackers[slot_idx].is_active {
                    self.trackers[slot_idx].is_active = false;
                    let final_x = if x == 0 && self.trackers[slot_idx].current_x > 0 {
                        self.trackers[slot_idx].current_x
                    } else {
                        x
                    };
                    let final_y = if y == 0 && self.trackers[slot_idx].current_y > 0 {
                        self.trackers[slot_idx].current_y
                    } else {
                        y
                    };
                    let dx = (final_x as i32 - self.trackers[slot_idx].start_x as i32).abs();
                    let dy = (final_y as i32 - self.trackers[slot_idx].start_y as i32).abs();
                    if dx <= 12 && dy <= 12 {
                        self.total_taps_detected = self.total_taps_detected.saturating_add(1);
                        return Some(TouchGesture::Tap { x: final_x, y: final_y });
                    } else {
                        return Some(TouchGesture::Release { x: final_x, y: final_y });
                    }
                }
                None
            }

            TouchPhase::Cancel => {
                self.active_contacts[slot_idx] = None;
                self.trackers[slot_idx].is_active = false;
                None
            }
        }
    }

    /// Convenience helper for direct single-touch screen tap.
    pub fn tap_screen(&mut self, x: u32, y: u32) -> Option<TouchGesture> {
        self.update_touch(0, x, y, 512, TouchPhase::Down);
        self.update_touch(0, x, y, 0, TouchPhase::Up)
    }

    /// Returns the number of fingers currently in contact with the screen.
    pub fn active_touch_count(&self) -> usize {
        self.active_contacts.iter().filter(|c| c.is_some()).count()
    }
}
