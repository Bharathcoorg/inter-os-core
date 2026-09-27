//! Persistent block-backed filesystem manager.
//!
//! Owns the physical placement policy for the InterFS region so the filesystem
//! cannot overlap the boot area or the persistent metadata journal.
use crate::drivers::{BlockDevice, ATA_PRIMARY};
use super::interfs::{FsBackendError, InterFs};

/// Leave the first 4096 sectors to the bootloader/partition/device-specific area.
pub const BLOCK_FS_BASE_LBA: u32 = 4096;
/// Persistent journal currently reserves 16 * 128 sectors at the disk tail.
pub const BLOCK_FS_TAIL_RESERVED: u32 = 16 * 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemLayout {
    pub base_lba: u32,
    pub tail_reserved_sectors: u32,
}

impl FilesystemLayout {
    pub const fn canonical() -> Self { Self { base_lba: BLOCK_FS_BASE_LBA, tail_reserved_sectors: BLOCK_FS_TAIL_RESERVED } }
}

fn region_for_layout(total: u32, layout: FilesystemLayout) -> Result<u32, FsBackendError> {
    let end = total.checked_sub(layout.tail_reserved_sectors).ok_or(FsBackendError::NoSpace)?;
    end.checked_sub(layout.base_lba).filter(|v| *v > 42).ok_or(FsBackendError::NoSpace)
}

fn filesystem_region_sectors(total: u32) -> Result<u32, FsBackendError> {
    let end = total.checked_sub(BLOCK_FS_TAIL_RESERVED).ok_or(FsBackendError::NoSpace)?;
    end.checked_sub(BLOCK_FS_BASE_LBA).filter(|v| *v > 42).ok_or(FsBackendError::NoSpace)
}

/// Mount the canonical InterFS region, formatting it only when no valid
/// filesystem exists. Formatting is deliberately bounded away from the boot
/// area and journal tail.
pub fn init_block_filesystem_with_device(device: &mut dyn BlockDevice) -> Result<(), FsBackendError> {
    init_block_filesystem_with_layout(device, FilesystemLayout::canonical())
}

pub fn init_block_filesystem_with_layout(device: &mut dyn BlockDevice, layout: FilesystemLayout) -> Result<(), FsBackendError> {
    let region = region_for_layout(device.sector_count(), layout)?;
    match InterFs::mount_region(device, layout.base_lba, region) {
        Ok(fs) if fs.capacity_sectors() == region - 41 => Ok(()),
        Ok(_) => Err(FsBackendError::CorruptMetadata),
        Err(FsBackendError::InvalidFormat) => { let _ = InterFs::format_region(device, layout.base_lba, region)?; Ok(()) }
        Err(e) => Err(e),
    }
}

pub fn init_block_filesystem() -> Result<(), FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    init_block_filesystem_with_layout(device, FilesystemLayout::canonical())
}

pub fn load_data_file_with_device(device: &mut dyn BlockDevice, path: &str, out: &mut [u8]) -> Result<usize, FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.read_file(path, out)
}

pub fn load_data_file(path: &str, out: &mut [u8]) -> Result<usize, FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    load_data_file_with_device(device, path, out)
}

pub fn store_data_file_with_device(device: &mut dyn BlockDevice, path: &str, data: &[u8]) -> Result<(), FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    match fs.write_file(path, data) {
        Ok(()) => Ok(()),
        Err(FsBackendError::FileNotFound) => { fs.create_file(path, data)?; Ok(()) }
        Err(e) => Err(e),
    }
}

pub fn store_data_file(path: &str, data: &[u8]) -> Result<(), FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    store_data_file_with_device(device, path, data)
}
