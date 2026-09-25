//! Persistent block-backed filesystem manager.
//!
//! Owns the physical placement policy for the InterFS region so the filesystem
//! cannot overlap the boot area or the persistent metadata journal.
use crate::drivers::ATA_PRIMARY;
use super::interfs::{FsBackendError, InterFs};

/// Leave the first 4096 sectors to the bootloader/partition/device-specific area.
pub const BLOCK_FS_BASE_LBA: u32 = 4096;
/// Persistent journal currently reserves 16 * 128 sectors at the disk tail.
pub const BLOCK_FS_TAIL_RESERVED: u32 = 16 * 128;

fn filesystem_region_sectors(total: u32) -> Result<u32, FsBackendError> {
    let end = total.checked_sub(BLOCK_FS_TAIL_RESERVED).ok_or(FsBackendError::NoSpace)?;
    end.checked_sub(BLOCK_FS_BASE_LBA).filter(|v| *v > 42).ok_or(FsBackendError::NoSpace)
}

/// Mount the canonical InterFS region, formatting it only when no valid
/// filesystem exists. Formatting is deliberately bounded away from the boot
/// area and journal tail.
pub fn init_block_filesystem() -> Result<(), FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    let region = filesystem_region_sectors(device.sector_count())?;
    match InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region) {
        Ok(fs) if fs.capacity_sectors() == region - 41 => Ok(()),
        Ok(_) => Err(FsBackendError::CorruptMetadata),
        Err(FsBackendError::InvalidFormat) => {
            let _ = InterFs::format_region(device, BLOCK_FS_BASE_LBA, region)?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Read one persistent file from the canonical block filesystem.
pub fn load_data_file(path: &str, out: &mut [u8]) -> Result<usize, FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    let region = filesystem_region_sectors(device.sector_count())?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.read_file(path, out)
}

/// Create or replace one persistent file in the canonical block filesystem.
pub fn store_data_file(path: &str, data: &[u8]) -> Result<(), FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    let region = filesystem_region_sectors(device.sector_count())?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    match fs.write_file(path, data) {
        Ok(()) => Ok(()),
        Err(FsBackendError::FileNotFound) => {
            fs.create_file(path, data)?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}
