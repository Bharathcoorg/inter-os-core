// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Mobile Capacitive Multi-Touch Digitizer Driver (Goodix GT911 / GT917D I2C Protocol)
//!
//! Standard mobile smartphone touch controller protocol over I2C bus.
//! Target hardware: PinePhone A64 (Goodix GT917D on Allwinner A64 TWI0 at `0x01c2ac00`).
//! Decodes up to 10 concurrent capacitive touch contacts, tracking IDs,
//! and coordinates with zero heap allocation.

pub const MAX_TOUCH_SLOTS: usize = 10;
pub const BYTES_PER_POINT: usize = 8;

pub const GT911_I2C_ADDR_PRIMARY: u8 = 0x5D;
pub const GT911_I2C_ADDR_SECONDARY: u8 = 0x14;

pub const GT917D_I2C_ADDR_PRIMARY: u8 = 0x5D;
pub const GT917D_I2C_ADDR_FALLBACK: u8 = 0x14;

pub const PINEPHONE_TOUCH_WIDTH: u32 = 720;
pub const PINEPHONE_TOUCH_HEIGHT: u32 = 1440;

pub const ALLWINNER_A64_TWI0_BASE: usize = 0x01c2ac00;

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

// ============================================================================
// Allwinner A64 TWI (Two-Wire Interface / I2C0) MMIO Driver
// ============================================================================

pub const TWI_ADDR: usize = 0x00;
pub const TWI_XADDR: usize = 0x04;
pub const TWI_DATA: usize = 0x08;
pub const TWI_CNTR: usize = 0x0C;
pub const TWI_STAT: usize = 0x10;
pub const TWI_CCR: usize = 0x14;
pub const TWI_SRST: usize = 0x18;

pub const TWI_CNTR_INTFLG: u32 = 1 << 3; // Clear by writing 1 or 0 depending on core
pub const TWI_CNTR_A_ACK: u32 = 1 << 2;
pub const TWI_CNTR_M_STP: u32 = 1 << 4;
pub const TWI_CNTR_M_STA: u32 = 1 << 5;
pub const TWI_CNTR_BUSEN: u32 = 1 << 6;

pub struct AllwinnerA64Twi {
    pub base_address: usize,
}

impl AllwinnerA64Twi {
    pub const fn new(base_address: usize) -> Self {
        Self { base_address }
    }

    pub fn new_twi0() -> Self {
        Self::new(ALLWINNER_A64_TWI0_BASE)
    }

    #[inline(always)]
    unsafe fn read_reg(&self, offset: usize) -> u32 {
        let ptr = (self.base_address + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    unsafe fn write_reg(&self, offset: usize, val: u32) {
        let ptr = (self.base_address + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    unsafe fn wait_status(&self) -> u32 {
        let mut spins = 0;
        while (self.read_reg(TWI_CNTR) & (1 << 3)) == 0 && spins < 100_000 {
            core::hint::spin_loop();
            spins += 1;
        }
        self.read_reg(TWI_STAT)
    }
}

impl I2cBusAdapter for AllwinnerA64Twi {
    fn read_reg16(&mut self, addr: u8, reg: u16, buf: &mut [u8]) -> Result<(), &'static str> {
        if self.base_address == 0 {
            return Err("Allwinner TWI: hardware unavailable");
        }
        unsafe {
            // 1. Send START condition
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | TWI_CNTR_M_STA | (1 << 3));
            let status = self.wait_status();
            if status != 0x08 && status != 0x10 {
                return Err("TWI START failed");
            }

            // 2. Transmit Slave Address (Write mode)
            self.write_reg(TWI_DATA, ((addr as u32) << 1) | 0);
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
            if self.wait_status() != 0x18 {
                return Err("TWI Addr Write ACK failed");
            }

            // 3. Transmit Register High Byte
            self.write_reg(TWI_DATA, (reg >> 8) as u32);
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
            if self.wait_status() != 0x28 {
                return Err("TWI Reg High ACK failed");
            }

            // 4. Transmit Register Low Byte
            self.write_reg(TWI_DATA, (reg & 0xFF) as u32);
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
            if self.wait_status() != 0x28 {
                return Err("TWI Reg Low ACK failed");
            }

            // 5. Send Repeated START condition
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | TWI_CNTR_M_STA | (1 << 3));
            let rep_status = self.wait_status();
            if rep_status != 0x10 && rep_status != 0x08 {
                return Err("TWI Repeated START failed");
            }

            // 6. Transmit Slave Address (Read mode)
            self.write_reg(TWI_DATA, ((addr as u32) << 1) | 1);
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
            if self.wait_status() != 0x40 {
                return Err("TWI Addr Read ACK failed");
            }

            // 7. Receive Data Bytes
            for i in 0..buf.len() {
                let ack_bit = if i + 1 < buf.len() { TWI_CNTR_A_ACK } else { 0 };
                self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | ack_bit | (1 << 3));
                let _ = self.wait_status();
                buf[i] = (self.read_reg(TWI_DATA) & 0xFF) as u8;
            }

            // 8. Send STOP condition
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | TWI_CNTR_M_STP | (1 << 3));
        }
        Ok(())
    }

    fn write_reg16(&mut self, addr: u8, reg: u16, data: &[u8]) -> Result<(), &'static str> {
        if self.base_address == 0 {
            return Err("Allwinner TWI: hardware unavailable");
        }
        unsafe {
            // 1. Send START
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | TWI_CNTR_M_STA | (1 << 3));
            if self.wait_status() != 0x08 {
                return Err("TWI START failed");
            }

            // 2. Transmit Slave Address (Write mode)
            self.write_reg(TWI_DATA, ((addr as u32) << 1) | 0);
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
            if self.wait_status() != 0x18 {
                return Err("TWI Addr ACK failed");
            }

            // 3. Register address
            self.write_reg(TWI_DATA, (reg >> 8) as u32);
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
            if self.wait_status() != 0x28 {
                return Err("TWI Reg High ACK failed");
            }

            self.write_reg(TWI_DATA, (reg & 0xFF) as u32);
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
            if self.wait_status() != 0x28 {
                return Err("TWI Reg Low ACK failed");
            }

            // 4. Data payload
            for &byte in data {
                self.write_reg(TWI_DATA, byte as u32);
                self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | (1 << 3));
                if self.wait_status() != 0x28 {
                    return Err("TWI Data ACK failed");
                }
            }

            // 5. STOP
            self.write_reg(TWI_CNTR, TWI_CNTR_BUSEN | TWI_CNTR_M_STP | (1 << 3));
        }
        Ok(())
    }
}

// ============================================================================
// Goodix Multi-Touch Controller Protocol Driver
// ============================================================================

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

    /// Convenience constructor for PinePhone A64 720x1440 panel.
    pub const fn new_pinephone(i2c_addr: u8) -> Self {
        Self::new(i2c_addr, PINEPHONE_TOUCH_WIDTH, PINEPHONE_TOUCH_HEIGHT)
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

    /// Probes the Goodix controller over I2C and verifies the 4-byte product ID.
    /// Supports GT911, GT917D (PinePhone A64), GT9271, and GT928.
    pub fn probe<B: I2cBusAdapter>(&self, bus: &mut B) -> Result<[u8; 4], &'static str> {
        let mut id = [0u8; 4];
        bus.read_reg16(self.i2c_addr, GT911_REG_PRODUCT_ID, &mut id)?;
        if &id == b"911\0"
            || &id == b"9110"
            || &id == b"917D"
            || &id == b"917\0"
            || &id == b"9271"
            || &id == b"928\0"
        {
            Ok(id)
        } else {
            Err("GT911/GT917D: Unrecognized product ID")
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
