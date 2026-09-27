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
}

pub trait BlockDevice: Send + Sync {
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError>;
    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError>;
    fn sector_count(&self) -> u32;
    fn model(&self) -> &str;
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
