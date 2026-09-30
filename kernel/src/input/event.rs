//! Unified hardware input event representation.
//!
//! Transport-specific details belong at the boundary. Consumers receive
//! explicit source and coordinate-space information so mouse, touch, keyboard,
//! QEMU, and Device Studio events follow one routing contract.

pub use super::coordinate::{CoordinateSpace, Point};
pub use super::touch::{TouchGesture, TouchPhase, TouchPoint};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputSource {
    Touch,
    Mouse,
    Keyboard,
    Qemu,
    DeviceStudio,
    Hardware,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PointerEvent {
    pub source: InputSource,
    pub space: CoordinateSpace,
    pub pointer_id: u8,
    pub phase: PointerPhase,
    pub position: Point,
    pub buttons: u16,
    pub wheel_x: i16,
    pub wheel_y: i16,
}

impl PointerEvent {
    pub const fn new(
        source: InputSource,
        space: CoordinateSpace,
        pointer_id: u8,
        phase: PointerPhase,
        x: i32,
        y: i32,
    ) -> Self {
        Self {
            source,
            space,
            pointer_id,
            phase,
            position: Point { x, y },
            buttons: 0,
            wheel_x: 0,
            wheel_y: 0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Down,
    Up,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeyCode {
    Character(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Unknown(u32),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub source: InputSource,
    pub action: KeyAction,
    pub code: KeyCode,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Generic unified hardware input event.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Key {
        char_code: char,
        pressed: bool,
    },
    KeyEvent(KeyEvent),
    Touch(TouchPoint),
    Pointer(PointerEvent),
    Gesture(TouchGesture),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_event_preserves_source_and_space() {
        let event = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::StudioDisplay,
            7,
            PointerPhase::Down,
            120,
            240,
        );
        assert_eq!(event.source, InputSource::DeviceStudio);
        assert_eq!(event.space, CoordinateSpace::StudioDisplay);
        assert_eq!(event.pointer_id, 7);
        assert_eq!(event.position, Point { x: 120, y: 240 });
    }

    #[test]
    fn keyboard_event_supports_navigation_keys() {
        let event = KeyEvent {
            source: InputSource::Keyboard,
            action: KeyAction::Down,
            code: KeyCode::ArrowLeft,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        };
        assert_eq!(event.code, KeyCode::ArrowLeft);
    }
}
