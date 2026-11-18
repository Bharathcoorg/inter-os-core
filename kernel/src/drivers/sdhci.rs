// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Mobile eMMC 5.1 & SDHCI Storage Host Controller Driver
//!
//! Conforms to the SD Host Controller Standard Specification (v3.00 / v4.20)
//! driving non-volatile flash storage on physical smartphones:
//! - PinePhone (Allwinner A64 eMMC 5.1 & microSD)
//! - Qualcomm Snapdragon mobile platforms (SDHCI / eMMC / UFS bridge)
//! - Raspberry Pi Compute Module 4 (Broadcom eMMC)
//!
//! Provides bare-metal register-level block reads and writes (512-byte sectors)
//! conforming to the microkernel's `BlockDevice` interface.

use super::ata::AtaError;
use crate::storage::block::{BlockDevice, BlockDeviceError, SECTOR_SIZE};
use core::sync::atomic::{compiler_fence, Ordering};

// -----------------------------------------------------------------------------
// SDHCI Standard MMIO Register Offsets
// -----------------------------------------------------------------------------

pub const SDHCI_SDMA_ARG2: usize = 0x00;        // SDMA System Address / Argument 2
pub const SDHCI_BLOCK_SIZE: usize = 0x04;       // Transfer Block Size
pub const SDHCI_BLOCK_COUNT: usize = 0x06;      // 16-bit Block Count
pub const SDHCI_ARGUMENT: usize = 0x08;        // 32-bit Command Argument
pub const SDHCI_TRANSFER_MODE: usize = 0x0C;   // Transfer Mode Register
pub const SDHCI_COMMAND: usize = 0x0E;         // 16-bit Command Register
pub const SDHCI_RESPONSE_0: usize = 0x10;      // Response Register 0 (Bits 31:0)
pub const SDHCI_RESPONSE_1: usize = 0x14;      // Response Register 1 (Bits 63:32)
pub const SDHCI_RESPONSE_2: usize = 0x18;      // Response Register 2 (Bits 95:64)
pub const SDHCI_RESPONSE_3: usize = 0x1C;      // Response Register 3 (Bits 127:96)
pub const SDHCI_BUFFER_PORT: usize = 0x20;     // 32-bit Buffer Data Port (FIFO)
pub const SDHCI_PRESENT_STATE: usize = 0x24;   // Present State Flags
pub const SDHCI_HOST_CONTROL: usize = 0x28;    // Host Control 1 (Bus width, High speed)
pub const SDHCI_POWER_CONTROL: usize = 0x29;   // Power Control (Bus power on, 3.3V/1.8V)
pub const SDHCI_CLOCK_CONTROL: usize = 0x2C;   // Clock Control (Internal clock, Divisor)
pub const SDHCI_TIMEOUT_CONTROL: usize = 0x2E; // Timeout Control
pub const SDHCI_SOFTWARE_RESET: usize = 0x2F;  // Software Reset Register
pub const SDHCI_INT_STATUS: usize = 0x30;      // Normal & Error Interrupt Status
pub const SDHCI_INT_ENABLE: usize = 0x34;      // Interrupt Status Enable
pub const SDHCI_SIGNAL_ENABLE: usize = 0x38;   // Interrupt Signal Enable
pub const SDHCI_HOST_VERSION: usize = 0xFE;    // Host Controller Version

// -----------------------------------------------------------------------------
// Present State Bitfield Flags
// -----------------------------------------------------------------------------

pub const STATE_CMD_INHIBIT: u32 = 1 << 0;     // Command Line Active (Wait for CMD complete)
pub const STATE_DAT_INHIBIT: u32 = 1 << 1;     // Data Line Active (Wait for DAT complete)
pub const STATE_DAT_ACTIVE: u32 = 1 << 2;      // Data Line Active
pub const STATE_WRITE_TRANSFER: u32 = 1 << 8;  // Write Transfer Active
pub const STATE_READ_TRANSFER: u32 = 1 << 9;   // Read Transfer Active
pub const STATE_BUFFER_WRITE_EN: u32 = 1 << 10;// Buffer Write Enable (Space in TX FIFO)
pub const STATE_BUFFER_READ_EN: u32 = 1 << 11; // Buffer Read Enable (Data in RX FIFO)
pub const STATE_CARD_INSERTED: u32 = 1 << 16;  // Card Inserted Debounced State
pub const STATE_CARD_STABLE: u32 = 1 << 17;    // Card State Stable

// -----------------------------------------------------------------------------
// Interrupt Status Flags
// -----------------------------------------------------------------------------

pub const INT_CMD_COMPLETE: u32 = 1 << 0;      // Command Complete
pub const INT_TRANSFER_COMPLETE: u32 = 1 << 1; // Transfer Complete
pub const INT_BLOCK_GAP: u32 = 1 << 2;         // Block Gap Event
pub const INT_DMA: u32 = 1 << 3;               // DMA Interrupt
pub const INT_BUF_WRITE_READY: u32 = 1 << 4;   // Buffer Write Ready
pub const INT_BUF_READ_READY: u32 = 1 << 5;    // Buffer Read Ready
pub const INT_CARD_INSERTION: u32 = 1 << 6;    // Card Inserted
pub const INT_CARD_REMOVAL: u32 = 1 << 7;      // Card Removed
pub const INT_ERROR: u32 = 1 << 15;            // Error Interrupt Flag
pub const INT_TIMEOUT_ERROR: u32 = 1 << 16;    // Command Timeout Error

// -----------------------------------------------------------------------------
// Standard SD / eMMC Commands
// -----------------------------------------------------------------------------

pub const CMD0_GO_IDLE: u16 = 0x0000;
pub const CMD2_ALL_SEND_CID: u16 = (2 << 8) | 0x09; // R2 (136-bit)
pub const CMD3_SEND_RELATIVE_ADDR: u16 = (3 << 8) | 0x1A; // R6 (48-bit with CRC)
pub const CMD7_SELECT_CARD: u16 = (7 << 8) | 0x1B; // R1b (48-bit with busy)
pub const CMD8_SEND_IF_COND: u16 = (8 << 8) | 0x1A; // R7 (48-bit)
pub const CMD9_SEND_CSD: u16 = (9 << 8) | 0x09; // R2 (136-bit)
pub const CMD16_SET_BLOCKLEN: u16 = (16 << 8) | 0x1A; // R1
pub const CMD17_READ_SINGLE: u16 = (17 << 8) | 0x3A; // R1 with Data Read
pub const CMD24_WRITE_SINGLE: u16 = (24 << 8) | 0x3A; // R1 with Data Write
pub const CMD55_APP_CMD: u16 = (55 << 8) | 0x1A; // R1
pub const ACMD41_SD_SEND_OP_COND: u16 = (41 << 8) | 0x02; // R3 (48-bit without CRC)

pub const TIMEOUT_CYCLES: u32 = 200_000;

// -----------------------------------------------------------------------------
// MMIO Volatile Helpers
// -----------------------------------------------------------------------------

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

#[allow(dead_code)]
#[inline(always)]
unsafe fn mmio_read16(base: usize, offset: usize) -> u16 {
    let ptr = (base + offset) as *const u16;
    compiler_fence(Ordering::SeqCst);
    let v = core::ptr::read_volatile(ptr);
    compiler_fence(Ordering::SeqCst);
    v
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

// -----------------------------------------------------------------------------
// SDHCI Controller
// -----------------------------------------------------------------------------

/// Bare-metal SDHCI Mobile Flash Storage Controller (eMMC / SD).
pub struct SdhciController {
    /// Virtual base address of the MMIO registers mapped into kernel address space.
    pub base_vaddr: usize,
    /// Relative Card Address (RCA) assigned during enumeration.
    pub rca: u16,
    /// Total 512-byte addressable sector count.
    pub total_sectors: u32,
    /// High Capacity (SDHC / SDXC / eMMC 5.1) block addressing flag.
    pub high_capacity: bool,
    /// Model name string.
    pub model_name: [u8; 16],
    /// Loopback register-model mode for in-kernel self-tests ONLY.
    ///
    /// WHY this flag exists: the self-test rig is plain memory with no
    /// controller logic behind it, so nothing ever raises Command Complete.
    /// When (and only when) this flag is set, `send_command` synthesizes the
    /// completion bit in the rig. On real silicon the controller raises it,
    /// and this flag stays false (the `new` default), so software never
    /// fabricates an interrupt status on hardware.
    pub loopback: bool,
}

impl SdhciController {
    /// Instantiates a new SDHCI controller handle.
    pub const fn new(base_vaddr: usize) -> Self {
        Self {
            base_vaddr,
            rca: 0,
            total_sectors: 0,
            high_capacity: false,
            model_name: *b"eMMC 5.1 Mobile ",
            loopback: false,
        }
    }

    /// Clears exactly the given interrupt-status bits using W1C semantics.
    ///
    /// WHY write only `bits`: SDHCI interrupt status is write-1-to-clear.
    /// Writing a 1 clears that bit; writing a 0 leaves it untouched. The old
    /// `cur & !bits` pattern did the opposite of what it intended: it wrote 0
    /// to the target bit (never clearing it) and 1 to every *other* set bit,
    /// silently acknowledging unrelated interrupts. The correct clear is a
    /// write of exactly the bits being acknowledged.
    fn clear_interrupts(&self, bits: u32) {
        unsafe {
            mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, bits);
        }
    }

    /// Performs software reset of the host controller lines.
    pub fn reset(&self) -> Result<(), AtaError> {
        unsafe {
            // Software reset for ALL (0x01)
            mmio_write8(self.base_vaddr, SDHCI_SOFTWARE_RESET, 0x01);
            for _ in 0..TIMEOUT_CYCLES {
                let status = mmio_read32(self.base_vaddr, SDHCI_SOFTWARE_RESET) as u8;
                if (status & 0x01) == 0 {
                    return Ok(());
                }
            }
        }
        Err(AtaError::Timeout)
    }

    /// Checks whether physical media is inserted in the slot.
    pub fn is_card_inserted(&self) -> bool {
        unsafe {
            let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
            (state & STATE_CARD_INSERTED) != 0
        }
    }

    /// Sends a raw command to the card and waits for response.
    pub fn send_command(&self, cmd_idx: u16, arg: u32, is_data: bool) -> Result<u32, AtaError> {
        unsafe {
            // Wait while command inhibit bit is active
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                let mask = if is_data { STATE_CMD_INHIBIT | STATE_DAT_INHIBIT } else { STATE_CMD_INHIBIT };
                if (state & mask) == 0 {
                    break;
                }
            }

            // Clear pending error interrupts (W1C semantics: only clear active error bits)
            let pending = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
            if (pending & INT_ERROR) != 0 {
                self.clear_interrupts(pending & INT_ERROR);
            }

            // Write argument and command
            mmio_write32(self.base_vaddr, SDHCI_ARGUMENT, arg);
            mmio_write16(self.base_vaddr, SDHCI_COMMAND, cmd_idx);

            // Loopback-only completion synthesis. WHY gated: on real silicon
            // the controller raises INT_CMD_COMPLETE itself; software writing
            // it would fabricate a command result. The plain-memory self-test
            // rig has no controller logic, so only when `loopback` is explicitly
            // set (never the default) do we raise the bit for the rig.
            if self.loopback {
                mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, INT_CMD_COMPLETE);
            }

            // Wait for command complete
            for _ in 0..TIMEOUT_CYCLES {
                let int_stat = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
                if (int_stat & INT_ERROR) != 0 || (int_stat & INT_TIMEOUT_ERROR) != 0 {
                    return Err(AtaError::DeviceFault);
                }
                if (int_stat & INT_CMD_COMPLETE) != 0 {
                    // W1C semantics: clear only the CMD_COMPLETE bit
                    self.clear_interrupts(INT_CMD_COMPLETE);
                    let resp0 = mmio_read32(self.base_vaddr, SDHCI_RESPONSE_0);
                    return Ok(resp0);
                }

            }
        }
        Err(AtaError::Timeout)
    }

    /// Reads a 512-byte sector at the specified LBA block address using PIO Buffer Port.
    pub fn read_block(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), AtaError> {
        // WHY bounds before any MMIO: an out-of-range LBA must be refused
        // without touching the controller at all. Same convention as the ATA
        // driver: a zero sector count means "capacity unknown", so only a
        // known capacity enforces the range.
        if self.total_sectors > 0 && lba >= self.total_sectors {
            return Err(AtaError::LbaOutOfRange);
        }
        // WHY checked_mul: byte-addressed (non-high-capacity) cards take
        // lba * 512 as a u32 argument; lba >= 2^23 wraps and would silently
        // read the wrong sector. Refuse instead of wrapping.
        let addr_arg = if self.high_capacity {
            lba
        } else {
            lba.checked_mul(SECTOR_SIZE as u32).ok_or(AtaError::LbaOutOfRange)?
        };

        unsafe {
            // 1. Set block size to 512 bytes and block count to 1
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_SIZE, 512);
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_COUNT, 1);

            // 2. Transfer Mode: Read direction (bit 4 = 1), Block Count Enable (bit 1 = 1)
            mmio_write16(self.base_vaddr, SDHCI_TRANSFER_MODE, 0x0012);

            // 3. Issue CMD17 READ_SINGLE_BLOCK
            self.send_command(CMD17_READ_SINGLE, addr_arg, true)?;

            // 4. Wait for Buffer Read Ready
            let mut ready = false;
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                if (state & STATE_BUFFER_READ_EN) != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err(AtaError::ReadError);
            }

            // 5. Read 128 x 32-bit words (512 bytes) from FIFO.
            // WHY write_unaligned: the destination is a byte buffer with
            // alignment 1; casting it to *mut u32 and dereferencing is UB
            // whenever the buffer is not 4-byte aligned. The MMIO port read
            // stays volatile; only the buffer store is unaligned-safe.
            for i in 0..128 {
                let dword = mmio_read32(self.base_vaddr, SDHCI_BUFFER_PORT);
                core::ptr::write_unaligned(buf.as_mut_ptr().add(i * 4) as *mut u32, dword);
            }

            // 6. Clear Transfer Complete interrupt (W1C: only that bit)
            self.clear_interrupts(INT_TRANSFER_COMPLETE);
        }

        Ok(())
    }

    /// Writes a 512-byte sector at the specified LBA block address using PIO Buffer Port.
    pub fn write_block(&self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), AtaError> {
        // Same pre-MMIO refusal convention as read_block (see WHY there).
        if self.total_sectors > 0 && lba >= self.total_sectors {
            return Err(AtaError::LbaOutOfRange);
        }
        let addr_arg = if self.high_capacity {
            lba
        } else {
            lba.checked_mul(SECTOR_SIZE as u32).ok_or(AtaError::LbaOutOfRange)?
        };

        unsafe {
            // 1. Set block size to 512 bytes and block count to 1
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_SIZE, 512);
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_COUNT, 1);

            // 2. Transfer Mode: Write direction (bit 4 = 0), Block Count Enable (bit 1 = 1)
            mmio_write16(self.base_vaddr, SDHCI_TRANSFER_MODE, 0x0002);

            // 3. Issue CMD24 WRITE_SINGLE_BLOCK
            self.send_command(CMD24_WRITE_SINGLE, addr_arg, true)?;

            // 4. Wait for Buffer Write Ready
            let mut ready = false;
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                if (state & STATE_BUFFER_WRITE_EN) != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err(AtaError::WriteError);
            }

            // 5. Write 128 x 32-bit words (512 bytes) into FIFO.
            // WHY read_unaligned: the source is a byte buffer with alignment
            // 1; a *const u32 dereference is UB when it is not 4-byte aligned.
            for i in 0..128 {
                let dword = core::ptr::read_unaligned(buf.as_ptr().add(i * 4) as *const u32);
                mmio_write32(self.base_vaddr, SDHCI_BUFFER_PORT, dword);
            }

            // 6. Clear Transfer Complete interrupt (W1C: only that bit)
            self.clear_interrupts(INT_TRANSFER_COMPLETE);

        }

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// BlockDevice Trait Implementation
// -----------------------------------------------------------------------------

impl BlockDevice for SdhciController {
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
        self.read_block(lba, buf).map_err(|e| match e { AtaError::DeviceNotFound => BlockDeviceError::DeviceNotFound, AtaError::Timeout => BlockDeviceError::Timeout, AtaError::LbaOutOfRange => BlockDeviceError::LbaOutOfRange, _ => BlockDeviceError::DeviceFault })
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
        self.write_block(lba, buf).map_err(|e| match e { AtaError::DeviceNotFound => BlockDeviceError::DeviceNotFound, AtaError::Timeout => BlockDeviceError::Timeout, AtaError::LbaOutOfRange => BlockDeviceError::LbaOutOfRange, _ => BlockDeviceError::DeviceFault })
    }

    fn sector_count(&self) -> u32 {
        self.total_sectors
    }

    fn model(&self) -> &str {
        core::str::from_utf8(&self.model_name).unwrap_or("eMMC 5.1 Mobile")
    }
}

// -----------------------------------------------------------------------------
// Self-Test & Hardware Verification Rig
// -----------------------------------------------------------------------------

/// Verified self-test suite for the mobile SDHCI / eMMC storage controller driver.
///
/// Verification level: LOOPBACK REGISTER MODEL ONLY. The "controller" here is
/// a 256-byte plain-memory array, not physical silicon and not a QEMU device.
/// Passing this test exercises driver register logic; it is not a hardware
/// verification claim.
pub fn test_sdhci_controller() {
    crate::serial_println!("--- [Testing Mobile SDHCI / eMMC 5.1 Storage Driver: loopback register model only] ---");

    // Allocate an emulated 256-byte MMIO register window on the stack
    let mut mmio_space = [0u32; 64]; // 256 bytes

    // Initialize hardware register defaults
    // Present state: Card Inserted (bit 16), Buffer Read Ready (bit 11), Buffer Write Ready (bit 10)
    let state_offset = SDHCI_PRESENT_STATE / 4;
    mmio_space[state_offset] = STATE_CARD_INSERTED | STATE_BUFFER_READ_EN | STATE_BUFFER_WRITE_EN;

    // Interrupt status: Command Complete (bit 0)
    let int_offset = SDHCI_INT_STATUS / 4;
    mmio_space[int_offset] = INT_CMD_COMPLETE;

    let base = mmio_space.as_mut_ptr() as usize;

    let mut controller = SdhciController::new(base);
    // The rig is plain memory: opt into loopback completion synthesis
    // explicitly. Real hardware never sets this (new() defaults to false).
    controller.loopback = true;
    controller.total_sectors = 125_829_120; // 64 GB eMMC (64 * 1024 * 1024 * 1024 / 512)
    controller.high_capacity = true;

    // Test 1: Card Insertion check
    assert!(controller.is_card_inserted(), "SDHCI: Card detection failed");
    crate::serial_println!("[TEST] SDHCI Controller: Card insertion debounced signal (STATE_CARD_INSERTED) VERIFIED.");

    // Test 2: Command issuance (CMD0 GO_IDLE)
    let resp = controller.send_command(CMD0_GO_IDLE, 0, false).expect("CMD0 failed");
    assert_eq!(resp, 0);
    crate::serial_println!("[TEST] SDHCI Controller: CMD0 GO_IDLE transmission and status handshake PASSED.");

    // Test 3: Block Write Cycle (512-byte payload to sector 100)
    let mut write_payload = [0x5Au8; 512];
    write_payload[0..8].copy_from_slice(b"INTEROS1");
    write_payload[504..512].copy_from_slice(b"SOVEREGN");

    let write_res = controller.write_sector(100, &write_payload);
    assert!(write_res.is_ok(), "SDHCI: Write sector failed");
    crate::serial_println!("[TEST] SDHCI Controller: 512-byte sector write (LBA 100) via Buffer Port FIFO PASSED.");

    // Test 4: Block Read Cycle
    let mut read_buf = [0u8; 512];
    // Populate buffer port register with matching 32-bit test pattern
    let buf_offset = SDHCI_BUFFER_PORT / 4;
    unsafe {
        core::ptr::write_volatile(mmio_space.as_mut_ptr().add(buf_offset), 0x5A5A_5A5A);
    }

    let read_res = controller.read_sector(100, &mut read_buf);
    assert!(read_res.is_ok(), "SDHCI: Read sector failed");
    assert_eq!(read_buf[0], 0x5A);
    assert_eq!(read_buf[1], 0x5A);

    assert_eq!(controller.sector_count(), 125_829_120);
    crate::serial_println!(
        "[TEST] SDHCI register model: capacity counter arithmetic exercised against a synthetic {}-sector descriptor.",
        controller.sector_count(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain-memory register rig initializer. The array must live in the
    /// caller's frame; this only presets registers and returns the base.
    /// Loopback register model only — no physical or QEMU device involved.
    fn rig_base(mmio: &mut [u32; 64], state: u32, int_status: u32) -> usize {
        mmio[SDHCI_PRESENT_STATE / 4] = state;
        mmio[SDHCI_INT_STATUS / 4] = int_status;
        mmio.as_mut_ptr() as usize
    }

    #[test]
    fn sdhci_bounds_w1c_clear_writes_only_intended_bits() {
        // The rig is plain memory, so the VALUE written to INT_STATUS is
        // directly observable. W1C requires writing exactly the bits being
        // acknowledged: a 1 clears that bit on silicon, a 0 leaves it alone.
        let mut mmio = [0u32; 64];
        let base = rig_base(&mut mmio, 0, u32::MAX);
        let controller = SdhciController::new(base);
        controller.clear_interrupts(INT_CMD_COMPLETE);
        assert_eq!(
            mmio[SDHCI_INT_STATUS / 4],
            INT_CMD_COMPLETE,
            "W1C clear must write only the target bit, not cur & !bit"
        );

        mmio[SDHCI_INT_STATUS / 4] = u32::MAX;
        controller.clear_interrupts(INT_CMD_COMPLETE | INT_TRANSFER_COMPLETE);
        assert_eq!(
            mmio[SDHCI_INT_STATUS / 4],
            INT_CMD_COMPLETE | INT_TRANSFER_COMPLETE
        );

        // Error-acknowledge path in send_command must also write only the
        // error bit it intends to clear.
        mmio[SDHCI_INT_STATUS / 4] = u32::MAX;
        mmio[SDHCI_PRESENT_STATE / 4] = 0;
        let mut c2 = SdhciController::new(base);
        c2.loopback = true;
        // INT_ERROR is set: send_command clears it (writes INT_ERROR only),
        // then loopback raises CMD_COMPLETE and the wait loop acknowledges it.
        // The rig is plain memory, so the final word is the LAST W1C write
        // value: exactly INT_CMD_COMPLETE, never cur & !INT_CMD_COMPLETE.
        let _ = c2.send_command(CMD0_GO_IDLE, 0, false);
        assert_eq!(mmio[SDHCI_INT_STATUS / 4], INT_CMD_COMPLETE, "all acks must be exact-bit W1C writes");
    }

    #[test]
    fn sdhci_bounds_lba_byte_address_overflow_refused_before_mmio() {
        // base = 0 is deliberately unmapped: any MMIO access would crash the
        // test process. The refusal must happen before the first register touch.
        let mut controller = SdhciController::new(0);
        controller.high_capacity = false; // byte-addressed: arg = lba * 512
        controller.total_sectors = 0; // capacity unknown -> range check skipped
        let overflow_lba: u32 = 0x0100_0000; // 2^24 * 512 = 2^33 > u32::MAX
        let mut buf = [0u8; SECTOR_SIZE];
        assert_eq!(controller.read_block(overflow_lba, &mut buf), Err(AtaError::LbaOutOfRange));
        assert_eq!(controller.write_block(overflow_lba, &buf), Err(AtaError::LbaOutOfRange));
        // Largest LBA that still fits the u32 byte argument must not be
        // refused by the multiply check itself (it proceeds to MMIO, so we
        // only assert the arithmetic boundary here, not a device result).
        assert_eq!((0x007F_FFFFu32).checked_mul(512), Some(0xFFFF_FE00u32));
        assert_eq!((0x0080_0000u32).checked_mul(512), None);
    }

    #[test]
    fn sdhci_bounds_lba_past_known_capacity_refused_before_mmio() {
        let mut controller = SdhciController::new(0);
        controller.high_capacity = true;
        controller.total_sectors = 100;
        let mut buf = [0u8; SECTOR_SIZE];
        assert_eq!(controller.read_block(100, &mut buf), Err(AtaError::LbaOutOfRange));
        assert_eq!(controller.write_block(100, &buf), Err(AtaError::LbaOutOfRange));
        assert_eq!(controller.read_block(u32::MAX, &mut buf), Err(AtaError::LbaOutOfRange));
    }

    #[test]
    fn sdhci_bounds_no_loopback_never_fabricates_cmd_complete() {
        // Hardware-like rig: no completion bit preset, loopback OFF. The
        // driver must wait and time out, never self-raise INT_CMD_COMPLETE.
        let mut mmio = [0u32; 64];
        let base = rig_base(&mut mmio, 0, 0);
        let controller = SdhciController::new(base);
        assert!(!controller.loopback, "new() must default to hardware mode");
        assert_eq!(controller.send_command(CMD0_GO_IDLE, 0, false), Err(AtaError::Timeout));
        assert_eq!(mmio[SDHCI_INT_STATUS / 4], 0, "status must stay untouched");
    }

    #[test]
    fn sdhci_bounds_unaligned_buffer_fifo_roundtrip() {
        let mut mmio = [0u32; 64];
        let base = rig_base(
            &mut mmio,
            STATE_CARD_INSERTED | STATE_BUFFER_READ_EN | STATE_BUFFER_WRITE_EN,
            0,
        );
        let mut controller = SdhciController::new(base);
        controller.loopback = true;
        controller.high_capacity = true;
        controller.total_sectors = 1024;

        // Deliberately misaligned 512-byte array views (odd addresses): a
        // *mut u32 / *const u32 dereference on these is UB; the FIFO loops
        // must use write_unaligned / read_unaligned.
        let mut src_backing = [0u8; SECTOR_SIZE + 4];
        for (i, b) in src_backing.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        src_backing[1 + 508..1 + 512].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let payload: &[u8; SECTOR_SIZE] =
            unsafe { &*(src_backing.as_ptr().add(1) as *const [u8; SECTOR_SIZE]) };
        assert_eq!(payload.as_ptr() as usize % 4, 1, "source must be misaligned");

        controller.write_block(7, payload).expect("loopback write");
        // The last FIFO dword written must be the little-endian tail of the
        // misaligned source buffer, proving every byte lane was read correctly.
        assert_eq!(
            mmio[SDHCI_BUFFER_PORT / 4],
            u32::from_le_bytes([0xDE, 0xAD, 0xBE, 0xEF])
        );

        // Read path into a second misaligned buffer.
        mmio[SDHCI_BUFFER_PORT / 4] = 0xA5A5_A5A5;
        let mut dst_backing = [0u8; SECTOR_SIZE + 4];
        let dest: &mut [u8; SECTOR_SIZE] =
            unsafe { &mut *(dst_backing.as_mut_ptr().add(1) as *mut [u8; SECTOR_SIZE]) };
        assert_eq!(dest.as_ptr() as usize % 4, 1, "destination must be misaligned");
        controller.read_block(7, dest).expect("loopback read");
        assert!(dest.iter().all(|&b| b == 0xA5), "all 512 bytes must land intact");
        // W1C ack of transfer complete: the plain-memory rig holds the last
        // written value, which must be exactly INT_TRANSFER_COMPLETE.
        assert_eq!(mmio[SDHCI_INT_STATUS / 4], INT_TRANSFER_COMPLETE);
    }
}

