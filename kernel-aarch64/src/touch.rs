//! Mobile Capacitive Multi-Touch Digitizer Driver (Goodix GT911 / I2C Protocol)
//!
//! Standard mobile smartphone touch controller protocol over I2C bus.
//! Decodes up to 5 concurrent capacitive touch contacts, tracking IDs,
//! and coordinates with zero heap allocation.

pub const MAX_TOUCH_SLOTS: usize = 5;
pub const BYTES_PER_POINT: usize = 8;

pub const GT911_I2C_ADDR_PRIMARY: u8 = 0x5D;
pub const GT911_REG_PRODUCT_ID: u16 = 0x8140;
pub const GT911_REG_TOUCH_STATUS: u16 = 0x814E;
pub const GT911_REG_POINT1: u16 = 0x814F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy)]
pub struct TouchPoint {
    pub slot: u8,
    pub x: u32,
    pub y: u32,
    pub pressure: u16,
    pub phase: TouchPhase,
}

pub trait I2cBusAdapter {
    fn read_reg16(&mut self, addr: u8, reg: u16, buf: &mut [u8]) -> Result<(), &'static str>;
    fn write_reg16(&mut self, addr: u8, reg: u16, data: &[u8]) -> Result<(), &'static str>;
}

pub struct GoodixGt911Driver {
    pub i2c_addr: u8,
    pub active_slots: [bool; MAX_TOUCH_SLOTS],
    pub last_coords: [(u32, u32); MAX_TOUCH_SLOTS],
    pub touch_width: u32,
    pub touch_height: u32,
    /// False until a real I2C digitizer has been probed and bound. No contact
    /// may be reported as observed hardware while this is false.
    pub hardware_present: bool,
}

impl GoodixGt911Driver {
    pub const fn new(i2c_addr: u8, touch_width: u32, touch_height: u32) -> Self {
        Self {
            i2c_addr,
            active_slots: [false; MAX_TOUCH_SLOTS],
            last_coords: [(0, 0); MAX_TOUCH_SLOTS],
            touch_width,
            touch_height,
            hardware_present: false,
        }
    }

    /// Binds the driver to a digitizer that answered on a real I2C bus.
    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        if !present {
            self.active_slots = [false; MAX_TOUCH_SLOTS];
            self.last_coords = [(0, 0); MAX_TOUCH_SLOTS];
        }
    }

    /// Reads a contact frame from the physical digitizer.
    ///
    /// Fails closed when no digitizer is bound: a frame may only enter the
    /// system from a controller that actually responded on the bus.
    pub fn read_contact_frame<B: I2cBusAdapter>(
        &mut self,
        bus: &mut B,
        buf: &mut [u8],
    ) -> Result<usize, &'static str> {
        if !self.hardware_present {
            return Err("GT911: digitizer hardware unavailable");
        }
        if buf.len() < 1 + 2 * BYTES_PER_POINT {
            return Err("GT911: frame buffer too small");
        }
        bus.read_reg16(self.i2c_addr, GT911_REG_TOUCH_STATUS, buf)?;
        Ok(buf.len())
    }

    pub fn probe<B: I2cBusAdapter>(&self, bus: &mut B) -> Result<[u8; 4], &'static str> {
        let mut id = [0u8; 4];
        bus.read_reg16(self.i2c_addr, GT911_REG_PRODUCT_ID, &mut id)?;
        if &id == b"911\0" || &id == b"9110" || &id == b"9271" || &id == b"928\0" {
            Ok(id)
        } else {
            Err("GT911: Unrecognized product ID")
        }
    }

    pub fn parse_frame(&mut self, frame_data: &[u8], out_points: &mut [TouchPoint; MAX_TOUCH_SLOTS]) -> usize {
        if frame_data.is_empty() {
            return 0;
        }

        let status = frame_data[0];
        let is_ready = (status & 0x80) != 0;
        if !is_ready {
            return 0;
        }

        let touch_count = (status & 0x0F) as usize;
        let mut count = 0;
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

                if count < MAX_TOUCH_SLOTS {
                    out_points[count] = TouchPoint {
                        slot: slot as u8,
                        x,
                        y,
                        pressure,
                        phase,
                    };
                    count += 1;
                }
            }
        }

        for slot in 0..MAX_TOUCH_SLOTS {
            if self.active_slots[slot] && !current_frame_slots[slot] {
                self.active_slots[slot] = false;
                let (last_x, last_y) = self.last_coords[slot];
                if count < MAX_TOUCH_SLOTS {
                    out_points[count] = TouchPoint {
                        slot: slot as u8,
                        x: last_x,
                        y: last_y,
                        pressure: 0,
                        phase: TouchPhase::Up,
                    };
                    count += 1;
                }
            }
        }

        count
    }
}
