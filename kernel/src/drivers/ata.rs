// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Primary ATA / IDE PIO Block Device Driver
//!
//! Provides bare-metal sector-level read and write access to IDE/ATA hard drives
//! on the x86_64 architecture (Ports 0x1F0-0x1F7, 0x3F6).
//! This is the foundational block device driver for persistent storage in Inter OS.

use alloc::string::String;
use spin::Mutex;

pub use crate::storage::block::{BlockDevice, BlockDeviceError};
pub type AtaError = BlockDeviceError;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

/// Sector size in bytes for standard ATA drives.
pub const SECTOR_SIZE: usize = 512;

/// Primary ATA I/O base ports.
const ATA_DATA_PORT: u16 = 0x1F0;
const ATA_ERROR_PORT: u16 = 0x1F1;
const ATA_SECTOR_COUNT_PORT: u16 = 0x1F2;
const ATA_LBA_LOW_PORT: u16 = 0x1F3;
const ATA_LBA_MID_PORT: u16 = 0x1F4;
const ATA_LBA_HIGH_PORT: u16 = 0x1F5;
const ATA_DRIVE_HEAD_PORT: u16 = 0x1F6;
const ATA_STATUS_CMD_PORT: u16 = 0x1F7;
const ATA_CONTROL_PORT: u16 = 0x3F6;

/// The secondary ATA channel's base ports.
///
/// WHY this channel is reached at all: the demo device occupies the primary
/// channel with the boot image (unit 0) and the data disk (unit 1), so a third
/// disk has nowhere else to sit on this transport. QEMU's `piix3-ide` exposes
/// `ide.1` for exactly that, and provisioning needs one more disk -- the
/// disposable staging image a host tool writes -- without moving either of the
/// two the device already depends on.
const ATA2_DATA_PORT: u16 = 0x170;
const ATA2_ERROR_PORT: u16 = 0x171;
const ATA2_SECTOR_COUNT_PORT: u16 = 0x172;
const ATA2_LBA_LOW_PORT: u16 = 0x173;
const ATA2_LBA_MID_PORT: u16 = 0x174;
const ATA2_LBA_HIGH_PORT: u16 = 0x175;
const ATA2_DRIVE_HEAD_PORT: u16 = 0x176;
const ATA2_STATUS_CMD_PORT: u16 = 0x177;
const ATA2_CONTROL_PORT: u16 = 0x376;

/// ATA Commands.
const CMD_IDENTIFY: u8 = 0xEC;
const CMD_READ_SECTORS: u8 = 0x20;
const CMD_WRITE_SECTORS: u8 = 0x30;
const CMD_CACHE_FLUSH: u8 = 0xE7;

/// ATA Status bits.
const STATUS_ERR: u8 = 1 << 0;  // Error
const STATUS_DRQ: u8 = 1 << 3;  // Data Request Ready
const STATUS_DF: u8 = 1 << 5;   // Device Fault
#[allow(dead_code)]
const STATUS_RDY: u8 = 1 << 6;  // Drive Ready
const STATUS_BSY: u8 = 1 << 7;  // Drive Busy

/// Primary ATA Controller Hardware Driver.
pub struct AtaDrive {
    data_port: Port<u16>,
    _error_port: PortReadOnly<u8>,
    sector_count_port: Port<u8>,
    lba_low_port: Port<u8>,
    lba_mid_port: Port<u8>,
    lba_high_port: Port<u8>,
    drive_head_port: Port<u8>,
    status_port: PortReadOnly<u8>,
    cmd_port: PortWriteOnly<u8>,
    _control_port: PortWriteOnly<u8>,
    alt_status_port: PortReadOnly<u8>,
    total_sectors: u32,
    model_name: String,
    is_master: bool,
    /// Whether this instance is a *presence probe* rather than a working disk.
    ///
    /// WHY a flag on the drive and not a flag on the caller: an empty IDE slot
    /// never asserts DRQ, and `wait_drq` spins ten million status reads before
    /// it says so. That is the right patience for a disk the OS is about to
    /// store on and the wrong patience for a device that is usually not there --
    /// every boot of a real phone would pay it. A probe waits a bounded number of
    /// reads and reports absence instead of timing out on the console, because a
    /// provisioning channel that was never attached is not a fault.
    probe_only: bool,
}

impl AtaDrive {
    /// Creates and initializes the primary ATA controller for the master drive.
    pub fn new_primary_master() -> Self {
        Self {
            data_port: Port::new(ATA_DATA_PORT),
            _error_port: PortReadOnly::new(ATA_ERROR_PORT),
            sector_count_port: Port::new(ATA_SECTOR_COUNT_PORT),
            lba_low_port: Port::new(ATA_LBA_LOW_PORT),
            lba_mid_port: Port::new(ATA_LBA_MID_PORT),
            lba_high_port: Port::new(ATA_LBA_HIGH_PORT),
            drive_head_port: Port::new(ATA_DRIVE_HEAD_PORT),
            status_port: PortReadOnly::new(ATA_STATUS_CMD_PORT),
            cmd_port: PortWriteOnly::new(ATA_STATUS_CMD_PORT),
            _control_port: PortWriteOnly::new(ATA_CONTROL_PORT),
            alt_status_port: PortReadOnly::new(ATA_CONTROL_PORT),
            total_sectors: 0,
            model_name: String::new(),
            is_master: true,
            probe_only: false,
        }
    }

    /// Creates and initializes the primary ATA controller for the slave drive.
    pub fn new_primary_slave() -> Self {
        Self {
            data_port: Port::new(ATA_DATA_PORT),
            _error_port: PortReadOnly::new(ATA_ERROR_PORT),
            sector_count_port: Port::new(ATA_SECTOR_COUNT_PORT),
            lba_low_port: Port::new(ATA_LBA_LOW_PORT),
            lba_mid_port: Port::new(ATA_LBA_MID_PORT),
            lba_high_port: Port::new(ATA_LBA_HIGH_PORT),
            drive_head_port: Port::new(ATA_DRIVE_HEAD_PORT),
            status_port: PortReadOnly::new(ATA_STATUS_CMD_PORT),
            cmd_port: PortWriteOnly::new(ATA_STATUS_CMD_PORT),
            _control_port: PortWriteOnly::new(ATA_CONTROL_PORT),
            alt_status_port: PortReadOnly::new(ATA_CONTROL_PORT),
            total_sectors: 0,
            model_name: String::new(),
            is_master: false,
            probe_only: false,
        }
    }

    /// The staging channel: the master drive of the secondary ATA controller.
    ///
    /// WHY a drive of its own: the boot image and the data disk already hold both
    /// units of the primary channel, and a payload copied in from the host has to
    /// arrive on a disk that is not either of them -- one that provisioning can
    /// attach for a single run and detach forever after.
    ///
    /// WHY `is_probe` decides the patience: presence is established with bounded
    /// waits, because an empty slot never answers and a real phone boots without
    /// this disk attached. Once the slot has answered, the same drive is taken
    /// again with the ordinary patience, so a slow sector read on a drive that is
    /// genuinely there is never mistaken for absence.
    fn secondary(is_probe: bool) -> Self {
        Self {
            data_port: Port::new(ATA2_DATA_PORT),
            _error_port: PortReadOnly::new(ATA2_ERROR_PORT),
            sector_count_port: Port::new(ATA2_SECTOR_COUNT_PORT),
            lba_low_port: Port::new(ATA2_LBA_LOW_PORT),
            lba_mid_port: Port::new(ATA2_LBA_MID_PORT),
            lba_high_port: Port::new(ATA2_LBA_HIGH_PORT),
            drive_head_port: Port::new(ATA2_DRIVE_HEAD_PORT),
            status_port: PortReadOnly::new(ATA2_STATUS_CMD_PORT),
            cmd_port: PortWriteOnly::new(ATA2_STATUS_CMD_PORT),
            _control_port: PortWriteOnly::new(ATA2_CONTROL_PORT),
            alt_status_port: PortReadOnly::new(ATA2_CONTROL_PORT),
            total_sectors: 0,
            model_name: String::new(),
            // The drive/head byte selects the drive *within* the channel the
            // ports already chose, so the secondary master uses the same 0xA0 and
            // 0xE0 values the primary master does.
            is_master: true,
            probe_only: is_probe,
        }
    }

    /// Probes the secondary channel's master for a staging drive, patiently.
    pub fn new_secondary_master_probe() -> Self {
        Self::secondary(true)
    }

    /// The secondary channel's master as a working disk.
    pub fn new_secondary_master() -> Self {
        Self::secondary(false)
    }

    /// Performs bus delay by reading the alternate status register 4 times (~400ns).
    fn delay_400ns(&mut self) {
        unsafe {
            let _ = self.alt_status_port.read();
            let _ = self.alt_status_port.read();
            let _ = self.alt_status_port.read();
            let _ = self.alt_status_port.read();
        }
    }

    /// Pulses SRST once so the drive starts from a known state.
    ///
    /// WHY this is not done per operation: the device control register bit 1
    /// is software reset, and holding it asserted keeps the whole channel in
    /// reset. Asserting it before every sector read/write (as this driver used
    /// to) makes each I/O depend on when the asynchronous reset is serviced;
    /// a command issued before it lands is aborted, DRQ never asserts, and the
    /// poll below spins until its timeout. The register is left at 0x00
    /// (SRST low, interrupts enabled) and no I/O path touches it again.
    fn soft_reset(&mut self) {
        unsafe {
            self._control_port.write(0x02);
            // ATA requires SRST to be held for at least 5us.
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
            self._control_port.write(0x00);
            // After SRST is released the device may stay busy for up to 2ms;
            // wait_not_busy polls the status register for that window.
            let _ = self.wait_not_busy();
        }
    }

    /// The most status polls a presence probe spends before calling the slot
    /// empty. A working disk waits ten million; an empty IDE slot answers nothing
    /// at all, so a probe has to stop early and say "absent" rather than spin for
    /// the whole patience a real transfer deserves.
    const PROBE_POLL_LIMIT: u32 = 200_000;
    /// The most status polls a transfer waits out before the operation fails.
    const TRANSFER_POLL_LIMIT: u32 = 10_000_000;

    /// Waits until the drive clears the BSY status bit with a timeout counter.
    fn wait_not_busy(&mut self) -> Result<u8, AtaError> {
        let limit = if self.probe_only { Self::PROBE_POLL_LIMIT } else { Self::TRANSFER_POLL_LIMIT };
        let mut timeout = limit;
        while timeout > 0 {
            let status = unsafe { self.status_port.read() };
            if (status & STATUS_BSY) == 0 {
                return Ok(status);
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
        let status = unsafe { self.status_port.read() };
        // WHY a probe stays quiet: an empty slot is the normal state of the
        // staging channel, and a timeout line on the console for a disk that was
        // never attached reads like a failing drive.
        if !self.probe_only {
            crate::serial_println!("[ATA] wait_not_busy timed out, status={:#04X}", status);
        }
        Err(AtaError::Timeout)
    }

    /// Waits until the drive asserts DRQ (Data Request Ready) or returns an error.
    fn wait_drq(&mut self) -> Result<(), AtaError> {
        let limit = if self.probe_only { Self::PROBE_POLL_LIMIT } else { Self::TRANSFER_POLL_LIMIT };
        let mut timeout = limit;
        while timeout > 0 {
            let status = unsafe { self.status_port.read() };
            if (status & STATUS_ERR) != 0 {
                return Err(AtaError::ReadError);
            }
            if (status & STATUS_DF) != 0 {
                return Err(AtaError::DeviceFault);
            }
            if (status & STATUS_DRQ) != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
        let status = unsafe { self.status_port.read() };
        if !self.probe_only {
            crate::serial_println!("[ATA] wait_drq timed out, status={:#04X}", status);
        }
        Err(AtaError::Timeout)
    }

    /// Probes the drive using the ATA `IDENTIFY` command (0xEC).
    pub fn identify(&mut self) -> Result<(), AtaError> {
        unsafe {
            self.soft_reset();

            // Select drive: 0xA0 for Master, 0xB0 for Slave
            let select = if self.is_master { 0xA0 } else { 0xB0 };
            self.drive_head_port.write(select);
            self.delay_400ns();

            // Clear sector count and LBA ports
            self.sector_count_port.write(0);
            self.lba_low_port.write(0);
            self.lba_mid_port.write(0);
            self.lba_high_port.write(0);

            // Send IDENTIFY command
            self.cmd_port.write(CMD_IDENTIFY);
            self.delay_400ns();

            // Check if drive is present
            let status = self.status_port.read();
            if status == 0 || status == 0xFF {
                return Err(AtaError::DeviceNotFound);
            }

            // Wait for drive to become ready
            self.wait_not_busy()?;

            // Check for ATAPI/SATA non-standard devices (lba_mid != 0 or lba_high != 0)
            let mid = self.lba_mid_port.read();
            let high = self.lba_high_port.read();
            if mid != 0 || high != 0 {
                return Err(AtaError::DeviceNotFound);
            }

            // Wait for DRQ
            self.wait_drq()?;

            // Read 256 16-bit words (512 bytes) of identity data
            let mut id_data = [0u16; 256];
            for word in id_data.iter_mut() {
                *word = self.data_port.read();
            }

            // Word 60 and 61 contain total 28-bit LBA addressable sectors
            let lba_sectors = (id_data[60] as u32) | ((id_data[61] as u32) << 16);
            self.total_sectors = lba_sectors;

            // Words 27..47 contain the ASCII Model Name (byte-swapped pairs)
            let mut model_bytes = [0u8; 40];
            for i in 0..20 {
                let word = id_data[27 + i];
                model_bytes[i * 2] = (word >> 8) as u8;
                model_bytes[i * 2 + 1] = (word & 0xFF) as u8;
            }

            let raw_str = core::str::from_utf8(&model_bytes).unwrap_or("QEMU HARDDISK");
            self.model_name = String::from(raw_str.trim());

            Ok(())
        }
    }
}

fn map_block_error(e: AtaError) -> BlockDeviceError {
    match e { AtaError::DeviceNotFound => BlockDeviceError::DeviceNotFound, AtaError::Timeout => BlockDeviceError::Timeout, AtaError::LbaOutOfRange => BlockDeviceError::LbaOutOfRange, _ => BlockDeviceError::DeviceFault }
}

impl BlockDevice for AtaDrive {
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
        if self.total_sectors > 0 && lba >= self.total_sectors {
            return Err(BlockDeviceError::LbaOutOfRange);
        }

        unsafe {
            // Select Drive/Head with LBA mode enabled (0xE0 for Master, 0xF0 for Slave) + top 4 bits of LBA
            let drive_head = (if self.is_master { 0xE0 } else { 0xF0 }) | ((lba >> 24) & 0x0F) as u8;
            self.drive_head_port.write(drive_head);
            self.delay_400ns();
            self.wait_not_busy().map_err(map_block_error)?;

            // Write sector count = 1
            self.sector_count_port.write(1);

            // Write LBA registers
            self.lba_low_port.write((lba & 0xFF) as u8);
            self.lba_mid_port.write(((lba >> 8) & 0xFF) as u8);
            self.lba_high_port.write(((lba >> 16) & 0xFF) as u8);

            // Send READ SECTORS command
            self.cmd_port.write(CMD_READ_SECTORS);
            self.delay_400ns();

            // Wait for drive to process and assert DRQ
            self.wait_not_busy().map_err(map_block_error)?;
            self.wait_drq().map_err(map_block_error)?;

            // Read 256 16-bit words from Data port
            for i in 0..256 {
                let word = self.data_port.read();
                buf[i * 2] = (word & 0xFF) as u8;
                buf[i * 2 + 1] = (word >> 8) as u8;
            }
            let _ = self.status_port.read();

            Ok(())
        }
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
        if self.total_sectors > 0 && lba >= self.total_sectors {
            return Err(AtaError::LbaOutOfRange);
        }

        unsafe {
            // Select Drive/Head with LBA mode enabled (0xE0 for Master, 0xF0 for Slave) + top 4 bits of LBA
            let drive_head = (if self.is_master { 0xE0 } else { 0xF0 }) | ((lba >> 24) & 0x0F) as u8;
            self.drive_head_port.write(drive_head);
            self.delay_400ns();
            self.wait_not_busy().map_err(map_block_error)?;

            // Write sector count = 1
            self.sector_count_port.write(1);

            // Write LBA registers
            self.lba_low_port.write((lba & 0xFF) as u8);
            self.lba_mid_port.write(((lba >> 8) & 0xFF) as u8);
            self.lba_high_port.write(((lba >> 16) & 0xFF) as u8);

            // Send WRITE SECTORS command
            self.cmd_port.write(CMD_WRITE_SECTORS);
            self.delay_400ns();

            // Wait for drive to signal ready for data (DRQ)
            self.wait_not_busy().map_err(map_block_error)?;
            self.wait_drq().map_err(map_block_error)?;

            // Write 256 16-bit words to Data port
            for i in 0..256 {
                let word = (buf[i * 2] as u16) | ((buf[i * 2 + 1] as u16) << 8);
                self.data_port.write(word);
            }

            self.delay_400ns();
            self.wait_not_busy().map_err(map_block_error)?;
            let _ = self.status_port.read();

            Ok(())
        }
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        unsafe {
            let select = if self.is_master { 0xE0 } else { 0xF0 };
            self.drive_head_port.write(select);
            self.delay_400ns();
            if self.wait_not_busy().is_ok() {
                self.cmd_port.write(CMD_CACHE_FLUSH);
                self.delay_400ns();
                let _ = self.wait_not_busy();
                let _ = self.status_port.read();
            }
            Ok(())
        }
    }

    fn sector_count(&self) -> u32 {
        self.total_sectors
    }

    fn model(&self) -> &str {
        if self.model_name.is_empty() {
            "QEMU ATA HARDDISK"
        } else {
            &self.model_name
        }
    }
}

/// Global synchronized ATA controller instance for boot disk.
pub static ATA_BOOT: Mutex<Option<AtaDrive>> = Mutex::new(None);

/// Global synchronized ATA controller instance for persistent storage.
pub static ATA_PRIMARY: Mutex<Option<AtaDrive>> = Mutex::new(None);

/// Secondary ATA channel master drive.
///
/// NOTE on platform topology and VFS provisioning truth: in canonical QEMU
/// instances (`emulator/server.py`), only primary master (`ATA_BOOT` on `ide.0,unit=0`)
/// and primary slave (`ATA_PRIMARY` on `ide.0,unit=1`) are attached. The secondary
/// channel (`ide.1`) has no physical disk attached, so `probe_staging_channel()`
/// consistently finds no device (`None`). Model provisioning in InterOS operates
/// directly via `ATA_PRIMARY` using host-prepared InterFS images (via `tools/model_provisioning.py`),
/// imported at boot via `kernel/src/fs/vfs.rs:import_provisioned_models()`. This secondary
/// staging drive handle is preserved for physical secondary-channel controller hardware,
/// but is unattached in standard QEMU emulation.
pub static ATA_STAGING: Mutex<Option<AtaDrive>> = Mutex::new(None);

/// Initializes and probes the ATA hard drives.
/// If a dedicated secondary drive (Primary Slave) is present, it is mounted
/// as the persistent storage disk (ATA_PRIMARY), separating persistent state
/// from the read-only boot image on Primary Master (ATA_BOOT).
/// If no secondary drive is detected, falls back to Primary Master.
pub fn init() -> Result<(), AtaError> {
    let mut master = AtaDrive::new_primary_master();
    let master_found = master.identify().is_ok();
    if master_found {
        *ATA_BOOT.lock() = Some(master);
    }

    let mut slave = AtaDrive::new_primary_slave();
    let slave_found = slave.identify().is_ok();

    probe_staging_channel();

    if slave_found {
        *ATA_PRIMARY.lock() = Some(slave);
        Ok(())
    } else if master_found {
        // Fallback: use master as storage disk if slave is not attached
        let mut master_primary = AtaDrive::new_primary_master();
        master_primary.identify()?;
        *ATA_PRIMARY.lock() = Some(master_primary);
        Ok(())
    } else {
        Err(AtaError::DeviceNotFound)
    }
}

/// Looks once, patiently-but-boundedly, for a staging disk on the secondary
/// channel and keeps it if it answers.
///
/// WHY the answer is silent when nothing is there: a device that has never been
/// provisioned is the ordinary case, and a console line for it on every boot
/// trains anyone reading the log to ignore the lines that matter. When a disk
/// *is* there, provisioning is about to spend minutes on it, so the fact it was
/// seen belongs on the console.
fn probe_staging_channel() {
    let mut probe = AtaDrive::new_secondary_master_probe();
    if probe.identify().is_err() {
        return;
    }
    // Re-identify without the probe's impatience, so the disk the OS is about to
    // read six hundred thousand sectors from is held to the same patience as any
    // other drive.
    let mut staging = AtaDrive::new_secondary_master();
    if staging.identify().is_err() {
        return;
    }
    let sectors = staging.sector_count();
    let model = String::from(staging.model());
    crate::serial_println!("[ATA] staging channel: model '{}' online, {} sectors.", model, sectors);
    *ATA_STAGING.lock() = Some(staging);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_probe_configures_probe_patience_and_secondary_master() {
        let probe = AtaDrive::new_secondary_master_probe();
        assert!(probe.is_master);
        assert!(probe.probe_only);
    }

    #[test]
    fn staging_working_drive_is_not_probe_only() {
        let working = AtaDrive::new_secondary_master();
        assert!(working.is_master);
        assert!(!working.probe_only);
    }

    #[test]
    fn staging_poll_limits_are_honest_and_bounded() {
        assert_eq!(AtaDrive::PROBE_POLL_LIMIT, 200_000);
        assert_eq!(AtaDrive::TRANSFER_POLL_LIMIT, 10_000_000);
        assert!(AtaDrive::PROBE_POLL_LIMIT < AtaDrive::TRANSFER_POLL_LIMIT);
    }
}
