//! Mobile Capacitive Multi-Touch Digitizer Driver (Goodix GT911 / I2C Protocol)
//!
//! Standard mobile smartphone touch controller protocol over I2C bus.
//! Decodes up to 5 concurrent capacitive touch contacts, tracking IDs,
//! pressure readings, and touch contact areas directly into the microkernel's InputEventQueue.

use crate::input::touch::{TouchPhase, TouchPoint, MAX_TOUCH_SLOTS};
use alloc::vec::Vec;

/// Standard Goodix GT911 I2C slave addresses (7-bit).
pub const GT911_I2C_ADDR_PRIMARY: u8 = 0x5D;
pub const GT911_I2C_ADDR_SECONDARY: u8 = 0x14;

/// Goodix GT911 Register Map (16-bit big-endian register addresses).
pub const GT911_REG_COMMAND: u16 = 0x8040;
pub const GT911_REG_CONFIG_VERSION: u16 = 0x8047;
pub const GT911_REG_PRODUCT_ID: u16 = 0x8140; // 4 bytes ASCII
pub const GT911_REG_TOUCH_STATUS: u16 = 0x814E; // Bit 7: ready, Bits 3-0: touch count
pub const GT911_REG_POINT1: u16 = 0x814F; // 8 bytes per touch point

/// Size in bytes of a single hardware touch point record in the I2C frame.
pub const BYTES_PER_POINT: usize = 8;

/// Trait representing an I2C bus adapter for mobile peripheral communication.
pub trait I2cBusAdapter {
    /// Reads `len` bytes from a 16-bit register address of a 7-bit slave device.
    fn read_reg16(&mut self, addr: u8, reg: u16, buf: &mut [u8]) -> Result<(), &'static str>;
    /// Writes `len` bytes to a 16-bit register address of a 7-bit slave device.
    fn write_reg16(&mut self, addr: u8, reg: u16, data: &[u8]) -> Result<(), &'static str>;
}

/// Goodix GT911 Mobile Touchscreen Driver.
pub struct GoodixGt911Driver {
    i2c_addr: u8,
    active_slots: [bool; MAX_TOUCH_SLOTS],
    last_coords: [(u32, u32); MAX_TOUCH_SLOTS],
    touch_width: u32,
    touch_height: u32,
}

impl GoodixGt911Driver {
    pub fn new(i2c_addr: u8, touch_width: u32, touch_height: u32) -> Self {
        Self {
            i2c_addr,
            active_slots: [false; MAX_TOUCH_SLOTS],
            last_coords: [(0, 0); MAX_TOUCH_SLOTS],
            touch_width,
            touch_height,
        }
    }


    /// Verifies hardware communication and retrieves product identification.
    pub fn probe<B: I2cBusAdapter>(&self, bus: &mut B) -> Result<[u8; 4], &'static str> {
        let mut id = [0u8; 4];
        bus.read_reg16(self.i2c_addr, GT911_REG_PRODUCT_ID, &mut id)?;
        if &id == b"911\0" || &id == b"9110" || &id == b"9271" || &id == b"928\0" {
            Ok(id)
        } else {
            Err("GT911: Unrecognized product ID")
        }
    }

    /// Parses a raw touch frame buffer read from register `0x814E`.
    /// Decodes coordinates and updates internal slot state machine.
    pub fn parse_frame(&mut self, frame_data: &[u8]) -> Result<Vec<TouchPoint>, &'static str> {
        if frame_data.is_empty() {
            return Err("GT911: Empty touch frame buffer");
        }

        let status = frame_data[0];
        let is_ready = (status & 0x80) != 0;
        if !is_ready {
            return Ok(Vec::new()); // No new touch data ready
        }

        let touch_count = (status & 0x0F) as usize;
        let mut points = Vec::new();
        let mut current_frame_slots = [false; MAX_TOUCH_SLOTS];

        if touch_count > 0 {
            let max_points = touch_count.min(MAX_TOUCH_SLOTS);
            for i in 0..max_points {
                let offset = 1 + (i * BYTES_PER_POINT);
                if offset + BYTES_PER_POINT > frame_data.len() {
                    break;
                }

                let track_id = frame_data[offset];
                let slot = (track_id as usize) % MAX_TOUCH_SLOTS;

                let raw_x = u16::from_le_bytes([frame_data[offset + 1], frame_data[offset + 2]]) as u32;
                let raw_y = u16::from_le_bytes([frame_data[offset + 3], frame_data[offset + 4]]) as u32;
                let raw_size = u16::from_le_bytes([frame_data[offset + 5], frame_data[offset + 6]]);

                // Clamp coordinates to screen dimensions
                let x = raw_x.min(self.touch_width.saturating_sub(1));
                let y = raw_y.min(self.touch_height.saturating_sub(1));
                let pressure = if raw_size > 0 { raw_size } else { 512 };

                let phase = if !self.active_slots[slot] {
                    TouchPhase::Down
                } else {
                    TouchPhase::Move
                };

                self.active_slots[slot] = true;
                self.last_coords[slot] = (x, y);
                current_frame_slots[slot] = true;

                points.push(TouchPoint {
                    slot: slot as u8,
                    x,
                    y,
                    pressure,
                    phase,
                });
            }
        }

        // Check for lifted fingers (slots active previously but missing in current frame)
        for slot in 0..MAX_TOUCH_SLOTS {
            if self.active_slots[slot] && !current_frame_slots[slot] {
                self.active_slots[slot] = false;
                let (last_x, last_y) = self.last_coords[slot];
                points.push(TouchPoint {
                    slot: slot as u8,
                    x: last_x,
                    y: last_y,
                    pressure: 0,
                    phase: TouchPhase::Up,
                });
            }
        }


        Ok(points)
    }

    /// Emits decoded touch points into the kernel's global InputEventQueue.
    pub fn dispatch_to_input_queue(&self, points: &[TouchPoint]) {
        for pt in points {
            crate::input::post_touch_event(pt.slot, pt.x, pt.y, pt.pressure, pt.phase);
        }
    }

    /// Reads raw touch data directly from the physical I2C bus adapter, clears the
    /// hardware buffer ready flag, decodes multi-touch contacts, and dispatches them
    /// into the kernel's sovereign input subsystem.
    pub fn poll_and_dispatch<B: I2cBusAdapter>(&mut self, bus: &mut B) -> Result<usize, &'static str> {
        // 1. Read the touch status register (0x814E)
        let mut status_buf = [0u8; 1];
        bus.read_reg16(self.i2c_addr, GT911_REG_TOUCH_STATUS, &mut status_buf)?;

        let status = status_buf[0];
        let is_ready = (status & 0x80) != 0;
        if !is_ready {
            return Ok(0); // Hardware frame not ready yet
        }

        let touch_count = (status & 0x0F) as usize;
        let points_to_read = touch_count.min(MAX_TOUCH_SLOTS);
        let frame_len = 1 + (points_to_read * BYTES_PER_POINT);

        // 2. Read full touch status and coordinate frame from hardware
        let mut frame_buf = [0u8; 1 + (MAX_TOUCH_SLOTS * BYTES_PER_POINT)];
        bus.read_reg16(self.i2c_addr, GT911_REG_TOUCH_STATUS, &mut frame_buf[..frame_len])?;

        // 3. Clear hardware ready flag (Write 0 to 0x814E as required by GT911 spec)
        let zero_flag = [0u8; 1];
        bus.write_reg16(self.i2c_addr, GT911_REG_TOUCH_STATUS, &zero_flag)?;

        // 4. Parse frame coordinates and track slots
        let points = self.parse_frame(&frame_buf[..frame_len])?;
        let count = points.len();

        // 5. Dispatch decoded contacts directly into kernel input subsystem
        self.dispatch_to_input_queue(&points);

        Ok(count)
    }
}

/// In-memory I2C bus mock adapter used for deterministic hardware self-testing.
pub struct MemoryI2cAdapter {
    product_id: [u8; 4],
    touch_status_buffer: Vec<u8>,
}

impl MemoryI2cAdapter {
    pub fn new() -> Self {
        Self {
            product_id: *b"911\0",
            touch_status_buffer: Vec::new(),
        }
    }

    pub fn set_touch_buffer(&mut self, data: &[u8]) {
        self.touch_status_buffer = data.to_vec();
    }
}

impl I2cBusAdapter for MemoryI2cAdapter {
    fn read_reg16(&mut self, _addr: u8, reg: u16, buf: &mut [u8]) -> Result<(), &'static str> {
        match reg {
            GT911_REG_PRODUCT_ID => {
                let len = buf.len().min(4);
                buf[..len].copy_from_slice(&self.product_id[..len]);
                Ok(())
            }
            GT911_REG_TOUCH_STATUS => {
                let len = buf.len().min(self.touch_status_buffer.len());
                buf[..len].copy_from_slice(&self.touch_status_buffer[..len]);
                Ok(())
            }
            _ => Err("MemoryI2c: Unmapped register"),
        }
    }

    fn write_reg16(&mut self, _addr: u8, reg: u16, data: &[u8]) -> Result<(), &'static str> {
        if reg == GT911_REG_TOUCH_STATUS && !data.is_empty() && data[0] == 0 {
            // Host cleared touch buffer flag
            if !self.touch_status_buffer.is_empty() {
                self.touch_status_buffer[0] = 0; // Clear buffer ready bit
            }
            Ok(())
        } else {
            Ok(())
        }
    }
}

/// End-to-end verification of Goodix GT911 I2C driver pipeline, hardware status handshake,
/// and touch gesture routing to Desktop Shell.
pub fn test_gt911_pipeline() {
    use crate::framebuffer::{PHONE_X, PHONE_Y};
    use crate::input::{InputEvent, TouchGesture};

    crate::serial_println!("--- [Testing Goodix GT911 Touch Digitizer Hardware Pipeline] ---");

    let mut adapter = MemoryI2cAdapter::new();
    let mut gt911 = GoodixGt911Driver::new(GT911_I2C_ADDR_PRIMARY, 1080, 2400);

    // 1. Probe hardware
    let id = gt911.probe(&mut adapter).expect("GT911 probe failed");
    assert_eq!(&id, b"911\0");

    // 2. Synthesize contact down at Card 4 (Settings App: PHONE_X + 50, PHONE_Y + 430)
    let card4_x = (PHONE_X + 50) as u16;
    let card4_y = (PHONE_Y + 430) as u16;

    let mut down_frame = [0u8; 1 + BYTES_PER_POINT];
    down_frame[0] = 0x81; // Ready bit (0x80) | 1 touch point (0x01)
    down_frame[1] = 0;    // Track ID 0 (slot 0)
    down_frame[2] = (card4_x & 0xFF) as u8;
    down_frame[3] = ((card4_x >> 8) & 0xFF) as u8;
    down_frame[4] = (card4_y & 0xFF) as u8;
    down_frame[5] = ((card4_y >> 8) & 0xFF) as u8;
    down_frame[6] = 50;   // Pressure / contact size
    down_frame[7] = 0;

    adapter.set_touch_buffer(&down_frame);

    // 3. Poll hardware and verify status register cleared
    let count = gt911.poll_and_dispatch(&mut adapter).expect("Poll failed");
    assert_eq!(count, 1);
    assert_eq!(adapter.touch_status_buffer[0], 0x00, "GT911 status register not cleared after frame read");

    // 4. Synthesize finger release frame (Ready bit set, 0 active touches)
    let up_frame = [0x80u8];
    adapter.set_touch_buffer(&up_frame);

    let count_up = gt911.poll_and_dispatch(&mut adapter).expect("Release poll failed");
    assert_eq!(count_up, 1); // 1 point released (Phase::Up)

    // 5. Verify that the gesture engine recognized Tap and pushed to INPUT_QUEUE
    let mut found_tap = false;
    let mut q = crate::input::INPUT_QUEUE.lock();
    while let Some(event) = q.pop() {
        if let InputEvent::Gesture(TouchGesture::Tap { x, y }) = event {
            assert_eq!(x, card4_x as u32);
            assert_eq!(y, card4_y as u32);
            found_tap = true;

            // Route gesture directly to mobile shell
            let intent = crate::gui::MOBILE_SHELL.lock().handle_event(event);
            assert_eq!(intent, Some("APP_LAUNCH_SETTINGS"));
            break;
        }
    }
    drop(q);

    assert!(found_tap, "Expected Tap gesture in input queue from GT911 hardware dispatch");
    crate::serial_println!(
        "[TEST] Goodix GT911 Hardware Pipeline: I2C status handshake, Down/Up cycle, and Mobile Shell dispatch ('APP_LAUNCH_SETTINGS') VERIFIED."
    );
}

