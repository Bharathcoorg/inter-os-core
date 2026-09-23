//! Mobile Touch & Hardware Input Event Abstraction
//!
//! Native touch-first input event representation for smartphone digitizers.

pub use super::touch::{TouchGesture, TouchPhase, TouchPoint};

/// Generic unified hardware input event.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// Character key press or virtual keyboard entry.
    Key {
        char_code: char,
        pressed: bool,
    },
    /// Capacitive screen touch contact update (slot, position, pressure, phase).
    Touch(TouchPoint),
    /// Recognized high-level touch gesture (Tap, Drag, Swipe).
    Gesture(TouchGesture),
}
