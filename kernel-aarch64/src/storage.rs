//! Mobile eMMC 5.1 & SDHCI Storage Host Controller Driver (AArch64)
//!
//! Conforms to SD Host Controller Standard Specification (v3.00 / v4.20)
//! driving non-volatile flash storage on physical smartphones:
//! - PinePhone (Allwinner A64 eMMC 5.1 & microSD)
//! - Qualcomm Snapdragon mobile platforms (SDHCI / eMMC / UFS bridge)
//! - Raspberry Pi Compute Module 4 (Broadcom eMMC)

use core::sync::atomic::{compiler_fence, Ordering};

pub const SECTOR_SIZE: usize = 512;

pub const SDHCI_BLOCK_SIZE: usize = 0x04;
pub const SDHCI_BLOCK_COUNT: usize = 0x06;
pub const SDHCI_ARGUMENT: usize = 0x08;
pub const SDHCI_TRANSFER_MODE: usize = 0x0C;
pub const SDHCI_COMMAND: usize = 0x0E;
pub const SDHCI_RESPONSE_0: usize = 0x10;
pub const SDHCI_BUFFER_PORT: usize = 0x20;
pub const SDHCI_PRESENT_STATE: usize = 0x24;
pub const SDHCI_SOFTWARE_RESET: usize = 0x2F;
pub const SDHCI_INT_STATUS: usize = 0x30;

pub const STATE_CMD_INHIBIT: u32 = 1 << 0;
pub const STATE_DAT_INHIBIT: u32 = 1 << 1;
pub const STATE_BUFFER_WRITE_EN: u32 = 1 << 10;
pub const STATE_BUFFER_READ_EN: u32 = 1 << 11;
pub const STATE_CARD_INSERTED: u32 = 1 << 16;

pub const INT_CMD_COMPLETE: u32 = 1 << 0;
pub const INT_TRANSFER_COMPLETE: u32 = 1 << 1;
pub const INT_ERROR: u32 = 1 << 15;
pub const INT_TIMEOUT_ERROR: u32 = 1 << 16;

pub const CMD0_GO_IDLE: u16 = 0x0000;
pub const CMD17_READ_SINGLE: u16 = (17 << 8) | 0x3A;
pub const CMD24_WRITE_SINGLE: u16 = (24 << 8) | 0x3A;

pub const TIMEOUT_CYCLES: u32 = 200_000;

#[inline(always)]
unsafe fn mmio_read32(base: usize, offset: usize) -> u32 {
    let ptr = (base + offset) as *const u32;
    compiler_fence(Ordering::SeqCst);
    let v = core::ptr::read_volatile(ptr);
    compiler_fence(Ordering::SeqCst);
    v
}

#[inline(always)]
unsafe fn mmio_write32(base: usize, offset: usize, val: u32) {
    let ptr = (base + offset) as *mut u32;
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile(ptr, val);
    compiler_fence(Ordering::SeqCst);
}

#[inline(always)]
unsafe fn mmio_write16(base: usize, offset: usize, val: u16) {
    let ptr = (base + offset) as *mut u16;
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile(ptr, val);
    compiler_fence(Ordering::SeqCst);
}

#[inline(always)]
unsafe fn mmio_write8(base: usize, offset: usize, val: u8) {
    let ptr = (base + offset) as *mut u8;
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile(ptr, val);
    compiler_fence(Ordering::SeqCst);
}

pub struct SdhciController {
    pub base_vaddr: usize,
    pub rca: u16,
    pub total_sectors: u32,
    pub high_capacity: bool,
    pub model_name: [u8; 16],
}

impl SdhciController {
    pub const fn new(base_vaddr: usize) -> Self {
        Self {
            base_vaddr,
            rca: 0,
            total_sectors: 0,
            high_capacity: false,
            model_name: *b"eMMC 5.1 Mobile ",
        }
    }

    pub fn reset(&self) -> Result<(), &'static str> {
        unsafe {
            mmio_write8(self.base_vaddr, SDHCI_SOFTWARE_RESET, 0x01);
            for _ in 0..TIMEOUT_CYCLES {
                let status = mmio_read32(self.base_vaddr, SDHCI_SOFTWARE_RESET) as u8;
                if (status & 0x01) == 0 {
                    return Ok(());
                }
            }
        }
        Err("SDHCI Reset Timeout")
    }

    pub fn is_card_inserted(&self) -> bool {
        unsafe {
            let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
            (state & STATE_CARD_INSERTED) != 0
        }
    }

    pub fn send_command(&self, cmd_idx: u16, arg: u32, is_data: bool) -> Result<u32, &'static str> {
        unsafe {
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                let mask = if is_data { STATE_CMD_INHIBIT | STATE_DAT_INHIBIT } else { STATE_CMD_INHIBIT };
                if (state & mask) == 0 {
                    break;
                }
            }

            let pending = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
            if (pending & INT_ERROR) != 0 {
                mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, pending & INT_ERROR);
            }

            mmio_write32(self.base_vaddr, SDHCI_ARGUMENT, arg);
            mmio_write16(self.base_vaddr, SDHCI_COMMAND, cmd_idx);

            let cur = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
            mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, (cur & !INT_ERROR) | INT_CMD_COMPLETE);

            for _ in 0..TIMEOUT_CYCLES {
                let int_stat = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
                if (int_stat & INT_ERROR) != 0 || (int_stat & INT_TIMEOUT_ERROR) != 0 {
                    return Err("SDHCI Device Error");
                }
                if (int_stat & INT_CMD_COMPLETE) != 0 {
                    let cur = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
                    mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, cur & !INT_CMD_COMPLETE);
                    let resp0 = mmio_read32(self.base_vaddr, SDHCI_RESPONSE_0);
                    return Ok(resp0);
                }
            }
        }
        Err("SDHCI Command Timeout")
    }

    pub fn read_block(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), &'static str> {
        let addr_arg = if self.high_capacity { lba } else { lba * 512 };

        unsafe {
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_SIZE, 512);
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_COUNT, 1);
            mmio_write16(self.base_vaddr, SDHCI_TRANSFER_MODE, 0x0012);

            self.send_command(CMD17_READ_SINGLE, addr_arg, true)?;

            let mut ready = false;
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                if (state & STATE_BUFFER_READ_EN) != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err("SDHCI Read Buffer Not Ready");
            }

            let u32_ptr = buf.as_mut_ptr() as *mut u32;
            for i in 0..128 {
                let dword = mmio_read32(self.base_vaddr, SDHCI_BUFFER_PORT);
                core::ptr::write(u32_ptr.add(i), dword);
            }

            let cur = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
            mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, cur & !INT_TRANSFER_COMPLETE);
        }

        Ok(())
    }

    pub fn write_block(&self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {
        let addr_arg = if self.high_capacity { lba } else { lba * 512 };

        unsafe {
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_SIZE, 512);
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_COUNT, 1);
            mmio_write16(self.base_vaddr, SDHCI_TRANSFER_MODE, 0x0002);

            self.send_command(CMD24_WRITE_SINGLE, addr_arg, true)?;

            let mut ready = false;
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                if (state & STATE_BUFFER_WRITE_EN) != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err("SDHCI Write Buffer Not Ready");
            }

            let u32_ptr = buf.as_ptr() as *const u32;
            for i in 0..128 {
                let dword = core::ptr::read(u32_ptr.add(i));
                mmio_write32(self.base_vaddr, SDHCI_BUFFER_PORT, dword);
            }

            let cur = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
            mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, cur & !INT_TRANSFER_COMPLETE);
        }

        Ok(())
    }
}
