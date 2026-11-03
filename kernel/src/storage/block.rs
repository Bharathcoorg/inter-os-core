// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Platform-neutral block storage contract.

use alloc::string::String;

pub const SECTOR_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDeviceError {
    DeviceNotFound,
    DeviceFault,
    Timeout,
    ReadError,
    WriteError,
    LbaOutOfRange,
    InvalidBuffer,
}

pub trait BlockDevice: Send + Sync {
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError>;
    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError>;
    fn sector_count(&self) -> u32;
    fn model(&self) -> &str;

    /// Flushes volatile hardware write caches to persistent media.
    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        Ok(())
    }

    /// Total capacity of the device in bytes.
    fn capacity(&self) -> u64 {
        (self.sector_count() as u64).saturating_mul(self.block_size() as u64)
    }

    /// Hardware sector / block size in bytes.
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    /// Validated multi-sector read operation.
    /// Enforces sector alignment, non-empty buffers, capacity bounds, and checked arithmetic.
    fn read(&mut self, block: u32, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        if buffer.is_empty() || buffer.len() % SECTOR_SIZE != 0 {
            return Err(BlockDeviceError::InvalidBuffer);
        }
        let sectors = (buffer.len() / SECTOR_SIZE) as u32;
        let end_block = block.checked_add(sectors).ok_or(BlockDeviceError::LbaOutOfRange)?;
        let total = self.sector_count();
        if total > 0 && end_block > total {
            return Err(BlockDeviceError::LbaOutOfRange);
        }
        for (i, chunk) in buffer.chunks_exact_mut(SECTOR_SIZE).enumerate() {
            let lba = block.checked_add(i as u32).ok_or(BlockDeviceError::LbaOutOfRange)?;
            let mut sec_buf = [0u8; SECTOR_SIZE];
            self.read_sector(lba, &mut sec_buf)?;
            chunk.copy_from_slice(&sec_buf);
        }
        Ok(())
    }

    /// Validated multi-sector write operation followed by hardware cache flush.
    /// Enforces sector alignment, non-empty buffers, capacity bounds, and checked arithmetic.
    fn write(&mut self, block: u32, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        if buffer.is_empty() || buffer.len() % SECTOR_SIZE != 0 {
            return Err(BlockDeviceError::InvalidBuffer);
        }
        let sectors = (buffer.len() / SECTOR_SIZE) as u32;
        let end_block = block.checked_add(sectors).ok_or(BlockDeviceError::LbaOutOfRange)?;
        let total = self.sector_count();
        if total > 0 && end_block > total {
            return Err(BlockDeviceError::LbaOutOfRange);
        }
        for (i, chunk) in buffer.chunks_exact(SECTOR_SIZE).enumerate() {
            let lba = block.checked_add(i as u32).ok_or(BlockDeviceError::LbaOutOfRange)?;
            let mut sec_buf = [0u8; SECTOR_SIZE];
            sec_buf.copy_from_slice(chunk);
            self.write_sector(lba, &sec_buf)?;
        }
        self.flush()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDeviceKind { Ata, Emmc, Sd, Nvme, Virtual }

#[derive(Debug, Clone)]
pub struct BlockDeviceInfo {
    pub kind: BlockDeviceKind,
    pub model: String,
    pub sector_count: u32,
    pub sector_size: usize,
}

impl BlockDeviceInfo {
    pub fn from_device(kind: BlockDeviceKind, device: &dyn BlockDevice) -> Self {
        Self { kind, model: String::from(device.model()), sector_count: device.sector_count(), sector_size: SECTOR_SIZE }
    }
}
