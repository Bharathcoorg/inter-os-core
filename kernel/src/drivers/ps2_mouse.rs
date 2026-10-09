//! QEMU/PC PS/2 mouse input for the canonical x86_64 smartphone target.
//!
//! This driver converts real emulated PS/2 packets into the same logical-portrait
//! pointer/gesture pipeline used by touch and Device Studio. It never fabricates
//! movement when the controller or mouse is unavailable.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use x86_64::instructions::port::Port;
use crate::input::TouchPhase;

const CONTROLLER_STATUS: u16 = 0x64;
const CONTROLLER_COMMAND: u16 = 0x64;
const DATA_PORT: u16 = 0x60;
const WAIT_LIMIT: usize = 100_000;
const MAX_X: i32 = 719;
const MAX_Y: i32 = 1439;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static PACKET_INDEX: AtomicU8 = AtomicU8::new(0);
static PACKET_0: AtomicU8 = AtomicU8::new(0);
static PACKET_1: AtomicU8 = AtomicU8::new(0);
static POINTER_X: AtomicI32 = AtomicI32::new(360);
static POINTER_Y: AtomicI32 = AtomicI32::new(720);
static LEFT_BUTTON_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MousePacket { dx: i32, dy: i32, left_down: bool, overflow: bool }

fn decode_packet(bytes: [u8; 3]) -> MousePacket {
    let flags = bytes[0];
    MousePacket {
        dx: bytes[1] as i8 as i32,
        dy: -(bytes[2] as i8 as i32),
        left_down: flags & 0x01 != 0,
        overflow: flags & 0xC0 != 0,
    }
}

fn wait_input_empty() -> bool {
    let mut status = Port::<u8>::new(CONTROLLER_STATUS);
    for _ in 0..WAIT_LIMIT {
        if unsafe { status.read() } & 0x02 == 0 { return true; }
        core::hint::spin_loop();
    }
    false
}

fn wait_output_full() -> Option<u8> {
    let mut status = Port::<u8>::new(CONTROLLER_STATUS);
    let mut data = Port::<u8>::new(DATA_PORT);
    for _ in 0..WAIT_LIMIT {
        let value = unsafe { status.read() };
        if value & 0x01 != 0 {
            let byte = unsafe { data.read() };
            // Controller responses to auxiliary commands must come from the mouse.
            if value & 0x20 != 0 { return Some(byte); }
        }
        core::hint::spin_loop();
    }
    None
}

fn controller_command(command: u8) -> bool {
    if !wait_input_empty() { return false; }
    let mut port = Port::<u8>::new(CONTROLLER_COMMAND);
    unsafe { port.write(command); }
    true
}

fn controller_data(value: u8) -> bool {
    if !wait_input_empty() { return false; }
    let mut port = Port::<u8>::new(DATA_PORT);
    unsafe { port.write(value); }
    true
}

fn mouse_command(command: u8) -> bool {
    if !controller_command(0xD4) || !controller_data(command) { return false; }
    wait_output_full() == Some(0xFA)
}

/// Initializes the emulated PS/2 auxiliary port and enables mouse packet reporting.
/// Returns false without stopping boot when a PS/2 controller is absent.
pub fn init() -> bool {
    if !controller_command(0xA8) || !controller_command(0x20) { return false; }
    let Some(mut command_byte) = wait_output_full() else { return false; };
    command_byte |= 0x02; // enable IRQ12
    command_byte &= !0x20; // enable auxiliary clock
    if !controller_command(0x60) || !controller_data(command_byte) { return false; }
    if !mouse_command(0xF6) || !mouse_command(0xF4) { return false; }
    PACKET_INDEX.store(0, Ordering::Release);
    POINTER_X.store(360, Ordering::Release);
    POINTER_Y.store(720, Ordering::Release);
    LEFT_BUTTON_DOWN.store(false, Ordering::Release);
    INITIALIZED.store(true, Ordering::Release);
    crate::serial_println!("[INPUT] QEMU PS/2 mouse reporting enabled; logical portrait pointer ready.");
    true
}

/// Called by IRQ12. The handler does no allocation and uses only bounded atomic work.
pub fn handle_irq_byte() {
    let mut data = Port::<u8>::new(DATA_PORT);
    let byte = unsafe { data.read() };
    if !INITIALIZED.load(Ordering::Acquire) { return; }
    let index = PACKET_INDEX.load(Ordering::Relaxed);
    match index {
        0 => {
            if byte & 0x08 == 0 { return; }
            PACKET_0.store(byte, Ordering::Relaxed);
            PACKET_INDEX.store(1, Ordering::Release);
        }
        1 => {
            PACKET_1.store(byte, Ordering::Relaxed);
            PACKET_INDEX.store(2, Ordering::Release);
        }
        _ => {
            PACKET_INDEX.store(0, Ordering::Release);
            let packet = decode_packet([PACKET_0.load(Ordering::Relaxed), PACKET_1.load(Ordering::Relaxed), byte]);
            let old_down = LEFT_BUTTON_DOWN.swap(packet.left_down, Ordering::AcqRel);
            if packet.overflow { return; }
            let x = (POINTER_X.load(Ordering::Relaxed) + packet.dx * 4).clamp(0, MAX_X);
            let y = (POINTER_Y.load(Ordering::Relaxed) + packet.dy * 4).clamp(0, MAX_Y);
            POINTER_X.store(x, Ordering::Release);
            POINTER_Y.store(y, Ordering::Release);
            if packet.left_down && !old_down {
                crate::input::post_mouse_event(x as u32, y as u32, TouchPhase::Down);
            } else if packet.left_down {
                crate::input::post_mouse_event(x as u32, y as u32, TouchPhase::Move);
            } else if old_down {
                crate::input::post_mouse_event(x as u32, y as u32, TouchPhase::Up);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_decode_preserves_signed_motion_and_button_state() {
        assert_eq!(decode_packet([0x09, 0xFE, 0x03]), MousePacket { dx: -2, dy: -3, left_down: true, overflow: false });
        assert!(decode_packet([0x48, 1, 1]).overflow);
    }

    #[test]
    fn logical_pointer_bounds_match_portrait_display() {
        assert_eq!((MAX_X + 1, MAX_Y + 1), (720, 1440));
    }
}
