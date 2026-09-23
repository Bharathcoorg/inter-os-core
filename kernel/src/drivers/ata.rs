//! Primary ATA / IDE PIO Block Device Driver
//!
//! Provides bare-metal sector-level read and write access to IDE/ATA hard drives
//! on the x86_64 architecture (Ports 0x1F0-0x1F7, 0x3F6).
//! This is the foundational block device driver for persistent storage in Inter OS.

use alloc::string::String;
use spin::Mutex;
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

/// ATA Block Driver Errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaError {
    DeviceNotFound,
    DeviceFault,
    Timeout,
    ReadError,
    WriteError,
    LbaOutOfRange,
}

/// Abstract block device trait.
pub trait BlockDevice: Send + Sync {
    /// Reads a 512-byte sector at the specified LBA.
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), AtaError>;
    /// Writes a 512-byte sector to the specified LBA.
    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), AtaError>;
    /// Returns the total addressable sector count.
    fn sector_count(&self) -> u32;
    /// Returns the model identifier string.
    fn model(&self) -> &str;
}

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
        }
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

    /// Waits until the drive clears the BSY status bit with a timeout counter.
    fn wait_not_busy(&mut self) -> Result<u8, AtaError> {
        let mut timeout = 100_000;
        while timeout > 0 {
            let status = unsafe { self.status_port.read() };
            if (status & STATUS_BSY) == 0 {
                return Ok(status);
            }
            timeout -= 1;
        }
        Err(AtaError::Timeout)
    }

    /// Waits until the drive asserts DRQ (Data Request Ready) or returns an error.
    fn wait_drq(&mut self) -> Result<(), AtaError> {
        let mut timeout = 100_000;
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
            timeout -= 1;
        }
        Err(AtaError::Timeout)
    }

    /// Probes the drive using the ATA `IDENTIFY` command (0xEC).
    pub fn identify(&mut self) -> Result<(), AtaError> {
        unsafe {
            // Suppress device interrupts in PIO mode
            self._control_port.write(0x02);

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

impl BlockDevice for AtaDrive {
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), AtaError> {
        if self.total_sectors > 0 && lba >= self.total_sectors {
            return Err(AtaError::LbaOutOfRange);
        }

        unsafe {
            self._control_port.write(0x02);
            self.wait_not_busy()?;

            // Send Drive/Head with LBA mode enabled (0xE0) + top 4 bits of LBA
            let drive_head = 0xE0 | ((lba >> 24) & 0x0F) as u8;
            self.drive_head_port.write(drive_head);
            self.delay_400ns();

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
            self.wait_not_busy()?;
            self.wait_drq()?;

            // Read 256 16-bit words from Data port
            for i in 0..256 {
                let word = self.data_port.read();
                buf[i * 2] = (word & 0xFF) as u8;
                buf[i * 2 + 1] = (word >> 8) as u8;
            }

            Ok(())
        }
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), AtaError> {
        if self.total_sectors > 0 && lba >= self.total_sectors {
            return Err(AtaError::LbaOutOfRange);
        }

        unsafe {
            self._control_port.write(0x02);
            self.wait_not_busy()?;

            // Send Drive/Head with LBA mode enabled (0xE0) + top 4 bits of LBA
            let drive_head = 0xE0 | ((lba >> 24) & 0x0F) as u8;
            self.drive_head_port.write(drive_head);
            self.delay_400ns();

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
            self.wait_not_busy()?;
            self.wait_drq()?;

            // Write 256 16-bit words to Data port
            for i in 0..256 {
                let word = (buf[i * 2] as u16) | ((buf[i * 2 + 1] as u16) << 8);
                self.data_port.write(word);
            }

            // Flush write cache
            self.cmd_port.write(CMD_CACHE_FLUSH);
            self.delay_400ns();
            self.wait_not_busy()?;

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

/// Global synchronized ATA controller instance.
pub static ATA_PRIMARY: Mutex<Option<AtaDrive>> = Mutex::new(None);

/// Initializes and probes the primary ATA hard drive.
pub fn init() -> Result<(), AtaError> {
    let mut drive = AtaDrive::new_primary_master();
    drive.identify()?;
    *ATA_PRIMARY.lock() = Some(drive);
    Ok(())
}
